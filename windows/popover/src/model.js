// Pure popover view-model. Node tests and the Vite UI both import this file.
// Keep it free of React/DOM/wry globals so the contract stays hermetic.
// The JSDoc below only types the exports for the TypeScript side (allowJs);
// it is inert at runtime.

/** @typedef {import("./lib/types").Card} Card */
/** @typedef {import("./lib/types").Layout} Layout */
/** @typedef {import("./lib/types").Payload} Payload */
/** @typedef {import("./lib/types").Row} Row */
/** @typedef {import("./lib/types").RowPrefs} RowPrefs */
/** @typedef {import("./lib/types").ExplainedError} ExplainedError */
/** @typedef {import("./lib/types").Pace} Pace */
/** @typedef {import("./lib/types").UpdateInfo} UpdateInfo */
/** @typedef {import("./lib/types").UpdateMode} UpdateMode */
/** @typedef {import("./lib/types").TimeFormat} TimeFormat */

export const LAYOUT_KEY = "aiub.tray.layout.v1";
const COLLAPSED_METRIC_CAP = 2;

/** @returns {Payload} */
export function parseHostPayload(raw) {
  if (raw && typeof raw === "object" && !Array.isArray(raw)) return normalizePayload(raw);
  try {
    return normalizePayload(JSON.parse(String(raw || "")));
  } catch {
    return emptyPayload("The usage report did not contain valid JSON.");
  }
}

/** @returns {Payload} */
export function emptyPayload(hostError) {
  return {
    version: "",
    generatedAt: 0,
    nextRefreshAt: 0,
    startupEnabled: false,
    hostError: hostError || "",
    primary: "",
    entries: [],
    refreshMinutes: 5,
    shortcut: "",
    shortcutError: "",
    updates: "notify",
    update: null,
    updateCheckedAt: 0,
  };
}

function clean(value, max) {
  let text = value === undefined || value === null ? "" : String(value);
  text = text.replace(/[\t\r]/g, " ")
    .replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f-\u009f]/g, "")
    .replace(/[\u200e\u200f\u202a-\u202e\u2066-\u2069]/g, "");
  const limit = Number(max) || 2048;
  if (text.length <= limit) return text;
  return text.slice(0, limit - 1) + "…";
}

/** @returns {Payload} */
function normalizePayload(parsed) {
  const entriesIn = Array.isArray(parsed.entries) ? parsed.entries : [];
  const entries = [];
  for (let i = 0; i < entriesIn.length && i < 64; i++) {
    const entry = normalizeEntry(entriesIn[i]);
    if (entry) entries.push(entry);
  }
  return {
    version: clean(parsed.version, 32),
    generatedAt: Number(parsed.generated_at) || 0,
    nextRefreshAt: Number(parsed.next_refresh_at) || 0,
    startupEnabled: parsed.startup_enabled === true,
    hostError: clean(parsed.host_error, 1200),
    primary: clean(parsed.primary, 180),
    entries,
    refreshMinutes: normalizeRefreshMinutes(parsed.refresh_minutes),
    shortcut: clean(parsed.shortcut, 64),
    shortcutError: clean(parsed.shortcut_error, 300),
    updates: normalizeUpdateMode(parsed.updates),
    update: normalizeUpdate(parsed.update),
    updateCheckedAt: finiteNumber(parsed.update_checked_at),
  };
}

function finiteNumber(value) {
  const number = Number(value);
  return Number.isFinite(number) ? number : 0;
}

// A metric's window length in seconds; 0 when the host reports none.
function windowSeconds(value) {
  const secs = finiteNumber(value);
  return secs > 0 ? secs : 0;
}

const REFRESH_MINUTES = [1, 5, 10];

// The host's refresh interval; anything outside the offered set reads as the
// 5-minute default.
function normalizeRefreshMinutes(value) {
  const minutes = Number(value);
  return REFRESH_MINUTES.indexOf(minutes) < 0 ? 5 : minutes;
}

/** @returns {UpdateMode} */
function normalizeUpdateMode(value) {
  return value === "auto" || value === "off" ? value : "notify";
}

const UPDATE_STATES = ["checking", "available", "downloading", "installing", "failed"];
const UPDATE_URL_PREFIX = "https://github.com/";

// The host's in-flight update, if any. Only a GitHub URL survives: it is the
// one origin the release workflow publishes to, and the banner opens it.
/** @returns {UpdateInfo|null} */
function normalizeUpdate(raw) {
  if (!isPlainObject(raw)) return null;
  const state = String(raw.state || "");
  const url = clean(raw.url, 400);
  return {
    error: clean(raw.error, 300),
    state: UPDATE_STATES.indexOf(state) < 0 ? "available" : state,
    url: url.startsWith(UPDATE_URL_PREFIX) ? url : "",
    version: clean(raw.version, 32),
  };
}

function normalizeEntry(raw) {
  if (!raw || typeof raw !== "object") return null;
  const id = clean(raw.id, 180).trim();
  if (id === "") return null;
  const source = Array.isArray(raw.sections) ? raw.sections : [];
  const sections = [];
  for (let i = 0; i < source.length && i < 96; i++) {
    const section = normalizeSection(source[i]);
    if (section) sections.push(section);
  }
  const error = clean(raw.error, 1200);
  return {
    id,
    displayName: clean(raw.display_name || raw.name, 240),
    shortName: clean(raw.short_name, 24),
    plan: clean(raw.plan, 240),
    status: error !== "" || raw.status === "error" ? "error" : "ready",
    error,
    stale: raw.stale === true,
    // How to sign this provider in, from VendorId::sign_in_hint on the host.
    // Deliberately not a table in this file; see signInHint.
    signIn: clean(raw.sign_in, 200),
    sections,
  };
}

function normalizeSection(raw) {
  if (!raw || typeof raw !== "object") return null;
  const type = String(raw.type || "");
  if (type === "metric") {
    const percent = Number(raw.percent);
    if (!Number.isFinite(percent)) return null;
    let severity = String(raw.severity || "");
    if (["low", "mid", "high", "critical"].indexOf(severity) < 0) severity = "low";
    return {
      type: "metric",
      label: clean(raw.label, 160),
      percent: Math.max(0, Math.min(100, Math.round(percent))),
      value: clean(raw.value, 240),
      detail: clean(raw.detail, 1000),
      severity,
      resetAt: clean(raw.reset_at, 80),
      window: windowSeconds(raw.window_secs),
    };
  }
  if (type === "text") {
    return { type: "text", label: clean(raw.label, 160), value: clean(raw.value, 1000) };
  }
  if (type === "block") {
    const body = Array.isArray(raw.body) ? raw.body : [];
    return {
      type: "block",
      label: clean(raw.label, 160),
      body: body.slice(0, 24).map((line) => clean(line, 1000)),
    };
  }
  return null;
}

export function formatDuration(milliseconds) {
  if (!(milliseconds > 0)) return "now";
  const minutes = Math.floor(milliseconds / 60000);
  const hours = Math.floor(minutes / 60);
  const days = Math.floor(hours / 24);
  if (days > 0) return days + "d " + (hours % 24) + "h";
  if (hours > 0) return hours + "h " + (minutes % 60) + "m";
  return Math.max(1, minutes) + "m";
}

export function resetLabel(section, nowMs) {
  if (!section || section.type !== "metric") return "";
  if (section.resetAt) {
    const at = Date.parse(section.resetAt);
    if (!Number.isNaN(at)) return "Resets in " + formatDuration(at - nowMs);
  }
  const detail = section.detail || "";
  if (/reset/i.test(detail)) return detail;
  return "";
}

export function displayPlan(title, plan) {
  const name = String(title || "").replace(/\s+/g, " ").trim();
  const label = String(plan || "").replace(/\s+/g, " ").trim();
  if (!label) return "";
  // A plan that merely repeats the card title ("SuperGrok" / "SuperGrok")
  // adds nothing next to it.
  if (name && label.toLowerCase() === name.toLowerCase()) return "";
  if (name && label.toLowerCase().startsWith(name.toLowerCase() + " ")) {
    return label.slice(name.length).trim();
  }
  return label;
}

export function leftLabel(leftPercent) {
  if (leftPercent === 0) return "Limit reached";
  return leftPercent + "% left";
}

export function quotaLabel(row, showAs) {
  if (!row || row.kind !== "metric") return "";
  if (row.leftPercent === 0) return "Limit reached";
  if (showAs === "used") return row.usedPercent + "% used";
  return row.leftPercent + "% left";
}

// The headline in the other mode, for hover tooltips. "Limit reached" has
// no alternate reading, so it yields "".
export function quotaAlternate(row, showAs) {
  if (!row || row.kind !== "metric") return "";
  if (row.leftPercent === 0) return "";
  return quotaLabel(row, showAs === "used" ? "left" : "used");
}

// Dashboard headline under the meter. Unlike quotaLabel it never collapses a spent
// row into "Limit reached": the flame beside the label carries that verdict, and the
// headline keeps reading "0% left" / "100% used" like OpenUsage's WidgetRowView.
export function headlineLabel(row, showAs) {
  if (!row || row.kind !== "metric") return "";
  if (showAs === "used") return row.usedPercent + "% used";
  return row.leftPercent + "% left";
}

export function headlineAlternate(row, showAs) {
  if (!row || row.kind !== "metric") return "";
  return headlineLabel(row, showAs === "used" ? "left" : "used");
}

export function meterColor(severity) {
  switch (severity) {
    case "mid":
    case "high":
      return "yellow";
    case "critical":
      return "red";
    default:
      return "blue";
  }
}

function dayKey(atMs, locale, timeZone) {
  const options = { year: "numeric", month: "2-digit", day: "2-digit" };
  if (timeZone) options.timeZone = timeZone;
  return new Intl.DateTimeFormat(locale, options).format(new Date(atMs));
}

// "today at 6:38 PM", "tomorrow at 6:38 PM", or "Sep 12 at 6:38 PM".
// `opts.timeZone` is optional; tests pass "UTC" so the output is deterministic.
// `opts.timeFormat` forces a 12- or 24-hour clock; "auto" (or absent) leaves
// it to the locale.
export function formatResetExact(atMs, nowMs, opts) {
  const locale = (opts && opts.locale) || "en-US";
  const timeZone = opts && opts.timeZone ? opts.timeZone : undefined;
  const timeFormat = opts && opts.timeFormat;
  const at = new Date(atMs);
  const timeOptions = { hour: "numeric", minute: "2-digit" };
  if (timeZone) timeOptions.timeZone = timeZone;
  if (timeFormat === "12") timeOptions.hour12 = true;
  // "h23" rather than `hour12: false`: older ICU builds render midnight as
  // "24:05" under the latter.
  else if (timeFormat === "24") timeOptions.hourCycle = "h23";
  // Newer ICU builds separate "6:38" and "PM" with a narrow no-break space;
  // fold it to a plain space so the string is stable across runtimes.
  const time = new Intl.DateTimeFormat(locale, timeOptions).format(at).replace(/ /g, " ");
  const atDay = dayKey(atMs, locale, timeZone);
  if (atDay === dayKey(nowMs, locale, timeZone)) return "today at " + time;
  if (atDay === dayKey(nowMs + 86_400_000, locale, timeZone)) return "tomorrow at " + time;
  const dayOptions = { month: "short", day: "numeric" };
  if (timeZone) dayOptions.timeZone = timeZone;
  const day = new Intl.DateTimeFormat(locale, dayOptions).format(at).replace(/ /g, " ");
  return day + " at " + time;
}

function parseResetAt(row) {
  if (!row || !row.resetAt) return NaN;
  return Date.parse(row.resetAt);
}

// Reset display for a projected metric row. Falls back to the row's own
// `reset` text when there is no parseable absolute timestamp.
export function resetText(row, mode, nowMs, opts) {
  const at = parseResetAt(row);
  if (Number.isNaN(at)) return (row && row.reset) || "";
  if (mode === "exact") return "Resets " + formatResetExact(at, Number(nowMs) || 0, opts);
  return "Resets in " + formatDuration(at - (Number(nowMs) || 0));
}

// Same row in the other mode, for hover tooltips; "" without a timestamp.
export function resetAlternate(row, mode, nowMs, opts) {
  if (Number.isNaN(parseResetAt(row))) return "";
  return resetText(row, mode === "exact" ? "countdown" : "exact", nowMs, opts);
}

// Burn-rate pacing, ported from OpenUsage's Pace.swift. Projects the row's
// usage at its current rate to the end of the reset window. Null when there is
// no signal: no window length, no parseable reset, the window already reset,
// nothing spent yet, or too early in the window (under 1% of it, at least a
// minute) for the projection to be stable.
/** @returns {Pace|null} */
export function pace(row, nowMs) {
  if (!row || typeof row !== "object") return null;
  const window = windowSeconds(row.window);
  if (window === 0) return null;
  const now = Number(nowMs) || 0;
  const resetMs = Date.parse(String(row.resetAt || ""));
  if (Number.isNaN(resetMs) || resetMs <= now) return null;
  const windowMs = window * 1000;
  const elapsed = windowMs - (resetMs - now);
  if (elapsed < Math.max(60_000, window * 10)) return null;
  const used = finiteNumber(row.usedPercent);
  if (used <= 0) return null;
  const rate = used / elapsed; // percent per millisecond
  // Multiply before dividing so a whole-percent meter at a clean fraction of the
  // window lands exactly on the 90 / 100 thresholds instead of a hair past them.
  const projected = used * windowMs / elapsed;
  let state = "behind";
  if (projected <= 90) state = "ahead";
  else if (projected <= 100) state = "onTrack";
  let runsOutMs = null;
  if (state === "behind") {
    // Only a run-out still ahead of us and before the reset is worth a warning.
    const at = now + (100 - used) / rate;
    if (at > now && at < resetMs) runsOutMs = at;
  }
  return {
    elapsedPercent: clampPercent(elapsed * 100 / windowMs),
    projectedPercent: projected,
    runsOutMs,
    sparePercent: Math.round(100 - projected),
    state,
  };
}

function clampPercent(value) {
  const number = finiteNumber(value);
  return Math.max(0, Math.min(100, number));
}

// Where the "you should be here" tick sits on the meter, as a percent of its
// width. The meter fills with what is consumed in Used mode and with what
// remains in Left mode, so the tick follows the same reading.
export function paceTickPercent(pace, showAs) {
  if (!pace) return null;
  const elapsed = clampPercent(pace.elapsedPercent);
  return showAs === "used" ? elapsed : 100 - elapsed;
}

// One-line pace verdict beside the row label, in OpenUsage's WidgetRowView
// wording. `opts` is the reset-time display: with `resetTimes: "exact"` the
// run-out instant is a clock time and `timeFormat`/`locale`/`timeZone` flow
// through to formatResetExact.
export function paceText(pace, nowMs, opts) {
  if (!pace) return "";
  const now = Number(nowMs) || 0;
  if (pace.state === "ahead") return "~" + Math.round(pace.sparePercent) + "% left at reset";
  if (pace.state === "onTrack") return "~" + Math.max(Math.round(pace.sparePercent), 0) + "% spare";
  if (pace.runsOutMs === null || pace.runsOutMs === undefined) return "";
  if (opts && opts.resetTimes === "exact") return "Limit " + formatResetExact(pace.runsOutMs, now, opts);
  return "Limit in " + formatDuration(pace.runsOutMs - now);
}

// Ahead-of-pace rows stay quiet unless the layout asks for pacing everywhere.
export function paceVisible(pace, layout) {
  return !!pace && !!(layout && layout.alwaysShowPace || pace.state !== "ahead");
}

export function rowKey(row) {
  if (row && row.key) return String(row.key);
  return String(row && row.kind || "") + ":" + String(row && row.label || "");
}

// Row keys index layout preferences, so two rows with the same kind and label
// (a vendor repeating a metric name) must not collapse into one: the repeat
// gets a numbered suffix, in report order, so the preference stays stable.
function dedupeRowKeys(rows) {
  const seen = new Map();
  for (const row of rows) {
    const base = row.key;
    const count = seen.get(base) || 0;
    seen.set(base, count + 1);
    if (count > 0) row.key = base + " #" + (count + 1);
  }
}

/** @returns {Card[]} */
export function projectCards(payload, nowMs) {
  const now = Number(nowMs) || 0;
  const cards = [];
  for (const entry of payload.entries || []) {
    const rows = [];
    // A text section with a label and no value is a group heading (Antigravity
    // reports "Session" and "Weekly" groups that both contain "Gemini"). It gets
    // no row of its own; the metrics under it carry the group in their label so
    // the two "Gemini" meters stay distinct rows.
    let group = "";
    let warning = null;
    for (const section of entry.sections || []) {
      if (isWarningSection(section)) {
        const explained = explainError(section.value || section.label, entry);
        warning = { title: explained.title, hint: explained.hint, raw: shortenDiagnostic(section.value || section.label) };
        continue;
      }
      if (section.type === "text" && section.label && !section.value) {
        group = section.label;
        continue;
      }
      if (section.type === "metric") {
        const left = Math.max(0, 100 - section.percent);
        const label = metricLabel(entry.id, section.label);
        const row = {
          kind: "metric",
          label: group ? label + " (" + group + ")" : label,
          leftPercent: left,
          usedPercent: section.percent,
          severity: section.severity,
          reset: resetLabel(section, now),
          resetAt: section.resetAt || "",
          window: section.window || 0,
        };
        row.key = rowKey(row);
        rows.push(row);
      } else if (section.type === "text" && (section.label || section.value)) {
        const row = { kind: "text", label: section.label, value: section.value };
        row.key = rowKey(row);
        rows.push(row);
      } else if (section.type === "block" && section.body && section.body.length) {
        const row = { kind: "block", label: section.label, body: section.body };
        row.key = rowKey(row);
        rows.push(row);
      }
    }
    dropRedundantResetRows(rows);
    dedupeRowKeys(rows);
    const explained = explainError(entry.error, entry);
    cards.push({
      id: entry.id,
      title: entry.displayName || entry.shortName || entry.id,
      plan: entry.plan,
      stale: entry.stale,
      error: joinError(explained),
      errorTitle: explained.title,
      errorHint: explained.hint,
      errorDetail: entry.error || "",
      rows,
      warning,
    });
  }
  return cards;
}

// SuperGrok names every meter "<Window> Build credits"; the card title already
// says what is being counted, so the row keeps only the window.
function metricLabel(entryId, label) {
  if (vendorSlug(entryId) !== "supergrok") return label;
  return String(label || "").replace(/\s+Build credits$/i, "");
}

/** @returns {Layout} */
export function emptyLayout() {
  return {
    alwaysShowPace: false,
    cardOrder: [],
    hidden: {},
    collapsed: {},
    density: "regular",
    hideExtras: false,
    hintDismissed: false,
    resetTimes: "countdown",
    rows: {},
    seeded: false,
    showAs: "left",
    theme: "system",
    timeFormat: "auto",
  };
}

function normalizeDensity(value) {
  return value === "compact" ? "compact" : "regular";
}

/** @returns {TimeFormat} */
function normalizeTimeFormat(value) {
  return value === "12" || value === "24" ? value : "auto";
}

function normalizeResetTimes(value) {
  return value === "exact" ? "exact" : "countdown";
}

function normalizeShowAs(value) {
  return value === "used" ? "used" : "left";
}

function normalizeTheme(value) {
  return value === "light" || value === "dark" ? value : "system";
}

function cleanIdList(list) {
  const out = [];
  if (!Array.isArray(list)) return out;
  for (let i = 0; i < list.length && i < 96; i++) {
    const id = clean(list[i], 180).trim();
    if (id && out.indexOf(id) < 0) out.push(id);
  }
  return out;
}

function copyFlagMap(source, dest) {
  if (!source || typeof source !== "object" || Array.isArray(source)) return;
  for (const key of Object.keys(source)) {
    if (source[key] === true) dest[clean(key, 180).trim()] = true;
  }
}

/** @returns {Layout} */
export function normalizeLayout(raw) {
  const layout = emptyLayout();
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return layout;
  layout.alwaysShowPace = raw.alwaysShowPace === true;
  layout.cardOrder = cleanIdList(raw.cardOrder);
  copyFlagMap(raw.hidden, layout.hidden);
  copyFlagMap(raw.collapsed, layout.collapsed);
  layout.density = normalizeDensity(raw.density);
  layout.timeFormat = normalizeTimeFormat(raw.timeFormat);
  layout.hideExtras = raw.hideExtras === true;
  layout.hintDismissed = raw.hintDismissed === true;
  layout.seeded = raw.seeded === true;
  layout.resetTimes = normalizeResetTimes(raw.resetTimes);
  layout.showAs = normalizeShowAs(raw.showAs);
  layout.theme = normalizeTheme(raw.theme);
  if (raw.rows && typeof raw.rows === "object" && !Array.isArray(raw.rows)) {
    for (const id of Object.keys(raw.rows)) {
      const prefs = raw.rows[id];
      if (!prefs || typeof prefs !== "object") continue;
      const key = clean(id, 180).trim();
      if (!key) continue;
      const off = {};
      copyFlagMap(prefs.off, off);
      layout.rows[key] = {
        always: cleanIdList(prefs.always),
        demand: cleanIdList(prefs.demand),
        off,
      };
    }
  }
  return layout;
}

export function memoryStorage(seed) {
  const map = new Map();
  if (seed && typeof seed === "object") {
    for (const key of Object.keys(seed)) map.set(key, String(seed[key]));
  }
  return {
    getItem(key) {
      return map.has(key) ? map.get(key) : null;
    },
    setItem(key, value) {
      map.set(String(key), String(value));
    },
    removeItem(key) {
      map.delete(String(key));
    },
  };
}

/** @returns {Layout} */
export function loadLayout(storage) {
  try {
    const raw = storage && storage.getItem ? storage.getItem(LAYOUT_KEY) : null;
    if (!raw) return emptyLayout();
    return normalizeLayout(JSON.parse(raw));
  } catch {
    return emptyLayout();
  }
}

export function saveLayout(storage, layout) {
  if (!storage || typeof storage.setItem !== "function") return;
  storage.setItem(LAYOUT_KEY, JSON.stringify(normalizeLayout(layout)));
}

/** @returns {Layout} */
export function syncLayout(layout, cardIds) {
  const ids = Array.isArray(cardIds) ? cardIds.filter(Boolean) : [];
  const known = new Set(ids);
  const order = (layout.cardOrder || []).filter((id) => known.has(id));
  for (const id of ids) {
    if (order.indexOf(id) < 0) order.push(id);
  }
  const hidden = {};
  const collapsed = {};
  const rows = {};
  for (const id of Object.keys(layout.hidden || {})) {
    if (known.has(id) && layout.hidden[id]) hidden[id] = true;
  }
  for (const id of Object.keys(layout.collapsed || {})) {
    if (known.has(id) && layout.collapsed[id]) collapsed[id] = true;
  }
  for (const id of Object.keys(layout.rows || {})) {
    if (known.has(id)) rows[id] = layout.rows[id];
  }
  return {
    alwaysShowPace: layout.alwaysShowPace === true,
    cardOrder: order,
    hidden,
    collapsed,
    density: normalizeDensity(layout.density),
    hideExtras: layout.hideExtras === true,
    hintDismissed: layout.hintDismissed === true,
    resetTimes: normalizeResetTimes(layout.resetTimes),
    seeded: layout.seeded === true,
    rows,
    showAs: normalizeShowAs(layout.showAs),
    theme: normalizeTheme(layout.theme),
    timeFormat: normalizeTimeFormat(layout.timeFormat),
  };
}

export function metricCount(card) {
  const rows = card && Array.isArray(card.rows) ? card.rows : [];
  let count = 0;
  for (const row of rows) {
    if (row && row.kind === "metric") count += 1;
  }
  return count;
}

// Indexes of one-line (non-metric) rows sitting directly under another
// one-line row, so the UI can tighten the gap between them.
export function condensedTextRowIndexes(rows) {
  if (!Array.isArray(rows)) return [];
  const out = [];
  for (let i = 1; i < rows.length; i++) {
    const row = rows[i];
    const prev = rows[i - 1];
    if (!row || !prev) continue;
    if (row.kind !== "metric" && prev.kind !== "metric") out.push(i);
  }
  return out;
}

/** @returns {Card[]} */
export function orderedCards(cards, layout) {
  const list = Array.isArray(cards) ? cards : [];
  const byId = new Map();
  for (const card of list) byId.set(card.id, card);
  const seen = new Set();
  const ordered = [];
  for (const id of layout.cardOrder || []) {
    const card = byId.get(id);
    if (card) {
      ordered.push(card);
      seen.add(id);
    }
  }
  for (const card of list) {
    if (!seen.has(card.id)) ordered.push(card);
  }
  return ordered;
}

/** @returns {Card[]} */
export function applyCardLayout(cards, layout) {
  return orderedCards(cards, layout).filter((card) => !layout.hidden || !layout.hidden[card.id]);
}

export function moveCardBefore(order, draggedId, beforeId) {
  const from = Array.isArray(order) ? order.slice() : [];
  const drag = String(draggedId || "");
  if (!drag) return from;
  const next = from.filter((id) => id !== drag);
  if (!beforeId) {
    next.push(drag);
    return next;
  }
  const idx = next.indexOf(String(beforeId));
  if (idx < 0) next.push(drag);
  else next.splice(idx, 0, drag);
  return next;
}

export function nudgeCard(order, id, dir) {
  const next = Array.isArray(order) ? order.slice() : [];
  const idx = next.indexOf(id);
  const j = idx + (Number(dir) || 0);
  if (idx < 0 || j < 0 || j >= next.length) return next;
  const tmp = next[idx];
  next[idx] = next[j];
  next[j] = tmp;
  return next;
}

export function mergeVisibleOrder(fullOrder, visibleOrder) {
  const vis = Array.isArray(visibleOrder) ? visibleOrder.filter(Boolean) : [];
  const seen = new Set(vis);
  const next = vis.slice();
  for (const id of fullOrder || []) {
    if (!seen.has(id)) {
      next.push(id);
      seen.add(id);
    }
  }
  return next;
}

// First launch, as OpenUsage does it: providers with no credential on this
// machine start hidden, and the dashboard's welcome card points at Customize.
// Only "No API key" counts — an expired sign-in means a credential exists.
export function lacksCredentials(entry) {
  if (!entry || typeof entry !== "object") return false;
  return explainError(entry.error, entry).title === "No API key";
}

// Runs once, on the first payload that carries entries. A host error (no
// entries) is not a first launch. When every provider lacks a credential the
// starter set stays visible, so a fresh install never opens on an empty
// dashboard.
/** @returns {Layout} */
export function seedLayout(layout, entries) {
  const list = Array.isArray(entries) ? entries.filter((e) => e && typeof e === "object" && e.id) : [];
  if (!layout || layout.seeded === true || list.length === 0) return layout;
  const missing = list.filter(lacksCredentials);
  const hidden = Object.assign({}, layout.hidden || {});
  if (missing.length < list.length) {
    for (const entry of missing) hidden[String(entry.id)] = true;
  }
  return Object.assign({}, layout, { hidden, seeded: true });
}

// What a fresh host payload does to the stored layout. A payload without
// entries (the host's placeholder before the first report, or a host error)
// must leave the layout alone: syncing against an empty id list would wipe
// every remembered order, hidden flag and row preference.
/** @returns {Layout} */
export function absorbPayload(layout, entries) {
  const list = Array.isArray(entries) ? entries : [];
  if (list.length === 0) return layout;
  const ids = list.map((entry) => entry && entry.id).filter(Boolean);
  return syncLayout(seedLayout(layout, list), ids);
}

export function hintPending(layout) {
  return !!layout && layout.seeded === true && layout.hintDismissed !== true;
}

/** @returns {RowPrefs} */
export function defaultRowPrefs(rows) {
  const always = [];
  const demand = [];
  let metrics = 0;
  for (const row of rows || []) {
    const key = rowKey(row);
    if (row.kind === "metric" && metrics < COLLAPSED_METRIC_CAP) {
      always.push(key);
      metrics += 1;
    } else {
      demand.push(key);
    }
  }
  return { always, demand, off: {} };
}

function hasStoredPrefs(prefs) {
  if (!prefs || typeof prefs !== "object") return false;
  if (Array.isArray(prefs.always) && prefs.always.length) return true;
  if (Array.isArray(prefs.demand) && prefs.demand.length) return true;
  if (prefs.off && Object.keys(prefs.off).length) return true;
  return false;
}

/** @returns {RowPrefs} */
export function mergeRowPrefs(rows, prefs, opts) {
  const hideExtras = !!(opts && opts.hideExtras);
  const def = defaultRowPrefs(rows);
  const known = new Set((rows || []).map(rowKey));
  const off = {};
  const stored = hasStoredPrefs(prefs);
  if (stored) copyFlagMap(prefs.off, off);
  if (hideExtras && !stored) {
    for (const row of rows || []) {
      if (row.kind !== "metric") off[rowKey(row)] = true;
    }
  }
  if (!stored) {
    return {
      always: def.always.filter((key) => !off[key]),
      demand: def.demand.filter((key) => !off[key]),
      off,
    };
  }
  const always = [];
  const demand = [];
  const seen = new Set();
  for (const key of prefs.always || []) {
    if (known.has(key) && !off[key] && !seen.has(key)) {
      always.push(key);
      seen.add(key);
    }
  }
  for (const key of prefs.demand || []) {
    if (known.has(key) && !off[key] && !seen.has(key)) {
      demand.push(key);
      seen.add(key);
    }
  }
  for (const key of def.always.concat(def.demand)) {
    if (!seen.has(key) && !off[key] && known.has(key)) {
      if (def.always.indexOf(key) >= 0) always.push(key);
      else demand.push(key);
      seen.add(key);
    }
  }
  return { always, demand, off };
}

/** @returns {RowPrefs} */
export function prefsForCard(card, layout) {
  return mergeRowPrefs(card.rows || [], layout.rows && layout.rows[card.id], {
    hideExtras: layout.hideExtras,
  });
}

/** @returns {Row[]} */
export function visibleRowsFor(card, opts) {
  const rows = card.rows || [];
  const byKey = new Map();
  for (const row of rows) byKey.set(rowKey(row), row);
  const prefs = mergeRowPrefs(rows, opts && opts.prefs, { hideExtras: opts && opts.hideExtras });
  const keys = opts && opts.collapsed ? prefs.always : prefs.always.concat(prefs.demand);
  const out = [];
  for (const key of keys) {
    const row = byKey.get(key);
    if (row) out.push(row);
  }
  return out;
}

export function cardHasExtras(card, hideExtras, prefs) {
  const merged = mergeRowPrefs(card.rows || [], prefs, { hideExtras });
  return merged.demand.length > 0;
}

/** @returns {RowPrefs} */
export function setRowEnabled(prefs, key, enabled) {
  const next = {
    always: (prefs.always || []).filter((item) => item !== key),
    demand: (prefs.demand || []).filter((item) => item !== key),
    off: Object.assign({}, prefs.off || {}),
  };
  if (enabled) {
    delete next.off[key];
    next.demand.push(key);
  } else {
    next.off[key] = true;
  }
  return next;
}

/** @returns {RowPrefs} */
export function moveRowToList(prefs, key, list, beforeKey) {
  const next = {
    always: (prefs.always || []).filter((item) => item !== key),
    demand: (prefs.demand || []).filter((item) => item !== key),
    off: Object.assign({}, prefs.off || {}),
  };
  delete next.off[key];
  const target = list === "always" ? next.always : next.demand;
  if (!beforeKey) target.push(key);
  else {
    const idx = target.indexOf(beforeKey);
    if (idx < 0) target.push(key);
    else target.splice(idx, 0, key);
  }
  return next;
}

function vendorSlug(entryId) {
  return String(entryId || "").split("@")[0].toLowerCase();
}

const ICON_ALIAS = { supergrok: "grok" };

// Icon slug for a card: the vendor half of "vendor@account", with product
// aliases folded onto the shared icon.
export function providerIconId(entryId) {
  const slug = vendorSlug(entryId).trim();
  return ICON_ALIAS[slug] || slug;
}

// Two-letter avatar fallback when no icon exists for the provider.
export function initialsGlyph(title) {
  const text = String(title || "?").trim();
  return (text.length >= 2 ? text.slice(0, 2) : text.charAt(0) || "?").toUpperCase();
}

// The host attaches `sign_in` per entry from VendorId::sign_in_hint, so this
// file keeps no provider table: one that lived here disagreed with Rust for
// five of eight providers before it shipped, and a JS object cannot fail to
// compile when a provider is added.
function signInHint(entry) {
  const hint = entry && typeof entry === "object" ? entry.signIn : undefined;
  return (typeof hint === "string" && hint.trim()) || "Open TUI → Settings to sign in.";
}

function joinError(explained) {
  if (!explained || !explained.title) return "";
  if (!explained.hint) return explained.title;
  return explained.title + ". " + explained.hint;
}

function shortenDiagnostic(raw) {
  let text = String(raw || "")
    .replace(/credentials error:\s*/ig, "")
    .replace(/network transport error:\s*/ig, "")
    .replace(/schema mismatch:\s*/ig, "")
    .replace(/HTTP \d+:\s*/g, "")
    .replace(/[A-Za-z]:\\[^\s]+/g, "")
    .replace(/(?:\/home|\/Users|\/root|~)[^\s]*/g, "")
    .replace(/\s{2,}/g, " ")
    .trim();
  text = text.replace(/^[A-Za-z0-9. _-]+:\s+/, "");
  if (text === "") return "Open TUI for details.";
  // The card wraps long hints, so keep the whole diagnosis; only a runaway
  // body (an HTML error page pasted into the message) is cut.
  if (text.length <= 400) return text;
  return text.slice(0, 399) + "…";
}

const OPEN_TUI = { cmd: "open-tui", label: "Open TUI" };
const REFRESH = { cmd: "refresh", label: "Refresh" };

// Title + one-line hint for a raw vendor error, plus the one action the popover
// can offer (a host command) when there is one. Errors whose fix is a terminal
// command (sign-in) or waiting (429 backoff) carry no action.
/** @returns {ExplainedError} */
export function explainError(text, entry) {
  const raw = clean(text, 1200);
  if (raw === "") return { title: "", hint: "" };
  if (/no vendors enabled/i.test(raw)) {
    return { title: "No providers enabled", hint: "Open TUI → Settings to turn one on.", action: OPEN_TUI };
  }
  if (/no API key/i.test(raw)) {
    return { title: "No API key", hint: "Open TUI → Settings to add one.", action: OPEN_TUI };
  }
  if (/no local server found|Antigravity must be running|no local language server/i.test(raw)) {
    return {
      title: "Antigravity isn't running",
      hint: "Open the Antigravity app or an agy session, then Refresh.",
      action: REFRESH,
    };
  }
  if (/HTTP 429|rate limited|too many requests/i.test(raw)) {
    // The cache backs off after a 429 and says when it will retry; surface
    // that instead of inviting a manual Refresh the backoff would ignore.
    const retry = /next attempt in ([0-9]+[a-z]+(?: [0-9]+[a-z]+)?)/i.exec(raw);
    return {
      title: "Too many requests",
      hint: retry ? "Retrying automatically in " + retry[1] + "." : "Try Refresh in a minute.",
    };
  }
  if (/HTTP 401|HTTP 403|authentication rejected|not signed in|token refresh failed|re-auth|run `claude`|run `codex/i.test(raw)) {
    return { title: "Sign-in expired", hint: signInHint(entry) };
  }
  if (/HTTP 5\d\d|schema mismatch/i.test(raw)) {
    return { title: "Provider is unavailable", hint: "Try Refresh in a bit.", action: REFRESH };
  }
  if (/network transport|timed out|timeout|connection refused|dns|connect/i.test(raw)) {
    return { title: "Can't reach the server", hint: "Check your connection, then Refresh.", action: REFRESH };
  }
  if (/io error/i.test(raw)) {
    return { title: "Couldn't read local files", hint: "Open TUI for details.", action: OPEN_TUI };
  }
  if (/did not contain valid JSON/i.test(raw)) {
    return { title: "Couldn't read usage data", hint: "Try Refresh. If it keeps happening, Open TUI.", action: REFRESH };
  }
  return { title: "Couldn't update", hint: shortenDiagnostic(raw) };
}

// A vendor's "Warning" text section (the cached-data note built from
// last_error in the Rust panels; Kimi labels its schema warning itself).
/**
 * The TUI panels append a "Resets" text section because their meter footnote
 * shows something else. Each popover meter already prints its own countdown
 * from `resetAt`, so that section only repeats a value on screen. It stays when
 * no meter in the card carries a reset stamp (a prepaid balance with an expiry).
 * @param {Row[]} rows
 */
function dropRedundantResetRows(rows) {
  const meterHasReset = rows.some((row) => row.kind === "metric" && row.resetAt);
  if (!meterHasReset) return;
  for (let i = rows.length - 1; i >= 0; i -= 1) {
    const row = rows[i];
    if (row.kind === "text" && row.label === "Resets") rows.splice(i, 1);
  }
}

function isWarningSection(section) {
  return section.type === "text" && (section.label === "Warning" || /schema drift/i.test(section.label));
}

export function friendlyError(text, entry) {
  return joinError(explainError(text, entry));
}

export function nextUpdateLabel(payload, nowMs) {
  const remaining = (Number(payload.nextRefreshAt) || 0) - (Number(nowMs) || 0);
  if (!(remaining > 0)) return "Updating…";
  return "Next update in " + formatDuration(remaining);
}

// "just now", "5m ago", "2h ago", "3d ago".
export function formatAgo(milliseconds) {
  const ms = Number(milliseconds) || 0;
  if (ms < 60_000) return "just now";
  const minutes = Math.floor(ms / 60_000);
  if (minutes < 60) return minutes + "m ago";
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return hours + "h ago";
  return Math.floor(hours / 24) + "d ago";
}

// The Settings row under the update-mode picker.
export function updateStatusLabel(payload, nowMs) {
  const update = payload && payload.update;
  if (!update) {
    const checkedAt = finiteNumber(payload && payload.updateCheckedAt);
    if (checkedAt === 0) return "Not checked yet";
    return "Up to date · checked " + formatAgo((Number(nowMs) || 0) - checkedAt);
  }
  const version = update.version ? "v" + String(update.version).replace(/^v/i, "") : "";
  switch (update.state) {
    case "checking":
      return "Checking…";
    case "downloading":
      return "Downloading " + (version || "update") + "…";
    case "installing":
      return "Installing…";
    case "failed":
      return update.error ? "Couldn't update: " + update.error : "Couldn't update";
    default:
      return (version || "An update") + " available";
  }
}

// The dashboard shows an update banner while the host has one in hand.
// A check in flight is Settings feedback, not something to install.
export function updateBannerPending(payload) {
  const update = payload && payload.update;
  return !!update && update.state !== "checking";
}

export function updateModeLabel(mode) {
  switch (normalizeUpdateMode(mode)) {
    case "auto":
      return "Automatic";
    case "off":
      return "Off";
    default:
      return "Notify me";
  }
}

// Physical-key names for the shortcut recorder, keyed by `KeyboardEvent.code`.
const SHORTCUT_CODES = {
  Space: "Space",
  Enter: "Enter",
  Tab: "Tab",
  Backspace: "Backspace",
  Delete: "Delete",
  Insert: "Insert",
  Home: "Home",
  End: "End",
  PageUp: "PageUp",
  PageDown: "PageDown",
  ArrowUp: "Up",
  ArrowDown: "Down",
  ArrowLeft: "Left",
  ArrowRight: "Right",
  Minus: "-",
  Equal: "=",
  BracketLeft: "[",
  BracketRight: "]",
  Backslash: "\\",
  Semicolon: ";",
  Quote: "'",
  Comma: ",",
  Period: ".",
  Slash: "/",
  Backquote: "`",
};

function shortcutKeyName(code) {
  const text = String(code || "");
  let match = /^Key([A-Z])$/.exec(text);
  if (match) return match[1];
  match = /^Digit([0-9])$/.exec(text);
  if (match) return match[1];
  match = /^F([1-9]|1[0-9]|2[0-4])$/.exec(text);
  if (match) return "F" + match[1];
  return Object.prototype.hasOwnProperty.call(SHORTCUT_CODES, text) ? SHORTCUT_CODES[text] : null;
}

// Turns a keydown into the canonical "Ctrl+Shift+U" form the host registers,
// or null when the press is not a usable global shortcut: a modifier on its
// own, Escape, a key with no Ctrl/Alt/Win, or a key the host has no name for.
export function shortcutFromKeyEvent(event) {
  if (!event || typeof event !== "object") return null;
  if (!(event.ctrlKey || event.altKey || event.metaKey)) return null;
  if (event.key === "Escape" || event.code === "Escape") return null;
  const key = shortcutKeyName(event.code);
  if (!key) return null;
  const parts = [];
  if (event.ctrlKey) parts.push("Ctrl");
  if (event.altKey) parts.push("Alt");
  if (event.shiftKey) parts.push("Shift");
  if (event.metaKey) parts.push("Win");
  parts.push(key);
  return parts.join("+");
}

// Collapses "system" into the scheme the OS currently prefers.
export function resolvedTheme(theme) {
  const value = normalizeTheme(theme);
  if (value !== "system") return value;
  const dark =
    typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-color-scheme: dark)").matches;
  return dark ? "dark" : "light";
}

export function applyTheme(theme) {
  if (typeof document === "undefined") return;
  const value = normalizeTheme(theme);
  document.documentElement.setAttribute("data-theme", value);
  document.documentElement.classList.toggle("dark", resolvedTheme(value) === "dark");
}

export function applyDensity(density) {
  if (typeof document === "undefined") return;
  document.documentElement.dataset.density = normalizeDensity(density);
}

function isPlainObject(value) {
  return !!value && typeof value === "object" && !Array.isArray(value);
}

// `extra` fields ride along after `cmd`, e.g. {cmd:"resize", height: 512}.
export function sendCommand(cmd, extra) {
  const message = { cmd: String(cmd || "") };
  if (isPlainObject(extra)) {
    for (const key of Object.keys(extra)) {
      if (key !== "cmd") message[key] = extra[key];
    }
  }
  const msg = JSON.stringify(message);
  if (typeof window !== "undefined" && window.ipc && typeof window.ipc.postMessage === "function") {
    window.ipc.postMessage(msg);
  }
}

