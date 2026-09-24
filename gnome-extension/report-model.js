// Pure projection of `ai-usagebar usage --json` for the GNOME click menu.
// Labels, windows and errors come from the report. There is no vendor table,
// matching Omarchy's Model.js and the KDE card view.

const SEVERITIES = ['low', 'mid', 'high', 'critical'];

export function finitePercent(value) {
    // Number(null) is 0. An absent window must stay absent, not a 0% bar.
    if (value === null || value === undefined || value === '')
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

// Countdown only. The menu already prefixes "resets in".
export function formatReset(resetAt, nowMs) {
    if (!resetAt)
        return '';
    const resetMs = new Date(String(resetAt)).getTime();
    if (!Number.isFinite(resetMs))
        return '';
    const remaining = resetMs - Number(nowMs);
    if (!(remaining > 0))
        return 'now';
    const minutes = Math.floor(remaining / 60000);
    const hours = Math.floor(minutes / 60);
    const days = Math.floor(hours / 24);
    if (days > 0)
        return `${days}d ${hours % 24}h`;
    if (hours > 0)
        return `${hours}h ${minutes % 60}m`;
    return `${Math.max(1, minutes)}m`;
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
        group: clean(raw.group, 80),
    };
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
    for (const section of sections.slice(0, 96)) {
        if (!section || typeof section !== 'object')
            continue;
        if (section.type === 'metric') {
            const row = metricRow(section, nowMs);
            if (!row)
                continue;
            if (row.group && row.group !== lastGroup) {
                rows.push({type: 'group', label: row.group});
                lastGroup = row.group;
            }
            rows.push(row);
        } else if (section.type === 'text') {
            const label = clean(section.label, 160);
            const value = clean(section.value, 1000);
            if (label || value)
                rows.push({type: 'text', label, value});
        } else if (section.type === 'block') {
            const body = (Array.isArray(section.body) ? section.body : [])
                .map(line => clean(line, 1000))
                .filter(Boolean)
                .slice(0, 24);
            const label = clean(section.label, 160);
            if (label || body.length)
                rows.push({type: 'block', label, body});
        }
    }
    return {
        id,
        title: clean(raw.display_name, 240) || clean(raw.name, 240) || id,
        plan: clean(raw.plan, 240),
        stale: raw.stale === true,
        error,
        rows: error ? [] : rows,
    };
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
    for (const rawEntry of parsed.entries.slice(0, 64)) {
        const entry = projectEntry(rawEntry, nowMs);
        if (entry)
            entries.push(entry);
    }
    if (parsed.entries.length > 0 && entries.length === 0)
        return {ok: false, error: 'The usage report did not contain a valid provider entry.', entries: []};
    return {ok: true, error: '', entries};
}
