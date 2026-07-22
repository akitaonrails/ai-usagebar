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

function poolChars(model) {
    return Array.from(field(model)).filter(ch => /[\p{L}\p{N}]/u.test(ch));
}

// Short panel tag for a quota pool: the model group's initial. Work in Unicode
// code points rather than UTF-16 code units so a non-BMP letter is never split
// into an invalid surrogate. The panel is width-constrained, so the full name
// only lives in the dropdown.
export function poolTag(model) {
    const chars = poolChars(model);
    return chars.length ? chars[0].toUpperCase() : '';
}

// Two pools whose names share an initial would produce identical tags, so widen
// both until they differ. The names come from the binary and can change, which
// is exactly when a silent collision would be hardest to notice.
export function disambiguateTags(a, b) {
    const [ca, cb] = [poolChars(a), poolChars(b)];
    if (!ca.length || !cb.length)
        return [poolTag(a), poolTag(b)];
    for (let n = 1; n <= Math.max(ca.length, cb.length); n++) {
        const [ta, tb] = [ca.slice(0, n).join('').toUpperCase(), cb.slice(0, n).join('').toUpperCase()];
        if (ta !== tb)
            return [ta, tb];
    }
    // Identical names: nothing distinguishes them, so keep the plain initials.
    return [poolTag(a), poolTag(b)];
}

export function poolAvailable(pool) {
    return Number.isFinite(pool?.session) || Number.isFinite(pool?.weekly);
}

// Which pool the panel shows in "auto" mode. A pool counts as spent when *any*
// of its windows crosses the threshold — switching on the 5h alone would strand
// the user on a pool whose weekly is the one that ran out. Only switch if the
// other pool still has room; with both spent, stay put rather than flapping.
export function pickPool(primary, secondary, threshold) {
    // An unavailable secondary is not a pristine 0%-used pool. Treating it as
    // one would switch the panel to an empty label exactly when the primary
    // reaches the warning threshold.
    if (!poolAvailable(primary))
        return poolAvailable(secondary) ? 'secondary' : 'primary';
    if (!poolAvailable(secondary))
        return 'primary';
    const spent = w => Math.max(
        Number.isFinite(w.session) ? w.session : 0,
        Number.isFinite(w.weekly) ? w.weekly : 0) >= threshold;
    return spent(primary) && !spent(secondary) ? 'secondary' : 'primary';
}

// Resolve every panel-pools mode while filtering unavailable pools. Keeping
// this policy pure makes the partial-payload cases testable outside GNOME.
export function selectPools(primary, secondary, mode, threshold,
    windows = {session: true, weekly: true}) {
    const visible = pool => ({
        session: windows.session ? pool?.session : null,
        weekly: windows.weekly ? pool?.weekly : null,
    });
    const available = {
        primary: poolAvailable(visible(primary)),
        secondary: poolAvailable(visible(secondary)),
    };
    if (mode === 'primary')
        return available.primary ? ['primary'] : (available.secondary ? ['secondary'] : []);
    if (mode === 'secondary')
        return available.secondary ? ['secondary'] : (available.primary ? ['primary'] : []);
    if (mode === 'auto') {
        const selected = pickPool(primary, secondary, threshold);
        if (available[selected])
            return [selected];
        const fallback = selected === 'primary' ? 'secondary' : 'primary';
        return available[fallback] ? [fallback] : [];
    }
    return ['primary', 'secondary'].filter(name => available[name]);
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
