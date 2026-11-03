// AI Usage Bar — GNOME Shell indicator that renders ai-usagebar's
// 5-hour (session), weekly, and (optionally) extra-usage bars in the top
// panel next to the clock/network, with a native, aligned dropdown.
//
// The top bar reads the widget's Waybar JSON; provider submenus read
// `ai-usagebar usage --json`. Both commands run asynchronously, and the UI
// uses native St widgets. Bar colors are user-configurable.

import GObject from 'gi://GObject';
import St from 'gi://St';
import Clutter from 'gi://Clutter';
import GLib from 'gi://GLib';
import Gio from 'gi://Gio';
import Pango from 'gi://Pango';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';
import {barMarkup, colorForDelta, colorForPct, disambiguateTags, field, FIELD, FORMAT, hasUsageWindows, integer,
    isGrouped, MARKER, markerElapsed, paceGlyph, plainTextFromPango, selectPools,
    splitFormatOutput} from './marker-logic.js';
import {commandFailure, errorLine, parseReport} from './report-model.js';
import {terminalCandidates} from './terminal.js';

const ROLE = 'ai-usagebar';

// Newer shells gave St.BoxLayout an `orientation` property and deprecated
// `vertical`. Shell 46 has only `vertical`, and GJS throws on a property the
// class does not have, so a hardcoded `orientation` kept the extension from
// loading there. Every vertical box goes through here; a contract test keeps
// it that way.
const HAS_ORIENTATION = !!GObject.Object.find_property.call(St.BoxLayout, 'orientation');

function verticalBox(props) {
    return new St.BoxLayout(HAS_ORIENTATION
        ? {orientation: Clutter.Orientation.VERTICAL, ...props}
        : {vertical: true, ...props});
}

// Report text (errors, block lines, details) has no length the menu can
// predict. Unwrapped, one long line sets the whole menu's width, past the edge
// of the screen; wrapped, it stays inside the menu's bounded width.
function wrappedLabel(text, styleClass) {
    const label = new St.Label({text, x_expand: true, style_class: styleClass});
    label.clutter_text.line_wrap = true;
    label.clutter_text.line_wrap_mode = Pango.WrapMode.WORD_CHAR;
    label.clutter_text.ellipsize = Pango.EllipsizeMode.NONE;
    return label;
}

// The detail bar fits inside the menu's bounded content width.
const PANEL_BAR_W = 320;

// A bar built from St widgets rather than █ cells: a track, a fill in the
// severity color, and an optional pace marker at the elapsed share of the
// window.
function barWidget(percent, width, height, color, elapsed) {
    const track = new St.Widget({
        style_class: 'aiub-track',
        width,
        height,
        y_align: Clutter.ActorAlign.CENTER,
    });
    if (percent > 0) {
        track.add_child(new St.Widget({
            width: Math.max(height, Math.round(width * percent / 100)),
            height,
            style: `background-color: ${color}; border-radius: ${height / 2}px;`,
        }));
    }
    if (Number.isFinite(elapsed)) {
        track.add_child(new St.Widget({
            x: Math.min(width - 2, Math.max(0, Math.round(width * elapsed / 100) - 1)),
            y: -3,
            width: 2,
            height: height + 6,
            style: `background-color: ${MARKER}; border-radius: 1px;`,
        }));
    }
    return track;
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

// One subprocess slot: at most one run in flight, a request made meanwhile
// remembered and run once it settles, and a superseded or timed-out run never
// painted. See AiUsageBarIndicator._run.
function newJob() {
    return {busy: false, pending: false, token: 0, timeoutId: 0, cancellable: null, proc: null};
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
    _init(settings, openPrefs, iconDir, vendor) {
        super._init(0.0, 'AI Usage Bar', false);

        this._settings = settings;
        this._openPrefs = openPrefs;
        this._vendor = vendor;
        this._iconDir = iconDir;
        this._marks = new Map();
        this._data = null;          // parsed snapshot for redraws
        this._report = null;        // parsed `usage --json` for the menu
        this._panelError = '';      // why the top bar shows ⚠, shown in the menu
        this._tab = null;           // selected provider tab (entry id)
        this._reportAt = 0;
        this._destroyed = false;
        this._timer = 0;
        // The top bar (`--vendor --format`) and the menu (`usage --json`) are
        // separate commands on separate schedules; each gets its own slot.
        this._panelJob = newJob();
        this._reportJob = newJob();

        // Panel: one markup label holds tags + percentages + bars. The icon
        // stands in when the settings leave nothing for the label to draw,
        // so the indicator never collapses into an empty click target.
        this._label = new St.Label({
            text: '5h …',
            y_align: Clutter.ActorAlign.CENTER,
            style_class: 'aiub-label',
        });
        this._icon = new St.Icon({
            style_class: 'system-status-icon',
            y_align: Clutter.ActorAlign.CENTER,
            visible: false,
        });
        const panelBox = new St.BoxLayout({style_class: 'panel-status-menu-box'});
        // With several indicators side by side, the brand mark ahead of the
        // bars tells them apart.
        const mark = this._markIcon(vendor);
        if (mark) {
            panelBox.add_child(new St.Icon({gicon: mark, style_class: 'system-status-icon',
                y_align: Clutter.ActorAlign.CENTER}));
        }
        panelBox.add_child(this._icon);
        panelBox.add_child(this._label);
        this.add_child(panelBox);

        this._buildMenu();

        // Re-render cached data when any display setting changes (no refetch).
        const viewKeys = [
            'bar-width', 'show-percent', 'show-bars', 'show-pace', 'show-session',
            'show-weekly', 'show-extra', 'color-low', 'color-mid',
            'color-high', 'color-critical', 'color-empty',
            'panel-pools', 'panel-auto-threshold',
            'menu-summary-style', 'menu-show-icons', 'menu-compact',
        ];
        this._viewIds = viewKeys.map(k =>
            this._settings.connect(`changed::${k}`, () => this._render()));

        this._intervalId = this._settings.connect('changed::refresh-interval',
            () => this._restartTimer());
        this._sourceIds = [
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
        this._restartTimer();
    }

    _buildMenu() {
        // One non-reactive row hosts the whole tabbed panel: header, provider
        // tabs, the selected provider's metrics, and a footer.
        this._panel = verticalBox({style_class: 'aiub-panel', x_expand: true});
        const host = new PopupMenu.PopupBaseMenuItem({reactive: false, can_focus: false});
        host.add_child(this._panel);
        this.menu.addMenuItem(host);
        this.menu.addAction('Open TUI', () => this._openTui());
        this._paintReport(null);
    }

    _markIcon(brand) {
        if (!brand)
            return null;
        if (!this._marks.has(brand)) {
            const file = this._iconDir.get_child(`${brand}-symbolic.svg`);
            this._marks.set(brand, file.query_exists(null) ? new Gio.FileIcon({file}) : null);
        }
        return this._marks.get(brand);
    }

    _iconButton(iconName, label, onClick) {
        const button = new St.Button({
            style_class: 'aiub-icon-button',
            can_focus: true,
            accessible_name: label,
            child: new St.Icon({icon_name: iconName, icon_size: 16}),
        });
        button.connect('clicked', onClick);
        return button;
    }

    _header(entry) {
        const header = new St.BoxLayout({x_expand: true, style_class: 'aiub-header'});
        const mark = this._settings.get_boolean('menu-show-icons')
            ? this._markIcon(entry.brand)
            : null;
        if (mark)
            header.add_child(new St.Icon({gicon: mark, icon_size: 28, style_class: 'aiub-header-icon',
                y_align: Clutter.ActorAlign.CENTER}));
        const titles = verticalBox({x_expand: true, y_align: Clutter.ActorAlign.CENTER});
        titles.add_child(new St.Label({text: entry.title, style_class: 'aiub-title'}));
        if (entry.plan)
            titles.add_child(new St.Label({text: entry.plan.toUpperCase(), style_class: 'aiub-plan'}));
        header.add_child(titles);
        const pcts = entry.rows.filter(r => r.type === 'metric' && Number.isFinite(r.percent)).map(r => r.percent);
        if (pcts.length) {
            header.add_child(new St.Label({text: `${Math.round(Math.max(...pcts))}%`, style_class: 'aiub-pill',
                y_align: Clutter.ActorAlign.CENTER}));
        }
        header.add_child(this._iconButton('view-refresh-symbolic', 'Refresh now', () => {
            this._refresh();
            this._refreshReport();
        }));
        header.add_child(this._iconButton('emblem-system-symbolic', 'Settings', () => {
            this.menu.close();
            this._openPrefs();
        }));
        return header;
    }

    _tabs(entries, selectedId) {
        const tabs = new St.BoxLayout({x_expand: true, style_class: 'aiub-tabs'});
        for (const e of entries) {
            const tab = new St.Button({
                label: e.title,
                can_focus: true,
                style_class: 'aiub-tab' + (e.id === selectedId ? ' aiub-tab-active' : ''),
            });
            tab.connect('clicked', () => {
                this._tab = e.id;
                this._paintReport(this._report);
            });
            tabs.add_child(tab);
        }
        return new St.ScrollView({
            hscrollbar_policy: St.PolicyType.AUTOMATIC,
            vscrollbar_policy: St.PolicyType.NEVER,
            overlay_scrollbars: true,
            child: tabs,
        });
    }

    _entryBody(entry, colors) {
        const body = verticalBox({x_expand: true, style_class: 'aiub-body'});
        body.add_child(new St.Label({text: 'USAGE & BALANCE', style_class: 'aiub-section'}));
        if (entry.error)
            body.add_child(wrappedLabel(entry.error, 'aiub-detail'));
        for (const row of entry.rows) {
            if (row.type === 'metric') {
                const box = verticalBox({x_expand: true, style_class: 'aiub-metric'});
                const top = new St.BoxLayout({x_expand: true});
                top.add_child(new St.Label({text: row.label, x_expand: true, style_class: 'aiub-heading'}));
                top.add_child(new St.Label({text: row.valueText, style_class: 'aiub-value'}));
                box.add_child(top);
                if (this._settings.get_string('menu-summary-style') === 'bars') {
                    box.add_child(barWidget(row.percent, PANEL_BAR_W, 5,
                        colors[row.severity] || colors.low, row.elapsed));
                }
                const reset = row.reset === 'now'
                    ? 'Resets now'
                    : row.reset
                        ? (row.regen ? `Regenerates ${row.regen} in ${row.reset}` : `Resets in ${row.reset}`)
                        : '';
                const detail = [reset, row.detail].filter(Boolean).join(' · ');
                if (detail)
                    box.add_child(wrappedLabel(detail, 'aiub-detail'));
                body.add_child(box);
            } else if (row.type === 'block') {
                if (row.label)
                    body.add_child(wrappedLabel(row.label, 'aiub-heading'));
                for (const line of row.body)
                    body.add_child(wrappedLabel(line, 'aiub-detail'));
            } else {
                const text = row.value ? `${row.label ? row.label + ': ' : ''}${row.value}` : row.label;
                body.add_child(wrappedLabel(text, row.value ? 'aiub-detail' : 'aiub-heading'));
            }
        }
        if (!entry.error && entry.rows.length === 0)
            body.add_child(wrappedLabel('No usage data reported', 'aiub-detail'));
        return body;
    }

    _updatedAgo() {
        if (!this._reportAt)
            return '';
        const mins = Math.floor((Date.now() - this._reportAt) / 60000);
        return `Updated ${mins < 1 ? 'just now' : `${mins}m ago`}`;
    }

    _paintReport(report) {
        const panel = this._panel;
        panel.set_style_class_name('aiub-panel' +
            (this._settings.get_boolean('menu-compact') ? ' aiub-compact' : ''));
        panel.destroy_all_children();
        if (this._panelError)
            panel.add_child(wrappedLabel(`Top bar: ${this._panelError}`, 'aiub-detail'));
        if (!report?.ok || report.entries.length === 0) {
            panel.add_child(wrappedLabel(!report ? 'Loading…' : report.ok ? 'No providers enabled' : report.error,
                'aiub-detail'));
            return;
        }
        const entries = report.entries;
        const entry = entries.find(e => e.id === this._tab) || entries[0];
        this._tab = entry.id;
        panel.add_child(this._header(entry));
        if (entries.length > 1)
            panel.add_child(this._tabs(entries, entry.id));
        panel.add_child(this._entryBody(entry, this._colors()));
        const ago = this._updatedAgo();
        if (ago)
            panel.add_child(new St.Label({text: ago, style_class: 'aiub-footer',
                x_align: Clutter.ActorAlign.CENTER}));
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
            if (this.menu.isOpen)
                this._refreshReport();
            return GLib.SOURCE_CONTINUE;
        });
    }

    // Spawn `argv` in `job`'s slot (see newJob). `handlers.done(out, err, ok)`
    // receives a finished, current run's output; `handlers.failed(short,
    // detail)` a spawn failure, a timeout, or output that could not be read;
    // `handlers.again()` re-requests a run that was asked for while busy.
    _run(job, argv, handlers) {
        if (this._destroyed)
            return;
        if (job.busy) {
            job.pending = true;
            return;
        }
        job.busy = true;
        const token = ++job.token;
        const cancellable = new Gio.Cancellable();
        job.cancellable = cancellable;
        // Run whatever was requested while we were busy — never after
        // destroy, where it would spawn into a torn-down indicator.
        const again = () => {
            if (job.pending && !this._destroyed) {
                job.pending = false;
                handlers.again();
            }
        };

        let proc;
        try {
            proc = new Gio.Subprocess({
                argv,
                flags: Gio.SubprocessFlags.STDOUT_PIPE | Gio.SubprocessFlags.STDERR_PIPE,
            });
            proc.init(cancellable);
        } catch (e) {
            job.busy = false;
            job.cancellable = null;
            job.pending = false;
            handlers.failed(`could not run "${argv[0]}"`, String(e));
            return;
        }
        job.proc = proc;

        let timedOut = false;
        const timeoutId = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, REFRESH_TIMEOUT_SECS, () => {
            timedOut = true;
            if (job.timeoutId === timeoutId)
                job.timeoutId = 0;
            try {
                proc.force_exit();
            } catch (e) {}
            cancellable.cancel();
            if (job.token === token) {
                job.busy = false;
                handlers.failed('ai-usagebar took too long', `timed out after ${REFRESH_TIMEOUT_SECS}s`);
                // Do not strand a request that arrived while this one hung.
                again();
            }
            return GLib.SOURCE_REMOVE;
        });
        job.timeoutId = timeoutId;

        const cleanup = () => {
            if (job.timeoutId === timeoutId) {
                GLib.source_remove(timeoutId);
                job.timeoutId = 0;
            }
            if (job.cancellable === cancellable)
                job.cancellable = null;
            if (job.proc === proc)
                job.proc = null;
        };

        proc.communicate_utf8_async(null, cancellable, (p, res) => {
            // A superseded attempt must not paint: its output belongs to
            // whatever was selected when it started.
            const current = job.token === token && !this._destroyed;
            if (current)
                job.busy = false;
            try {
                const [, out, err] = p.communicate_utf8_finish(res);
                cleanup();
                if (timedOut || !current)
                    return;
                handlers.done(out || '', err || '', p.get_successful());
            } catch (e) {
                cleanup();
                if (current && !(e instanceof GLib.Error &&
                      e.matches(Gio.IOErrorEnum, Gio.IOErrorEnum.CANCELLED)) && !timedOut)
                    handlers.failed('could not read the output', String(e));
            } finally {
                if (current)
                    again();
            }
        });
    }

    _stop(job) {
        job.pending = false;
        if (job.timeoutId) {
            GLib.source_remove(job.timeoutId);
            job.timeoutId = 0;
        }
        if (job.cancellable)
            job.cancellable.cancel();
        if (job.proc) {
            try {
                job.proc.force_exit();
            } catch (e) {}
            job.proc = null;
        }
    }

    _refresh() {
        // The vendor is fixed per indicator; a change to the `vendor` setting
        // rebuilds the indicators instead (AiUsageBarExtension._place).
        // `_run` remembers a refresh asked for while busy and runs it as soon
        // as the current one settles.
        const argv = [resolveBinary(this._settings), '--vendor', this._vendor, '--format', FORMAT];
        this._run(this._panelJob, argv, {
            again: () => this._refresh(),
            failed: (short, detail) => this._setError(short, detail),
            done: (out, err, ok) => {
                if (!out.trim() && !ok) {
                    this._setError('ai-usagebar failed', err);
                    return;
                }
                this._consume(out);
            },
        });
    }

    _refreshReport() {
        const argv = [resolveBinary(this._settings), 'usage', '--json'];
        this._run(this._reportJob, argv, {
            again: () => this._refreshReport(),
            failed: (short, detail) =>
                this._showReport({ok: false, error: errorLine(short, detail), entries: []}),
            done: (out, err, ok) =>
                this._showReport(!out.trim() && !ok ? commandFailure(err) : parseReport(out)),
        });
    }

    _showReport(report) {
        this._report = report;
        if (report?.ok)
            this._reportAt = Date.now();
        this._paintReport(report);
    }

    _consume(stdout) {
        let data;
        try {
            data = JSON.parse(stdout);
        } catch (e) {
            this._setError('invalid output', stdout);
            return;
        }
        // The command answered, so a reason left over from a failed run no
        // longer describes the top bar.
        if (this._panelError) {
            this._panelError = '';
            this._paintReport(this._report);
        }
        const raw = plainTextFromPango(data.text);
        const f = splitFormatOutput(raw);
        if (f.length <= FIELD.extraLimit) {
            // Loading… / ⚠ — show the binary's own text.
            this._data = null;
            this._setPanelMarkup(`<span foreground="${FG}">${esc(raw) || '…'}</span>`);
            return;
        }
        // Only what the top bar draws. The click menu reads `usage --json`.
        this._data = {
            hasUsageWindows: hasUsageWindows(f[FIELD.vendorShort]),
            grouped: isGrouped(f[FIELD.sessionModel]),
            session: {pct: integer(f[FIELD.sessionPct]),
                model: field(f[FIELD.sessionModel]),
                elapsed: markerElapsed(field(f[FIELD.sessionReset]), integer(f[FIELD.sessionElapsed]))},
            weekly: {pct: integer(f[FIELD.weeklyPct]),
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
                        return {pct: scopedPct, model: scopedModel,
                            elapsed: markerElapsed(field(f[FIELD.scopedReset]), integer(f[FIELD.scopedElapsed]))};
                    // A scoped model with malformed data is unavailable; do
                    // not fall back to a potentially unrelated Sonnet window.
                    return {pct: null, model: scopedModel, elapsed: null};
                }
                return {pct: integer(f[FIELD.sonnetPct]), model: '', elapsed: null};
            })(),
            // A named extra window (model + reset) renders as a percentage bar;
            // without a name the slot stays a spent/limit money budget.
            extra: {pct: integer(f[FIELD.extraPct]), spent: field(f[FIELD.extraSpent]),
                limit: field(f[FIELD.extraLimit]), model: field(f[FIELD.extraModel]),
                elapsed: markerElapsed(field(f[FIELD.extraReset]), integer(f[FIELD.extraElapsed]))},
        };
        this._render();
    }

    // Redraw both the panel and the dropdown from cached data + settings.
    _render() {
        if (this._data)
            this._renderPanel(this._data, this._colors());
        this._paintReport(this._report);
    }

    _renderPanel(d, colors) {
        const w = Math.max(4, Math.min(20, this._settings.get_int('bar-width')));
        const showPct = this._settings.get_boolean('show-percent');
        const showBars = this._settings.get_boolean('show-bars');
        const showPace = !showBars && this._settings.get_boolean('show-pace');

        const seg = (tag, pct, valueText, elapsed) => {
            // Pace mode: without bars there is no pace marker, so the value
            // carries the pace instead, colored by usage minus elapsed, with
            // its arrow.
            const paced = showPace && Number.isFinite(elapsed);
            const color = paced ? colorForDelta(pct - elapsed, colors) : colorForPct(pct, colors);
            const text = paced ? `${valueText}${paceGlyph(pct - elapsed)}` : valueText;
            const toks = [`<span foreground="${DIM}">${tag}</span>`];
            if (showPct || !showBars) // never render an empty segment
                toks.push(`<span foreground="${color}">${esc(text)}</span>`);
            if (showBars)
                toks.push(barMarkup(pct, w, colors, elapsed));
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
        this._setPanelMarkup(parts.join(gap));
    }

    // Every top-bar write goes through here. Empty markup, e.g. both windows
    // switched off, hides the label; what stays visible is decided below.
    _setPanelMarkup(markup) {
        const empty = !markup;
        // The brand mark is already prepended in _init, so assigning it to
        // _icon as well would draw the same mark twice. The icon slot is the
        // stand-in — keeping the indicator from collapsing — only when the
        // vendor has no mark file, falling back to the generic icon.
        const standIn = empty && !this._markIcon(this._vendor);
        if (standIn)
            this._icon.icon_name = 'application-x-executable-symbolic';
        this._icon.visible = standIn;
        this._label.visible = !empty;
        this._label.clutter_text.set_markup(markup);
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
        // The top bar stays a compact ⚠; the reason is the menu's first line.
        this._setPanelMarkup(`<span foreground="${RED}">⚠ ai</span>`);
        this._panelError = errorLine(short, detail);
        this._paintReport(this._report);
    }

    _openTui() {
        const tui = GLib.find_program_in_path('ai-usagebar-tui') ||
            `${GLib.get_home_dir()}/.cargo/bin/ai-usagebar-tui`;
        const line = `export PATH="$HOME/.local/bin:$PATH" && ${GLib.shell_quote(tui)}`;
        for (const argv of terminalCandidates(line)) {
            if (!GLib.find_program_in_path(argv[0]))
                continue;
            try {
                Gio.Subprocess.new(argv, Gio.SubprocessFlags.NONE);
                return;
            } catch (e) {
                // try the next terminal
            }
        }
        Main.notify('AI Usage Bar', 'No terminal found (ptyxis / kgx / gnome-terminal / xterm).');
    }

    destroy() {
        this._destroyed = true;
        if (this._timer) {
            GLib.source_remove(this._timer);
            this._timer = 0;
        }
        this._stop(this._panelJob);
        this._stop(this._reportJob);
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
        this._indicators = [];
        this._place();
        this._placeIds = [
            this._settings.connect('changed::panel-box', () => this._place()),
            this._settings.connect('changed::panel-index', () => this._place()),
            this._settings.connect('changed::vendor', () => this._place()),
        ];
    }

    // `vendor` takes a comma-separated list ("anthropic,openai"): one
    // indicator per provider, side by side in the order given. An empty list
    // (every vendor switch off in prefs) means NO indicator — the panel must
    // agree with prefs rather than silently defaulting to one provider.
    _vendors() {
        const list = this._settings.get_string('vendor').split(',')
            .map(v => v.trim()).filter(Boolean);
        return [...new Set(list)];
    }

    _place() {
        this._clear();
        const box = this._settings.get_string('panel-box') || 'right';
        const index = Math.max(0, this._settings.get_int('panel-index'));
        this._vendors().forEach((vendor, i) => {
            const indicator = new Indicator(this._settings, () => this.openPreferences(),
                this.dir.get_child('icons'), vendor);
            Main.panel.addToStatusArea(`${ROLE}:${vendor}`, indicator, index + i, box);
            this._indicators.push({role: `${ROLE}:${vendor}`, indicator});
        });
    }

    _clear() {
        for (const {role, indicator} of this._indicators) {
            indicator.destroy();
            delete Main.panel.statusArea[role];
        }
        this._indicators = [];
    }

    disable() {
        for (const id of this._placeIds ?? [])
            this._settings.disconnect(id);
        this._placeIds = null;
        this._clear();
        this._settings = null;
    }
}
