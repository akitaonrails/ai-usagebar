// Pure marker helpers shared by the GNOME indicator and its Node table tests.
export const MARKER = '#61afef';
export const POINT_MID_MIN = -10;
export const POINT_CRITICAL_MIN = 10;
// Keep the stale suffix after this ignored literal field, never on elapsed.
export const FORMAT = '{plan};;{session_pct};;{session_reset};;{weekly_pct};;{weekly_reset};;' +
    '{sonnet_pct};;{sonnet_reset};;{extra_pct};;{extra_spent};;{extra_limit};;' +
    '{scoped_model};;{scoped_pct};;{scoped_reset};;' +
    '{session_elapsed};;{weekly_elapsed};;{scoped_elapsed};;__aiub_end__';
export const FIELD = Object.freeze({
    plan: 0, sessionPct: 1, sessionReset: 2, weeklyPct: 3, weeklyReset: 4,
    sonnetPct: 5, sonnetReset: 6, extraPct: 7, extraSpent: 8, extraLimit: 9,
    scopedModel: 10, scopedPct: 11, scopedReset: 12,
    sessionElapsed: 13, weeklyElapsed: 14, scopedElapsed: 15, sentinel: 16,
});

export function splitFormatOutput(text) {
    return String(text).split(';;');
}

export function field(value) {
    const text = String(value ?? '').trim();
    return text && !/^\{[^}]+\}$/.test(text) ? text : '';
}

// Do not accept a numeric prefix: a stale suffix such as "27 ⏸" is not elapsed.
export function integer(value) {
    const text = field(value);
    return /^-?\d+$/.test(text) ? Number(text) : null;
}

export function markerElapsed(reset, elapsed) {
    return reset && reset !== '—' && Number.isFinite(elapsed) ? elapsed : null;
}

// Matches pacing::pace_severity: < -10 low, -10..=0 mid, 1..=9 high, >= 10 critical.
export function colorForDelta(delta, colors) {
    if (delta >= POINT_CRITICAL_MIN)
        return colors.critical;
    if (delta > 0)
        return colors.high;
    if (delta >= POINT_MID_MIN)
        return colors.mid;
    return colors.low;
}

export function colorForPct(pct, colors) {
    if (pct >= 90)
        return colors.critical;
    if (pct >= 75)
        return colors.high;
    if (pct >= 50)
        return colors.mid;
    return colors.low;
}

export function barMarkup(pct, width, colors, elapsed) {
    const p = Math.max(0, Math.min(100, Math.round(pct)));
    const filled = Math.round((p * width) / 100);

    if (!Number.isFinite(elapsed)) {
        return `<span foreground="${colorForPct(p, colors)}">${'█'.repeat(filled)}</span>` +
            `<span foreground="${colors.empty}">${'░'.repeat(width - filled)}</span>`;
    }

    const e = Math.max(0, Math.min(100, Math.round(elapsed)));
    const base = colorForPct(p, colors);
    const over = colorForDelta(p - e, colors);
    let marker = Math.floor((e * width) / 100);
    if (marker > width - 1)
        marker = width - 1;
    const preFilled = Math.min(filled, marker);
    const postFilled = filled > marker + 1 ? filled - marker - 1 : 0;
    const preEmpty = marker - preFilled;
    const postEmpty = width - marker - 1 - postFilled;
    return `<span foreground="${base}">${'█'.repeat(preFilled)}</span>` +
        `<span foreground="${colors.empty}">${'░'.repeat(preEmpty)}</span>` +
        `<span foreground="${MARKER}">│</span>` +
        `<span foreground="${over}">${'█'.repeat(postFilled)}</span>` +
        `<span foreground="${colors.empty}">${'░'.repeat(postEmpty)}</span>`;
}
