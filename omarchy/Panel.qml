import QtQuick
import QtQuick.Controls
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Ui
import "Model.js" as Model

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
  readonly property var visibleEntries: Model.filteredEntries(entries, configuredProvider)
  readonly property int entryIndex: Model.selectedIndex(visibleEntries, selectedEntryId)
  readonly property var entry: entryIndex >= 0 ? visibleEntries[entryIndex] : null
  readonly property string entryFetchedAt: {
    if (!entry) return ""
    return String(entry.fetched_at || "")
  }
  readonly property var summary: Model.headline(entry)
  readonly property var entrySections: entry ? entry.sections : []
  readonly property bool filterMiss: configuredProvider !== "" && entries.length > 0 && visibleEntries.length === 0
  readonly property bool alarming: Model.isAlarming(entry) || loadError !== "" || filterMiss

  function alpha(color, opacity) {
    return Qt.rgba(color.r, color.g, color.b, opacity)
  }

  function clamp(value, low, high) {
    return Math.max(low, Math.min(high, value))
  }

  function syncSelection() {
    if (visibleEntries.length === 0) {
      selectedEntryId = ""
      return
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

  function selectEntry(index) {
    if (visibleEntries.length === 0) return
    var wrapped = ((index % visibleEntries.length) + visibleEntries.length) % visibleEntries.length
    selectedEntryId = visibleEntries[wrapped].id
    persistSelection(selectedEntryId)
    if (providerList.visible) providerList.positionViewAtIndex(wrapped, ListView.Contain)
    if (panelFlick) panelFlick.contentY = 0
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

  function switchPanel(direction) {
    if (bar && typeof bar.switchPanelFrom === "function")
      return bar.switchPanelFrom(barIdentity, direction)
    return false
  }

  function statusMessage() {
    if (filterMiss) return "No configured entry matches ‘" + configuredProvider + "’. Clear the provider setting or use an id from ai-usagebar usage --json."
    if (entry && entry.error !== "") return entry.error
    if (loadError !== "") return entries.length > 0
      ? "Refresh failed; showing the previous report. " + loadError
      : loadError
    if (entry && entry.stale) return "Cached data · the provider could not supply a fresh response."
    return ""
  }

  function statusIsUrgent() {
    return filterMiss || (entry && entry.error !== "") || loadError !== ""
  }

  function heroMeta() {
    if (!entry) return loading ? "Loading providers" : "Usage report"
    if (entry.error !== "") return "Provider unavailable"
    var text = entry.plan || "Usage and limits"
    if (entry.stale) text += " · cached"
    return Model.autoTextSafe(text)
  }

  function barText() {
    return Model.barLabel(alarming, vertical, showValue, loading,
      entry !== null, summary.text)
  }

  function tooltipText() {
    if (!entry) return Model.autoTextSafe(statusMessage() || "AI usage")
    var text = Model.providerName(entry)
    if (summary.text !== "") text += " · " + Model.autoTextSafe(summary.text)
    if (entry.stale) text += " · cached"
    return text
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
          panelFlick.contentY = root.clamp(panelFlick.contentY + dy * Style.space(56), 0,
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

        Column {
          id: column
          width: panelFlick.width
          spacing: Style.space(12)

          PanelHero {
            width: parent.width
            title: root.settingsOpen ? "Settings"
              : (root.entry ? Model.providerName(root.entry) : "AI usage")
            meta: root.settingsOpen ? "Display, provider & API keys" : root.heroMeta()
            detail: root.settingsOpen
              ? "Existing configuration stays in place until you save."
              : (root.entry && root.summary.text !== "Ready" ? Model.autoTextSafe(root.summary.text) : "")
            foreground: root.foreground
            fontFamily: root.fontFamily

            iconComponent: Component {
              Text {
                text: root.settingsOpen ? "󰒓" : "󰚩"
                color: root.alarming ? root.urgent : root.foreground
                font.family: root.fontFamily
                font.pixelSize: Style.font.display
              }
            }

            trailingControl: Component {
              Row {
                spacing: Style.space(4)

                PanelActionButton {
                  visible: !root.settingsOpen
                  iconText: "󰑐"
                  tooltipText: "Refresh usage"
                  foreground: root.foreground
                  fontFamily: root.fontFamily
                  enabled: !usageProcess.running
                  onClicked: root.refresh()
                }

                PanelActionButton {
                  iconText: root.settingsOpen ? "󰁍" : "󰒓"
                  tooltipText: root.settingsOpen ? "Back to usage" : "Settings"
                  foreground: root.foreground
                  fontFamily: root.fontFamily
                  onClicked: root.settingsOpen ? root.closeSettings() : root.openSettings()
                }
              }
            }
          }

          SettingsView {
            id: settingsView
            visible: root.settingsOpen
            width: parent.width
            foreground: root.foreground
            urgent: root.urgent
            fontFamily: root.fontFamily
            showValue: root.showValue
            onSaved: root.startRefresh()
            onShowValueRequested: function(enabled) { root.setShowValue(enabled) }
            onFallbackRequested: root.openTerminalSettings()
            onNousLoginRequested: root.openNousLogin()
            onCloseRequested: root.closeSettings()
          }

          ListView {
            id: providerList
            visible: !root.settingsOpen && root.visibleEntries.length > 1
            width: parent.width
            height: visible ? Style.spacing.controlHeight : 0
            orientation: ListView.Horizontal
            spacing: Style.spacing.md
            clip: true
            boundsBehavior: Flickable.StopAtBounds
            model: root.visibleEntries
            currentIndex: root.entryIndex

            delegate: Button {
              required property var modelData
              required property int index

              height: providerList.height
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
              text: "USAGE"
              foreground: root.foreground
              fontFamily: root.fontFamily
            }

            Text {
              width: parent.width
              text: "Collecting configured providers…"
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
              text: "USAGE & BALANCE"
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
            text: "No configured provider reported usage."
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
            text: Model.formatUpdated(root.entryFetchedAt, root.nowMs)
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
    readonly property bool critical: row && row.severity === "critical"
    readonly property string detailText: Model.metricDetail(row)
    readonly property string resetText: row ? Model.formatReset(row.reset_at, root.nowMs) : ""

    spacing: Style.space(6)

    Item {
      width: parent.width
      implicitHeight: Math.max(metricLabel.implicitHeight, metricValue.implicitHeight)

      Text {
        id: metricLabel
        text: metricRow.row ? metricRow.row.label : ""
        textFormat: Text.PlainText
        color: root.foreground
        font.family: root.fontFamily
        font.pixelSize: Style.font.body
        elide: Text.ElideRight
        anchors.left: parent.left
        anchors.right: metricValue.left
        anchors.rightMargin: Style.spacing.sm
        anchors.verticalCenter: parent.verticalCenter
      }

      Text {
        id: metricValue
        text: metricRow.row && metricRow.row.value !== ""
          ? metricRow.row.value : (metricRow.row ? metricRow.row.percent + "%" : "")
        textFormat: Text.PlainText
        color: metricRow.critical ? root.urgent : root.foreground
        font.family: root.fontFamily
        font.pixelSize: Style.font.caption
        font.bold: true
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
      }
    }

    Item {
      width: parent.width
      implicitHeight: Math.max(Style.space(4), Math.round(Style.spacing.controlHeight * 0.14))

      Rectangle {
        id: meterTrack
        anchors.fill: parent
        radius: height / 2
        color: root.track
      }

      Rectangle {
        anchors.left: meterTrack.left
        anchors.verticalCenter: meterTrack.verticalCenter
        height: meterTrack.height
        radius: meterTrack.radius
        width: meterTrack.width * root.clamp(metricRow.row ? metricRow.row.percent / 100 : 0, 0, 1)
        color: metricRow.critical ? root.urgent : root.foreground

        Behavior on width {
          NumberAnimation { duration: 160; easing.type: Easing.OutCubic }
        }
      }
    }

    Text {
      visible: text !== ""
      width: parent.width
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
      text: metricRow.resetText
      textFormat: Text.PlainText
      color: root.dim
      font.family: root.fontFamily
      font.pixelSize: Style.font.caption
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
      text: detailRow.row ? detailRow.row.label : ""
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
