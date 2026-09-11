// Pure layer for the KDE plasmoid. No Qt imports, so every rule here is table
// tested under Node in kde-plasmoid/plasmoid-logic.test.mjs.
//
// Consumes `ai-usagebar usage --json` — the same report the native Omarchy
// panel consumes — rather than the `--format` placeholder string. That contract
// already carries display_name, plan, status, stale, fetched_at, per-metric
// severity and an absolute reset_at, so the widget stays a presentation layer
// and needs no copy of the shared marker-logic module.
//
// Formatting semantics (headline, formatDuration, formatReset, formatUpdated,
// metricDetail) deliberately mirror omarchy/Model.js so the two native panels
// word the same data the same way.
//
// Two QML V4 engine rules apply to this file, asserted by the test:
//   - no ES2019 optional catch binding (`catch {`), which V4 rejects outright;
//   - no Unicode property escapes (\p{...}), which V4 evaluates to false
//     *silently* rather than throwing.

export const DEFAULT_BINARY = 'ai-usagebar';
export const DEFAULT_TIMEOUT_SECS = 600;
export const MIN_TIMEOUT_SECS = 60;
export const MAX_TIMEOUT_SECS = 3600;

// The executable data engine hands QML no handle on the child process, so the
// applet cannot kill a hung binary: disconnectSource only stops us listening,
// and the process keeps running — one more of them every tick. Wrapping the
// spawn in timeout(1) is what actually bounds it, and is the nearest equivalent
// to the GNOME extension's proc.force_exit(). -k escalates to SIGKILL for a
// binary that ignores SIGTERM. coreutils provides timeout on any system that
// can run Plasma, and the QML watchdog stays on as a backstop for the case
// where the engine itself never reports back.
export const TIMEOUT_KILL_GRACE_SECS = 5;
export const EXIT_TIMED_OUT = 124;  // timeout(1)'s own code, after SIGTERM
export const EXIT_KILLED = 137;     // 128 + SIGKILL, after -k escalated

// KProcess::setShellCommand runs the source name through
// KShell::splitArgs(AbortOnMeta), which REFUSES the whole string when it meets
// an unquoted metacharacter. Single-quote everything; the closing/escaping
// dance is the only portable way to carry an embedded quote through.
export function shellQuote(value) {
    return `'${String(value ?? '').replace(/'/g, `'\\''`)}'`;
}

// One call returns every configured vendor, so the applet never passes
// --vendor and never reads ~/.cache/ai-usagebar/active_vendor — that file
// belongs to the Waybar module's --cycle-next. Which vendor this instance
// shows is decided here, client side, from its own KConfigXT value.
export function timeoutSeconds(value) {
    if (value === null || value === undefined || String(value).trim() === '')
        return DEFAULT_TIMEOUT_SECS;
    const seconds = Number(value);
    if (!Number.isFinite(seconds))
        return DEFAULT_TIMEOUT_SECS;
    return Math.max(MIN_TIMEOUT_SECS, Math.min(MAX_TIMEOUT_SECS, Math.round(seconds)));
}

export function buildArgv(binary, timeoutSecs) {
    const bin = String(binary ?? '').trim() || DEFAULT_BINARY;
    const call = [bin, 'usage', '--json'];
    return ['timeout', '-k', String(TIMEOUT_KILL_GRACE_SECS), String(timeoutSeconds(timeoutSecs))]
        .concat(call);
}

export function buildCommand(binary, timeoutSecs) {
    return buildArgv(binary, timeoutSecs).map(shellQuote).join(' ');
}

// Launching the TUI needs a terminal, and there is no portable way to probe
// PATH from QML. Emit a shell fallback chain instead and let KProcess take its
// /bin/sh -c path — here the metacharacters are the point, unlike buildCommand
// where they had to be quoted away. Mirrors the candidate list the GNOME
// extension already walks.
export function buildTuiCommand(terminalCommand, tui = 'ai-usagebar-tui') {
    const custom = String(terminalCommand ?? '').trim();
    if (custom)
        return `${custom} ${shellQuote(tui)}`;
    const q = shellQuote(tui);
    return [
        `konsole -e ${q}`,
        `gnome-terminal -- ${q}`,
        `xterm -e ${q}`,
    ].join(' || ');
}

// ---------------------------------------------------------------------------
// The report
// ---------------------------------------------------------------------------

export function safeText(value, maxLength = 400) {
    // Report fields ultimately come from remote provider responses. QML Labels
    // default to AutoText, where an HTML-looking value can become rich text and
    // load an inline image. The views also opt into Text.PlainText, but remove
    // markup delimiters and display-control characters here as defence in depth
    // for controls (buttons and check boxes) that expose no textFormat property.
    const s = String(value ?? '')
        .replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f\u202a-\u202e\u2066-\u2069]/g, '')
        .replace(/</g, '‹')
        .replace(/>/g, '›');
    return s.length > maxLength ? s.slice(0, maxLength) : s;
}

function finitePercent(value) {
    // Number(null) === 0 in JS: without this guard a metric the Rust core
    // reports with percent null (a balance-style row, or a window the vendor
    // does not report) would normalize into a fabricated 0% bar instead of
    // keeping the null that means "not reported".
    if (value === null || value === undefined)
        return null;
    const n = Number(value);
    return Number.isFinite(n) ? Math.max(0, Math.min(100, n)) : null;
}

// Severity comes from the Rust core, which computes it once for every frontend.
// Fall back to the documented 50/75/90 bands only when a field is missing or
// unrecognised, so an older binary still renders sensible colours instead of
// everything reading as "low".
export const SEVERITIES = ['low', 'mid', 'high', 'critical'];

export function severityOf(percent, declared) {
    const s = String(declared ?? '');
    if (SEVERITIES.indexOf(s) >= 0)
        return s;
    const p = finitePercent(percent);
    if (p === null)
        return 'low';
    return p >= 90 ? 'critical' : p >= 75 ? 'high' : p >= 50 ? 'mid' : 'low';
}

export function severityColor(severity, colors) {
    const c = colors || {};
    switch (severity) {
    case 'critical': return c.critical;
    case 'high':     return c.high;
    case 'mid':      return c.mid;
    default:         return c.low;
    }
}

function normalizeSection(raw) {
    const type = String(raw && raw.type || '');
    if (type === 'spacer')
        return {type: 'spacer'};
    if (type === 'metric') {
        const percent = finitePercent(raw.percent);
        return {
            type: 'metric',
            label: safeText(raw.label, 120),
            value: safeText(raw.value, 40),
            percent: percent,
            detail: safeText(raw.detail, 400),
            resetAt: safeText(raw.reset_at, 64),
            severity: severityOf(percent, raw.severity),
        };
    }
    if (type === 'block') {
        const body = Array.isArray(raw.body)
            ? raw.body.slice(0, 128).map(line => safeText(line, 200)) : [];
        return {type: 'block', label: safeText(raw.label, 120), body: body};
    }
    // Unknown types are carried as plain text rather than dropped: a future
    // section kind should degrade to something readable, not vanish.
    return {
        type: 'text',
        label: safeText(raw && raw.label, 120),
        value: safeText(raw && raw.value, 200),
    };
}

function normalizeEntry(raw) {
    if (!raw || typeof raw !== 'object')
        return null;
    const id = safeText(raw.id, 60).trim();
    if (!id)
        return null;
    const sections = Array.isArray(raw.sections)
        ? raw.sections.slice(0, 128).map(normalizeSection) : [];
    return {
        id: id,
        // display_name is the canonical label the Rust core owns. Falling back
        // to the raw id keeps a slug visible rather than an empty tab.
        label: safeText(raw.display_name, 60).trim() || safeText(raw.name, 60).trim() || id,
        plan: safeText(raw.plan, 80),
        status: safeText(raw.status, 24) || 'ready',
        stale: raw.stale === true,
        error: safeText(raw.error, 500),
        fetchedAt: safeText(raw.fetched_at, 64),
        sections: sections,
    };
}

// The binary always exits 0 and prints a report, so anything unparseable here
// is a missing binary or a crash — never a vendor-side failure, which arrives
// as an entry with status "error".
export function parseReport(stdout) {
    const raw = String(stdout ?? '').trim();
    if (!raw)
        return {ok: false, raw: '', entries: [], primary: ''};
    let parsed;
    try {
        parsed = JSON.parse(raw);
    } catch (e) {
        return {ok: false, raw: raw, entries: [], primary: ''};
    }
    if (!parsed || typeof parsed !== 'object' || !Array.isArray(parsed.entries))
        return {ok: false, raw: raw, entries: [], primary: ''};
    return {
        ok: true,
        raw: raw,
        entries: parsed.entries.slice(0, 64).map(normalizeEntry).filter(Boolean),
        primary: safeText(parsed.primary, 60).trim(),
    };
}

// Which entry this applet instance shows. The configured vendor wins; the
// report's own primary is the fallback, then the first entry. Never throws on
// an id the report no longer carries — a vendor removed from config.toml must
// degrade to something visible, not to a blank panel.
export function entryFor(report, vendorId) {
    const entries = (report && report.entries) || [];
    if (!entries.length)
        return null;
    const wanted = String(vendorId ?? '').trim();
    for (const e of entries)
        if (e.id === wanted)
            return e;
    const primary = String((report && report.primary) ?? '').trim();
    for (const e of entries)
        if (e.id === primary)
            return e;
    return entries[0];
}

// The popup's tab strip. Every configured vendor is offered, including ones
// currently failing — hiding an errored vendor is what made it impossible to
// tell "not configured" from "configured and broken".
export function vendorTabs(report, activeId) {
    const entries = (report && report.entries) || [];
    const active = entryFor(report, activeId);
    return entries.map(e => ({
        id: e.id,
        label: e.label,
        active: !!active && e.id === active.id,
        failing: e.status === 'error' || !!e.error,
    }));
}

export function nextVendor(ring, current, delta) {
    const list = toRing(ring);
    if (!list.length)
        return current;
    const step = Number(delta) || 0;
    const at = list.indexOf(current);
    if (at === -1)
        return list[0];
    const n = list.length;
    return list[(((at + step) % n) + n) % n];
}

// A KConfigXT StringList reaches QML as an array-LIKE object: right length,
// right contents, but Array.isArray() === false. Guarding on isArray made
// every scroll a silent no-op in the real panel.
function toRing(ring) {
    if (ring === null || ring === undefined || typeof ring === 'string')
        return [];
    const len = Number(ring.length);
    if (!Number.isFinite(len) || len <= 0)
        return [];
    return Array.from(ring);
}

// ---------------------------------------------------------------------------
// Projection
// ---------------------------------------------------------------------------

// The worst metric, which is what the panel shows when it has room for one
// number. A balance-style row reports money rather than a percentage, so it
// contributes its own value text instead.
export function headline(entry) {
    if (!entry)
        return {text: '', percent: null, severity: 'low', label: ''};
    let best = null;
    for (const s of entry.sections)
        if (s.type === 'metric' && s.percent !== null && (!best || s.percent > best.percent))
            best = s;
    if (best) {
        const isBalance = /balance/i.test(best.label) && best.value !== '';
        return {
            text: isBalance ? best.value : `${best.percent}%`,
            percent: best.percent,
            severity: best.severity,
            label: best.label,
        };
    }
    for (const s of entry.sections)
        if (s.type === 'text' && /(balance|available|spend|prepaid)/i.test(s.label) && s.value !== '')
            return {text: s.value, percent: null, severity: 'low', label: s.label};
    return {
        text: entry.status === 'error' ? 'Error' : 'Ready',
        percent: null,
        severity: 'low',
        label: '',
    };
}

export function isAlarming(entry) {
    if (!entry)
        return false;
    return entry.status === 'error' || entry.stale === true || headline(entry).severity === 'critical';
}

// The metric rows the popup renders, spacers dropped: Column spacing handles
// the rhythm, so a spacer would double it.
export function detailRows(entry) {
    if (!entry)
        return [];
    return entry.sections.filter(s => s.type !== 'spacer');
}

// The panel cells. Each carries its own severity so the compact representation
// colours per cell rather than by the entry's worst value.
export function panelCells(entry, options) {
    const opts = options || {};
    const max = Number.isFinite(Number(opts.max)) ? Math.max(1, Number(opts.max)) : 2;
    if (!entry)
        return [];
    if (entry.status === 'error')
        return [{label: '', text: '⚠', severity: 'critical', percent: null}];
    const cells = [];
    for (const s of entry.sections) {
        if (s.type !== 'metric' || cells.length >= max)
            continue;
        cells.push({
            label: shortLabel(s.label),
            text: s.percent === null ? s.value : `${s.percent}%`,
            severity: s.severity,
            percent: s.percent,
        });
    }
    return cells;
}

// The card view (viewMode "VendorCards") projects one card per entry the
// aggregate report already returned — never a hardcoded vendor table. The
// Rust core owns names, metric projection and severity bands; this only
// restates what `usage --json` handed over in a shape the QML can lay out.
function severityRank(severity) {
    return SEVERITIES.indexOf(severityOf(null, severity));
}

export function cardState(entry) {
    if (!entry)
        return 'ok';
    if (entry.status === 'error' || entry.error)
        return 'error';
    if (entry.stale === true)
        return 'stale';
    return 'ok';
}

// One card's view model. Windows are the entry's metric sections; a window
// with percent === null renders as "not reported" rather than as a fabricated
// 0% bar. Block sections (balances, free-form notes) ride along untouched so
// the card shows everything the tab view would.
export function cardFor(entry) {
    if (!entry)
        return null;
    let worst = -1;
    let accent = 'low';
    for (const s of entry.sections) {
        if (s.type !== 'metric')
            continue;
        const rank = severityRank(s.severity);
        if (rank > worst) {
            worst = rank;
            accent = SEVERITIES[worst];
        }
    }
    const state = cardState(entry);
    return {
        id: entry.id,
        label: entry.label,
        plan: entry.plan,
        state: state,
        // An errored vendor outranks whatever its last good numbers said.
        accent: state === 'error' ? 'critical' : accent,
        error: state === 'error' ? errorMessage(entry.error) : '',
        windows: entry.sections.filter(s => s.type === 'metric').map(s => ({
            label: s.label,
            window: shortLabel(s.label),
            percent: s.percent,
            value: s.percent === null ? s.value : `${s.percent}%`,
            severity: s.severity,
            resetAt: s.resetAt,
            detail: metricDetail(s),
        })),
        blocks: entry.sections.filter(s => s.type === 'block'),
    };
}

export function cardModel(report) {
    const entries = (report && report.entries) || [];
    return entries.map(cardFor);
}

// "Session (5h)" -> "5h", "Weekly (7d)" -> "7d". The panel is width
// constrained; the full label lives in the popup. Falls back to the first word
// so an unrecognised label still shortens to something rather than to nothing.
export function shortLabel(label) {
    const s = String(label ?? '').trim();
    const paren = s.match(/\(([^)]{1,8})\)\s*$/);
    if (paren)
        return paren[1];
    return s.split(/\s+/)[0] || '';
}

// detail still carries a human-readable "Resets in …" written for CLI readers.
// The panel renders a live countdown from reset_at instead, so that fragment
// would be both stale and duplicated. Mirrors omarchy/Model.js metricDetail.
export function metricDetail(row) {
    let detail = safeText(row && row.detail, 1000);
    if (!row || !row.resetAt)
        return detail.trim();
    detail = detail.replace(/^Resets in [^·]+\s*(?:·\s*)?/i, '');
    detail = detail.replace(/\s*·\s*reset\s+[^·]+$/i, '');
    return detail.trim();
}

export function formatDuration(milliseconds) {
    const ms = Number(milliseconds);
    if (!(ms > 0))
        return 'now';
    const minutes = Math.floor(ms / 60000);
    const hours = Math.floor(minutes / 60);
    const days = Math.floor(hours / 24);
    if (days > 0)
        return `${days}d ${hours % 24}h`;
    if (hours > 0)
        return `${hours}h ${minutes % 60}m`;
    return `${Math.max(1, minutes)}m`;
}

// Measured against a locally ticking clock rather than baked into the report,
// so the countdown stays live between fetches. That is what lets the refresh
// interval be minutes rather than seconds without the popup looking frozen.
//
// These return milliseconds, not sentences: the words are i18n()'d in QML, and
// keeping the arithmetic here is what makes it testable under Node. Returns
// null when the timestamp is absent or unparseable, which the caller renders as
// "unknown" rather than as an accidental "0m".
export function resetRemainingMs(resetAt, nowMs) {
    if (!resetAt)
        return null;
    const resetMs = new Date(String(resetAt)).getTime();
    if (!Number.isFinite(resetMs))
        return null;
    return resetMs - Number(nowMs);
}

export function updatedAgeMs(fetchedAt, nowMs) {
    if (!fetchedAt)
        return null;
    const fetchedMs = new Date(String(fetchedAt)).getTime();
    if (!Number.isFinite(fetchedMs))
        return null;
    return Math.max(0, Number(nowMs) - fetchedMs);
}

export function errorMessage(value) {
    const message = safeText(value, 500).trim();
    return message === '' ? 'The usage command failed without an error message.' : message;
}

// ---------------------------------------------------------------------------
// Theme
// ---------------------------------------------------------------------------

// Referencing the Kirigami roles is what makes the applet follow the Plasma
// colour scheme. Mapping severity onto the semantic roles rather than onto
// fixed hexes is what makes a Breeze Light user see Breeze Light.
export function paletteFromTheme(theme) {
    const t = theme || {};
    return {
        low: t.positiveTextColor || t.textColor,
        mid: t.neutralTextColor || t.textColor,
        high: t.neutralTextColor || t.textColor,
        critical: t.negativeTextColor || t.textColor,
        empty: t.disabledTextColor || t.textColor,
    };
}

// Only start a fetch when the previous one is done, and never let a config
// change queue a second in-flight command: the source name IS the command, so
// two of them would race to paint the same panel.
export function shouldStartFetch(pendingCommand, nextCommand) {
    return String(pendingCommand ?? '') === '' && String(nextCommand ?? '') !== '';
}
