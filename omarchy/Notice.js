// Presentation only: turns a status/error string from the report into a
// structured notice (kind, category, readable body, copyable command). The
// text is data — nothing parsed here is ever executed, only copied.
var PREFIXES = [
  { re: /^credentials? error:\s*/i, category: "credentials" },
  { re: /^auth(?:entication)? error:\s*/i, category: "credentials" },
  { re: /^http error:\s*/i, category: "network" },
  { re: /^network error:\s*/i, category: "network" }
]

function categoryOf(text) {
  for (var i = 0; i < PREFIXES.length; i++) if (PREFIXES[i].re.test(text)) return PREFIXES[i].category
  if (/not signed in|sign in|log ?in|unauthori[sz]ed|\b401\b|\b403\b|expired token|re-?auth/i.test(text)) return "credentials"
  if (/rate.?limit|\b429\b|too many requests|backoff/i.test(text)) return "rate_limit"
  if (/timed? ?out|timeout|connect|dns|network|unreachable|\b5\d\d\b/i.test(text)) return "network"
  return "generic"
}

// The first `backticked` run that looks like a shell command (no spaces at
// the edges, bounded length) is offered as a copy target.
function commandOf(text) {
  var match = /`([^`\n]{2,120})`/.exec(text)
    || /\b(?:with|run):\s*([a-z][\w.\/-]*(?: [\w@.\/=-]+){0,6})\s*\.?$/i.exec(text)
  if (!match) return ""
  var command = match[1].trim()
  return /^[A-Za-z0-9_.\/-]+(?: [^`]+)?$/.test(command) ? command : ""
}

function cleanBody(text) {
  var body = String(text || "").trim()
  for (var i = 0; i < PREFIXES.length; i++) body = body.replace(PREFIXES[i].re, "")
  body = body.replace(/`([^`\n]+)`/g, "\"$1\"")
  return body.charAt(0).toUpperCase() + body.slice(1)
}

function glyph(notice) {
  if (!notice) return ""
  if (notice.category === "credentials") return "󰌾"
  if (notice.category === "network") return "󰖪"
  if (notice.category === "rate_limit") return "󰔟"
  return notice.kind === "error" ? "󰀦" : "󰋼"
}

// kind: "error" | "info". Category drives the title and whether a retry helps.
function build(message, kind, forcedCategory) {
  var text = String(message || "").trim()
  if (text === "") return null
  var category = forcedCategory || categoryOf(text)
  return {
    kind: kind === "info" ? "info" : "error",
    category: category,
    body: cleanBody(text),
    command: commandOf(text),
    retry: kind !== "info" && (category === "network" || category === "rate_limit" || category === "refresh")
  }
}
