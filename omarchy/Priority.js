// Presentation only. Rust owns ranking, consumption rates and urgency.
function clampInt(value, min, max, fallback) {
  if (value === null || value === undefined || String(value).trim() === "") return fallback
  var n = Number(value)
  return isFinite(n) ? Math.max(min, Math.min(max, Math.round(n))) : fallback
}

function normalizeHours(value) { return clampInt(value, 1, 24, 3) }
function normalizeAlertPercent(value) { return clampInt(value, 1, 100, 50) }
function normalizeAlertHours(value) { return clampInt(value, 1, 168, 24) }

// Preset choices plus the configured value when it was set outside the form.
function choices(presets, current, label) {
  var values = presets.slice()
  if (values.indexOf(current) < 0) values.push(current)
  values.sort(function(a, b) { return a - b })
  return values.map(function(v) { return { value: String(v), label: label(v) } })
}

function reasonKey(row) {
  var key = String((row && (row.reason_code || row.status)) || "unknown")
  return ["priority", "on_track", "blocked", "unavailable", "unknown"].indexOf(key) >= 0 ? key : "unknown"
}

function stateKey(row) {
  return row && row.urgent === true && row.status === "priority" ? "urgent" : reasonKey({status: row && row.status})
}

// Under a day reads in hours; a weekly reset reads in days and hours.
function duration(hours, tr) {
  var h = Math.max(0, Math.round(hours))
  return h < 24 ? tr("priority.hours", { hours: h })
    : tr("priority.days", { days: Math.floor(h / 24), hours: h % 24 })
}

function summary(row, nowMs, tr) {
  if (!hasForecast(row)) return tr("priority.reason." + reasonKey(row))
  var hours = (Date.parse(row.reset_at) - nowMs) / 3600000
  return tr("priority.summary", { unused: num(row.unused_percent, tr), left: num(row.remaining_percent, tr), when: duration(hours, tr) })
}

function hasForecast(row) {
  return !!(row && row.unused_percent !== null && row.unused_percent !== undefined && row.reset_at)
}

// One decimal at most, with the locale's decimal separator ("1,5" in pt-BR).
function num(value, tr) {
  var n = Math.round(Number(value) * 10) / 10
  if (!isFinite(n)) return "–"
  var text = n === Math.round(n) ? String(Math.round(n)) : n.toFixed(1)
  return text.replace(".", tr("priority.decimal"))
}

function rate(value, tr) { return tr("priority.rate", { rate: num(value, tr) }) }

// The numbers behind a card's position, as label/value pairs. Empty when the
// row has no forecast (blocked, unavailable, unknown): its notes explain why.
function facts(row, nowMs, historyAvailable, tr) {
  if (!hasForecast(row)) return []
  var when = duration((Date.parse(row.reset_at) - nowMs) / 3600000, tr)
  var measured = row.recent_percent_per_hour !== null && row.recent_percent_per_hour !== undefined
  return [
    { label: tr("priority.fact.window"), value: tr("priority.window_value", { window: String(row.window || ""), when: when }) },
    { label: tr("priority.fact.left"), value: tr("priority.percent", { value: num(row.remaining_percent, tr) }) },
    { label: tr("priority.fact.unused"), value: tr("priority.percent", { value: num(row.unused_percent, tr) }), strong: true },
    { label: tr("priority.fact.pace"), value: rate(row.pace_percent_per_hour, tr) },
    { label: measured ? tr("priority.fact.recent", { hours: num(row.recent_hours, tr) }) : tr("priority.fact.recent_none"),
      value: measured ? rate(row.recent_percent_per_hour, tr) : tr(historyAvailable ? "priority.collecting" : "priority.history_error") },
    { label: tr("priority.fact.cycle"), value: rate(row.cycle_percent_per_hour, tr) },
    { label: tr("priority.fact.needed"), value: rate(row.required_percent_per_hour, tr) }
  ]
}

// The conclusion first, then whatever qualifies it.
function notes(row, nowMs, tr) {
  if (!row) return []
  var out = [tr("priority.reason." + reasonKey(row))]
  if (stateKey(row) === "urgent")
    out.push(tr("priority.urgent_help", { unused: row.alert_unused_percent, hours: row.alert_hours }))
  if (row.session_exhausted === true) out.push(tr("priority.session_exhausted"))
  if (row.stale === true && row.fetched_at)
    out.push(tr("priority.stale", { when: duration((nowMs - Date.parse(row.fetched_at)) / 3600000, tr) }))
  return out
}
