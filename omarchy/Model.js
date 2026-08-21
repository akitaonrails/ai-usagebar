// Pure data shaping for the Omarchy Quattro widget. Keep this file free of
// QML globals so the exact report contract and selection behavior can also be
// exercised by Node in CI.

function cleanText(value, maxLength) {
  var text = value === undefined || value === null ? "" : String(value)
  // The Rust projection already strips terminal controls. This second, cheap
  // boundary keeps hand-authored/older JSON from putting controls in a
  // long-lived shell process.
  text = text.replace(/[\t\r]/g, " ")
    .replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f-\u009f]/g, "")
    .replace(/[\u200e\u200f\u202a-\u202e\u2066-\u2069]/g, "")
  var limit = Number(maxLength) || 2048
  if (text.length <= limit) return text
  var end = limit - 1
  var finalCodeUnit = text.charCodeAt(end - 1)
  if (finalCodeUnit >= 0xd800 && finalCodeUnit <= 0xdbff) end--
  return text.slice(0, end) + "…"
}

// Shared Omarchy components use Text.AutoText. Replace angle brackets before
// passing provider-controlled labels into those components so they can never
// be reclassified as rich text (including an image tag with a remote URL).
function autoTextSafe(value) {
  return cleanText(value, 1000)
    .replace(/[\n\u2028\u2029]/g, " ")
    .replace(/</g, "‹")
    .replace(/>/g, "›")
}

function finitePercent(value) {
  var number = Number(value)
  if (!isFinite(number)) return null
  return Math.max(0, Math.min(100, Math.round(number)))
}

function normalizeSection(raw) {
  if (!raw || typeof raw !== "object") return null
  var type = String(raw.type || "")
  if (type === "spacer") return { type: "spacer" }
  if (type === "metric") {
    var percent = finitePercent(raw.percent)
    if (percent === null) return null
    var severity = String(raw.severity || "")
    if (["low", "mid", "high", "critical"].indexOf(severity) < 0)
      severity = percent >= 90 ? "critical" : percent >= 75 ? "high" : percent >= 50 ? "mid" : "low"
    return {
      type: "metric",
      label: cleanText(raw.label, 160),
      percent: percent,
      value: cleanText(raw.value, 240),
      detail: cleanText(raw.detail, 1000),
      severity: severity,
      reset_at: cleanText(raw.reset_at, 80)
    }
  }
  if (type === "text") {
    return {
      type: "text",
      label: cleanText(raw.label, 160),
      value: cleanText(raw.value, 1000)
    }
  }
  if (type === "block") {
    var body = Array.isArray(raw.body) ? raw.body : []
    var lines = []
    for (var i = 0; i < body.length && i < 24; i++) lines.push(cleanText(body[i], 1000))
    return { type: "block", label: cleanText(raw.label, 160), body: lines }
  }
  return null
}

function normalizeEntry(raw) {
  if (!raw || typeof raw !== "object") return null
  var id = cleanText(raw.id, 180).trim()
  if (id === "") return null
  var sourceSections = Array.isArray(raw.sections) ? raw.sections : []
  var sections = []
  for (var i = 0; i < sourceSections.length && i < 96; i++) {
    var section = normalizeSection(sourceSections[i])
    if (section) sections.push(section)
  }
  var error = raw.error === undefined || raw.error === null ? "" : cleanText(raw.error, 1200)
  return {
    id: id,
    name: cleanText(raw.name, 240),
    display_name: cleanText(raw.display_name, 240),
    plan: cleanText(raw.plan, 240),
    status: error !== "" || raw.status === "error" ? "error" : "ready",
    error: error,
    stale: raw.stale === true,
    fetched_at: cleanText(raw.fetched_at, 80),
    sections: sections
  }
}

function parseReport(raw) {
  try {
    var parsed = JSON.parse(String(raw || ""))
    if (!parsed || !Array.isArray(parsed.entries))
      return { ok: false, error: "The usage command returned an unsupported report.", primary: "", entries: [] }
    var entries = []
    for (var i = 0; i < parsed.entries.length && i < 64; i++) {
      var entry = normalizeEntry(parsed.entries[i])
      if (entry) entries.push(entry)
    }
    if (parsed.entries.length > 0 && entries.length === 0)
      return { ok: false, error: "The usage report did not contain a valid provider entry.", primary: "", entries: [] }
    return { ok: true, error: "", primary: cleanText(parsed.primary, 180).trim(), entries: entries }
  } catch (error) {
    return { ok: false, error: "The usage command returned invalid JSON.", primary: "", entries: [] }
  }
}

function baseProvider(id) {
  return String(id || "").split("@")[0]
}

function providerName(entry) {
  if (!entry) return "AI usage"
  // The Rust report owns canonical product names. `name` is the compatible
  // fallback for older binaries; the machine id is only a last resort.
  var title = cleanText(entry.display_name || entry.name, 240).trim()
  if (title === "") title = baseProvider(entry.id).replace(/_/g, " ") || "AI usage"
  return autoTextSafe(title)
}

function filteredEntries(entries, configuredProvider) {
  var list = Array.isArray(entries) ? entries : []
  var wanted = String(configuredProvider || "").trim().toLowerCase()
  if (wanted === "") return list.slice()
  var exact = list.filter(function(entry) { return String(entry.id).toLowerCase() === wanted })
  if (exact.length > 0) return exact
  return list.filter(function(entry) { return baseProvider(entry.id).toLowerCase() === wanted })
}

function selectedIndex(entries, selectedId) {
  var list = Array.isArray(entries) ? entries : []
  for (var i = 0; i < list.length; i++) if (list[i].id === selectedId) return i
  return list.length > 0 ? 0 : -1
}

function preferredEntryId(entries, primaryProvider, rememberedEntryId) {
  var list = Array.isArray(entries) ? entries : []
  if (list.length === 0) return ""
  var remembered = cleanText(rememberedEntryId, 180).trim().toLowerCase()
  for (var r = 0; r < list.length; r++)
    if (String(list[r].id).toLowerCase() === remembered) return list[r].id
  var primary = String(primaryProvider || "").toLowerCase()
  for (var i = 0; i < list.length; i++)
    if (String(list[i].id).toLowerCase() === primary) return list[i].id
  for (var j = 0; j < list.length; j++)
    if (baseProvider(list[j].id).toLowerCase() === primary) return list[j].id
  return list[0].id
}

function settingsWithOverrides(settings, moduleName, overrides) {
  var moduleId = cleanText(moduleName, 180).trim()
  if (moduleId === "" || !overrides || typeof overrides !== "object" || Array.isArray(overrides))
    return null

  var next = { id: moduleId }
  var current = settings && typeof settings === "object" && !Array.isArray(settings)
    ? settings : {}
  for (var key in current) {
    if (key === "id" || key === "__proto__" || key === "constructor" || key === "prototype")
      continue
    next[key] = current[key]
  }
  for (var overrideKey in overrides) {
    if (overrideKey === "id" || overrideKey === "__proto__" || overrideKey === "constructor"
        || overrideKey === "prototype") continue
    next[overrideKey] = overrides[overrideKey]
  }
  return next
}

function settingsWithSelectedEntry(settings, moduleName, entryId) {
  var selected = cleanText(entryId, 180).trim()
  if (selected === "") return null
  return settingsWithOverrides(settings, moduleName, { lastSelectedEntryId: selected })
}

function booleanSetting(value, fallback) {
  if (value === true || value === false) return value
  var normalized = String(value === undefined || value === null ? "" : value).trim().toLowerCase()
  if (["true", "1", "yes", "on"].indexOf(normalized) >= 0) return true
  if (["false", "0", "no", "off"].indexOf(normalized) >= 0) return false
  return fallback === true
}

function barLabel(alarming, vertical, showValue, loading, hasEntry, summaryText) {
  var icon = "󰚩"
  if (vertical) return alarming ? "󰅙" : icon
  if (loading && !hasEntry) return icon + "  …"
  if (!hasEntry) return alarming ? "󰅙" : icon
  if (!showValue) return icon
  var summary = autoTextSafe(summaryText).trim()
  return summary === "" ? icon : icon + "  " + summary
}

function headline(entry) {
  if (!entry) return { text: "", percent: null, severity: "low", label: "" }
  var best = null
  var sections = entry.sections || []
  for (var i = 0; i < sections.length; i++) {
    var section = sections[i]
    if (section.type === "metric" && (!best || section.percent > best.percent)) best = section
  }
  if (best) {
    var bestText = /balance/i.test(best.label) && best.value !== ""
      ? best.value : best.percent + "%"
    return {
      text: bestText,
      percent: best.percent,
      severity: best.severity,
      label: best.label
    }
  }
  for (var j = 0; j < sections.length; j++) {
    var row = sections[j]
    if (row.type === "text" && /(balance|available|spend|prepaid)/i.test(row.label) && row.value !== "")
      return { text: row.value, percent: null, severity: "low", label: row.label }
  }
  return { text: entry.status === "error" ? "Error" : "Ready", percent: null, severity: "low", label: "" }
}

function isAlarming(entry) {
  if (!entry) return false
  var summary = headline(entry)
  return entry.status === "error" || entry.stale === true || summary.severity === "critical"
}

function formatDuration(milliseconds) {
  if (!(milliseconds > 0)) return "now"
  var minutes = Math.floor(milliseconds / 60000)
  var hours = Math.floor(minutes / 60)
  var days = Math.floor(hours / 24)
  if (days > 0) return days + "d " + (hours % 24) + "h"
  if (hours > 0) return hours + "h " + (minutes % 60) + "m"
  return Math.max(1, minutes) + "m"
}

function formatReset(resetAt, nowMs) {
  if (!resetAt) return ""
  var resetMs = new Date(String(resetAt)).getTime()
  if (!isFinite(resetMs)) return ""
  var remaining = resetMs - Number(nowMs)
  return remaining > 0 ? "Resets in " + formatDuration(remaining) : "Reset due"
}

function formatUpdated(fetchedAt, nowMs) {
  if (!fetchedAt) return "Updated time unavailable"
  var fetchedMs = new Date(String(fetchedAt)).getTime()
  if (!isFinite(fetchedMs)) return "Updated time unavailable"
  var elapsed = Math.max(0, Number(nowMs) - fetchedMs)
  if (elapsed < 60000) return "Updated just now"
  return "Updated " + formatDuration(elapsed) + " ago"
}

function metricDetail(row) {
  var detail = cleanText(row && row.detail, 1000)
  if (!row || !row.reset_at) return detail
  // Older human-readable reset text remains useful to CLI consumers. Strip
  // just that fragment in the native panel, which renders a live countdown.
  detail = detail.replace(/^Resets in [^·]+\s*(?:·\s*)?/i, "")
  detail = detail.replace(/\s*·\s*reset\s+[^·]+$/i, "")
  return detail.trim()
}

function errorMessage(value) {
  var message = cleanText(value, 500).trim()
  return message === "" ? "The usage command failed without an error message." : message
}

// The panel launches ai-usagebar through /usr/bin/env, so a missing binary
// comes back as exit 127 instead of the process simply never starting.
// Quickshell does not emit `exited` when it cannot launch a binary directly --
// it only logs an internal warning -- which used to leave the widget stuck on
// its loading state with no way to explain that the binary was not installed.
function launchErrorMessage(exitCode, stderrText) {
  if (Number(exitCode) === 127)
    return "ai-usagebar is not installed. The plugin is only the display frontend. Install the binary with: omarchy pkg aur add ai-usagebar-bin"
  return errorMessage(stderrText)
}

function settingsId(value) {
  var id = cleanText(value, 80).trim()
  if (!/^[a-z0-9_-]+$/.test(id)
      || id === "__proto__" || id === "constructor" || id === "prototype") return ""
  return id
}

// The Rust bridge deliberately returns only key-presence booleans. Keep this
// parser strict so a compromised/older helper cannot smuggle rich text or an
// unbounded model into the long-lived shell process.
function parseSettingsSnapshot(raw) {
  try {
    var parsed = JSON.parse(String(raw || ""))
    if (!parsed || Number(parsed.schema_version) !== 1
        || !Array.isArray(parsed.primary_choices) || !Array.isArray(parsed.keys))
      return { ok: false, error: "The settings command returned an unsupported response.", primary: "", primary_choices: [], keys: [] }

    var choices = []
    for (var i = 0; i < parsed.primary_choices.length && i < 64; i++) {
      var choice = parsed.primary_choices[i]
      var choiceId = settingsId(choice && choice.id)
      if (choiceId === "") continue
      choices.push({ id: choiceId, value: choiceId, label: cleanText(choice.label, 120) || choiceId })
    }

    var keys = []
    for (var j = 0; j < parsed.keys.length && j < 32; j++) {
      var key = parsed.keys[j]
      var keyId = settingsId(key && key.id)
      if (keyId === "") continue
      keys.push({
        id: keyId,
        label: cleanText(key.label, 120) || keyId,
        environment: cleanText(key.environment, 160),
        note: cleanText(key.note, 240),
        configured: key.configured === true,
        inline_configured: key.inline_configured === true,
        environment_configured: key.environment_configured === true
      })
    }

    var primary = settingsId(parsed.primary)
    var primaryAvailable = false
    for (var k = 0; k < choices.length; k++) {
      if (choices[k].id === primary) {
        primaryAvailable = true
        break
      }
    }
    if (!primaryAvailable) primary = choices.length > 0 ? choices[0].id : ""
    return { ok: true, error: "", primary: primary, primary_choices: choices, keys: keys }
  } catch (error) {
    return { ok: false, error: "The settings command returned invalid JSON.", primary: "", primary_choices: [], keys: [] }
  }
}

function buildSettingsPatch(primary, changes) {
  var primaryId = settingsId(primary)
  var rawPrimary = String(primary || "").trim()
  if (rawPrimary !== "" && primaryId === "")
    return { ok: false, error: "Choose a valid primary provider.", payload: "" }
  var keys = {}
  var list = Array.isArray(changes) ? changes : []
  var seen = []
  for (var i = 0; i < list.length; i++) {
    var change = list[i] || {}
    var id = settingsId(change.id)
    if (id === "" || seen.indexOf(id) >= 0)
      return { ok: false, error: "A settings row has an invalid provider id.", payload: "" }
    seen.push(id)
    if (change.action === "clear") {
      keys[id] = { action: "clear" }
    } else if (change.action === "set") {
      var value = String(change.value || "")
      if (value === "") return { ok: false, error: "An edited API key is empty.", payload: "" }
      if (value.length > 16384) return { ok: false, error: "An API key is too long.", payload: "" }
      keys[id] = { action: "set", value: value }
    } else return { ok: false, error: "A settings row has an invalid action.", payload: "" }
  }
  if (primaryId === "" && seen.length === 0)
    return { ok: false, error: "There are no settings changes to save.", payload: "" }
  var patch = { schema_version: 1, keys: keys }
  if (primaryId !== "") patch.primary = primaryId
  return {
    ok: true,
    error: "",
    payload: JSON.stringify(patch)
  }
}

function parseSettingsApplyResult(raw) {
  try {
    var parsed = JSON.parse(String(raw || ""))
    return parsed && parsed.ok === true
  } catch (error) {
    return false
  }
}
