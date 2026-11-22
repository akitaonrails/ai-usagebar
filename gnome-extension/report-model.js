// Pure projection of `ai-usagebar usage --json` for the GNOME click menu.
// Labels, windows and errors come from the report. There is no vendor table,
// matching Omarchy's Model.js and the KDE card view.

const SEVERITIES = ['low', 'mid', 'high', 'critical'];

export function finitePercent(value) {
    // Number(null) is 0. An absent window must stay absent, not a 0% bar.
    if ((typeof value !== 'number' && typeof value !== 'string') || String(value).trim() === '')
        return null;
    const n = Number(value);
    if (!Number.isFinite(n) || n < 0 || n > 100)
        return null;
    return Math.round(n);
}

function clean(value, max) {
    return String(value ?? '').replace(/\s+/g, ' ').trim().slice(0, max);
}

function severityFor(percent, given) {
    if (SEVERITIES.includes(given))
        return given;
    if (percent >= 90)
        return 'critical';
    if (percent >= 75)
        return 'high';
    if (percent >= 50)
        return 'mid';
    return 'low';
}

function timestampMs(value) {
    if (!value)
        return null;
    const ms = new Date(String(value)).getTime();
    return Number.isFinite(ms) ? ms : null;
}

// "3h 35m", "6d 23h", "1m": the two largest units, never below a minute.
export function formatDuration(ms) {
    const minutes = Math.floor(ms / 60000);
    const hours = Math.floor(minutes / 60);
    const days = Math.floor(hours / 24);
    if (days > 0)
        return `${days}d ${hours % 24}h`;
    if (hours > 0)
        return `${hours}h ${minutes % 60}m`;
    return `${Math.max(1, minutes)}m`;
}

// Countdown only. The menu already prefixes "resets in".
export function formatReset(resetAt, nowMs) {
    const reset = timestampMs(resetAt);
    if (reset === null)
        return '';
    const remaining = reset - Number(nowMs);
    if (!(remaining > 0))
        return 'now';
    return formatDuration(remaining);
}

// Percent of the window already gone, for the menu's pace marker. It needs
// both the absolute reset and the window's exact length; the report omits
// `window_secs` for a calendar month or an unstated window, and that row gets
// no marker rather than a guessed one. Floored like src/pacing.rs, so the
// marker and the "N% elapsed" the detail line quotes agree.
export function elapsedPercent(resetAt, windowSecs, nowMs) {
    const reset = timestampMs(resetAt);
    const windowMs = Number(windowSecs) * 1000;
    if (reset === null || windowSecs == null || !Number.isFinite(windowMs) || windowMs <= 0)
        return null;
    const elapsed = (windowMs - (reset - Number(nowMs))) / windowMs * 100;
    return Math.max(0, Math.min(100, Math.floor(elapsed)));
}

// `detail` still opens with a "Resets in …" written for CLI readers. The menu
// draws its own countdown from reset_at, so that fragment would repeat it.
// Mirrors kde-plasmoid's and omarchy/Model.js's metricDetail.
export function metricDetail(detail, resetAt) {
    let text = clean(detail, 400);
    if (timestampMs(resetAt) === null)
        return text;
    text = text.replace(/^Resets in [^·]+\s*(?:·\s*)?/i, '');
    text = text.replace(/^Regenerates [^·]+? in [^·]+\s*(?:·\s*)?/i, '');
    text = text.replace(/\s*·\s*reset\s+[^·]+$/i, '');
    return text.trim();
}

function metricRow(raw, nowMs) {
    // A window the report does not include must not become a 0% bar.
    const percent = finitePercent(raw.percent);
    if (percent === null)
        return null;
    const headline = raw.headline === 'value' ? 'value' : 'percent';
    const value = clean(raw.value, 240);
    return {
        type: 'metric',
        label: clean(raw.label, 160) || 'Usage',
        percent,
        headline,
        valueText: headline === 'value' && value ? value : `${percent}%`,
        severity: severityFor(percent, String(raw.severity || '')),
        reset: formatReset(raw.reset_at, nowMs),
        elapsed: elapsedPercent(raw.reset_at, raw.window_secs, nowMs),
        projected: raw.projected_percent == null
            ? null
            : Math.max(0, Math.round(Number(raw.projected_percent))),
        spark: typeof raw.spark === 'string' ? raw.spark : '',
        sparkPoints: Array.isArray(raw.spark_points)
            ? raw.spark_points.filter(value => Number.isFinite(value))
                .map(value => Math.max(0, Math.min(100, Math.round(value))))
            : [],
        detail: metricDetail(raw.detail, raw.reset_at),
        group: clean(raw.group, 80),
        regen: clean(raw.regen, 40),
    };
}

function sectionRow(section, nowMs) {
    if (section.type === 'spacer')
        return null;
    if (section.type === 'metric')
        return metricRow(section, nowMs);
    if (section.type === 'block') {
        const body = (Array.isArray(section.body) ? section.body : [])
            .map(line => clean(line, 1000))
            .filter(Boolean);
        const label = clean(section.label, 160);
        return label || body.length ? {type: 'block', label, body} : null;
    }
    // `text`, and any kind a newer binary adds, reads as a label/value line:
    // a future section should degrade to something readable, not vanish.
    const label = clean(section.label, 160);
    const value = clean(section.value, 1000);
    return label || value ? {type: 'text', label, value} : null;
}

// Keep report order and heading context. A collapsed overview is a preview,
// not a synthetic quota: never aggregate pools or infer window names.
export function summarize(rows) {
    const metrics = [];
    let heading = '';
    for (const row of rows) {
        if (row.type === 'group') {
            // Explicit groups already travel on their metrics and must not
            // leak onto a subsequent ungrouped row.
            heading = '';
        } else if (row.type === 'text' && !row.value) {
            heading = row.label;
        } else if (row.type === 'metric') {
            const context = row.group || heading;
            metrics.push({...row, label: context && context !== row.label
                ? `${context} · ${row.label}` : row.label});
        } else {
            heading = '';
        }
    }
    const candidates = metrics.length ? metrics : rows
        .filter(row => row.type === 'text' && row.value)
        .map(row => ({label: row.label, valueText: row.value, headline: 'value'}));
    return {rows: candidates.slice(0, 2), remaining: Math.max(0, candidates.length - 2)};
}

// Marks are optional assets, addressed by the report's brand, not a vendor
// registry. Older binaries can use the vendor part of a named account ID.
export function brandSlug(brand, id) {
    const slug = clean(brand, 32) || clean(id, 180).split('@')[0];
    return /^[a-z0-9][a-z0-9_-]{0,31}$/.test(slug) ? slug : '';
}

export function projectEntry(raw, nowMs = Date.now()) {
    if (!raw || typeof raw !== 'object')
        return null;
    const id = clean(raw.id, 180);
    if (!id)
        return null;
    const error = raw.error == null ? '' : clean(raw.error, 1200);
    const sections = Array.isArray(raw.sections) ? raw.sections : [];
    const rows = [];
    let lastGroup = '';
    for (const section of sections) {
        if (!section || typeof section !== 'object')
            continue;
        const row = sectionRow(section, nowMs);
        if (!row)
            continue;
        // A heading covers the grouped rows right below it. Anything else
        // closes it, so a later row of the same group gets its heading again
        // instead of reading as part of whatever came between.
        const group = row.type === 'metric' ? row.group : '';
        if (group && group !== lastGroup)
            rows.push({type: 'group', label: group});
        lastGroup = group;
        rows.push(row);
    }
    return {
        id,
        title: clean(raw.display_name, 240) || clean(raw.name, 240) || id,
        brand: brandSlug(raw.brand, id),
        plan: clean(raw.plan, 240),
        stale: raw.stale === true,
        error,
        rows: error ? [] : rows,
    };
}

// The report for a command that printed nothing and failed: `usage` exits 1
// with only a message on stderr when no provider is enabled.
export function commandFailure(stderr) {
    const error = clean(stderr, 500) || 'ai-usagebar failed without an error message.';
    return {ok: false, error, entries: []};
}

// One line for a failure: the top bar keeps its compact ⚠ and shows this in
// the menu instead, and a menu command that could not run shows it in place.
export function errorLine(short, detail) {
    const s = clean(short, 120);
    const d = clean(detail, 300);
    return d ? `${s}: ${d}` : s;
}

export function parseReport(raw, nowMs = Date.now()) {
    let parsed;
    try {
        parsed = JSON.parse(String(raw || ''));
    } catch (e) {
        return {ok: false, error: 'The usage command returned invalid JSON.', entries: []};
    }
    if (!parsed || !Array.isArray(parsed.entries))
        return {ok: false, error: 'The usage command returned an unsupported report.', entries: []};
    const entries = [];
    for (const rawEntry of parsed.entries) {
        const entry = projectEntry(rawEntry, nowMs);
        if (entry)
            entries.push(entry);
    }
    if (parsed.entries.length > 0 && entries.length === 0)
        return {ok: false, error: 'The usage report did not contain a valid provider entry.', entries: []};
    return {ok: true, error: '', entries};
}
