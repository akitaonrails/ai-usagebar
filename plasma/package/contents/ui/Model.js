// Pure data shaping for the Plasma applet. Keep this file free of QML globals
// (and of user-visible wording, which belongs in the QML where i18n lives) so
// the report contract can also be exercised by Node in CI.
//
// The Rust binary owns provider fetching, credentials, canonical product names
// and reset metadata. Everything here only reshapes what `ai-usagebar` already
// produced.

function cleanText(value, maxLength) {
    var text = value === undefined || value === null ? "" : String(value)
    // The Rust projection already strips terminal controls. This second, cheap
    // boundary keeps hand-authored/older JSON from putting controls into a
    // long-lived shell process.
    text = text.replace(/[\t\r]/g, " ")
        .replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f-\u009f]/g, "")
        .replace(/[\u200e\u200f\u202a-\u202e\u2066-\u2069]/g, "")
    var limit = Number(maxLength) || 2048
    if (text.length <= limit) {
        return text
    }
    var end = limit - 1
    var finalCodeUnit = text.charCodeAt(end - 1)
    if (finalCodeUnit >= 0xd800 && finalCodeUnit <= 0xdbff) {
        end--
    }
    return text.slice(0, end) + "…"
}

// Single-line text for labels. Angle brackets are neutralized so a provider
// string can never be reclassified as rich text by a Text.AutoText consumer.
function safeText(value) {
    return cleanText(value, 1000)
        .replace(/[\n\u2028\u2029]/g, " ")
        .replace(/</g, "‹")
        .replace(/>/g, "›")
}

function finitePercent(value) {
    var number = Number(value)
    if (!isFinite(number)) {
        return null
    }
    return Math.max(0, Math.min(100, Math.round(number)))
}

function normalizeSection(raw) {
    if (!raw || typeof raw !== "object") {
        return null
    }
    var type = String(raw.type || "")
    if (type === "spacer") {
        return { type: "spacer" }
    }
    if (type === "metric") {
        var percent = finitePercent(raw.percent)
        if (percent === null) {
            return null
        }
        var severity = String(raw.severity || "")
        if (["low", "mid", "high", "critical"].indexOf(severity) < 0) {
            severity = percent >= 90 ? "critical" : percent >= 75 ? "high" : percent >= 50 ? "mid" : "low"
        }
        return {
            type: "metric",
            label: safeText(raw.label),
            percent: percent,
            value: safeText(raw.value),
            detail: cleanText(raw.detail, 1000),
            severity: severity,
            reset_at: cleanText(raw.reset_at, 80)
        }
    }
    if (type === "text") {
        return { type: "text", label: safeText(raw.label), value: safeText(raw.value) }
    }
    if (type === "block") {
        var body = Array.isArray(raw.body) ? raw.body : []
        var lines = []
        for (var i = 0; i < body.length && i < 24; i++) {
            lines.push(safeText(body[i]))
        }
        return { type: "block", label: safeText(raw.label), body: lines }
    }
    return null
}

function normalizeEntry(raw) {
    if (!raw || typeof raw !== "object") {
        return null
    }
    var id = cleanText(raw.id, 180).trim()
    if (id === "") {
        return null
    }
    var sourceSections = Array.isArray(raw.sections) ? raw.sections : []
    var sections = []
    for (var i = 0; i < sourceSections.length && i < 96; i++) {
        var section = normalizeSection(sourceSections[i])
        if (section && section.type !== "spacer") {
            sections.push(section)
        }
    }
    var error = raw.error === undefined || raw.error === null ? "" : cleanText(raw.error, 1200)
    return {
        id: id,
        name: safeText(raw.name),
        display_name: safeText(raw.display_name),
        plan: safeText(raw.plan),
        status: error !== "" || raw.status === "error" ? "error" : "ready",
        error: error,
        stale: raw.stale === true,
        fetched_at: cleanText(raw.fetched_at, 80),
        sections: sections
    }
}

// `ai-usagebar usage --json`
function parseReport(raw) {
    try {
        var parsed = JSON.parse(String(raw || ""))
        if (!parsed || !Array.isArray(parsed.entries)) {
            return { ok: false, reason: "unsupported", primary: "", entries: [] }
        }
        var entries = []
        for (var i = 0; i < parsed.entries.length && i < 64; i++) {
            var entry = normalizeEntry(parsed.entries[i])
            if (entry) {
                entries.push(entry)
            }
        }
        if (parsed.entries.length > 0 && entries.length === 0) {
            return { ok: false, reason: "empty", primary: "", entries: [] }
        }
        return { ok: true, reason: "", primary: cleanText(parsed.primary, 180).trim(), entries: entries }
    } catch (error) {
        return { ok: false, reason: "invalid-json", primary: "", entries: [] }
    }
}

// `ai-usagebar settings show` — only the default provider is read here.
function parseSettings(raw) {
    try {
        var parsed = JSON.parse(String(raw || ""))
        if (!parsed || Number(parsed.schema_version) !== 1) {
            return { ok: false, primary: "" }
        }
        var primary = cleanText(parsed.primary, 80).trim()
        return { ok: true, primary: /^[a-z0-9_]+$/.test(primary) ? primary : "" }
    } catch (error) {
        return { ok: false, primary: "" }
    }
}

function baseProvider(id) {
    return String(id || "").split("@")[0]
}

// A provider the user never configured is not an outage: cycling onto it would
// only put a "⚠" in the panel.
function isUnconfigured(entry) {
    if (!entry || entry.status !== "error") {
        return false
    }
    return /no api key|credentials error|not configured/i.test(entry.error || "")
}

function cycleIds(entries, skipUnconfigured) {
    var list = Array.isArray(entries) ? entries : []
    var ids = []
    for (var i = 0; i < list.length; i++) {
        if (!skipUnconfigured || !isUnconfigured(list[i])) {
            ids.push(list[i].id)
        }
    }
    return ids
}

function stepId(ids, currentId, forward) {
    var list = Array.isArray(ids) ? ids : []
    if (list.length === 0) {
        return ""
    }
    var index = list.indexOf(String(currentId || ""))
    if (index < 0) {
        return forward ? list[0] : list[list.length - 1]
    }
    return list[(index + (forward ? 1 : -1) + list.length) % list.length]
}

function entryById(entries, id) {
    var list = Array.isArray(entries) ? entries : []
    if (list.length === 0) {
        return null
    }
    var wanted = String(id || "")
    if (wanted === "") {
        return list[0]
    }
    for (var i = 0; i < list.length; i++) {
        if (list[i].id === wanted) {
            return list[i]
        }
    }
    for (var j = 0; j < list.length; j++) {
        if (baseProvider(list[j].id) === baseProvider(wanted)) {
            return list[j]
        }
    }
    return null
}

function formatDuration(milliseconds) {
    var total = Number(milliseconds)
    if (!isFinite(total) || total <= 0) {
        return ""
    }
    var minutes = Math.floor(total / 60000)
    var hours = Math.floor(minutes / 60)
    var days = Math.floor(hours / 24)
    if (days > 0) {
        return days + "d " + (hours % 24) + "h"
    }
    if (hours > 0) {
        return hours + "h " + pad2(minutes % 60) + "m"
    }
    return Math.max(1, minutes) + "m"
}

function pad2(value) {
    return value < 10 ? "0" + value : String(value)
}

// Absolute reset timestamps travel with each metric, so an open popup keeps
// counting down between refreshes instead of freezing the rendered text.
function resetRemainingMs(resetAt, nowMs) {
    if (!resetAt) {
        return null
    }
    var resetMs = new Date(String(resetAt)).getTime()
    if (!isFinite(resetMs)) {
        return null
    }
    return resetMs - Number(nowMs)
}

function elapsedMs(fetchedAt, nowMs) {
    if (!fetchedAt) {
        return null
    }
    var fetchedMs = new Date(String(fetchedAt)).getTime()
    if (!isFinite(fetchedMs)) {
        return null
    }
    return Math.max(0, Number(nowMs) - fetchedMs)
}

// The Rust `detail` is one rendered line: "Resets in 4h 09m · 16% elapsed ·
// 14pts ahead". It is split into its parts so the panel can render a live
// countdown and translate the pacing hints, keeping anything unrecognized
// verbatim.
function parseDetail(detail) {
    var parts = String(detail || "").split("·")
    var out = { reset: "", elapsed: null, pacePoints: null, paceDirection: "", onPace: false, extras: [] }
    for (var i = 0; i < parts.length; i++) {
        var part = cleanText(parts[i], 240).trim()
        if (part === "") {
            continue
        }
        if (/^(resets?\b|no reset\b)/i.test(part)) {
            out.reset = part
            continue
        }
        var elapsed = part.match(/^([\d.]+)\s*%\s+elapsed$/i)
        if (elapsed) {
            out.elapsed = Math.max(0, Math.min(100, parseFloat(elapsed[1])))
            continue
        }
        var pace = part.match(/^([\d.]+)\s*pts?\s+(ahead|over|under|behind)$/i)
        if (pace) {
            out.pacePoints = parseFloat(pace[1])
            out.paceDirection = /^(ahead|over)$/i.test(pace[2]) ? "ahead" : "under"
            continue
        }
        if (/^on pace$/i.test(part)) {
            out.onPace = true
            continue
        }
        out.extras.push(safeText(part))
    }
    return out
}

// Block bodies are "key: value" lines; the panel aligns them on both edges.
function splitPair(line) {
    var text = safeText(line)
    var index = text.indexOf(":")
    if (index <= 0) {
        return { key: text, value: "" }
    }
    return { key: text.substring(0, index).trim(), value: text.substring(index + 1).trim() }
}

// `ai-usagebar --json` renders the panel text as Pango markup. Qt's rich text
// understands a different dialect, so the supported span attributes are
// translated and every other angle bracket is escaped rather than trusted.
function pangoToHtml(source) {
    var text = String(source || "")
    if (text === "") {
        return ""
    }
    var spans = []
    text = text.replace(/<span\s+([^>]*)>/g, function(match, attributes) {
        spans.push(spanStyle(attributes))
        return "\u0001" + (spans.length - 1) + "\u0002"
    })
    text = text.replace(/<\/span>/g, "\u0003")
    text = text.replace(/&(?!(amp|lt|gt|quot|apos|#\d+);)/g, "&amp;")
        .replace(/</g, "&lt;")
        .replace(/>/g, "&gt;")
    text = text.replace(/\u0001(\d+)\u0002/g, function(match, index) {
        return '<span style="' + spans[Number(index)] + '">'
    })
    return text.replace(/\u0003/g, "</span>")
}

function spanStyle(attributes) {
    var style = ""
    var foreground = attributes.match(/foreground\s*=\s*['"]([^'"]+)['"]/)
    if (foreground && isSafeColor(foreground[1])) {
        style += "color:" + foreground[1] + ";"
    }
    var background = attributes.match(/background\s*=\s*['"]([^'"]+)['"]/)
    if (background && isSafeColor(background[1])) {
        style += "background-color:" + background[1] + ";"
    }
    var weight = attributes.match(/font_weight\s*=\s*['"]([a-z0-9]+)['"]/)
    if (weight) {
        style += "font-weight:" + weight[1] + ";"
    }
    var italic = attributes.match(/font_style\s*=\s*['"]([a-z]+)['"]/)
    if (italic) {
        style += "font-style:" + italic[1] + ";"
    }
    return style
}

function isSafeColor(value) {
    return /^#[0-9a-fA-F]{3,8}$/.test(value) || /^[a-zA-Z]+$/.test(value)
}
