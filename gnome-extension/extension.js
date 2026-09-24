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
import {barMarkup, colorForPct, disambiguateTags, field, FIELD, FORMAT, hasUsageWindows, integer,
    isGrouped, markerElapsed, plainTextFromPango, selectPools,
    splitFormatOutput} from './marker-logic.js';
import {parseReport} from './report-model.js';

const ROLE = 'ai-usagebar';

// GNOME 45–46 St.BoxLayout has `vertical`. Later shells replaced it with
// `orientation` and reject the old property. Try the current one first.
function verticalBox(props) {
    try {
        return new St.BoxLayout({orientation: Clutter.Orientation.VERTICAL, ...props});
    } catch (e) {
        return new St.BoxLayout({vertical: true, ...props});
    }
}

// Fixed accent colors (tags / dim text). Bar colors are user-configurable.
const DIM = '#5c6370';
const FG = '#abb2bf';
const RED = '#e06c75';
// FORMAT's final ignored literal sentinel receives a stale suffix, keeping the
// preceding elapsed fields numeric. It and its field indexes live in marker-logic.
const REFRESH_TIMEOUT_SECS = 60;

function esc(s) {
    return String(s)
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;');
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
        this._report = null;
        this._busy = false;
        this._reportBusy = false;
        // A refresh asked for while one was in flight, to run once it settles.
        this._refreshPending = false;
        this._reportPending = false;
        this._timer = 0;
        this._refreshTimeoutId = 0;
        this._reportTimeoutId = 0;
        this._refreshCancellable = null;
        this._reportCancellable = null;
        this._refreshProc = null;
        this._reportProc = null;
        this._refreshToken = 0;
        this._reportToken = 0;

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
            'panel-pools', 'panel-auto-threshold',
        ];
        this._viewIds = viewKeys.map(k =>
            this._settings.connect(`changed::${k}`, () => this._render()));

        this._intervalId = this._settings.connect('changed::refresh-interval',
            () => this._restartTimer());
        this._sourceIds = [
            this._settings.connect('changed::vendor', () => this._refresh()),
            this._settings.connect('changed::binary-path', () => {
                this._refresh();
                this._refreshReport();
            }),
        ];

        this.menu.connect('open-state-changed', (_m, open) => {
            if (open) {
                this._refresh();
                this._refreshReport();
            }
        });

        this._refresh();
        this._refreshReport();
        this._restartTimer();
    }

    _buildMenu() {
        this.menu.removeAll();

        const monitor = Main.layoutManager.primaryMonitor;
        const maxH = Math.max(240, Math.floor(((monitor && monitor.height) || 800) * 0.7));
        const holder = new PopupMenu.PopupBaseMenuItem({reactive: false, can_focus: false});
        this._scroll = new St.ScrollView({
            style_class: 'aiub-scroll',
            overlay_scrollbars: true,
            x_expand: true,
            hscrollbar_policy: St.PolicyType.NEVER,
            vscrollbar_policy: St.PolicyType.AUTOMATIC,
        });
        this._scroll.style = `max-height: ${maxH}px;`;
        this._providers = verticalBox({x_expand: true, style_class: 'aiub-providers'});
        this._scroll.add_child(this._providers);
        holder.add_child(this._scroll);
        this.menu.addMenuItem(holder);

        this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());

        const refreshItem = new PopupMenu.PopupMenuItem('Refresh now');
        refreshItem.connect('activate', () => {
            this._refresh();
            this._refreshReport();
        });
        this.menu.addMenuItem(refreshItem);

        const tuiItem = new PopupMenu.PopupMenuItem('Open TUI');
        tuiItem.connect('activate', () => this._openTui());
        this.menu.addMenuItem(tuiItem);

        const prefsItem = new PopupMenu.PopupMenuItem('Settings');
        prefsItem.connect('activate', () => this._openPrefs());
        this.menu.addMenuItem(prefsItem);

        this._paintReport(this._report);
    }

    _clearProviders() {
        const children = this._providers.get_children();
        for (const child of children)
            child.destroy();
    }

    _metricRow(row, colors) {
        const item = verticalBox({x_expand: true, style_class: 'aiub-row'});
        const head = new St.BoxLayout({x_expand: true});
        head.add_child(new St.Label({
            text: row.label,
            x_expand: true,
            style_class: 'aiub-row-name',
        }));
        const value = new St.Label({text: row.valueText, style_class: 'aiub-row-val'});
        value.clutter_text.set_markup(
            `<span foreground="${colorForPct(row.percent, colors)}">${esc(row.valueText)}</span>`);
        head.add_child(value);
        const bar = new St.Label({style_class: 'aiub-row-bar'});
        bar.clutter_text.set_markup(barMarkup(row.percent, 18, colors, null));
        item.add_child(head);
        item.add_child(bar);
        if (row.reset) {
            item.add_child(new St.Label({
                text: `↺ resets in ${row.reset}`,
                style_class: 'aiub-row-reset',
            }));
        }
        return item;
    }

    _paintReport(report) {
        this._clearProviders();
        const colors = this._colors();
        if (!report || !report.ok) {
            const message = report && report.error ? report.error : 'Loading…';
            this._providers.add_child(new St.Label({
                text: message,
                x_expand: true,
                style_class: 'aiub-header',
            }));
            return;
        }
        if (report.entries.length === 0) {
            this._providers.add_child(new St.Label({
                text: 'No providers enabled',
                x_expand: true,
                style_class: 'aiub-header',
            }));
            return;
        }
        for (const entry of report.entries) {
            const block = verticalBox({x_expand: true, style_class: 'aiub-provider'});
            const title = entry.stale ? `${entry.title} · cached` : entry.title;
            block.add_child(new St.Label({
                text: title,
                x_expand: true,
                style_class: 'aiub-provider-title',
            }));
            if (entry.plan) {
                block.add_child(new St.Label({
                    text: entry.plan,
                    x_expand: true,
                    style_class: 'aiub-provider-plan',
                }));
            }
            if (entry.error) {
                block.add_child(new St.Label({
                    text: entry.error,
                    x_expand: true,
                    style_class: 'aiub-row-reset',
                }));
            }
            for (const row of entry.rows) {
                if (row.type === 'group') {
                    block.add_child(new St.Label({
                        text: row.label,
                        x_expand: true,
                        style_class: 'aiub-header',
                    }));
                } else if (row.type === 'metric') {
                    block.add_child(this._metricRow(row, colors));
                } else if (row.type === 'text') {
                    const line = new St.BoxLayout({x_expand: true, style_class: 'aiub-row'});
                    line.add_child(new St.Label({
                        text: row.label,
                        x_expand: true,
                        style_class: 'aiub-row-name',
                    }));
                    line.add_child(new St.Label({text: row.value, style_class: 'aiub-row-val'}));
                    block.add_child(line);
                } else if (row.type === 'block') {
                    if (row.label) {
                        block.add_child(new St.Label({
                            text: row.label,
                            x_expand: true,
                            style_class: 'aiub-header',
                        }));
                    }
                    for (const line of row.body) {
                        block.add_child(new St.Label({
                            text: line,
                            x_expand: true,
                            style_class: 'aiub-row-reset',
                        }));
                    }
                }
            }
            this._providers.add_child(block);
        }
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
            this._refreshReport();
            return GLib.SOURCE_CONTINUE;
        });
    }

    _refresh() {
        // Dropping the request while busy meant a vendor change *during* a
        // fetch never started one for the new vendor: the in-flight result for
        // the OLD vendor was applied and stayed on the panel until the next
        // timer tick. Remember that a refresh was asked for and run it as soon
        // as the current one settles.
        if (this._busy) {
            this._refreshPending = true;
            return;
        }
        this._busy = true;
        const token = ++this._refreshToken;

        const bin = resolveBinary(this._settings);
        // Captured for THIS attempt: the setting can change while we wait, and
        // a late result must not be rendered as if it belonged to the vendor
        // now selected.
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
            this._refreshPending = false;
            this._setError(`could not run "${bin}"`, String(e));
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
                this._setError('ai-usagebar took too long', `timed out after ${REFRESH_TIMEOUT_SECS}s`);
                // Do not strand a request that arrived while this one hung.
                if (this._refreshPending) {
                    this._refreshPending = false;
                    this._refresh();
                }
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
            const current = this._refreshToken === token;
            if (current)
                this._busy = false;
            try {
                const [, out, err] = p.communicate_utf8_finish(res);
                cleanup();
                if (timedOut)
                    return;
                // A superseded attempt must not paint the panel: its numbers
                // belong to whatever vendor was selected when it started.
                if (!current)
                    return;
                // The selection may have changed while this ran even without a
                // newer attempt (the change is queued as `_refreshPending`).
                if ((this._settings.get_string('vendor') || 'anthropic') !== vendor)
                    return;
                if ((!out || !out.trim()) && !p.get_successful()) {
                    this._setError('ai-usagebar falhou', err || '');
                    return;
                }
                this._consume(out || '');
            } catch (e) {
                cleanup();
                if (current && !(e instanceof GLib.Error &&
                      e.matches(Gio.IOErrorEnum, Gio.IOErrorEnum.CANCELLED)) && !timedOut)
                    this._setError('could not read the output', String(e));
            } finally {
                // Run whatever was requested while we were busy.
                if (current && this._refreshPending) {
                    this._refreshPending = false;
                    this._refresh();
                }
            }
        });
    }

    _consume(stdout) {
        let data;
        try {
            data = JSON.parse(stdout);
        } catch (e) {
            this._setError('invalid output', stdout);
            return;
        }
        const raw = plainTextFromPango(data.text);
        const f = splitFormatOutput(raw);
        if (f.length <= FIELD.extraLimit) {
            // Loading… / ⚠ — show the binary's own text.
            this._data = null;
            this._label.clutter_text.set_markup(`<span foreground="${FG}">${esc(raw) || '…'}</span>`);
            return;
        }
        this._data = {
            plan: field(f[FIELD.plan]),
            hasUsageWindows: hasUsageWindows(f[FIELD.vendorShort]),
            grouped: isGrouped(f[FIELD.sessionModel]),
            session: {pct: integer(f[FIELD.sessionPct]), reset: field(f[FIELD.sessionReset]),
                model: field(f[FIELD.sessionModel]),
                elapsed: markerElapsed(field(f[FIELD.sessionReset]), integer(f[FIELD.sessionElapsed]))},
            weekly: {pct: integer(f[FIELD.weeklyPct]), reset: field(f[FIELD.weeklyReset]),
                model: field(f[FIELD.weeklyModel]),
                elapsed: markerElapsed(field(f[FIELD.weeklyReset]), integer(f[FIELD.weeklyElapsed]))},
            // Per-model weekly bar: a non-empty scoped model is the presence
            // signal. A reset may be unavailable, which must not make us show
            // the unrelated legacy Sonnet window instead.
            sonnet: (() => {
                const scopedModel = field(f[FIELD.scopedModel]);
                if (scopedModel) {
                    const scopedPct = integer(f[FIELD.scopedPct]);
                    if (scopedPct != null && scopedPct >= 0 && scopedPct <= 100)
                        return {pct: scopedPct, reset: field(f[FIELD.scopedReset]) || '—',
                            model: scopedModel, label: scopedModel,
                            elapsed: markerElapsed(field(f[FIELD.scopedReset]), integer(f[FIELD.scopedElapsed]))};
                    // A scoped model with malformed data is unavailable; do
                    // not fall back to a potentially unrelated Sonnet window.
                    return {pct: null, reset: '—', model: scopedModel, label: scopedModel, elapsed: null};
                }
                return {pct: integer(f[FIELD.sonnetPct]), reset: field(f[FIELD.sonnetReset]),
                    model: '', label: 'Sonnet only', elapsed: null};
            })(),
            // A named extra window (model + reset) renders as a percentage bar;
            // without a name the slot stays a spent/limit money budget.
            extra: {pct: integer(f[FIELD.extraPct]), spent: field(f[FIELD.extraSpent]),
                limit: field(f[FIELD.extraLimit]), model: field(f[FIELD.extraModel]),
                reset: field(f[FIELD.extraReset]),
                elapsed: markerElapsed(field(f[FIELD.extraReset]), integer(f[FIELD.extraElapsed]))},
        };
        this._render();
    }

    // Redraw both the panel and the dropdown from cached data + settings.
    _render() {
        if (this._data)
            this._renderPanel(this._data, this._colors());
        if (this._report)
            this._paintReport(this._report);
    }

    _renderPanel(d, colors) {
        const w = Math.max(4, Math.min(20, this._settings.get_int('bar-width')));
        const showPct = this._settings.get_boolean('show-percent');
        const showBars = this._settings.get_boolean('show-bars');

        const seg = (tag, pct, valueText, elapsed) => {
            const toks = [`<span foreground="${DIM}">${tag}</span>`];
            if (showPct)
                toks.push(`<span foreground="${colorForPct(pct, colors)}">${esc(valueText)}</span>`);
            if (showBars)
                toks.push(barMarkup(pct, w, colors, elapsed));
            if (!showPct && !showBars) // never render an empty segment
                toks.push(`<span foreground="${colorForPct(pct, colors)}">${esc(valueText)}</span>`);
            return toks.join(' ');
        };

        const showSession = this._settings.get_boolean('show-session');
        const showWeekly = this._settings.get_boolean('show-weekly');
        const parts = [];

        if (d.grouped) {
            // Two independent pools. panel-pools picks the pools, show-session /
            // show-weekly still pick the windows, so segments are pools ×
            // windows and "just the 5h of both" needs no mode of its own.
            for (const pool of this._selectedPools(d, showSession, showWeekly)) {
                if (showSession && pool.session.pct != null) {
                    parts.push(seg(`${pool.tag} 5h`, pool.session.pct,
                        `${pool.session.pct}%`, pool.session.elapsed));
                }
                if (showWeekly && pool.weekly.pct != null) {
                    parts.push(seg(`${pool.tag} 7d`, pool.weekly.pct,
                        `${pool.weekly.pct}%`, pool.weekly.elapsed));
                }
            }
        } else {
            if (d.hasUsageWindows && showSession && d.session.pct != null)
                parts.push(seg('5h', d.session.pct, `${d.session.pct}%`, d.session.elapsed));
            if (d.hasUsageWindows && showWeekly && d.weekly.pct != null)
                parts.push(seg('7d', d.weekly.pct, `${d.weekly.pct}%`, d.weekly.elapsed));
            if (this._settings.get_boolean('show-extra') &&
                d.extra.pct != null && d.extra.spent && d.extra.limit)
                parts.push(seg('ex', d.extra.pct, d.extra.spent, null)); // $ budget → no meta
        }

        const gap = `<span foreground="${DIM}">   </span>`;
        this._label.clutter_text.set_markup(parts.join(gap) || ' ');
    }

    // The pools the panel should draw, tagged and in display order. Primary is
    // the generic session/weekly pair; secondary reuses the scoped and extra
    // slots, which for a grouped vendor hold the second pool's two windows.
    _selectedPools(d, showSession, showWeekly) {
        // Either secondary window may be absent. Derive its tag from whichever
        // model-bearing slot exists instead of assuming the weekly one does.
        const secondaryModel = d.sonnet.model || d.extra.model;
        const [primaryTag, secondaryTag] = disambiguateTags(d.session.model, secondaryModel);
        const primary = {tag: primaryTag, session: d.session, weekly: d.weekly};
        const secondary = {tag: secondaryTag, session: d.sonnet, weekly: d.extra};
        const pct = pool => ({
            session: pool.session.pct,
            weekly: pool.weekly.pct,
        });
        const pools = {primary, secondary};
        return selectPools(pct(primary), pct(secondary),
            this._settings.get_string('panel-pools'),
            this._settings.get_int('panel-auto-threshold'),
            {session: showSession, weekly: showWeekly})
            .map(name => pools[name]);
    }

    _setError(short, detail) {
        this._data = null;
        const msg = detail ? `${short}: ${detail}` : short;
        this._label.clutter_text.set_markup(
            `<span foreground="${RED}">⚠ ${esc(msg).slice(0, 80)}</span>`);
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
        Main.notify('AI Usage Bar', 'No terminal found (kgx / gnome-terminal / xterm).');
    }

    _refreshReport() {
        if (this._reportBusy) {
            this._reportPending = true;
            return;
        }
        this._reportBusy = true;
        const token = ++this._reportToken;
        const bin = resolveBinary(this._settings);
        const cancellable = new Gio.Cancellable();
        this._reportCancellable = cancellable;

        let proc;
        try {
            proc = new Gio.Subprocess({
                argv: [bin, 'usage', '--json'],
                flags: Gio.SubprocessFlags.STDOUT_PIPE | Gio.SubprocessFlags.STDERR_PIPE,
            });
            proc.init(cancellable);
        } catch (e) {
            this._reportBusy = false;
            this._reportCancellable = null;
            this._reportPending = false;
            this._report = {ok: false, error: `could not run "${bin}"`, entries: []};
            this._paintReport(this._report);
            return;
        }
        this._reportProc = proc;

        let timedOut = false;
        const timeoutId = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, REFRESH_TIMEOUT_SECS, () => {
            timedOut = true;
            if (this._reportTimeoutId === timeoutId)
                this._reportTimeoutId = 0;
            try {
                proc.force_exit();
            } catch (e) {}
            cancellable.cancel();
            if (this._reportToken === token) {
                this._reportBusy = false;
                this._report = {ok: false, error: 'ai-usagebar took too long', entries: []};
                this._paintReport(this._report);
                if (this._reportPending) {
                    this._reportPending = false;
                    this._refreshReport();
                }
            }
            return GLib.SOURCE_REMOVE;
        });
        this._reportTimeoutId = timeoutId;

        const cleanup = () => {
            if (this._reportTimeoutId === timeoutId) {
                GLib.source_remove(timeoutId);
                this._reportTimeoutId = 0;
            }
            if (this._reportCancellable === cancellable)
                this._reportCancellable = null;
            if (this._reportProc === proc)
                this._reportProc = null;
        };

        proc.communicate_utf8_async(null, cancellable, (p, res) => {
            const current = this._reportToken === token;
            if (current)
                this._reportBusy = false;
            try {
                const [, out, err] = p.communicate_utf8_finish(res);
                cleanup();
                if (timedOut || !current)
                    return;
                if ((!out || !out.trim()) && !p.get_successful()) {
                    this._report = {ok: false, error: err || 'ai-usagebar failed', entries: []};
                } else {
                    this._report = parseReport(out || '');
                }
                this._paintReport(this._report);
            } catch (e) {
                cleanup();
                if (current && !(e instanceof GLib.Error &&
                      e.matches(Gio.IOErrorEnum, Gio.IOErrorEnum.CANCELLED)) && !timedOut) {
                    this._report = {ok: false, error: 'could not read the output', entries: []};
                    this._paintReport(this._report);
                }
            } finally {
                if (current && this._reportPending) {
                    this._reportPending = false;
                    this._refreshReport();
                }
            }
        });
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
        if (this._reportTimeoutId) {
            GLib.source_remove(this._reportTimeoutId);
            this._reportTimeoutId = 0;
        }
        if (this._reportCancellable)
            this._reportCancellable.cancel();
        if (this._reportProc) {
            try {
                this._reportProc.force_exit();
            } catch (e) {}
            this._reportProc = null;
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
