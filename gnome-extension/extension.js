// AI Usage Bar — GNOME Shell indicator that renders ai-usagebar's
// 5-hour (session), weekly, and (optionally) extra-usage bars in the top
// panel next to the clock/network, with a native, aligned dropdown.
//
// It shells out to the `ai-usagebar` binary (always exits 0, emits Waybar
// JSON `{text, tooltip, class}`) and draws everything with native St
// widgets. Bar colors and thresholds default to the binary's One Dark
// theme but are user-configurable.

import GObject from 'gi://GObject';
import St from 'gi://St';
import Clutter from 'gi://Clutter';
import GLib from 'gi://GLib';
import Gio from 'gi://Gio';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';

const ROLE = 'ai-usagebar';

// Fixed accent colors (tags / dim text). Bar colors are user-configurable.
const DIM = '#5c6370';
const FG = '#abb2bf';
const RED = '#e06c75';

// All ten fields we pull from the binary, joined by ';;'.
const FORMAT = '{plan};;{session_pct};;{session_reset};;{weekly_pct};;{weekly_reset};;' +
    '{sonnet_pct};;{sonnet_reset};;{extra_pct};;{extra_spent};;{extra_limit}';
const REFRESH_TIMEOUT_SECS = 60;

// severity_for(pct) from src/pango.rs: >=90 critical, >=75 high, >=50 mid, else low.
function colorForPct(pct, colors) {
    if (pct >= 90)
        return colors.critical;
    if (pct >= 75)
        return colors.high;
    if (pct >= 50)
        return colors.mid;
    return colors.low;
}

function esc(s) {
    return String(s)
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;');
}

// Two-segment block bar as Pango markup, `width` cells wide.
function barMarkup(pct, width, colors) {
    const p = Math.max(0, Math.min(100, Math.round(pct)));
    const filled = Math.round((p * width) / 100);
    return `<span foreground="${colorForPct(p, colors)}">${'█'.repeat(filled)}</span>` +
        `<span foreground="${colors.empty}">${'░'.repeat(width - filled)}</span>`;
}

function resolveBinary(settings) {
    const configured = settings.get_string('binary-path');
    if (configured && GLib.file_test(configured, GLib.FileTest.IS_EXECUTABLE))
        return configured;
    const onPath = GLib.find_program_in_path('ai-usagebar');
    if (onPath)
        return onPath;
    const cargo = `${GLib.get_home_dir()}/.cargo/bin/ai-usagebar`;
    if (GLib.file_test(cargo, GLib.FileTest.IS_EXECUTABLE))
        return cargo;
    return 'ai-usagebar';
}

const Indicator = GObject.registerClass(
class AiUsageBarIndicator extends PanelMenu.Button {
    _init(settings, openPrefs) {
        super._init(0.0, 'AI Usage Bar', false);

        this._settings = settings;
        this._openPrefs = openPrefs;
        this._data = null;          // parsed snapshot for redraws
        this._busy = false;
        this._timer = 0;
        this._refreshTimeoutId = 0;
        this._refreshCancellable = null;
        this._refreshProc = null;
        this._refreshToken = 0;
        this._rows = {};

        // Panel: one markup label holds tags + percentages + bars.
        this._label = new St.Label({
            text: '5h …',
            y_align: Clutter.ActorAlign.CENTER,
            style_class: 'aiub-label',
        });
        this.add_child(this._label);

        this._buildMenu();

        // Re-render cached data when any display setting changes (no refetch).
        const viewKeys = [
            'bar-width', 'show-percent', 'show-bars', 'show-session',
            'show-weekly', 'show-extra', 'color-low', 'color-mid',
            'color-high', 'color-critical', 'color-empty',
        ];
        this._viewIds = viewKeys.map(k =>
            this._settings.connect(`changed::${k}`, () => this._render()));

        this._intervalId = this._settings.connect('changed::refresh-interval',
            () => this._restartTimer());
        this._sourceIds = [
            this._settings.connect('changed::vendor', () => this._refresh()),
            this._settings.connect('changed::binary-path', () => this._refresh()),
        ];

        this.menu.connect('open-state-changed', (_m, open) => {
            if (open)
                this._refresh();
        });

        this._refresh();
        this._restartTimer();
    }

    _buildMenu() {
        // Header (plan name).
        const header = new PopupMenu.PopupBaseMenuItem({reactive: false, can_focus: false});
        this._planLabel = new St.Label({text: 'AI Usage', x_expand: true, style_class: 'aiub-header'});
        header.add_child(this._planLabel);
        this.menu.addMenuItem(header);

        this._addRow('session', 'Session');
        this._addRow('weekly', 'Weekly');
        this._addRow('sonnet', 'Sonnet only');
        this._addRow('extra', 'Extra usage');

        this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());

        const refreshItem = new PopupMenu.PopupMenuItem('Atualizar agora');
        refreshItem.connect('activate', () => this._refresh());
        this.menu.addMenuItem(refreshItem);

        const tuiItem = new PopupMenu.PopupMenuItem('Abrir TUI');
        tuiItem.connect('activate', () => this._openTui());
        this.menu.addMenuItem(tuiItem);

        const prefsItem = new PopupMenu.PopupMenuItem('Configurações');
        prefsItem.connect('activate', () => this._openPrefs());
        this.menu.addMenuItem(prefsItem);
    }

    // A native, font-independent row: [name ........ value] / bar / reset.
    _addRow(key, name) {
        const item = new PopupMenu.PopupBaseMenuItem({reactive: false, can_focus: false});
        const vbox = new St.BoxLayout({
            orientation: Clutter.Orientation.VERTICAL,
            x_expand: true,
            style_class: 'aiub-row',
        });

        const head = new St.BoxLayout({x_expand: true});
        const nameL = new St.Label({text: name, x_expand: true, style_class: 'aiub-row-name'});
        const valL = new St.Label({style_class: 'aiub-row-val'});
        head.add_child(nameL);
        head.add_child(valL);

        const barL = new St.Label({style_class: 'aiub-row-bar'});
        const resetL = new St.Label({style_class: 'aiub-row-reset'});

        vbox.add_child(head);
        vbox.add_child(barL);
        vbox.add_child(resetL);
        item.add_child(vbox);
        this.menu.addMenuItem(item);

        this._rows[key] = {item, valL, barL, resetL};
    }

    _colors() {
        const g = k => this._settings.get_string(k);
        return {
            low: g('color-low'),
            mid: g('color-mid'),
            high: g('color-high'),
            critical: g('color-critical'),
            empty: g('color-empty'),
        };
    }

    _restartTimer() {
        if (this._timer) {
            GLib.source_remove(this._timer);
            this._timer = 0;
        }
        const secs = Math.max(5, this._settings.get_int('refresh-interval'));
        this._timer = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, secs, () => {
            this._refresh();
            return GLib.SOURCE_CONTINUE;
        });
    }

    _refresh() {
        if (this._busy)
            return;
        this._busy = true;
        const token = ++this._refreshToken;

        const bin = resolveBinary(this._settings);
        const vendor = this._settings.get_string('vendor') || 'anthropic';
        const argv = [bin, '--vendor', vendor, '--format', FORMAT];
        const cancellable = new Gio.Cancellable();
        this._refreshCancellable = cancellable;

        let proc;
        try {
            proc = new Gio.Subprocess({
                argv,
                flags: Gio.SubprocessFlags.STDOUT_PIPE | Gio.SubprocessFlags.STDERR_PIPE,
            });
            proc.init(cancellable);
        } catch (e) {
            this._busy = false;
            this._refreshCancellable = null;
            this._setError(`não consegui executar "${bin}"`, String(e));
            return;
        }
        this._refreshProc = proc;

        let timedOut = false;
        const timeoutId = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, REFRESH_TIMEOUT_SECS, () => {
            timedOut = true;
            if (this._refreshTimeoutId === timeoutId)
                this._refreshTimeoutId = 0;
            try {
                proc.force_exit();
            } catch (e) {}
            cancellable.cancel();
            if (this._refreshToken === token) {
                this._busy = false;
                this._setError('ai-usagebar demorou demais', `timeout após ${REFRESH_TIMEOUT_SECS}s`);
            }
            return GLib.SOURCE_REMOVE;
        });
        this._refreshTimeoutId = timeoutId;

        const cleanup = () => {
            if (this._refreshTimeoutId === timeoutId) {
                GLib.source_remove(timeoutId);
                this._refreshTimeoutId = 0;
            }
            if (this._refreshCancellable === cancellable)
                this._refreshCancellable = null;
            if (this._refreshProc === proc)
                this._refreshProc = null;
        };

        proc.communicate_utf8_async(null, cancellable, (p, res) => {
            if (this._refreshToken === token)
                this._busy = false;
            try {
                const [, out, err] = p.communicate_utf8_finish(res);
                cleanup();
                if (timedOut)
                    return;
                if ((!out || !out.trim()) && !p.get_successful()) {
                    this._setError('ai-usagebar falhou', err || '');
                    return;
                }
                this._consume(out || '');
            } catch (e) {
                cleanup();
                if (!(e instanceof GLib.Error &&
                      e.matches(Gio.IOErrorEnum, Gio.IOErrorEnum.CANCELLED)) && !timedOut)
                    this._setError('erro ao ler a saída', String(e));
            }
        });
    }

    _consume(stdout) {
        let data;
        try {
            data = JSON.parse(stdout);
        } catch (e) {
            this._setError('saída inválida', stdout);
            return;
        }
        const raw = (data.text ?? '').toString().replace(/<[^>]*>/g, '');
        const f = raw.split(';;');
        if (f.length < 10) {
            // Loading… / ⚠ — show the binary's own text.
            this._data = null;
            this._label.clutter_text.set_markup(`<span foreground="${FG}">${esc(raw) || '…'}</span>`);
            return;
        }
        const isPlaceholder = s => /^\{[^}]+\}$/.test(String(s).trim());
        const field = s => {
            const t = String(s ?? '').trim();
            return t && !isPlaceholder(t) ? t : '';
        };
        const num = s => {
            const t = field(s);
            if (!t)
                return null;
            const n = parseInt(t, 10);
            return Number.isFinite(n) ? n : null;
        };
        this._data = {
            plan: field(f[0]),
            session: {pct: num(f[1]) ?? 0, reset: field(f[2])},
            weekly: {pct: num(f[3]) ?? 0, reset: field(f[4])},
            sonnet: {pct: num(f[5]), reset: field(f[6])},
            extra: {pct: num(f[7]), spent: field(f[8]), limit: field(f[9])},
        };
        this._render();
    }

    // Redraw both the panel and the dropdown from cached data + settings.
    _render() {
        const d = this._data;
        if (!d)
            return;
        const colors = this._colors();
        this._renderPanel(d, colors);
        this._renderDropdown(d, colors);
    }

    _renderPanel(d, colors) {
        const w = Math.max(4, Math.min(20, this._settings.get_int('bar-width')));
        const showPct = this._settings.get_boolean('show-percent');
        const showBars = this._settings.get_boolean('show-bars');

        const seg = (tag, pct, valueText) => {
            const toks = [`<span foreground="${DIM}">${tag}</span>`];
            if (showPct)
                toks.push(`<span foreground="${colorForPct(pct, colors)}">${esc(valueText)}</span>`);
            if (showBars)
                toks.push(barMarkup(pct, w, colors));
            if (!showPct && !showBars) // never render an empty segment
                toks.push(`<span foreground="${colorForPct(pct, colors)}">${esc(valueText)}</span>`);
            return toks.join(' ');
        };

        const parts = [];
        if (this._settings.get_boolean('show-session'))
            parts.push(seg('5h', d.session.pct, `${d.session.pct}%`));
        if (this._settings.get_boolean('show-weekly'))
            parts.push(seg('7d', d.weekly.pct, `${d.weekly.pct}%`));
        if (this._settings.get_boolean('show-extra') &&
            d.extra.pct != null && d.extra.spent && d.extra.limit)
            parts.push(seg('ex', d.extra.pct, d.extra.spent));

        const gap = `<span foreground="${DIM}">   </span>`;
        this._label.clutter_text.set_markup(parts.join(gap) || ' ');
    }

    _renderDropdown(d, colors) {
        this._planLabel.text = d.plan || 'AI Usage';

        const upd = (key, pct, valueText, reset, visible) => {
            const r = this._rows[key];
            r.item.visible = visible;
            if (!visible)
                return;
            r.valL.text = valueText;
            r.barL.clutter_text.set_markup(barMarkup(pct ?? 0, 18, colors));
            if (reset) {
                r.resetL.text = `↺ resets in ${reset}`;
                r.resetL.visible = true;
            } else {
                r.resetL.visible = false;
            }
        };

        upd('session', d.session.pct, `${d.session.pct}%`, d.session.reset, true);
        upd('weekly', d.weekly.pct, `${d.weekly.pct}%`, d.weekly.reset, true);
        upd('sonnet', d.sonnet.pct, `${d.sonnet.pct ?? 0}%`, d.sonnet.reset, d.sonnet.pct != null);
        upd('extra', d.extra.pct, `${d.extra.spent} / ${d.extra.limit}`, null,
            d.extra.pct != null && !!d.extra.spent && !!d.extra.limit);
    }

    _setError(short, detail) {
        this._data = null;
        this._label.clutter_text.set_markup(`<span foreground="${RED}">⚠ ai</span>`);
        const msg = detail ? `${short}\n${esc(detail).slice(0, 300)}` : short;
        this._planLabel.clutter_text.set_markup(`<span foreground="${FG}">${esc(msg)}</span>`);
        for (const r of Object.values(this._rows))
            r.item.visible = false;
    }

    _openTui() {
        const tui = GLib.find_program_in_path('ai-usagebar-tui') ||
            `${GLib.get_home_dir()}/.cargo/bin/ai-usagebar-tui`;
        const candidates = [
            ['kgx', '--', tui],
            ['gnome-terminal', '--', tui],
            ['xterm', '-e', tui],
        ];
        for (const argv of candidates) {
            if (!GLib.find_program_in_path(argv[0]))
                continue;
            try {
                Gio.Subprocess.new(argv, Gio.SubprocessFlags.NONE);
                return;
            } catch (e) {
                // try the next terminal
            }
        }
        Main.notify('AI Usage Bar', 'Nenhum terminal encontrado (kgx / gnome-terminal / xterm).');
    }

    destroy() {
        if (this._timer) {
            GLib.source_remove(this._timer);
            this._timer = 0;
        }
        if (this._refreshTimeoutId) {
            GLib.source_remove(this._refreshTimeoutId);
            this._refreshTimeoutId = 0;
        }
        if (this._refreshCancellable)
            this._refreshCancellable.cancel();
        if (this._refreshProc) {
            try {
                this._refreshProc.force_exit();
            } catch (e) {}
            this._refreshProc = null;
        }
        for (const id of this._viewIds ?? [])
            this._settings.disconnect(id);
        for (const id of this._sourceIds ?? [])
            this._settings.disconnect(id);
        if (this._intervalId)
            this._settings.disconnect(this._intervalId);
        this._viewIds = this._sourceIds = null;
        this._intervalId = 0;
        super.destroy();
    }
});

export default class AiUsageBarExtension extends Extension {
    enable() {
        this._settings = this.getSettings();
        this._place();
        this._placeIds = [
            this._settings.connect('changed::panel-box', () => this._place()),
            this._settings.connect('changed::panel-index', () => this._place()),
        ];
    }

    _place() {
        const existing = Main.panel.statusArea[ROLE];
        if (existing) {
            existing.destroy();
            delete Main.panel.statusArea[ROLE];
        }
        this._indicator = new Indicator(this._settings, () => this.openPreferences());
        const box = this._settings.get_string('panel-box') || 'right';
        const index = Math.max(0, this._settings.get_int('panel-index'));
        Main.panel.addToStatusArea(ROLE, this._indicator, index, box);
    }

    disable() {
        for (const id of this._placeIds ?? [])
            this._settings.disconnect(id);
        this._placeIds = null;
        if (this._indicator) {
            this._indicator.destroy();
            this._indicator = null;
        }
        delete Main.panel.statusArea[ROLE];
        this._settings = null;
    }
}
