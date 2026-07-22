// Pure marker helpers shared by the GNOME indicator and its Node table tests.
export const MARKER = '#61afef';
export const POINT_MID_MIN = -10;
export const POINT_CRITICAL_MIN = 10;
// Keep the stale suffix after this ignored literal field, never on elapsed.
// New fields are appended before the sentinel so existing indices never shift:
// an older binary simply echoes the unknown placeholders back and field()
// discards them.
export const FORMAT = '{plan};;{session_pct};;{session_reset};;{weekly_pct};;{weekly_reset};;' +
    '{sonnet_pct};;{sonnet_reset};;{extra_pct};;{extra_spent};;{extra_limit};;' +
    '{scoped_model};;{scoped_pct};;{scoped_reset};;' +
    '{session_elapsed};;{weekly_elapsed};;{scoped_elapsed};;{vendor_short};;' +
    '{extra_model};;{extra_reset};;{extra_elapsed};;' +
    '{session_model};;{weekly_model};;__aiub_end__';
export const FIELD = Object.freeze({
    plan: 0, sessionPct: 1, sessionReset: 2, weeklyPct: 3, weeklyReset: 4,
    sonnetPct: 5, sonnetReset: 6, extraPct: 7, extraSpent: 8, extraLimit: 9,
    scopedModel: 10, scopedPct: 11, scopedReset: 12,
    sessionElapsed: 13, weeklyElapsed: 14, scopedElapsed: 15, vendorShort: 16,
    extraModel: 17, extraReset: 18, extraElapsed: 19,
    sessionModel: 20, weeklyModel: 21,
    sentinel: 22,
});

// A vendor that names its primary rows is telling us its windows come in two
// independent pools, so the dropdown groups them under Session/Weekly headings
// instead of listing four flat rows. Data-driven on purpose: no vendor ids here.
export function isGrouped(sessionModel) {
    return field(sessionModel) !== '';
}

export function splitFormatOutput(text) {
    return String(text).split(';;');
}

// The CLI output is Pango markup. Strip only tags first, then decode one layer
// of XML entities so API labels escaped by Rust display as literal text rather
// than "&amp;". Decoding after tag removal cannot reintroduce active markup.
export function plainTextFromPango(value) {
    return String(value ?? '')
        .replace(/<[^>]*>/g, '')
        .replace(/&lt;/g, '<')
        .replace(/&gt;/g, '>')
        .replace(/&quot;/g, '"')
        .replace(/&apos;/g, "'")
        .replace(/&amp;/g, '&');
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

// Balance-only vendors do not expose generic rolling quota windows. Keep this
// vendor-aware at the native surface so their compatibility aliases cannot
// turn into confident 0% bars.
export function hasUsageWindows(vendorShort) {
    return field(vendorShort) !== 'dsk';
}

// Short panel tag for a quota pool: the model group's initial. The panel is
// width-constrained, so the full name only lives in the dropdown.
export function poolTag(model) {
    const name = field(model).replace(/[^\p{L}\p{N}]/gu, '');
    return name ? name[0].toUpperCase() : '';
}

// Two pools whose names share an initial would produce identical tags, so widen
// both until they differ. The names come from the binary and can change, which
// is exactly when a silent collision would be hardest to notice.
export function disambiguateTags(a, b) {
    const clean = m => field(m).replace(/[^\p{L}\p{N}]/gu, '');
    const [ca, cb] = [clean(a), clean(b)];
    if (!ca || !cb)
        return [poolTag(a), poolTag(b)];
    for (let n = 1; n <= Math.max(ca.length, cb.length); n++) {
        const [ta, tb] = [ca.slice(0, n), cb.slice(0, n)];
        if (ta.toUpperCase() !== tb.toUpperCase())
            return [ta.toUpperCase(), tb.toUpperCase()];
    }
    // Identical names: nothing distinguishes them, so keep the plain initials.
    return [poolTag(a), poolTag(b)];
}

// Which pool the panel shows in "auto" mode. A pool counts as spent when *any*
// of its windows crosses the threshold — switching on the 5h alone would strand
// the user on a pool whose weekly is the one that ran out. Only switch if the
// other pool still has room; with both spent, stay put rather than flapping.
export function pickPool(primary, secondary, threshold) {
    const spent = w => Math.max(w?.session ?? 0, w?.weekly ?? 0) >= threshold;
    return spent(primary) && !spent(secondary) ? 'secondary' : 'primary';
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
