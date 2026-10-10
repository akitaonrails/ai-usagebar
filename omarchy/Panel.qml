import QtQuick
import QtQuick.Controls
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Ui
import "Model.js" as Model
import "I18n.js" as I18n

// Native Omarchy Quattro popup. BarWidget.qml owns the bar slot and injects
// its button as this panel's anchor; collection stays in the Rust binary.
Panel {
  id: root
  moduleName: "akitaonrails.ai-usagebar"
  manageIpc: false

  property var anchorItem: null
  property var hostWidget: null
  readonly property var barIdentity: hostWidget || root

  readonly property color foreground: bar ? bar.foreground : Color.foreground
  readonly property color urgent: bar ? bar.urgent : Color.urgent
  readonly property color dim: Qt.darker(foreground, 1.45)
  readonly property color track: Style.selectedFillFor(foreground, Color.accent)
  readonly property string fontFamily: bar ? bar.fontFamily : Style.font.family
  readonly property bool vertical: bar ? bar.vertical : false

  // RAG palette from Omarchy colors.toml (same source as Waybar Theme).
  property string hexGreen: "#98c379"
  property string hexYellow: "#e5c07b"
  property string hexOrange: "#d19a66"
  property string hexRed: "#e06c75"
  property color themeGreen: hexGreen
  property color themeYellow: hexYellow
  property color themeOrange: hexOrange
  property color themeRed: hexRed

  property var entries: []
  property string primaryProvider: ""
  property string selectedEntryId: ""
  property string loadError: ""
  property string commandStderr: ""
  property string commandStdout: ""
  property bool loading: true
  property int lastExitCode: 0
  property bool refreshQueued: false
  property double lastSuccessfulMs: 0
  property double nowMs: Date.now()
  property bool cursorActive: false
  property bool settingsOpen: false

  readonly property int refreshIntervalSec: Math.max(30, Math.min(3600,
    Number(setting("refreshIntervalSec", 300)) || 300))
  readonly property string configuredProvider: String(setting("provider", "") || "").trim()
  readonly property string rememberedEntryId: String(setting("lastSelectedEntryId", "") || "").trim()
  readonly property bool showValue: Model.booleanSetting(setting("showValue", true), true)
  readonly property bool showProvider: Model.booleanSetting(setting("showProvider", false), false)
  readonly property bool showAll: Model.booleanSetting(setting("showAll", false), false)
  readonly property bool colorCodeUsage: Model.booleanSetting(setting("colorCodeUsage", false), false)
  readonly property string uiLocaleSetting: String(setting("uiLocale", "auto") || "auto")
  // Bind Qt.locale so Auto tracks the shell language without a restart.
  readonly property string uiLocale: I18n.resolveLocale(uiLocaleSetting, Qt.locale().name)
  readonly property string barWindow: Model.normalizeBarWindow(setting("barWindow", "auto"))
  readonly property bool showCursorModels: Model.booleanSetting(setting("showCursorModels", true), true)
  readonly property bool showCursorOther: Model.booleanSetting(setting("showCursorOther", true), true)
  readonly property bool showCursorOnDemand: Model.booleanSetting(setting("showCursorOnDemand", true), true)
  readonly property bool showCursorCredits: Model.booleanSetting(setting("showCursorCredits", true), true)
  readonly property bool showAntigravityGemini: Model.booleanSetting(setting("showAntigravityGemini", true), true)
  readonly property bool showAntigravityClaudeGpt: Model.booleanSetting(setting("showAntigravityClaudeGpt", true), true)
  readonly property bool brandIcons: Model.booleanSetting(setting("brandIcons", true), true)
  readonly property string showAs: Model.normalizeShowAs(setting("showAs", "used"))
  readonly property var hiddenMetrics: Model.normalizeHiddenMetrics(setting("hiddenMetrics", {}))
  readonly property var visibleEntries: Model.filteredEntries(entries, configuredProvider)
  readonly property int entryIndex: Model.selectedIndex(visibleEntries, selectedEntryId)
  readonly property var entry: entryIndex >= 0 ? visibleEntries[entryIndex] : null
  readonly property string entryFetchedAt: {
    if (!entry) return ""
    return String(entry.fetched_at || "")
  }
  readonly property var shapedEntries: visibleEntries.map(function(item) {
    return Model.panelEntry(
      item,
      hiddenMetrics,
      cursorPoolFlags(),
      antigravityPoolFlags())
  })
  readonly property var shapedEntry: entryIndex >= 0 ? shapedEntries[entryIndex] : null
  readonly property var summary: Model.headline(shapedEntry, barWindow, showAs)
  // barWindow pins the bar value and its echoes (hero detail, tooltip).
  // Panel rows and alert state keep the historical highest-percent headline,
  // matching every other frontend (Waybar class, KDE isAlarming, TUI).
  // Grouped sub-rows (SuperGrok's product slices) gain a heading row here so
  // they render as a breakdown of the meter above, not peers of it.
  readonly property bool cursorEntry: isCursorEntry(entry)
  readonly property bool antigravityEntry: isAntigravityEntry(entry)
  readonly property var entrySections: entry
    ? Model.groupedSections(Model.panelEntry(
      entry,
      hiddenMetrics,
      cursorPoolFlags(),
      antigravityPoolFlags()).sections) : []
  readonly property var metricEntries: visibleEntries.map(function(item) {
    return {
      id: item.id,
      name: Model.providerName(item),
      rows: Model.metricChoices(item, hiddenMetrics)
    }
  }).filter(function(item) { return item.rows.length > 0 })
  readonly property bool filterMiss: configuredProvider !== "" && entries.length > 0 && visibleEntries.length === 0
  readonly property bool entryAlarming: shownAlarming()
  // A cached or failed provider response stays a status line, but a report
  // that never arrived has nothing else to show on the bar.
  readonly property bool reportMissing: loadError !== "" && entries.length === 0
  readonly property bool alarming: (showAll ? shownAnyAlarming() : entryAlarming)
    || reportMissing

  function tr(key, params) {
    return I18n.t(uiLocale, key, params || null)
  }

  function setUiLocale(value) {
    var next = I18n.normalizeLocaleTag(value)
    if (next === "") next = "auto"
    if (next === uiLocaleSetting) return
    persistWidgetSettings({ uiLocale: next })
  }

  function alpha(color, opacity) {
    return Qt.rgba(color.r, color.g, color.b, opacity)
  }

  function clamp(value, low, high) {
    return Math.max(low, Math.min(high, value))
  }

  function applyThemePalette(raw) {
    var parsed = Model.parseThemePalette(raw)
    hexGreen = parsed.green
    hexYellow = parsed.yellow
    hexOrange = parsed.orange
    hexRed = parsed.red
    themeGreen = hexGreen
    themeYellow = hexYellow
    themeOrange = hexOrange
    themeRed = hexRed
  }

  // critical prefers Quattro urgent (theme-linked); other rungs use colors.toml.
  // When colorCodeUsage is off, everything falls back to the bar foreground.
  function severityColorOf(severity) {
    if (!colorCodeUsage) return foreground
    if (severity === "critical") return urgent
    if (severity === "high") return themeOrange
    if (severity === "mid") return themeYellow
    if (severity === "low") return themeGreen
    return foreground
  }

  function severityHexOf(severity) {
    if (!colorCodeUsage) return ""
    if (severity === "critical") return hexRed
    if (severity === "high") return hexOrange
    if (severity === "mid") return hexYellow
    if (severity === "low") return hexGreen
    return ""
  }

  FileView {
    id: themeColorsFile
    path: Color.currentThemePath + "/colors.toml"
    watchChanges: true
    printErrors: false
    onLoaded: root.applyThemePalette(text())
    onFileChanged: reload()
    onLoadFailed: root.applyThemePalette("")
  }

  Connections {
    target: Color
    function onUrgentChanged() { themeColorsFile.reload() }
    function onAccentChanged() { themeColorsFile.reload() }
    function onForegroundChanged() { themeColorsFile.reload() }
  }

  function syncSelection() {
    if (visibleEntries.length === 0) {
      selectedEntryId = ""
      return
    }
    // The persisted choice is the source of truth. A refresh gap (fetch
    // error, sleep/wake stale list) briefly drops entries; the fallback
    // below then re-resolves to the primary and that transient selection
    // used to stick — the chosen entry came back and was ignored until a
    // shell restart. Once the remembered entry is back in the list, it wins
    // over any selection that only exists because of that gap.
    var remembered = rememberedEntryId
    if (remembered !== "") {
      for (var r = 0; r < visibleEntries.length; r++)
        if (visibleEntries[r].id === remembered) {
          selectedEntryId = remembered
          return
        }
    }
    for (var i = 0; i < visibleEntries.length; i++)
      if (visibleEntries[i].id === selectedEntryId) return
    selectedEntryId = Model.preferredEntryId(visibleEntries, primaryProvider, rememberedEntryId)
  }

  function restoreRememberedSelection() {
    if (visibleEntries.length === 0) return
    selectedEntryId = Model.preferredEntryId(visibleEntries, primaryProvider, rememberedEntryId)
  }

  function persistWidgetSettings(values) {
    // Quattro persists inline widget settings in shell.json and pushes them
    // live to every monitor. Keep every existing setting, including settings
    // introduced by future versions, and apply only the requested overrides.
    var entry = Model.settingsWithOverrides(root.settings, root.moduleName, values)
    if (!entry) return false

    // Apply locally first so controls remain responsive. Older compatible hosts
    // without updateEntryInline still retain the choice for this session.
    root.settings = entry
    if (hostWidget && "settings" in hostWidget) hostWidget.settings = entry
    if (bar && bar.shell && typeof bar.shell.updateEntryInline === "function")
      bar.shell.updateEntryInline(root.moduleName, entry)
    return true
  }

  function persistSelection(entryId) {
    if (String(entryId || "").trim() === rememberedEntryId) return
    persistWidgetSettings({ lastSelectedEntryId: entryId })
  }

  function setShowValue(enabled) {
    var next = enabled === true
    if (next === showValue) return
    persistWidgetSettings({ showValue: next })
  }

  function setShowProvider(enabled) {
    var next = enabled === true
    if (next === showProvider) return
    persistWidgetSettings({ showProvider: next })
  }

  function setShowAll(enabled) {
    var next = enabled === true
    if (next === showAll) return
    persistWidgetSettings({ showAll: next })
  }

  function setColorCodeUsage(enabled) {
    var next = enabled === true
    if (next === colorCodeUsage) return
    persistWidgetSettings({ colorCodeUsage: next })
  }

  function setBarWindow(value) {
    var next = Model.normalizeBarWindow(value)
    if (next === barWindow) return
    persistWidgetSettings({ barWindow: next })
  }

  function setBrandIcons(enabled) {
    var next = enabled === true
    if (next === brandIcons) return
    persistWidgetSettings({ brandIcons: next })
  }

  function setShowAs(value) {
    var next = Model.normalizeShowAs(value)
    if (next === showAs) return
    persistWidgetSettings({ showAs: next })
  }

  function setMetricShown(entryId, key) {
    var target = null
    for (var i = 0; i < visibleEntries.length; i++)
      if (visibleEntries[i].id === entryId) target = visibleEntries[i]
    if (!target) return
    if (!Model.canToggleMetric(target, hiddenMetrics, key)) return
    persistWidgetSettings({ hiddenMetrics: Model.toggleMetricChoice(hiddenMetrics, target, key) })
  }

  function isCursorEntry(item) {
    if (!item) return false
    var id = String(item.id || "")
    var at = id.indexOf("@")
    return (at < 0 ? id : id.slice(0, at)) === "cursor"
  }

  function isAntigravityEntry(item) {
    if (!item) return false
    var id = String(item.id || "")
    var at = id.indexOf("@")
    return (at < 0 ? id : id.slice(0, at)) === "antigravity"
  }

  function cursorPoolFlags() {
    return {
      models: showCursorModels,
      other: showCursorOther,
      demand: showCursorOnDemand,
      credits: showCursorCredits
    }
  }

  function antigravityPoolFlags() {
    return {
      gemini: showAntigravityGemini,
      third_party: showAntigravityClaudeGpt
    }
  }

  function cursorShownFlags() {
    var base = typeof Model.cursorBarFlags === "function"
      ? Model.cursorBarFlags(Model.windowEntry(entry, hiddenMetrics), cursorPoolFlags())
      : cursorPoolFlags()
    var flags = {
      models: base.models === true,
      other: base.other === true,
      demand: base.demand === true,
      credits: base.credits === true
    }
    // Hot reload can keep an older Model.js that has no credits flag. The
    // grant is still in the report, so the switch follows this file.
    if (base.credits === undefined)
      flags.credits = showCursorCredits && creditGrantBits(entry).length > 0
    return flags
  }

  function cursorPoolOn(id) {
    return cursorShownFlags()[id] === true
  }

  function cursorPoolCanTurnOff(id) {
    var flags = cursorShownFlags()
    if (flags[id] !== true) return false
    var count = (flags.models ? 1 : 0) + (flags.other ? 1 : 0) + (flags.demand ? 1 : 0) + (flags.credits ? 1 : 0)
    return count > 1
  }

  function cursorPoolButtons() {
    var has = typeof Model.cursorPoolPresence === "function"
      ? Model.cursorPoolPresence(Model.windowEntry(entry, hiddenMetrics))
      : { models: true, other: true, demand: true, credits: false }
    var rows = []
    if (has.models) rows.push({ poolId: "models", label: root.tr("pool.models") })
    if (has.other) rows.push({ poolId: "other", label: root.tr("pool.other") })
    if (has.demand) rows.push({ poolId: "demand", label: root.tr("pool.demand") })
    if (has.credits || creditGrantBits(entry).length > 0)
      rows.push({ poolId: "credits", label: root.tr("pool.credits") })
    return rows
  }

  function antigravityShownFlags() {
    if (typeof Model.antigravityBarFlags !== "function") return antigravityPoolFlags()
    return Model.antigravityBarFlags(Model.windowEntry(entry, hiddenMetrics), antigravityPoolFlags())
  }

  function antigravityPoolOn(id) {
    return antigravityShownFlags()[id] === true
  }

  function antigravityPoolCanTurnOff(id) {
    var flags = antigravityShownFlags()
    if (flags[id] !== true) return false
    return (flags.gemini ? 1 : 0) + (flags.third_party ? 1 : 0) > 1
  }

  function antigravityPoolButtons() {
    var has = typeof Model.antigravityPoolPresence === "function"
      ? Model.antigravityPoolPresence(Model.windowEntry(entry, hiddenMetrics))
      : { gemini: true, third_party: true }
    var rows = []
    if (has.gemini) rows.push({ poolId: "gemini", label: root.tr("pool.antigravity_gemini") })
    if (has.third_party) rows.push({ poolId: "third_party", label: root.tr("pool.antigravity_third_party") })
    return rows
  }

  function toggleCursorPool(id) {
    var saved = cursorPoolFlags()
    var shown = cursorShownFlags()
    var next = Model.toggleCursorPool(shown, id)
    // Older Model.js ignores the credits id and hands the same flags back.
    if (id === "credits" && next.credits === shown.credits) {
      next = {
        models: shown.models,
        other: shown.other,
        demand: shown.demand,
        credits: !shown.credits
      }
      if (!next.models && !next.other && !next.demand && !next.credits) return
    }
    if (next.models === shown.models && next.other === shown.other
        && next.demand === shown.demand && next.credits === shown.credits) return
    var has = typeof Model.cursorPoolPresence === "function"
      ? Model.cursorPoolPresence(entry)
      : { models: true, other: true, demand: true, credits: false }
    persistWidgetSettings({
      showCursorModels: has.models ? next.models : saved.models,
      showCursorOther: has.other ? next.other : saved.other,
      showCursorOnDemand: has.demand ? next.demand : saved.demand,
      showCursorCredits: has.credits ? next.credits : saved.credits
    })
  }

  function toggleAntigravityPool(id) {
    var saved = antigravityPoolFlags()
    var shown = antigravityShownFlags()
    var next = Model.toggleAntigravityPool(shown, id)
    if (next.gemini === shown.gemini && next.third_party === shown.third_party) return
    var has = typeof Model.antigravityPoolPresence === "function"
      ? Model.antigravityPoolPresence(entry)
      : { gemini: true, third_party: true }
    persistWidgetSettings({
      showAntigravityGemini: has.gemini ? next.gemini : saved.gemini,
      showAntigravityClaudeGpt: has.third_party ? next.third_party : saved.third_party
    })
  }

  function providerPoolButtons() {
    return antigravityEntry ? antigravityPoolButtons() : cursorPoolButtons()
  }

  function providerPoolOn(id) {
    return antigravityEntry ? antigravityPoolOn(id) : cursorPoolOn(id)
  }

  function providerPoolCanTurnOff(id) {
    return antigravityEntry ? antigravityPoolCanTurnOff(id) : cursorPoolCanTurnOff(id)
  }

  function toggleProviderPool(id) {
    if (antigravityEntry) toggleAntigravityPool(id)
    else toggleCursorPool(id)
  }

  function entryIsAlarming(item) {
    if (!item) return false
    var dual = providerDualHeadline(item)
    if (isCursorEntry(item) || isAntigravityEntry(item)) {
      if (dual) return dual.allCritical === true
    }
    return Model.isAlarming(item)
  }

  function shownAlarming() {
    return entryIsAlarming(shapedEntry)
  }

  function shownAnyAlarming() {
    for (var i = 0; i < shapedEntries.length; i++)
      if (entryIsAlarming(shapedEntries[i])) return true
    return false
  }

  function selectEntry(index) {
    if (visibleEntries.length === 0) return
    var wrapped = ((index % visibleEntries.length) + visibleEntries.length) % visibleEntries.length
    selectedEntryId = visibleEntries[wrapped].id
    persistSelection(selectedEntryId)
    if (providerList.visible) providerList.forceLayout()
    if (panelFlick) panelFlick.contentY = 0
  }

  // A bar chip stands for one entry when the bar shows several: select it,
  // then open the panel on it. The chip the panel already shows toggles
  // closed, the way the bar button does.
  function openEntry(entryId) {
    var wanted = String(entryId || "")
    for (var i = 0; i < visibleEntries.length; i++) {
      if (visibleEntries[i].id !== wanted) continue
      if (opened && i === entryIndex) {
        close()
        return
      }
      selectEntry(i)
      open()
      return
    }
    open()
  }

  function startRefresh() {
    if (usageProcess.running) {
      refreshQueued = true
      return
    }
    refreshQueued = false
    commandStdout = ""
    commandStderr = ""
    if (entries.length === 0) loading = true
    usageProcess.running = true
  }

  function finishRefresh() {
    var parsed = Model.parseReport(commandStdout)
    if (parsed.ok) {
      primaryProvider = parsed.primary
      entries = parsed.entries
      loadError = ""
      lastSuccessfulMs = Date.now()
      syncSelection()
    } else {
      var detail = commandStderr.trim()
      loadError = lastExitCode === 127
        ? Model.launchErrorMessage(lastExitCode, detail)
        : (detail !== "" ? Model.errorMessage(detail) : parsed.error)
    }
    loading = false
    if (refreshQueued) Qt.callLater(startRefresh)
  }

  function refresh() { startRefresh() }

  function openSettings() {
    settingsOpen = true
    cursorActive = false
    if (panelFlick) panelFlick.contentY = 0
    Qt.callLater(function() { settingsView.forceActiveFocus() })
  }

  function closeSettings() {
    settingsOpen = false
    if (panelFlick) panelFlick.contentY = 0
    Qt.callLater(function() { keyCatcher.forceActiveFocus() })
  }

  function openTerminalSettings() {
    if (hostWidget && typeof hostWidget.launchDashboard === "function")
      hostWidget.launchDashboard()
    else if (bar)
      bar.run("omarchy-launch-floating-terminal-with-presentation ai-usagebar-tui")
  }

  function openNousLogin() {
    if (bar && typeof bar.run === "function")
      bar.run("omarchy-launch-floating-terminal-with-presentation ai-usagebar auth nous login")
  }

  function openCopilotLogin() {
    if (bar && typeof bar.run === "function")
      bar.run("omarchy-launch-floating-terminal-with-presentation gh auth login --web")
  }

  function switchPanel(direction) {
    if (bar && typeof bar.switchPanelFrom === "function")
      return bar.switchPanelFrom(barIdentity, direction)
    return false
  }

  function statusMessage() {
    if (filterMiss) return root.tr("status.filter_miss", { id: configuredProvider })
    if (entry && entry.error !== "") return entry.error
    if (loadError !== "") return entries.length > 0
      ? root.tr("status.refresh_failed", { error: loadError })
      : loadError
    if (entry && entry.stale) return root.tr("status.cached")
    return ""
  }

  function statusIsUrgent() {
    return filterMiss || (entry && entry.error !== "") || loadError !== ""
  }

  function heroMeta() {
    if (!entry) return loading ? root.tr("hero.loading") : root.tr("hero.usage_report")
    if (entry.error !== "") return root.tr("hero.provider_unavailable")
    var text = entry.plan || root.tr("hero.usage_limits")
    if (entry.stale) text += " · " + root.tr("tip.cached")
    return Model.autoTextSafe(text)
  }

  // A credit grant is a metric beside the two model pools. The bar and its
  // hover read it here, so a headline that only knows the pools still shows it.
  function creditGrantBits(item) {
    var sections = item && item.sections ? item.sections : []
    var bits = []
    for (var i = 0; i < sections.length; i++) {
      var row = sections[i]
      if (!row || row.type !== "metric") continue
      var label = String(row.label || "")
      if (label === "" || label === "Cursor Models" || label === "Other Models") continue
      if (row.percent === null || row.percent === undefined || row.percent === "") continue
      bits.push({
        text: Model.percentText(row.percent, showAs),
        line: label + " · " + Model.percentText(row.percent, showAs),
        severity: row.severity || "low"
      })
    }
    return bits
  }

  function withCreditGrants(pools, item) {
    if (!pools || !showCursorCredits) return pools
    var grants = creditGrantBits(item)
    if (grants.length === 0) return pools
    var text = String(pools.text || "")
    var tooltip = String(pools.tooltip || pools.text || "")
    var segments = pools.segments ? pools.segments.slice() : []
    var tooltipRows = pools.tooltipRows ? pools.tooltipRows.slice() : []
    if (tooltipRows.length === 0 && tooltip !== "") {
      var prior = tooltip.split("\n")
      for (var p = 0; p < prior.length; p++) {
        var priorLine = prior[p].trim()
        if (priorLine !== "") tooltipRows.push({ text: priorLine, severity: pools.severity || "low" })
      }
    }
    var added = false
    for (var i = 0; i < grants.length; i++) {
      var bit = grants[i]
      var name = bit.line.split(" · ")[0]
      if (tooltip.indexOf(name + " · ") >= 0) continue
      added = true
      text = text === "" ? bit.text : text + " · " + bit.text
      tooltip = tooltip === "" ? bit.line : tooltip + "\n" + bit.line
      if (segments.length > 0) {
        segments.push({ text: " · ", severity: "" })
        segments.push({ text: bit.text, severity: bit.severity })
      }
      tooltipRows.push({ text: bit.line, severity: bit.severity })
    }
    if (!added) return pools
    return {
      text: text,
      tooltip: tooltip,
      severity: pools.severity,
      allCritical: pools.allCritical === true,
      segments: segments,
      tooltipRows: tooltipRows
    }
  }

  function providerDualHeadline(item) {
    if (isAntigravityEntry(item) && typeof Model.antigravityDualHeadline === "function")
      return Model.antigravityDualHeadline(item, antigravityPoolFlags(), showAs, barWindow)
    if (isCursorEntry(item) && typeof Model.cursorDualHeadline === "function")
      return Model.cursorDualHeadline(item, cursorPoolFlags(), showAs)
    return null
  }

  function cursorPools(item) {
    var pools = null
    if (typeof Model.cursorDualHeadline === "function") {
      var dual = Model.cursorDualHeadline(item, cursorPoolFlags(), showAs)
      if (dual && dual.text) pools = {
        text: dual.text,
        tooltip: dual.tooltip || dual.text,
        severity: dual.severity,
        allCritical: dual.allCritical === true,
        segments: dual.segments || [],
        tooltipRows: dual.tooltipRows || []
      }
    }
    if (!pools && item) {
      var id = String(item.id || "")
      var at = id.indexOf("@")
      if ((at < 0 ? id : id.slice(0, at)) === "cursor") {
        var sections = item.sections || []
        var auto = null
        var api = null
        for (var i = 0; i < sections.length; i++) {
          var row = sections[i]
          if (!row || row.type !== "metric") continue
          if (row.label === "Cursor Models") auto = row.percent
          else if (row.label === "Other Models") api = row.percent
        }
        if (auto !== null && api !== null && auto !== undefined && api !== undefined) {
          // This branch exists for a STALE Model.js during a plugin hot
          // reload, so it must not call any Model.* helper the fresh panel
          // code expects — percentText included. Format locally.
          var fmt = function(p) {
            var v = (showAs === "left") ? (100 - p) : p
            return v + "%"
          }
          pools = {
            text: fmt(auto) + " · " + fmt(api),
            tooltip: "Cursor Models " + fmt(auto)
              + " · Other Models " + fmt(api),
            segments: [],
            tooltipRows: []
          }
        }
      }
    }
    return withCreditGrants(pools, item)
  }

  function providerPools(item) {
    if (!isAntigravityEntry(item)) return cursorPools(item)
    var dual = providerDualHeadline(item)
    return dual && dual.text ? {
      text: dual.text,
      tooltip: dual.tooltip || dual.text,
      severity: dual.severity,
      allCritical: dual.allCritical === true,
      segments: dual.segments || [],
      tooltipRows: dual.tooltipRows || []
    } : null
  }

  function panelHeadline(item) {
    var dual = providerDualHeadline(item)
    if ((isCursorEntry(item) || isAntigravityEntry(item)) && dual && dual.text) return dual.text
    return usageText(item, false)
  }

  function usageText(item, rich) {
    var pools = providerPools(item)
    if (pools) return rich ? pools.tooltip : pools.text
    var head = Model.headline(item, barWindow, showAs)
    return rich ? (head.tooltip || head.text) : head.text
  }

  function shownEntries() {
    if (showAll) return shapedEntries
    if (shapedEntry) return [shapedEntry]
    return shapedEntries.length > 0 ? [shapedEntries[0]] : []
  }

  readonly property var barChips: labeledChips()

  function labeledChips() {
    var chips = Model.barChips(
      shapedEntries, shapedEntry, showAll, showValue, showProvider, loading, alarming, vertical,
      barWindow, showAs, brandIcons)
    if (!showValue || vertical) return chips
    var rows = shownEntries()
    var next = []
    for (var i = 0; i < chips.length; i++) {
      var chip = chips[i]
      var pools = i < rows.length ? providerPools(rows[i]) : null
      if (!pools || chip.label === "!") {
        if (chip.label === "!" || i >= rows.length) {
          next.push(chip)
          continue
        }
        // Non-Cursor chips: headline severity drives icon RAG when colour-coding is on.
        var head = Model.headline(rows[i], barWindow, showAs)
        next.push({
          id: chip.id,
          brand: chip.brand,
          icon: chip.icon,
          labelOnly: chip.labelOnly,
          label: chip.label,
          providerPrefix: "",
          alarming: chip.alarming,
          severity: head && head.severity ? head.severity : "",
          segments: chip.segments || []
        })
        continue
      }
      var label = pools.text
      var providerPrefix = ""
      if (showProvider || chip.labelOnly === true) {
        var provider = Model.providerShort(rows[i])
        if (provider !== "") {
          label = provider + " " + label
          providerPrefix = provider + " "
        }
      }
      var segs = pools.segments || []
      // Cursor icon chrome (classic): allCritical. RAG icon uses severity
      // (worst / max-used among visible pools — status highest-severity rule).
      var chipAlarm = chip.alarming
      if (pools.allCritical === true || pools.allCritical === false) {
        chipAlarm = pools.allCritical === true
      } else if (pools.severity === "low" || pools.severity === "mid" || pools.severity === "high"
          || pools.severity === "critical") {
        chipAlarm = pools.severity === "critical"
      }
      next.push({
        id: chip.id,
        brand: chip.brand,
        icon: chip.icon,
        labelOnly: chip.labelOnly,
        label: label,
        providerPrefix: providerPrefix,
        alarming: chipAlarm,
        severity: pools.severity || "",
        segments: segs
      })
    }
    return next
  }

  function barText() {
    if (showAll)
      return Model.barStrip(shapedEntries, alarming, vertical, showValue, showProvider, loading,
        barWindow, showAs)
    var value = shapedEntry ? usageText(shapedEntry, false) : summary.text
    return Model.barLabel(alarming, vertical, showValue, loading,
      entry !== null, value, showProvider ? Model.providerShort(entry) : "",
      Model.providerIcon(entry))
  }

  function tooltipLines(item) {
    var raw = String(usageText(item, true) || "").split("\n")
    var lines = []
    for (var i = 0; i < raw.length; i++) {
      var line = Model.autoTextSafe(raw[i]).trim()
      if (line !== "") lines.push(line)
    }
    return lines
  }

  // Colored hover rows when Cursor (or showAll) exposes per-pool severity.
  function tooltipRows() {
    function rowsFor(item) {
      var pools = providerPools(item)
      if (pools && pools.tooltipRows && pools.tooltipRows.length > 0) {
        var out = []
        for (var r = 0; r < pools.tooltipRows.length; r++) {
          var row = pools.tooltipRows[r]
          var text = ""
          if (row.pool)
            text = I18n.tipPoolLine(root.uiLocale, row.pool, Model.shownPercent(row.percent, root.showAs))
          else
            text = Model.autoTextSafe(row.text || "").trim()
          if (text === "") continue
          if (item && item.stale && r === pools.tooltipRows.length - 1)
            text += " · " + root.tr("tip.cached")
          out.push({ text: text, severity: row.severity || "low" })
        }
        return out
      }
      var lines = tooltipLines(item)
      if (lines.length === 0) return []
      var head = Model.headline(item, barWindow, showAs)
      var sev = head && head.severity ? head.severity : "low"
      if (lines.length > 1) {
        var multi = []
        for (var i = 0; i < lines.length; i++) {
          var bit = lines[i]
          if (item && item.stale && i === lines.length - 1) bit += " · " + root.tr("tip.cached")
          multi.push({ text: bit, severity: sev })
        }
        return multi
      }
      var single = Model.providerName(item)
      if (lines.length === 1) single += " · " + lines[0]
      if (item && item.stale) single += " · " + root.tr("tip.cached")
      return [{ text: single, severity: sev }]
    }

    if (showAll && shapedEntries.length > 0) {
      var chips = []
      for (var e = 0; e < shapedEntries.length; e++) {
        var chunk = rowsFor(shapedEntries[e])
        for (var c = 0; c < chunk.length; c++) chips.push(chunk[c])
      }
      return chips
    }
    if (!entry) {
      var msg = Model.autoTextSafe(statusMessage() || root.tr("app.name"))
      return msg !== "" ? [{ text: msg, severity: "" }] : []
    }
    return rowsFor(shapedEntry)
  }

  readonly property var ragTooltipRows: tooltipRows()
  readonly property string plainTooltipText: {
    var rows = ragTooltipRows
    if (rows.length > 0) {
      var lines = []
      for (var i = 0; i < rows.length; i++) {
        var text = String((rows[i] && rows[i].text) || "").trim()
        if (text !== "") lines.push(text)
      }
      if (lines.length > 0) return lines.join("\n")
    }
    return Model.autoTextSafe(statusMessage() || root.tr("app.name"))
  }

  onEntriesChanged: Qt.callLater(syncSelection)
  onConfiguredProviderChanged: Qt.callLater(syncSelection)
  onRememberedEntryIdChanged: Qt.callLater(restoreRememberedSelection)
  onOpenedChanged: {
    if (opened) {
      cursorActive = false
      nowMs = Date.now()
      if (panelFlick) panelFlick.contentY = 0
      if (lastSuccessfulMs === 0 || nowMs - lastSuccessfulMs >= refreshIntervalSec * 1000)
        startRefresh()
      Qt.callLater(function() { keyCatcher.forceActiveFocus() })
    } else {
      settingsOpen = false
    }
  }

  Timer {
    interval: root.refreshIntervalSec * 1000
    running: true
    repeat: true
    triggeredOnStart: true
    onTriggered: root.startRefresh()
  }

  Timer {
    interval: 30000
    running: root.opened
    repeat: true
    onTriggered: root.nowMs = Date.now()
  }

  Process {
    id: usageProcess
    running: false
    // /usr/bin/env always starts on Omarchy and reports a missing ai-usagebar
    // as exit 127. Keep the command as structured argv: no shell is needed.
    command: ["/usr/bin/env", "ai-usagebar", "usage", "--json"]

    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        root.commandStdout = text
      }
    }

    stderr: StdioCollector {
      waitForEnd: true
      onStreamFinished: root.commandStderr = text
    }

    onExited: function(exitCode) {
      root.lastExitCode = exitCode
      // Let both waitForEnd collectors publish their buffers first.
      Qt.callLater(function() { root.finishRefresh() })
    }
  }

  KeyboardPanel {
    id: panel
    anchorItem: root.anchorItem
    owner: root.barIdentity
    bar: root.bar
    open: root.opened
    focusTarget: keyCatcher
    contentWidth: panel.fittedContentWidth(Style.space(390))
    contentHeight: panel.fittedContentHeight(column.implicitHeight, Style.space(640))

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      // Native form controls own Tab/Enter/Esc while settings are open.
      blocked: root.settingsOpen

      onMoveRequested: function(dx, dy) {
        if (!root.settingsOpen && dx !== 0) {
          root.cursorActive = true
          root.selectEntry(root.entryIndex + dx)
        }
        if (dy !== 0)
          panelFlick.contentY = root.clamp(panelFlick.contentY + dy * Style.space(96), 0,
            Math.max(0, panelFlick.contentHeight - panelFlick.height))
      }
      onActivateRequested: if (!root.settingsOpen) root.refresh()
      onCloseRequested: root.settingsOpen ? root.closeSettings() : root.close()
      onTabRequested: function(direction) { root.switchPanel(direction) }
      onTextKey: function(text) {
        if (!root.settingsOpen && (text === "r" || text === "R")) root.refresh()
        else if (!root.settingsOpen && (text === "s" || text === "S")) root.openSettings()
      }

      Flickable {
        id: panelFlick
        anchors.fill: parent
        contentWidth: width
        contentHeight: column.implicitHeight
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        flickableDirection: Flickable.VerticalFlick
        interactive: contentHeight > height
        ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }

        WheelHandler {
          target: null
          enabled: panelFlick.interactive
          acceptedDevices: PointerDevice.Mouse | PointerDevice.TouchPad
          onWheel: function(event) {
            if (event.pixelDelta.y === 0 && event.angleDelta.y === 0) {
              event.accepted = false
              return
            }
            // Keep touchpad motion smooth while covering more of the long settings form.
            var delta = event.pixelDelta.y !== 0
              ? event.pixelDelta.y * 5
              : event.angleDelta.y / 120 * Style.space(144)
            panelFlick.contentY = root.clamp(panelFlick.contentY - delta,
              0, Math.max(0, panelFlick.contentHeight - panelFlick.height))
          }
        }

        Column {
          id: column
          // The provider tabs are bordered buttons, and the first one in each
          // row sits flush against this Flickable's clip edge. At fractional
          // device scales (a 1.25 monitor scale, the shell font at its 12px
          // base) Qt snaps the 1px border to a device pixel that the clip
          // discards, so that tab renders with three borders. Keep a hairline
          // of slack on both sides so no control sits exactly on the clip
          // boundary. (#231)
          x: Style.spacing.hairline
          width: panelFlick.width - Style.spacing.hairline * 2
          spacing: Style.space(12)

          PanelHero {
            width: parent.width
            title: root.settingsOpen ? root.tr("hero.settings")
              : (root.entry ? Model.providerName(root.entry) : root.tr("app.name"))
            meta: root.settingsOpen ? root.tr("hero.settings_meta") : root.heroMeta()
            // Long settings copy must not live in the hero detail pill — that
            // control is a single-line chip and overflows in RU/pt-BR. The
            // sentence renders as a wrapped caption under the hero instead.
            detail: root.settingsOpen ? ""
              : (root.entry && root.summary.text !== "Ready" && root.summary.text !== root.tr("ready")
                ? Model.autoTextSafe(root.panelHeadline(root.shapedEntry)) : "")
            foreground: root.foreground
            fontFamily: root.fontFamily

            iconComponent: Component {
              BrandMark {
                brand: root.settingsOpen || !root.brandIcons ? "" : Model.brandIconFile(root.entry)
                fallback: root.settingsOpen ? "󰒓"
                  : (root.brandIcons ? Model.providerIcon(root.entry) : "󰚩")
                // Colour-coding: full RAG from worst pool. Off: same aggregate,
                // binary foreground vs urgent when worst is critical.
                foreground: root.colorCodeUsage
                  ? root.severityColorOf(root.summary.severity || "")
                  : ((root.summary.severity || "") === "critical"
                    ? root.urgent : root.foreground)
                fontFamily: root.fontFamily
                fontSize: Style.font.display
              }
            }

            trailingControl: Component {
              Row {
                spacing: Style.space(4)

                PanelActionButton {
                  visible: !root.settingsOpen
                  iconText: "󰑐"
                  tooltipText: root.tr("action.refresh")
                  foreground: root.foreground
                  fontFamily: root.fontFamily
                  enabled: !usageProcess.running
                  onClicked: root.refresh()
                }

                PanelActionButton {
                  iconText: root.settingsOpen ? "󰁍" : "󰒓"
                  tooltipText: root.settingsOpen ? root.tr("action.back") : root.tr("action.settings")
                  foreground: root.foreground
                  fontFamily: root.fontFamily
                  onClicked: root.settingsOpen ? root.closeSettings() : root.openSettings()
                }
              }
            }
          }

          Text {
            visible: root.settingsOpen
            width: parent.width
            text: root.tr("hero.settings_detail")
            textFormat: Text.PlainText
            color: root.dim
            font.family: root.fontFamily
            font.pixelSize: Style.font.caption
            wrapMode: Text.WordWrap
          }

          SettingsView {
            id: settingsView
            visible: root.settingsOpen
            width: parent.width
            foreground: root.foreground
            urgent: root.urgent
            fontFamily: root.fontFamily
            showValue: root.showValue
            showProvider: root.showProvider
            showAll: root.showAll
            colorCodeUsage: root.colorCodeUsage
            uiLocale: root.uiLocale
            uiLocaleSetting: root.uiLocaleSetting
            barWindow: root.barWindow
            showAs: root.showAs
            brandIcons: root.brandIcons
            metricEntries: root.metricEntries
            onSaved: root.startRefresh()
            onShowValueRequested: function(enabled) { root.setShowValue(enabled) }
            onShowProviderRequested: function(enabled) { root.setShowProvider(enabled) }
            onShowAllRequested: function(enabled) { root.setShowAll(enabled) }
            onColorCodeUsageRequested: function(enabled) { root.setColorCodeUsage(enabled) }
            onUiLocaleRequested: function(value) { root.setUiLocale(value) }
            onBarWindowRequested: function(value) { root.setBarWindow(value) }
            onShowAsRequested: function(value) { root.setShowAs(value) }
            onMetricToggleRequested: function(entryId, key) { root.setMetricShown(entryId, key) }
            onBrandIconsRequested: function(enabled) { root.setBrandIcons(enabled) }
            onFallbackRequested: root.openTerminalSettings()
            onNousLoginRequested: root.openNousLogin()
            onCopilotLoginRequested: root.openCopilotLogin()
            onCloseRequested: root.closeSettings()
          }

          // Providers wrap into additional rows instead of being clipped by
          // the panel edge once there are more configured entries than fit
          // on one line — a fixed-width ListView silently hid entries past
          // the visible edge, with no way to reach them (see #173).
          Flow {
            id: providerList
            visible: !root.settingsOpen && root.visibleEntries.length > 1
            width: parent.width
            height: visible ? implicitHeight : 0
            flow: Flow.LeftToRight
            spacing: Style.spacing.md

            Repeater {
              model: root.visibleEntries

              delegate: Button {
                required property var modelData
                required property int index

                height: Style.spacing.controlHeight
                width: implicitWidth
                text: Model.providerName(modelData)
                selected: index === root.entryIndex
                hasCursor: root.cursorActive && index === root.entryIndex
                bordered: true
                foreground: root.foreground
                fontFamily: root.fontFamily
                fontSize: Style.font.bodySmall
                verticalPadding: Style.spacing.controlPaddingY
                onClicked: {
                  root.cursorActive = true
                  root.selectEntry(index)
                }
                onHovered: function(isHovered) { if (isHovered) root.cursorActive = true }
              }
            }
          }

          Flow {
            id: cursorPoolToggles
            visible: !root.settingsOpen && (root.cursorEntry || root.antigravityEntry)
            width: parent.width
            height: visible ? implicitHeight : 0
            flow: Flow.LeftToRight
            spacing: Style.spacing.md

            Repeater {
              model: root.providerPoolButtons()

              delegate: Button {
                required property var modelData

                height: Style.spacing.controlHeight
                width: implicitWidth
                text: modelData.label
                selected: root.providerPoolOn(modelData.poolId)
                enabled: root.providerPoolCanTurnOff(modelData.poolId) || !root.providerPoolOn(modelData.poolId)
                bordered: true
                foreground: root.foreground
                fontFamily: root.fontFamily
                fontSize: Style.font.bodySmall
                verticalPadding: Style.spacing.controlPaddingY
                onClicked: root.toggleProviderPool(modelData.poolId)
              }
            }
          }

          BorderSurface {
            readonly property string message: root.statusMessage()
            visible: !root.settingsOpen && message !== ""
            width: parent.width
            implicitHeight: statusText.implicitHeight + Style.spacing.xl * 2
            color: root.alpha(root.statusIsUrgent() ? root.urgent : root.foreground, 0.09)
            borderSpec: Border.flat(root.alpha(root.statusIsUrgent() ? root.urgent : root.foreground, 0.35), 1)
            radius: Style.cornerRadius

            Text {
              id: statusText
              anchors.left: parent.left
              anchors.right: parent.right
              anchors.verticalCenter: parent.verticalCenter
              anchors.leftMargin: Style.space(12)
              anchors.rightMargin: Style.space(12)
              text: parent.message
              textFormat: Text.PlainText
              color: root.dim
              font.family: root.fontFamily
              font.pixelSize: Style.font.caption
              wrapMode: Text.WordWrap
            }
          }

          Column {
            visible: !root.settingsOpen && root.loading && root.entries.length === 0
            width: parent.width
            spacing: Style.space(8)

            PanelSectionHeader {
              text: root.tr("section.usage")
              foreground: root.foreground
              fontFamily: root.fontFamily
            }

            Text {
              width: parent.width
              text: root.tr("loading.providers")
              color: root.dim
              font.family: root.fontFamily
              font.pixelSize: Style.font.body
              horizontalAlignment: Text.AlignHCenter
            }
          }

          Column {
            id: usageSection
            visible: !root.settingsOpen && root.entrySections.length > 0
            width: parent.width
            spacing: Style.space(8)

            PanelSeparator {
              width: parent.width
              foreground: root.foreground
            }

            PanelSectionHeader {
              text: root.tr("section.usage_balance")
              foreground: root.foreground
              fontFamily: root.fontFamily
            }

            Repeater {
              model: root.entrySections

              Column {
                required property var modelData
                width: usageSection.width

                // `visible` alone is not enough: QML evaluates the bindings of
                // hidden items too, so every row used to be handed to all three
                // components and the two that did not match read fields the row
                // does not carry. A "spacer" row has no label or value, which is
                // what produced the TypeError below on every report.
                MetricRow {
                  visible: modelData.type === "metric"
                  width: parent.width
                  row: modelData.type === "metric" ? modelData : null
                }

                DetailRow {
                  visible: modelData.type === "text"
                  width: parent.width
                  row: modelData.type === "text" ? modelData : null
                }

                BlockRow {
                  visible: modelData.type === "block"
                  width: parent.width
                  row: modelData.type === "block" ? modelData : null
                }

                Item {
                  visible: modelData.type === "spacer"
                  width: 1
                  height: Style.space(4)
                }
              }
            }
          }

          Text {
            visible: !root.settingsOpen && !root.loading && !root.entry && root.statusMessage() === ""
            width: parent.width
            topPadding: Style.space(20)
            text: root.tr("empty.no_usage")
            color: root.dim
            font.family: root.fontFamily
            font.pixelSize: Style.font.body
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.WordWrap
          }

          Text {
            visible: !root.settingsOpen && root.entryFetchedAt !== ""
            width: parent.width
            topPadding: Style.space(2)
            text: I18n.formatUpdated(root.entryFetchedAt, root.nowMs, root.uiLocale)
              + (usageProcess.running ? " · refreshing…" : "")
            color: root.dim
            font.family: root.fontFamily
            font.pixelSize: Style.font.caption
            horizontalAlignment: Text.AlignHCenter
            elide: Text.ElideRight
          }
        }
      }
    }
  }

  component MetricRow: Column {
    id: metricRow
    property var row: null
    // A grouped metric is a sub-row (SuperGrok's product slices under
    // "Breakdown"): dim label, thin muted gauge, no RAG colouring — the
    // overall meter above stays the binding constraint. The indent is applied
    // as margins inside full-width children, never as positioner padding: a
    // Column's leftPadding shifts children without narrowing them, which
    // pushes right-anchored values past the panel edge.
    readonly property bool grouped: row ? String(row.group || "") !== "" : false
    readonly property string severity: !grouped && row ? String(row.severity || "") : ""
    readonly property color ragColor: root.severityColorOf(metricRow.severity)
    readonly property color labelColor: grouped ? root.dim : root.foreground
    readonly property color valueColor: grouped ? metricRow.labelColor : metricRow.ragColor
    readonly property color fillColor: grouped
      ? root.alpha(root.foreground, 0.38)
      : metricRow.ragColor
    readonly property int indent: grouped ? Style.space(10) : 0
    readonly property string detailText: I18n.displayDetail(root.uiLocale, Model.metricDetail(row))
    readonly property string resetText: row ? I18n.formatReset(row.reset_at, root.nowMs, root.uiLocale) : ""
    readonly property int shownPercent: row ? Model.shownPercent(row.percent, root.showAs) : 0
    readonly property var valueView: Model.metricValueView(row, root.showAs)
    readonly property string valueText: valueView.left
      ? I18n.t(root.uiLocale, "metric.left", { percent: valueView.percent })
      : valueView.text

    spacing: Style.space(grouped ? 4 : 6)

    Item {
      width: parent.width
      implicitHeight: Math.max(metricLabel.implicitHeight, metricValue.implicitHeight)

      Text {
        id: metricLabel
        text: metricRow.row ? I18n.displayLabel(root.uiLocale, metricRow.row.label) : ""
        textFormat: Text.PlainText
        color: metricRow.labelColor
        font.family: root.fontFamily
        font.pixelSize: metricRow.grouped ? Style.font.caption : Style.font.body
        elide: Text.ElideRight
        anchors.left: parent.left
        anchors.leftMargin: metricRow.indent
        anchors.right: metricValue.left
        anchors.rightMargin: Style.spacing.sm
        anchors.verticalCenter: parent.verticalCenter
      }

      Text {
        id: metricValue
        text: metricRow.valueText
        textFormat: Text.PlainText
        color: metricRow.valueColor
        font.family: root.fontFamily
        font.pixelSize: Style.font.caption
        font.bold: !metricRow.grouped
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
      }
    }

    Item {
      width: parent.width
      implicitHeight: Math.max(Style.space(4), Math.round(Style.spacing.controlHeight * (metricRow.grouped ? 0.10 : 0.14)))

      Rectangle {
        id: meterTrack
        anchors.fill: parent
        anchors.leftMargin: metricRow.indent
        radius: height / 2
        color: metricRow.grouped ? root.alpha(root.foreground, 0.10) : root.track
      }

      Rectangle {
        anchors.left: meterTrack.left
        anchors.verticalCenter: meterTrack.verticalCenter
        height: meterTrack.height
        radius: meterTrack.radius
        width: meterTrack.width * root.clamp(metricRow.shownPercent / 100, 0, 1)
        color: metricRow.fillColor

        Behavior on width {
          NumberAnimation { duration: 160; easing.type: Easing.OutCubic }
        }
      }
    }

    Text {
      visible: text !== ""
      width: parent.width
      leftPadding: metricRow.indent
      text: metricRow.detailText
      textFormat: Text.PlainText
      color: root.dim
      font.family: root.fontFamily
      font.pixelSize: Style.font.caption
      wrapMode: Text.WordWrap
    }

    Text {
      visible: text !== ""
      width: parent.width
      leftPadding: metricRow.indent
      text: metricRow.resetText
      textFormat: Text.PlainText
      color: root.dim
      font.family: root.fontFamily
      font.pixelSize: Style.font.caption
      // The row now carries a date and clock as well as the countdown, so it
      // can outgrow a narrow panel. Wrap like the detail line above it rather
      // than painting past the panel edge.
      wrapMode: Text.WordWrap
    }
  }

  component DetailRow: Item {
    id: detailRow
    property var row: null
    readonly property bool heading: row && row.label !== "" && row.value === ""

    implicitHeight: heading ? headingLabel.implicitHeight : Math.max(detailLabel.implicitHeight, detailValue.implicitHeight)

    PanelSectionHeader {
      id: headingLabel
      visible: detailRow.heading
      width: parent.width
      text: detailRow.row ? Model.autoTextSafe(String(detailRow.row.label || "").toUpperCase()) : ""
      foreground: root.foreground
      fontFamily: root.fontFamily
    }

    Text {
      id: detailLabel
      visible: !detailRow.heading && text !== ""
      text: detailRow.row ? I18n.displayLabel(root.uiLocale, detailRow.row.label) : ""
      textFormat: Text.PlainText
      color: root.foreground
      font.family: root.fontFamily
      font.pixelSize: Style.font.bodySmall
      font.bold: true
      anchors.left: parent.left
      anchors.top: parent.top
      width: Math.min(implicitWidth, parent.width * 0.42)
      elide: Text.ElideRight
    }

    Text {
      id: detailValue
      visible: !detailRow.heading
      text: detailRow.row ? detailRow.row.value : ""
      textFormat: Text.PlainText
      color: root.dim
      font.family: root.fontFamily
      font.pixelSize: Style.font.bodySmall
      horizontalAlignment: detailLabel.visible ? Text.AlignRight : Text.AlignLeft
      wrapMode: Text.WordWrap
      anchors.left: detailLabel.visible ? detailLabel.right : parent.left
      anchors.leftMargin: detailLabel.visible ? Style.spacing.md : 0
      anchors.right: parent.right
      anchors.top: parent.top
    }
  }

  component BlockRow: Column {
    id: blockRow
    property var row: null

    spacing: Style.space(4)

    Text {
      width: parent.width
      text: blockRow.row ? blockRow.row.label : ""
      textFormat: Text.PlainText
      color: root.foreground
      font.family: root.fontFamily
      font.pixelSize: Style.font.bodySmall
      font.bold: true
      elide: Text.ElideRight
    }

    Text {
      width: parent.width
      text: blockRow.row && blockRow.row.body ? blockRow.row.body.join("\n") : ""
      textFormat: Text.PlainText
      color: root.dim
      font.family: root.fontFamily
      font.pixelSize: Style.font.caption
      wrapMode: Text.WordWrap
    }
  }
}
