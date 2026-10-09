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
    var windowSecs = Math.floor(Number(raw.window_secs))
    if (!isFinite(windowSecs) || windowSecs <= 0) windowSecs = null
    // Which number the bar draws. Anything but an explicit "value" — including
    // a report old enough not to carry the field — leaves it a percentage.
    var headline = raw.headline === "value" ? "value" : "percent"
    return {
      type: "metric",
      label: cleanText(raw.label, 160),
      percent: percent,
      value: cleanText(raw.value, 240),
      detail: cleanText(raw.detail, 1000),
      headline: headline,
      severity: severity,
      reset_at: cleanText(raw.reset_at, 80),
      window_secs: windowSecs,
      group: cleanText(raw.group, 80)
    }
  }
  if (type === "text") {
    var text = {
      type: "text",
      label: cleanText(raw.label, 160),
      value: cleanText(raw.value, 1000)
    }
    // Cursor's On-Demand row. Kept only as safe integers so a report cannot
    // smuggle a string or a float in where the meter expects cents.
    var used = minorUnits(raw.used_cents)
    var limit = minorUnits(raw.limit_cents)
    var consumed = minorUnits(raw.percent)
    if (used !== null) text.used_cents = used
    if (limit !== null && limit > 0) text.limit_cents = limit
    if (consumed !== null && consumed >= 0) text.percent = consumed
    return text
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
    short_name: cleanText(raw.short_name, 24),
    icon: cleanText(raw.icon, 8).trim(),
    brand: cleanText(raw.brand, 32).trim(),
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

// The Waybar-style provider tag. `VendorId::short_name` in Rust owns the codes
// and ships them as `short_name`; a binary older than that field has none, so
// the machine id's vendor half stands in rather than a table living here.
function providerShort(entry) {
  if (!entry) return ""
  var code = cleanText(entry.short_name, 24).trim()
  if (code === "") code = baseProvider(entry.id).replace(/_/g, "-")
  return autoTextSafe(code).trim()
}

function providerIcon(entry) {
  if (!entry) return "󰚩"
  var icon = autoTextSafe(cleanText(entry.icon, 8).trim())
  return icon === "" ? "󰚩" : icon
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

// `providerLabel` is already resolved by the caller: empty when the opt-in
// provider switch is off, so the icon-and-value label is unchanged for everyone
// who never turns it on. A vertical bar has no width for either field and
// keeps showing the icon alone.
function barLabel(alarming, vertical, showValue, loading, hasEntry, summaryText,
                  providerLabel, icon) {
  icon = autoTextSafe(icon || "").trim() || "󰚩"
  if (vertical) return alarming ? "󰅙" : icon
  if (loading && !hasEntry) return icon + "  …"
  if (!hasEntry) return alarming ? "󰅙" : icon
  var provider = autoTextSafe(providerLabel).trim()
  var summary = showValue ? autoTextSafe(summaryText).trim() : ""
  if (provider === "") return summary === "" ? icon : icon + "  " + summary
  // One space between tag and value, matching Waybar's
  // `{vendor_short} {session_pct}%`; the wider gap stays next to the icon.
  return summary === "" ? icon + "  " + provider
    : icon + "  " + provider + " " + summary
}

/**
 * Normalizes how a percentage reads: the consumed share ("used") or what is left of the same window ("left").
 * Anything but an explicit "left" stays "used", so an existing shell.json keeps its reading.
 * @param {*} value Raw setting value.
 * @returns {"used"|"left"}
 */
function normalizeShowAs(value) {
  var text = cleanText(value, 16).trim().toLowerCase()
  return text === "left" || text === "remaining" ? "left" : "used"
}

/**
 * Converts a used percentage to the number drawn for the chosen reading.
 * Which window the bar picks never depends on this; only the number drawn for it does.
 * @param {number|string} percent Used percentage from the report.
 * @param {*} showAs Raw reading setting.
 * @returns {number|null} The percentage to draw, or null when the input is not a number.
 */
function shownPercent(percent, showAs) {
  var number = Number(percent)
  if (!isFinite(number)) return null
  return normalizeShowAs(showAs) === "left" ? Math.max(0, 100 - number) : number
}

/**
 * Formats a used percentage as drawn for the chosen reading.
 * @param {number|string} percent Used percentage from the report.
 * @param {*} showAs Raw reading setting.
 * @returns {string} For example "18%", or an empty string when the input is not a number.
 */
function percentText(percent, showAs) {
  var shown = shownPercent(percent, showAs)
  return shown === null ? "" : shown + "%"
}

var UNSAFE_KEYS = ["__proto__", "constructor", "prototype"]

/**
 * Tells whether an object owns a key itself, ignoring anything inherited.
 * @param {Object} object Object to inspect.
 * @param {string} key Key to look for.
 * @returns {boolean}
 */
function hasOwn(object, key) {
  return Object.prototype.hasOwnProperty.call(object, key)
}

/**
 * Computes the key each metric of an entry is hidden under: its label behind the heading it sits under
 * (the report's `group`, or the text heading row above it, as Antigravity's Session and Weekly), so rows
 * with the same label in different groups stay distinct. A label repeated under the same heading is numbered.
 * @param {Object} entry Entry from the report.
 * @returns {Array<{key: string, group: string}|null>} One item per section, null for a section that is not a metric.
 */
function metricKeys(entry) {
  var sections = entry && Array.isArray(entry.sections) ? entry.sections : []
  var keys = []
  var heading = ""
  var seen = {}
  for (var i = 0; i < sections.length; i++) {
    var section = sections[i]
    if (isHeadingRow(section)) {
      heading = cleanText(section.label, 80).trim()
      keys.push(null)
      continue
    }
    if (!section || section.type !== "metric") {
      keys.push(null)
      continue
    }
    var group = cleanText(section.group, 80).trim() || heading
    var label = cleanText(section.label, 160).trim()
    var base = group === "" ? label : group + " / " + label
    var count = hasOwn(seen, base) ? seen[base] : 0
    seen[base] = count + 1
    keys.push({ key: count > 0 ? base + " #" + (count + 1) : base, group: group })
  }
  return keys
}

/**
 * Tells whether a section is a group heading: a text row with a label and no value.
 * @param {Object} section Section from the report.
 * @returns {boolean}
 */
function isHeadingRow(section) {
  if (!section || section.type !== "text") return false
  var value = section.value === undefined || section.value === null ? "" : String(section.value)
  return cleanText(section.label, 160).trim() !== "" && value.trim() === ""
}

/**
 * Reads the `hiddenMetrics` setting, a map from an entry id to the metric keys switched off for it.
 * The value is hand-editable and lives in a long-lived shell process, so it is read strictly and kept bounded.
 * @param {*} raw Raw setting value.
 * @returns {Object<string, string[]>} Entry id to hidden metric keys, without empty lists.
 */
function normalizeHiddenMetrics(raw) {
  var out = {}
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return out
  var ids = Object.keys(raw)
  var kept = 0
  for (var i = 0; i < ids.length && kept < 64; i++) {
    var id = cleanText(ids[i], 180).trim()
    if (id === "" || UNSAFE_KEYS.indexOf(id) >= 0) continue
    var list = Array.isArray(raw[ids[i]]) ? raw[ids[i]] : []
    var keys = []
    for (var j = 0; j < list.length && j < 32; j++) {
      var key = cleanText(list[j], 240).trim()
      if (key !== "" && keys.indexOf(key) < 0) keys.push(key)
    }
    if (keys.length > 0) {
      out[id] = keys
      kept++
    }
  }
  return out
}

/**
 * Lists the metric keys switched off for one entry.
 * @param {*} hidden Raw `hiddenMetrics` setting.
 * @param {string} entryId Entry id, matched exactly (so `anthropic@work` and `anthropic` are independent).
 * @returns {string[]}
 */
function hiddenKeysFor(hidden, entryId) {
  var id = cleanText(entryId, 180).trim()
  var map = normalizeHiddenMetrics(hidden)
  return hasOwn(map, id) ? map[id] : []
}

/**
 * Computes the next `hiddenMetrics` after switching one metric of one entry on or off.
 * An entry left with nothing hidden drops out, so the map never carries empty lists. The input is not mutated.
 * @param {*} hidden Raw `hiddenMetrics` setting.
 * @param {string} entryId Entry id.
 * @param {string} key Metric key from `metricKey`.
 * @returns {Object<string, string[]>}
 */
function toggleHiddenMetric(hidden, entryId, key) {
  var next = normalizeHiddenMetrics(hidden)
  var id = cleanText(entryId, 180).trim()
  var metric = cleanText(key, 240).trim()
  if (id === "" || metric === "" || UNSAFE_KEYS.indexOf(id) >= 0) return next
  var list = hasOwn(next, id) ? next[id].slice() : []
  var at = list.indexOf(metric)
  if (at >= 0) list.splice(at, 1)
  else if (list.length < 32) list.push(metric)
  if (list.length === 0) delete next[id]
  else next[id] = list
  return next
}

/**
 * Returns the entry as the bar and the tooltip see it, without the metrics switched off for it, so the
 * highest-percent choice, the alert state and every echo of them never read a row the user hid.
 * A hidden metric takes the spacer that leads it when it closes its run of rows (a group that shares one spacer keeps it for
 * the next row), and a heading left with no metric under it goes too, with the
 * spacer that precedes it, so no gap is left behind. At least one metric always stays: a hidden list that
 * would remove every metric (a hand-edited shell.json) is ignored, so the bar never goes blank.
 * The settings page still offers the original entry, hidden rows included, so they can be switched back on.
 * @param {Object} entry Entry from the report.
 * @param {string[]} hiddenKeys Metric keys switched off for the entry.
 * @returns {Object} The entry itself when nothing is hidden, otherwise a copy without the hidden metrics.
 */
function visibleEntry(entry, hiddenKeys) {
  var hidden = Array.isArray(hiddenKeys) ? hiddenKeys : []
  if (!entry || hidden.length === 0) return entry
  var sections = Array.isArray(entry.sections) ? entry.sections : []
  var keys = metricKeys(entry)
  var kept = []
  var metrics = 0
  var keptMetrics = 0
  var block = null
  var closeBlock = function() {
    if (block && block.had > 0 && block.kept === 0) {
      var spaced = block.start > 0 && kept[block.start - 1].type === "spacer"
      kept.splice(spaced ? block.start - 1 : block.start, spaced ? 2 : 1)
    }
    block = null
  }
  for (var i = 0; i < sections.length; i++) {
    var section = sections[i]
    if (isHeadingRow(section)) {
      closeBlock()
      block = { start: kept.length, had: 0, kept: 0 }
    }
    if (keys[i]) {
      metrics++
      if (block) block.had++
      if (hidden.indexOf(keys[i].key) >= 0) {
        var after = sections[i + 1]
        var endsRun = !after || after.type === "spacer" || isHeadingRow(after)
        if (endsRun && kept.length > 0 && kept[kept.length - 1].type === "spacer") kept.pop()
        continue
      }
      keptMetrics++
      if (block) block.kept++
    }
    kept.push(section)
  }
  closeBlock()
  if (metrics > 0 && keptMetrics === 0) return entry
  if (metrics === keptMetrics) return entry
  var copy = {}
  for (var field in entry) copy[field] = entry[field]
  copy.sections = kept
  return copy
}

/**
 * Tells whether a metric's switch may be flipped. Showing a hidden metric is always allowed; hiding one is
 * refused when it is the last metric of the entry still on, the rule Cursor's pools already follow.
 * @param {Object} entry Entry from the report.
 * @param {*} hidden Raw `hiddenMetrics` setting.
 * @param {string} key Metric key from `metricKeys`, or a `window:` row key from `metricChoices`.
 * @returns {boolean}
 */
function canToggleMetric(entry, hidden, key) {
  if (!entry || key === "") return false
  if (String(key).indexOf(WINDOW_KEY_PREFIX) === 0) {
    var row = metricChoices(entry, hidden).filter(function(item) { return item.key === key })[0]
    return row ? row.canToggle : false
  }
  var off = hiddenKeysFor(hidden, entry.id)
  if (off.indexOf(key) >= 0) return true
  var visible = 0
  metricKeys(entry).forEach(function(item) {
    if (item && off.indexOf(item.key) < 0) visible++
  })
  return visible > 1
}

function barChip(entry, showValue, showProvider, barWindow, showAs) {
  if (!entry) return ""
  var icon = providerIcon(entry)
  var provider = showProvider ? providerShort(entry) : ""
  var summary = ""
  if (showValue) {
    if (entry.error) summary = "!"
    else summary = autoTextSafe(headline(entry, barWindow, showAs).text).trim()
  }
  if (provider === "") return summary === "" ? icon : icon + "  " + summary
  return summary === "" ? icon + "  " + provider : icon + "  " + provider + " " + summary
}

// Every visible entry as its own icon+value chip. A vertical bar has no
// width for the strip and keeps a single glyph, same as `barLabel`.
function barStrip(entries, alarming, vertical, showValue, showProvider, loading, barWindow, showAs) {
  var list = Array.isArray(entries) ? entries : []
  if (vertical) return alarming ? "󰅙" : "󰚩"
  if (loading && list.length === 0) return "󰚩  …"
  if (list.length === 0) return alarming ? "󰅙" : "󰚩"
  var chips = []
  for (var i = 0; i < list.length; i++) {
    var chip = barChip(list[i], showValue, showProvider, barWindow, showAs)
    if (chip !== "") chips.push(chip)
  }
  return chips.length === 0 ? "󰚩" : chips.join("  ")
}

function anyAlarming(entries) {
  var list = Array.isArray(entries) ? entries : []
  for (var i = 0; i < list.length; i++) {
    if (isAlarming(list[i])) return true
  }
  return false
}

// The mark an entry is drawn with. The report names the provider — its own
// for a built-in, the one a `[[custom]]` provider borrowed through `brand` —
// and this file owns the artwork, because each frontend ships its own. An
// older binary sends no `brand`, so the id still resolves the built-ins.
function brandIconFile(entry) {
  var declared = cleanText(entry && entry.brand, 32).trim()
  return brandFileFor(declared !== "" ? declared : baseProvider(entry && entry.id))
}

// Official brand marks shipped next to this file. A missing file falls back
// to the nerd-font glyph from the Rust report — the table is asset lookup,
// not a second copy of provider names.
function brandFileFor(provider) {
  switch (provider) {
    case "anthropic":
      return "claude.svg"
    case "anthropic_api":
      return "anthropic.svg"
    case "openai":
      return "openai.svg"
    case "copilot":
      return "copilot.svg"
    case "devin":
      return "devin.svg"
    case "zai":
      return "zhipu.svg"
    case "openrouter":
      return "openrouter.svg"
    case "deepseek":
      return "deepseek.svg"
    case "kimi":
      return "kimi.svg"
    case "kilo":
      return "kilo.svg"
    case "novita":
      return "novita.svg"
    case "moonshot":
      return "moonshot.svg"
    case "grok":
    case "supergrok":
      return "grok.svg"
    case "grokbot":
      return "grokbot.svg"
    case "antigravity":
      return "antigravity.svg"
    case "cursor":
      return "cursor.svg"
    case "minimax":
      return "minimax.svg"
    case "kiro":
      return "kiro.svg"
    case "nous":
      return "nous.svg"
    case "opencode-go":
      return "opencode.svg"
    case "ollama":
      return "ollama.svg"
    default:
      return ""
  }
}

function barChips(entries, selected, showAll, showValue, showProvider, loading, alarming, vertical, barWindow, showAs, brandIcons) {
  var list = Array.isArray(entries) ? entries : []
  if (vertical) {
    return [{ brand: "", icon: alarming ? "󰅙" : "󰚩", label: "", alarming: alarming === true }]
  }
  if (loading && list.length === 0) {
    return [{ brand: "", icon: "󰚩", label: "…", alarming: false }]
  }
  if (list.length === 0) {
    return [{ brand: "", icon: alarming ? "󰅙" : "󰚩", label: "", alarming: alarming === true }]
  }
  var shown = showAll ? list : list.filter(function(entry) { return selected && entry.id === selected.id })
  if (shown.length === 0) shown = [list[0]]
  var chips = []
  var plain = brandIcons === false
  for (var i = 0; i < shown.length; i++) {
    var entry = shown[i]
    var brand = plain ? "" : brandIconFile(entry)
    var labelOnly = brand === "" && !(plain && shown.length === 1)
    var tag = providerShort(entry)
    var label = ""
    if (showProvider && !labelOnly) label = tag
    if (showValue) {
      var summary = entry.error ? "!" : autoTextSafe(headline(entry, barWindow, showAs).text).trim()
      label = label === "" ? summary : (summary === "" ? label : label + " " + summary)
    }
    if (labelOnly) label = label === "" ? tag : tag + " " + label
    chips.push({
      // The bar turns each chip into a target for its own entry.
      id: entry.id,
      brand: brand,
      icon: brand !== "" ? providerIcon(entry) : (labelOnly ? "" : "󰚩"),
      tag: tag,
      labelOnly: labelOnly,
      label: label,
      // Alert state always follows the highest-percent window, never the
      // pinned one: barWindow changes only the displayed value.
      alarming: isAlarming(entry)
    })
  }
  return chips
}

// The bar presses a slot's widget by geometry: it maps the point into every
// registered click target and takes the newest one whose rect contains it,
// falling back to the slot's button. A chip that registered only its glyph left
// the button to swallow the rest of the slot, so a press aimed at a chip above,
// below or beside it toggled whichever entry was already selected. Each chip
// therefore registers its whole column of the slot, and the gaps between chips
// split at their midpoint: the first and last chips own half of the gap beside
// them, a middle chip a whole one. The outer columns also own the button's
// padding at either end (`edge`), so the columns tile the whole widget and its
// width stays exactly what the plain spacing and padding produced.
function chipHitGaps(index, count, gap, edge) {
  var half = (Number(gap) || 0) / 2
  var outer = Number(edge) || 0
  var chips = Number(count) || 0
  var position = Number(index) || 0
  return {
    left: position === 0 ? outer : half,
    right: position >= chips - 1 ? outer : half
  }
}

// Which usage window the top bar shows. "auto" keeps the historical
// highest-percent metric; the rest pin one window class across vendors.
// Unknown, empty, and legacy values fall back to "auto" so an existing
// shell.json never goes blank after an update.
function normalizeBarWindow(value) {
  var text = cleanText(value, 24).trim().toLowerCase()
  if (text === "" || text === "auto" || text === "highest" || text === "max") return "auto"
  if (text === "session" || text === "5h" || text === "5-hour" || text === "5hour"
      || text === "5hr" || text === "five-hour" || text === "rolling"
      || text === "shortest" || text === "session-5h") return "session"
  if (text === "weekly" || text === "week" || text === "7d" || text === "7-day"
      || text === "weekly-7d") return "weekly"
  if (text === "monthly" || text === "month" || text === "30d" || text === "monthly-cycle") return "monthly"
  return "auto"
}

// The two window lengths the report states exactly. Same values as the Rust
// constants that publish them: openai/types.rs SESSION_WINDOW_SECS /
// WEEKLY_WINDOW_SECS, opencode_go/vendor.rs and kimi/vendor.rs ROLLING_WINDOW /
// WEEKLY_WINDOW, anthropic/types.rs SESSION / WEEKLY. A vendor that states any
// other length falls through to label matching instead of being forced into
// one of these classes.
var SESSION_WINDOW_SECS = 18000
var WEEKLY_WINDOW_SECS = 604800

function metricMatchesWindow(section, want) {
  if (!section || section.type !== "metric") return false
  var label = String(section.label || "")
  var secs = Math.floor(Number(section.window_secs))
  var hasSecs = isFinite(secs) && secs > 0
  var knownSize = hasSecs && (secs === SESSION_WINDOW_SECS || secs === WEEKLY_WINDOW_SECS)
  // A known size is authoritative; an unknown positive size is treated as
  // missing so labels decide. Labels only cover older reports that predate
  // window_secs.
  if (want === "session") {
    if (knownSize) return secs === SESSION_WINDOW_SECS
    return /(\b5\s*-?\s*h(?:ours?|rs?)?\b|\bsessions?\b|\brolling\b)/i.test(label)
  }
  if (want === "weekly") {
    if (knownSize) return secs === WEEKLY_WINDOW_SECS
    return /(\bweek(?:ly|s)?\b|\b7\s*-?\s*d(?:ays?)?\b)/i.test(label)
  }
  if (want === "monthly") {
    // Monthly pools have no single fixed length (opencode-go publishes
    // none; Z.AI uses 30d; Cursor/custom vary), so no secs value is
    // authoritative here — except known 5h/7d sizes, which exclude.
    if (knownSize) return false
    return /(\bmonthly\b|\bmonth(?:ly|s)?\b|\bspend\s*\(mo\)|\b30\s*-?\s*d(?:ays?)?\b)/i.test(label)
  }
  return false
}

function maxPercent(sections) {
  var best = null
  for (var i = 0; i < sections.length; i++) {
    var section = sections[i]
    if (section.type === "metric" && (!best || section.percent > best.percent)) best = section
  }
  return best
}

function selectMetric(entry, barWindow) {
  var sections = entry && Array.isArray(entry.sections) ? entry.sections : []
  var metrics = []
  var grouped = []
  for (var i = 0; i < sections.length; i++) {
    var section = sections[i]
    if (!section || section.type !== "metric") continue
    // A grouped row sits under its own heading below the meters (the Claude
    // entry's context sessions, SuperGrok's product slices). It is not a
    // quota window, so it stands in only when the entry has nothing else.
    if (section.group) grouped.push(section)
    else metrics.push(section)
  }
  if (metrics.length === 0) metrics = grouped
  if (metrics.length === 0) return null
  var want = normalizeBarWindow(barWindow)
  if (want === "auto") return maxPercent(metrics)
  var candidates = []
  for (var j = 0; j < metrics.length; j++) {
    if (metricMatchesWindow(metrics[j], want)) candidates.push(metrics[j])
  }
  // A pinned window that a vendor does not offer (a balance-only provider,
  // a weekly-only response, no monthly pool) falls back to the historical
  // highest-percent value rather than blanking the bar.
  if (candidates.length === 0) return maxPercent(metrics)
  return maxPercent(candidates)
}

function severityRank(severity) {
  if (severity === "critical") return 3
  if (severity === "high") return 2
  if (severity === "mid") return 1
  return 0
}

// One Dark defaults — same hexes as src/theme.rs when colors.toml is missing.
var ONE_DARK_PALETTE = {
  green: "#98c379",
  yellow: "#e5c07b",
  orange: "#d19a66",
  red: "#e06c75"
}

// Parse Omarchy theme/colors.toml keys used for RAG (Waybar Theme's source).
// Named keys win over the older color1–3 aliases, matching theme.rs
// merged_with_omarchy after #289 (red.or(color1), not last-occurrence-wins).
// orange falls back to the resolved red when absent.
function parseThemePalette(raw) {
  var greenNamed = ""
  var greenAlias = ""
  var yellowNamed = ""
  var yellowAlias = ""
  var orangeNamed = ""
  var redNamed = ""
  var redAlias = ""
  var lines = String(raw || "").split("\n")
  for (var i = 0; i < lines.length; i++) {
    var match = lines[i].match(/^\s*([A-Za-z0-9_-]+)\s*=\s*["']?(#[0-9A-Fa-f]{6})/)
    if (!match) continue
    var key = match[1]
    var hex = match[2]
    if (key === "green") greenNamed = hex
    else if (key === "color2") greenAlias = hex
    else if (key === "yellow") yellowNamed = hex
    else if (key === "color3") yellowAlias = hex
    else if (key === "orange") orangeNamed = hex
    else if (key === "red") redNamed = hex
    else if (key === "color1") redAlias = hex
  }
  var red = redNamed || redAlias
  var green = greenNamed || greenAlias
  var yellow = yellowNamed || yellowAlias
  var orange = orangeNamed || red
  return {
    green: green || ONE_DARK_PALETTE.green,
    yellow: yellow || ONE_DARK_PALETTE.yellow,
    orange: orange || ONE_DARK_PALETTE.orange,
    red: red || ONE_DARK_PALETTE.red
  }
}

function severityColor(severity, palette) {
  var p = palette || ONE_DARK_PALETTE
  if (severity === "critical") return p.red
  if (severity === "high") return p.orange
  if (severity === "mid") return p.yellow
  if (severity === "low") return p.green
  return p.green
}

// Older reports print On-Demand as "$0.00 / $5.00" and nothing else. A
// current report carries used_cents, limit_cents, and percent, which win.
// This parser stays for a binary that predates those fields.
function parseMoneyToken(token) {
  var text = String(token || "").trim()
  var match = text.match(/^(-)?(\D*?)(\d+(?:\.\d+)?)(\D*)$/)
  if (!match) return null
  var amount = Number(match[3])
  if (!isFinite(amount)) return null
  if (match[1] === "-") amount = -amount
  var places = (match[3].split(".")[1] || "").length
  return { amount: amount, prefix: match[2], suffix: match[4], places: places }
}

function minorUnits(value) {
  if (typeof value !== "number" || !isFinite(value) || !Number.isSafeInteger(value)) return null
  return value
}

// USD cents, matching fmt_minor(..., 2, "USD"): the sign sits ahead of the
// symbol, and a zero amount is never "-$0.00".
function formatUsdCents(cents) {
  var negative = cents < 0
  var abs = negative ? -cents : cents
  var dollars = Math.floor(abs / 100)
  var frac = abs % 100
  var body = dollars + "." + (frac < 10 ? "0" : "") + frac
  if (dollars === 0 && frac === 0) negative = false
  return (negative ? "-" : "") + "$" + body
}

function formatMoney(sample, amount) {
  var negative = amount < 0
  var abs = Math.abs(amount)
  var body = sample.places > 0 ? abs.toFixed(sample.places) : String(Math.round(abs))
  return (negative ? "-" : "") + sample.prefix + body + sample.suffix
}

function usageSeverity(percent) {
  if (percent >= 90) return "critical"
  if (percent >= 75) return "high"
  if (percent >= 50) return "mid"
  return "low"
}

function onDemandView(usedPct, usedText, limitText, leftText) {
  return {
    bar: usedPct + "%",
    usedPct: usedPct,
    severity: usageSeverity(usedPct),
    display: leftText,
    detail: usedText + " of " + limitText + " used (" + usedPct + "%)",
    tooltip: "On-demand " + usedText + " of " + limitText + " used (" + usedPct + "%)"
  }
}

// Same reading as OpenRouter's credit chip: the percentage is how much of
// the prepaid balance is already used. The dollars still left stay beside
// the meter in the panel. Cents from the report are the contract; the
// formatted value is only read when those fields are absent.
function onDemandFromCents(section) {
  var used = minorUnits(section.used_cents)
  var limit = minorUnits(section.limit_cents)
  if (used === null || limit === null || !(limit > 0)) return null
  var usedPct = minorUnits(section.percent)
  if (usedPct === null || usedPct < 0)
    usedPct = Math.round((Math.max(0, used) / limit) * 100)
  var spent = Math.max(0, used)
  var left = Math.max(0, limit - used)
  return onDemandView(usedPct, formatUsdCents(spent), formatUsdCents(limit), formatUsdCents(left))
}

function onDemandFromText(value) {
  var text = String(value || "")
  var slash = text.indexOf("/")
  if (slash < 0) return null
  var used = parseMoneyToken(text.slice(0, slash))
  var limit = parseMoneyToken(text.slice(slash + 1))
  if (!used || !limit || !(limit.amount > 0)) return null
  var factor = Math.pow(10, limit.places)
  var left = Math.max(0, Math.round((limit.amount - used.amount) * factor) / factor)
  var usedPct = Math.round((Math.max(0, used.amount) / limit.amount) * 100)
  var usedText = formatMoney(used, Math.max(0, used.amount))
  var limitText = formatMoney(limit, limit.amount)
  var leftText = formatMoney(limit, left)
  return onDemandView(usedPct, usedText, limitText, leftText)
}

function onDemandRemaining(section) {
  if (section && typeof section === "object") {
    var fromCents = onDemandFromCents(section)
    if (fromCents) return fromCents
    return onDemandFromText(section.value)
  }
  return onDemandFromText(section)
}

function cursorOnDemand(entry) {
  var sections = entry && Array.isArray(entry.sections) ? entry.sections : []
  for (var i = 0; i < sections.length; i++) {
    var section = sections[i]
    if (section && section.type === "text" && section.label === "On-Demand")
      return onDemandRemaining(section)
  }
  return null
}

function cursorPoolPresence(entry) {
  var sections = entry && Array.isArray(entry.sections) ? entry.sections : []
  var models = false
  var other = false
  for (var i = 0; i < sections.length; i++) {
    var section = sections[i]
    if (!section || section.type !== "metric") continue
    if (section.label === "Cursor Models") models = true
    else if (section.label === "Other Models") other = true
  }
  return {
    models: models,
    other: other,
    demand: cursorOnDemand(entry) !== null,
    credits: cursorGrantMeters(sections).length > 0
  }
}

// Fixed display order. A missing or unknown flag stays on, and turning the
// last one off is refused so the bar never goes blank.
function cursorPoolVisibility(flags) {
  var models = !(flags && flags.models === false)
  var other = !(flags && flags.other === false)
  var demand = !(flags && flags.demand === false)
  var credits = !(flags && flags.credits === false)
  if (!models && !other && !demand && !credits) models = true
  return { models: models, other: other, demand: demand, credits: credits }
}

function toggleCursorPool(flags, id) {
  var current = cursorPoolVisibility(flags)
  var next = {
    models: current.models,
    other: current.other,
    demand: current.demand,
    credits: current.credits
  }
  if (id === "models") next.models = !next.models
  else if (id === "other") next.other = !next.other
  else if (id === "demand") next.demand = !next.demand
  else if (id === "credits") next.credits = !next.credits
  else return current
  if (!next.models && !next.other && !next.demand && !next.credits) return current
  return next
}

/**
 * Names the Antigravity pool a metric belongs to.
 * @param {Object} section Section from the report.
 * @returns {"gemini"|"third_party"|""} Empty for a row that is neither Antigravity pool.
 */
function antigravityPoolOf(section) {
  if (!section || section.type !== "metric") return ""
  if (section.label === "Gemini") return "gemini"
  if (section.label === "Claude & GPT OSS") return "third_party"
  return ""
}

/**
 * Lists the Antigravity model pools an entry really carries.
 * @param {Object} entry Entry from the report.
 * @returns {{gemini: boolean, third_party: boolean}}
 */
function antigravityPoolPresence(entry) {
  var sections = entry && Array.isArray(entry.sections) ? entry.sections : []
  var gemini = false
  var thirdParty = false
  for (var i = 0; i < sections.length; i++) {
    var pool = antigravityPoolOf(sections[i])
    if (pool === "gemini") gemini = true
    else if (pool === "third_party") thirdParty = true
  }
  return { gemini: gemini, third_party: thirdParty }
}

/**
 * Normalizes the Antigravity pool switches and keeps one pool visible.
 * @param {*} flags Raw widget switches.
 * @returns {{gemini: boolean, third_party: boolean}}
 */
function antigravityPoolVisibility(flags) {
  var gemini = !(flags && flags.gemini === false)
  var thirdParty = !(flags && flags.third_party === false)
  if (!gemini && !thirdParty) gemini = true
  return { gemini: gemini, third_party: thirdParty }
}

/**
 * Computes the next Antigravity pool switches after one toggle.
 * @param {*} flags Current raw switches.
 * @param {string} id Pool being toggled.
 * @returns {{gemini: boolean, third_party: boolean}}
 */
function toggleAntigravityPool(flags, id) {
  var current = antigravityPoolVisibility(flags)
  var next = {
    gemini: current.gemini,
    third_party: current.third_party
  }
  if (id === "gemini") next.gemini = !next.gemini
  else if (id === "third_party") next.third_party = !next.third_party
  else return current
  return antigravityPoolVisibility(next)
}

/**
 * Selects the Antigravity pools a chip can draw, falling back to one present pool.
 * @param {Object} entry Entry from the report.
 * @param {*} flags Raw widget switches.
 * @returns {{gemini: boolean, third_party: boolean}}
 */
function antigravityBarFlags(entry, flags) {
  var show = antigravityPoolVisibility(flags)
  var has = antigravityPoolPresence(entry)
  var visible = {
    gemini: show.gemini && has.gemini,
    third_party: show.third_party && has.third_party
  }
  if (!visible.gemini && !visible.third_party) {
    if (has.gemini) visible.gemini = true
    else if (has.third_party) visible.third_party = true
  }
  return visible
}

/**
 * Returns the entry without the metrics switched off for it, as the panel, the bar and the pool buttons all
 * read it. A Cursor or Antigravity entry loses only the pool metrics a window row can switch.
 * @param {Object} entry Entry from the report.
 * @param {*} hidden Raw `hiddenMetrics` setting.
 * @returns {Object}
 */
function windowEntry(entry, hidden) {
  if (!entry) return entry
  var provider = baseProvider(entry.id)
  var poolProvider = provider === "cursor" || provider === "antigravity"
  return visibleEntry(entry, poolProvider ? windowHiddenKeys(entry, hidden) : hiddenKeysFor(hidden, entry.id))
}

/**
 * Returns the entry as the panel lists it: the metrics the user switched off are gone, so their meters
 * do not draw. A provider hides metrics through `hiddenMetrics`; Cursor and Antigravity hide their time
 * windows through it (only the pool metrics a window row switches count) and their pools through their own
 * switches. The windows go first, so a pool button never brings a hidden window back. Only a pool the report
 * really carries can be switched off: an On-Demand row without a limit is plain text and always stays.
 * @param {Object} entry Entry from the report.
 * @param {*} hidden Raw `hiddenMetrics` setting.
 * @param {{models: boolean, other: boolean, demand: boolean, credits: boolean}} cursorFlags Cursor pool switches.
 * @param {{gemini: boolean, third_party: boolean}} antigravityFlags Antigravity pool switches.
 * @returns {Object} The entry itself when nothing is hidden, otherwise a copy without the hidden rows.
 */
function panelEntry(entry, hidden, cursorFlags, antigravityFlags) {
  if (!entry) return entry
  var provider = baseProvider(entry.id)
  var poolProvider = provider === "cursor" || provider === "antigravity"
  var base = windowEntry(entry, hidden)
  if (!poolProvider) return base
  var sections = Array.isArray(base.sections) ? base.sections : []
  if (provider === "antigravity") {
    var agyOn = antigravityBarFlags(base, antigravityFlags)
    var keys = metricKeys(base)
    var poolOff = []
    sections.forEach(function(section, i) {
      var id = antigravityPoolOf(section)
      if (id !== "" && agyOn[id] !== true && keys[i]) poolOff.push(keys[i].key)
    })
    return visibleEntry(base, poolOff)
  }
  var has = cursorPoolPresence(base)
  var shown = cursorPoolVisibility(cursorFlags)
  var kept = sections.filter(function(section) {
    var id = cursorPoolOf(section)
    return id === "" || has[id] !== true || shown[id] === true
  })
  if (kept.length === sections.length) return base
  var copy = {}
  for (var field in base) copy[field] = base[field]
  copy.sections = kept
  return copy
}

/**
 * Names the time window a metric belongs to: its stated 5h or 7d length, else its label, else the heading it sits under,
 * else Monthly.
 * @param {Object} section Metric section from the report.
 * @param {string} group Heading the metric sits under, empty when none.
 * @returns {"session"|"weekly"|"monthly"}
 */
function windowOfMetric(section, group) {
  var secs = Math.floor(Number(section && section.window_secs))
  if (secs === SESSION_WINDOW_SECS) return "session"
  if (secs === WEEKLY_WINDOW_SECS) return "weekly"
  var heading = { type: "metric", label: group || "" }
  if (metricMatchesWindow(section, "session") || metricMatchesWindow(heading, "session")) return "session"
  if (metricMatchesWindow(section, "weekly") || metricMatchesWindow(heading, "weekly")) return "weekly"
  return "monthly"
}

var WINDOW_LABELS = { session: "Session (5h)", weekly: "Weekly (7d)", monthly: "Monthly" }
var WINDOW_KEY_PREFIX = "window:"

/**
 * Groups the model-pool metrics of a Cursor or Antigravity entry by time window. The pools themselves are
 * chosen with the buttons on the panel, so the settings page offers the windows instead.
 * @param {Object} entry Entry from the report.
 * @returns {Array<{id: string, label: string, keys: string[]}>} Windows present, shortest first.
 */
function poolWindows(entry) {
  var sections = entry && Array.isArray(entry.sections) ? entry.sections : []
  var cursor = baseProvider(entry.id) === "cursor"
  var keys = metricKeys(entry)
  var found = {}
  for (var i = 0; i < sections.length; i++) {
    var pool = cursor ? cursorPoolOf(sections[i]) : antigravityPoolOf(sections[i])
    var isPool = cursor ? pool === "models" || pool === "other" : pool !== ""
    if (!isPool || !keys[i]) continue
    var id = windowOfMetric(sections[i], keys[i].group)
    if (!hasOwn(found, id)) found[id] = []
    found[id].push(keys[i].key)
  }
  return ["session", "weekly", "monthly"].filter(function(id) { return hasOwn(found, id) }).map(function(id) {
    return { id: id, label: WINDOW_LABELS[id], keys: found[id] }
  })
}

/**
 * Narrows the hidden keys of a Cursor or Antigravity entry to the pool metrics a window row can switch,
 * so a stale or hand-edited key for any other row (a spending grant) never hides what Settings cannot restore.
 * @param {Object} entry Entry from the report.
 * @param {*} hidden Raw `hiddenMetrics` setting.
 * @returns {string[]}
 */
function windowHiddenKeys(entry, hidden) {
  var offered = []
  poolWindows(entry).forEach(function(win) { offered = offered.concat(win.keys) })
  return hiddenKeysFor(hidden, entry.id).filter(function(key) { return offered.indexOf(key) >= 0 })
}

/**
 * Tells whether hiding more keys still fits the bound kept on one entry's hidden list.
 * @param {string[]} off Keys already hidden.
 * @param {string[]} more Keys about to be hidden.
 * @returns {boolean}
 */
function fitsHiddenLimit(off, more) {
  var extra = more.filter(function(key) { return off.indexOf(key) < 0 })
  return off.length + extra.length <= 32
}

/**
 * Lists the metrics of one entry as the settings page offers them, each with whether it is shown and
 * whether its switch may be flipped. Cursor and Antigravity offer one row per time window, hiding every
 * pool metric of that window; every other provider offers one row per metric.
 * @param {Object} entry Entry from the report.
 * @param {*} hidden Raw `hiddenMetrics` setting.
 * @returns {Array<{key: string, labelKey: string, label: string, group: string, checked: boolean, canToggle: boolean}>}
 * `labelKey` names the translated wording of a window row and is empty for a metric row.
 */
function metricChoices(entry, hidden) {
  var rows = []
  if (!entry) return rows
  var off = hiddenKeysFor(hidden, entry.id)
  var provider = baseProvider(entry.id)
  if (provider === "cursor" || provider === "antigravity") {
    var windows = poolWindows(entry)
    off = windowHiddenKeys(entry, hidden)
    var shownWindows = windows.filter(function(win) { return win.keys.every(function(key) { return off.indexOf(key) < 0 }) })
    return windows.map(function(win) {
      var checked = shownWindows.indexOf(win) >= 0
      return {
        key: WINDOW_KEY_PREFIX + win.id,
        labelKey: "metrics.window_" + win.id,
        label: win.label,
        group: "",
        checked: checked,
        canToggle: checked ? shownWindows.length > 1 && fitsHiddenLimit(off, win.keys) : true
      }
    })
  }
  var sections = Array.isArray(entry.sections) ? entry.sections : []
  var keys = metricKeys(entry)
  for (var i = 0; i < sections.length; i++) {
    if (!sections[i] || sections[i].type !== "metric") continue
    var named = keys[i] || { key: cleanText(sections[i].label, 160).trim(), group: "" }
    rows.push({
      key: named.key,
      labelKey: "",
      label: sections[i].label,
      group: named.group,
      checked: off.indexOf(named.key) < 0,
      canToggle: true
    })
  }
  var shown = rows.filter(function(row) { return row.checked }).length
  return rows.map(function(row) {
    row.canToggle = !row.checked || shown > 1
    return row
  })
}

/**
 * Computes the next `hiddenMetrics` after flipping one row the settings page offers: a window row hides or
 * restores every pool metric of that window (a window hidden in part counts as off, so one click restores it),
 * any other row flips its own metric.
 * @param {*} hidden Raw `hiddenMetrics` setting.
 * @param {Object} entry Entry from the report.
 * @param {string} key Row key from `metricChoices`.
 * @returns {Object<string, string[]>}
 */
function toggleMetricChoice(hidden, entry, key) {
  if (!entry || String(key).indexOf(WINDOW_KEY_PREFIX) !== 0) return toggleHiddenMetric(hidden, entry ? entry.id : "", key)
  var id = key.slice(WINDOW_KEY_PREFIX.length)
  var win = poolWindows(entry).filter(function(item) { return item.id === id })[0]
  var next = normalizeHiddenMetrics(hidden)
  var entryId = cleanText(entry.id, 180).trim()
  if (!win || entryId === "" || UNSAFE_KEYS.indexOf(entryId) >= 0) return next
  var list = windowHiddenKeys(entry, next)
  var anyHidden = win.keys.some(function(item) { return list.indexOf(item) >= 0 })
  var merged = anyHidden
    ? list.filter(function(item) { return win.keys.indexOf(item) < 0 })
    : list.concat(win.keys)
  if (merged.length > 32) return next
  if (merged.length === 0) delete next[entryId]
  else next[entryId] = merged
  return next
}

/**
 * Decides what the value beside a panel meter reads. A report value that carries information of its own
 * (a dollar balance, a count) is kept. A value that only echoes the percentage is replaced by the percentage
 * in the chosen reading, so the number beside the meter agrees with the bar drawn under it.
 * @param {Object} row Metric row from the report.
 * @param {*} showAs Raw reading setting.
 * @returns {{text: string, left: boolean, percent: number|null}} `left` is true when `percent` is the remainder
 *   and the caller should word it as "left".
 */
function metricValueView(row, showAs) {
  var value = row && row.value !== undefined && row.value !== null ? String(row.value).trim() : ""
  var left = normalizeShowAs(showAs) === "left"
  var echoesPercent = /^\d+(\.\d+)?%$/.test(value)
  if (value !== "" && !(left && echoesPercent)) return { text: value, left: false, percent: null }
  var shown = row ? shownPercent(row.percent, showAs) : null
  return {
    text: shown === null ? "" : shown + "%",
    left: left && shown !== null,
    percent: shown
  }
}

/**
 * Names the Cursor pool switch a row belongs to: the two model pools and On-Demand have their own,
 * and every other meter is a spending-page grant.
 * @param {Object} section Section from the report.
 * @returns {"models"|"other"|"demand"|"credits"|""} Empty for anything that is neither a metric nor On-Demand.
 */
function cursorPoolOf(section) {
  if (!section) return ""
  if (section.type === "text") return section.label === "On-Demand" ? "demand" : ""
  if (section.type !== "metric") return ""
  if (section.label === "Cursor Models") return "models"
  if (section.label === "Other Models") return "other"
  if (section.label === "On-Demand") return "demand"
  return "credits"
}

// Flags the chip can actually draw. A switch whose pool is absent (on-demand
// with no prepaid row) does not count, and when that would leave the bar
// blank the first pool that does exist stays on. Models win that fallback.
function cursorBarFlags(entry, flags) {
  var show = cursorPoolVisibility(flags)
  var has = cursorPoolPresence(entry)
  var visible = {
    models: show.models && has.models,
    other: show.other && has.other,
    demand: show.demand && has.demand,
    credits: show.credits && has.credits
  }
  if (!visible.models && !visible.other && !visible.demand && !visible.credits) {
    if (has.models) visible.models = true
    else if (has.other) visible.other = true
    else if (has.demand) visible.demand = true
    else if (has.credits) visible.credits = true
  }
  return visible
}

// A spending-page grant is a meter beside the two model pools and On-Demand.
// Its label is the grant's own name, so it is every Cursor metric that is
// not one of those two pools.
function cursorGrantMeters(sections, showAs) {
  var parts = []
  for (var i = 0; i < sections.length; i++) {
    var section = sections[i]
    if (!section || section.type !== "metric") continue
    if (section.label === "Cursor Models" || section.label === "Other Models") continue
    var label = section.label ? String(section.label) : "Credits"
    parts.push({
      text: percentText(section.percent, showAs),
      line: label + " · " + percentText(section.percent, showAs),
      percent: section.percent,
      severity: section.severity
    })
  }
  return parts
}

// Cursor's included usage is two model pools, not two time windows, so the
// 5-hour / weekly / monthly pin does not describe them. Visible pools stay in
// dashboard order: Cursor Models, then Other Models, then prepaid on-demand
// as a used percentage, the same way OpenRouter shows a credit balance.
// Severity follows whichever visible pool is furthest along.
function cursorDualHeadline(entry, flags, showAs) {
  if (baseProvider(entry && entry.id) !== "cursor") return null
  var sections = Array.isArray(entry.sections) ? entry.sections : []
  var auto = null
  var api = null
  for (var i = 0; i < sections.length; i++) {
    var section = sections[i]
    if (!section || section.type !== "metric") continue
    if (section.label === "Cursor Models") auto = section
    else if (section.label === "Other Models") api = section
  }
  if (!auto || !api) return null
  var show = cursorBarFlags(entry, flags)
  var demand = cursorOnDemand(entry)
  var parts = []
  if (show.models) parts.push({
    text: percentText(auto.percent, showAs),
    line: "Cursor Models · " + percentText(auto.percent, showAs),
    percent: auto.percent,
    severity: auto.severity,
    pool: "models"
  })
  if (show.other) parts.push({
    text: percentText(api.percent, showAs),
    line: "Cursor Other Models · " + percentText(api.percent, showAs),
    percent: api.percent,
    severity: api.severity,
    pool: "other"
  })
  if (show.demand && demand) parts.push({
    text: percentText(demand.usedPct, showAs),
    line: "Cursor On Demand · " + percentText(demand.usedPct, showAs),
    percent: demand.usedPct,
    severity: demand.severity,
    pool: "demand"
  })
  if (show.credits) {
    var grants = cursorGrantMeters(sections, showAs)
    for (var g = 0; g < grants.length; g++) parts.push(grants[g])
  }
  if (parts.length === 0) {
    parts.push({
      text: percentText(auto.percent, showAs),
      line: "Cursor Models · " + percentText(auto.percent, showAs),
      percent: auto.percent,
      severity: auto.severity,
      pool: "models"
    })
  }
  var worse = parts[0]
  for (var p = 1; p < parts.length; p++) {
    var part = parts[p]
    if (part.percent > worse.percent
        || (part.percent === worse.percent && severityRank(part.severity) > severityRank(worse.severity)))
      worse = part
  }
  var texts = []
  var lines = []
  var segments = []
  var tooltipRows = []
  for (var n = 0; n < parts.length; n++) {
    texts.push(parts[n].text)
    lines.push(parts[n].line)
    if (n > 0) segments.push({ text: " · ", severity: "" })
    segments.push({ text: parts[n].text, severity: parts[n].severity })
    tooltipRows.push({
      text: parts[n].line,
      severity: parts[n].severity,
      pool: parts[n].pool || "",
      percent: parts[n].percent
    })
  }
  var text = texts.join(" · ")
  var tooltip = lines.join("\n")
  var allCritical = parts.length > 0
  for (var c = 0; c < parts.length; c++) {
    if (parts[c].severity !== "critical") {
      allCritical = false
      break
    }
  }
  return {
    text: text,
    tooltip: tooltip,
    segments: segments,
    tooltipRows: tooltipRows,
    percent: worse.percent,
    severity: worse.severity,
    // Icon / button chrome: only when every visible pool is critical, so a
    // single exhausted pool does not paint the brand mark while siblings are fine.
    allCritical: allCritical,
    label: "Cursor Models · Other Models"
  }
}

/**
 * Selects the most-consumed Antigravity window of one pool.
 * @param {Object[]} sections Metric sections from the report.
 * @param {string} pool Pool id.
 * @param {string} barWindow Pinned quota window, or `auto` for the highest figure.
 * @returns {Object|null} The worst metric for the pool.
 */
function worstAntigravityPoolMetric(sections, pool, barWindow) {
  var candidates = []
  for (var i = 0; i < sections.length; i++) {
    var section = sections[i]
    if (antigravityPoolOf(section) !== pool) continue
    candidates.push(section)
  }
  var want = normalizeBarWindow(barWindow)
  var scoped = []
  for (var j = 0; j < candidates.length; j++) {
    if (metricMatchesWindow(candidates[j], want)) scoped.push(candidates[j])
  }
  var list = scoped.length > 0 ? scoped : candidates
  var worst = null
  for (var k = 0; k < list.length; k++) {
    if (!worst || list[k].percent > worst.percent
      || (list[k].percent === worst.percent && severityRank(list[k].severity) > severityRank(worst.severity)))
      worst = list[k]
  }
  return worst
}

/**
 * Builds the Antigravity bar headline from its two independent model pools.
 * Each pool is represented by its most-consumed Session or Weekly window.
 * @param {Object} entry Entry from the report.
 * @param {*} flags Raw Antigravity pool switches.
 * @param {*} showAs Raw reading setting.
 * @param {*} barWindow Raw pinned-window setting.
 * @returns {Object|null} Cursor-style dual headline, or null for an unrecognized report.
 */
function antigravityDualHeadline(entry, flags, showAs, barWindow) {
  if (baseProvider(entry && entry.id) !== "antigravity") return null
  var sections = Array.isArray(entry.sections) ? entry.sections : []
  var show = antigravityBarFlags(entry, flags)
  var definitions = [
    { id: "gemini", label: "Gemini" },
    { id: "third_party", label: "Claude & GPT OSS" }
  ]
  var parts = []
  for (var i = 0; i < definitions.length; i++) {
    var definition = definitions[i]
    if (!show[definition.id]) continue
    var worst = worstAntigravityPoolMetric(sections, definition.id, barWindow)
    if (!worst) continue
    parts.push({
      text: percentText(worst.percent, showAs),
      line: definition.label + " · " + percentText(worst.percent, showAs),
      percent: worst.percent,
      severity: worst.severity,
      pool: definition.id
    })
  }
  if (parts.length === 0) return null
  var worse = parts[0]
  for (var p = 1; p < parts.length; p++) {
    var part = parts[p]
    if (part.percent > worse.percent
      || (part.percent === worse.percent && severityRank(part.severity) > severityRank(worse.severity)))
      worse = part
  }
  var texts = []
  var lines = []
  var segments = []
  var tooltipRows = []
  for (var n = 0; n < parts.length; n++) {
    texts.push(parts[n].text)
    lines.push(parts[n].line)
    if (n > 0) segments.push({ text: " · ", severity: "" })
    segments.push({ text: parts[n].text, severity: parts[n].severity })
    tooltipRows.push({
      text: parts[n].line,
      severity: parts[n].severity,
      pool: parts[n].pool,
      percent: parts[n].percent
    })
  }
  var allCritical = parts.length > 0
  for (var c = 0; c < parts.length; c++) {
    if (parts[c].severity !== "critical") {
      allCritical = false
      break
    }
  }
  return {
    text: texts.join(" · "),
    tooltip: lines.join("\n"),
    segments: segments,
    tooltipRows: tooltipRows,
    percent: worse.percent,
    severity: worse.severity,
    allCritical: allCritical,
    label: "Gemini · Claude & GPT OSS"
  }
}

function headline(entry, barWindow, showAs) {
  if (!entry) return { text: "", percent: null, severity: "low", label: "" }
  var dual = cursorDualHeadline(entry, undefined, showAs)
  if (dual) return dual
  var antigravity = antigravityDualHeadline(entry, undefined, showAs, barWindow)
  if (antigravity) return antigravity
  var best = selectMetric(entry, barWindow)
  if (best) {
    // The metric names which of its two numbers goes on the bar; the other one
    // stays in the detail. Reading that beats guessing from the label, which
    // put OpenRouter's dollar figure on the bar and hid its consumed percent.
    // An older report omits the field, and a metric is a percentage by default.
    var bestText = best.headline === "value" && best.value !== ""
      ? best.value : percentText(best.percent, showAs)
    return {
      text: bestText,
      percent: best.percent,
      severity: best.severity,
      label: best.label
    }
  }
  var sections = entry.sections || []
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
  // Cached and failed fetches stay visible without turning the bar red on their own.
  return summary.severity === "critical"
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

var MONTH_NAMES = ["Jan", "Feb", "Mar", "Apr", "May", "Jun",
                   "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]

function pad2(value) {
  return ("0" + value).slice(-2)
}

function isSameLocalDay(a, b) {
  return a.getFullYear() === b.getFullYear()
    && a.getMonth() === b.getMonth()
    && a.getDate() === b.getDate()
}

function formatReset(resetAt, nowMs) {
  if (!resetAt) return ""
  var resetMs = new Date(String(resetAt)).getTime()
  if (!isFinite(resetMs)) return ""
  var remaining = resetMs - Number(nowMs)
  if (remaining <= 0) return "Reset due"
  // Show the real local clock time the window reopens, so the absolute
  // reset moment is visible next to the countdown. Date it whenever it
  // lands on another day: a bare "03:00" on an 18h countdown reads as a
  // time that has already passed, which is the ambiguity this row exists
  // to remove. Keyed on the calendar day rather than on "is it 24h away",
  // because tonight's reset crosses midnight long before it crosses 24h.
  var at = new Date(resetMs)
  var clock = pad2(at.getHours()) + ":" + pad2(at.getMinutes())
  if (!isSameLocalDay(at, new Date(Number(nowMs))))
    clock = MONTH_NAMES[at.getMonth()] + " " + at.getDate() + " " + clock
  return "Resets in " + formatDuration(remaining) + " · " + clock
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

// The report marks sub-rows with a group (SuperGrok's product slices under
// "Breakdown"). Render each group as a heading row — an empty-value text row,
// which DetailRow draws through its section-header path — followed by that
// group's metrics, so slices read as a breakdown of the meter above them
// instead of peers of it. Ungrouped sections pass through untouched, and a
// group heading appears once no matter how many rows carry it.
function groupedSections(sections) {
  var rows = Array.isArray(sections) ? sections : []
  var out = []
  var seen = {}
  for (var i = 0; i < rows.length; i++) {
    var row = rows[i]
    if (row && row.type === "metric") {
      var group = String(row.group || "")
      if (group !== "" && !seen[group]) {
        seen[group] = true
        out.push({ type: "text", label: group, value: "" })
      }
    }
    if (row && row.type === "text" && row.label === "On-Demand") {
      var left = onDemandRemaining(row)
      if (left) row = {
        type: "metric",
        label: row.label,
        percent: left.usedPct,
        value: left.display,
        headline: "percent",
        detail: left.detail,
        severity: left.severity,
        reset_at: "",
        window_secs: null,
        group: ""
      }
    }
    out.push(row)
  }
  return out
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
      return { ok: false, error: "The settings command returned an unsupported response.", primary: "", primary_choices: [], keys: [], vendors: [] }

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
        secret_label: cleanText(key.secret_label, 120),
        note: cleanText(key.note, 240),
        configured: key.configured === true,
        inline_configured: key.inline_configured === true,
        environment_configured: key.environment_configured === true
      })
    }

    // Provider on/off switches (#244). An older binary sends no vendors list;
    // the section simply stays hidden rather than failing the whole form.
    var vendors = []
    if (Array.isArray(parsed.vendors)) {
      for (var v = 0; v < parsed.vendors.length && v < 64; v++) {
        var vendor = parsed.vendors[v]
        var vendorId = settingsId(vendor && vendor.id)
        if (vendorId === "") continue
        vendors.push({
          id: vendorId,
          label: cleanText(vendor.label, 120) || vendorId,
          enabled: vendor.enabled === true
        })
      }
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
    return { ok: true, error: "", primary: primary, primary_choices: choices, keys: keys, vendors: vendors }
  } catch (error) {
    return { ok: false, error: "The settings command returned invalid JSON.", primary: "", primary_choices: [], keys: [], vendors: [] }
  }
}

// Build the stdin patch for `settings apply`. `vendorToggles` (#244) is an
// optional array of {id, enabled}; it is included in the payload only when a
// toggle is pending, so a display-only save keeps the older patch shape a
// pre-#244 binary still accepts.
function buildSettingsPatch(primary, changes, vendorToggles) {
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
      if (value === "") return { ok: false, error: "An edited credential is empty.", payload: "" }
      if (value.length > 16384) return { ok: false, error: "A credential is too long.", payload: "" }
      keys[id] = { action: "set", value: value }
    } else return { ok: false, error: "A settings row has an invalid action.", payload: "" }
  }
  var vendors = {}
  var toggles = Array.isArray(vendorToggles) ? vendorToggles : []
  var seenVendors = []
  for (var t = 0; t < toggles.length; t++) {
    var toggle = toggles[t] || {}
    var vendorId = settingsId(toggle.id)
    if (vendorId === "" || seenVendors.indexOf(vendorId) >= 0)
      return { ok: false, error: "A provider switch has an invalid provider id.", payload: "" }
    if (toggle.enabled !== true && toggle.enabled !== false)
      return { ok: false, error: "A provider switch needs an on or off state.", payload: "" }
    seenVendors.push(vendorId)
    vendors[vendorId] = toggle.enabled
  }
  if (primaryId === "" && seen.length === 0 && seenVendors.length === 0)
    return { ok: false, error: "There are no settings changes to save.", payload: "" }
  var patch = { schema_version: 1, keys: keys }
  if (primaryId !== "") patch.primary = primaryId
  if (seenVendors.length > 0) patch.vendors = vendors
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
