import QtQuick
import Quickshell
import qs.Commons
import qs.Ui
import "Model.js" as Model

// Quattro bar entry point. The popup is loaded separately so the object in
// the bar slot owns shell routing while Panel.qml remains focused on report
// collection and presentation.
BarWidget {
  id: root
  moduleName: "akitaonrails.ai-usagebar"

  readonly property var panelItem: panelLoader.item
  readonly property bool opened: panelItem ? panelItem.opened === true : false
  readonly property bool popoutSwitchClosing: panelItem
    ? panelItem.popoutSwitchClosing === true
    : false
  readonly property bool colorCodeUsage: panelItem ? panelItem.colorCodeUsage === true : false
  readonly property bool alarming: panelItem ? panelItem.alarming === true : false

  // Quickshell window that hosts this bar slot (not the usage panel popup).
  readonly property var barWindow: button.QsWindow ? button.QsWindow.window : null

  // The bar presses a slot's widget with no coordinates, so the button cannot
  // tell which chip was clicked. Each chip registers as its own click target
  // instead, and the bar presses the one under the pointer by geometry. It
  // scans targets last first, so the chips are re-registered behind the
  // button's whole-slot target whenever the row or the bar changes.
  function chipItems() {
    var items = []
    for (var i = 0; i < chipRepeater.count; i++) {
      var item = chipRepeater.itemAt(i)
      if (item) items.push(item)
    }
    return items
  }

  function syncChipTargets() {
    var host = root.bar
    if (!host || typeof host.registerClickTarget !== "function") return
    // Anything of ours that is not the button is a chip, current or rebuilt.
    var registered = host.clickTargets || []
    for (var i = 0; i < registered.length; i++)
      if (registered[i] !== button) host.unregisterClickTarget(registered[i])
    var chips = chipItems()
    // A lone chip, a vertical bar or an empty report keeps the button as the
    // only target, and its press toggles the panel the way it always did.
    if (chips.length <= 1) return
    for (var j = 0; j < chips.length; j++) host.registerClickTarget(chips[j])
  }

  function open() {
    if (panelItem) panelItem.open()
  }

  function close() {
    if (panelItem) panelItem.close()
  }

  function toggle() {
    if (panelItem) panelItem.toggle()
  }

  function closeForPopoutSwitch() {
    if (panelItem) panelItem.closeForPopoutSwitch()
  }

  function refresh() {
    if (panelItem) panelItem.refresh()
  }

  function nextEntry() {
    if (panelItem) panelItem.selectAdjacentBarEntry(1)
  }

  function launchDashboard() {
    if (root.bar) root.bar.run("omarchy-launch-floating-terminal-with-presentation ai-usagebar-tui")
    root.close()
  }

  function injectPanel() {
    var target = panelItem
    if (!target) return
    if ("bar" in target) target.bar = root.bar
    if ("settings" in target) target.settings = root.settings
    if ("anchorItem" in target) target.anchorItem = button
    if ("hostWidget" in target) target.hostWidget = root
  }

  function segmentColor(severity) {
    // Colour-coding on: full RAG. Off: only critical segments take the
    // classic alarm tint so "34% · 100%" paints just the exhausted pool red.
    if (root.colorCodeUsage) {
      if (!severity) return button.foreground
      if (root.panelItem && typeof root.panelItem.severityColorOf === "function")
        return root.panelItem.severityColorOf(severity)
      return button.foreground
    }
    return severity === "critical" ? root.alarmColor(true) : button.foreground
  }

  function alarmColor(isAlarming) {
    return isAlarming && button.useActiveColor ? button.activeColor : button.foreground
  }

  // Icon tint:
  // - colour-coding on: full RAG from worst visible pool (max used / min
  //   remaining) — status highest-severity aggregate, never an average.
  // - colour-coding off: the chip's own alarm flag (highest percent across
  //   windows), binary white/foreground vs red, as before the feature.
  function chipIconColor(chip) {
    if (root.colorCodeUsage) {
      var sev = chip && chip.severity ? String(chip.severity) : ""
      if (root.panelItem && typeof root.panelItem.severityColorOf === "function")
        return root.panelItem.severityColorOf(sev)
      return button.foreground
    }
    return chip && chip.alarming ? root.alarmColor(true) : button.foreground
  }

  // Non-Cursor chip value with colour-coding on: the headline severity paints
  // the label, so a critical bar value keeps its red. A chip without a
  // severity keeps the plain foreground.
  function chipValueColor(chip) {
    var sev = chip && chip.severity ? String(chip.severity) : ""
    if (sev !== "" && root.panelItem && typeof root.panelItem.severityColorOf === "function")
      return root.panelItem.severityColorOf(sev)
    return button.foreground
  }

  function escapeHtml(value) {
    return String(value || "")
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
  }

  readonly property string tipPlain: {
    var rows = root.panelItem ? (root.panelItem.ragTooltipRows || []) : []
    if (rows && rows.length > 0) {
      var lines = []
      for (var i = 0; i < rows.length; i++) {
        var text = String((rows[i] && rows[i].text) || "").trim()
        if (text !== "") lines.push(text)
      }
      if (lines.length > 0) return lines.join("\n")
    }
    if (root.panelItem && root.panelItem.plainTooltipText)
      return root.panelItem.plainTooltipText
    return root.panelItem && root.panelItem.tr
      ? root.panelItem.tr("app.name")
      : "AI usage"
  }

  readonly property string tipHtml: {
    var rows = root.panelItem ? (root.panelItem.ragTooltipRows || []) : []
    var colorOn = root.colorCodeUsage
    var _g = root.panelItem ? root.panelItem.hexGreen : ""
    var _y = root.panelItem ? root.panelItem.hexYellow : ""
    var _o = root.panelItem ? root.panelItem.hexOrange : ""
    var _r = root.panelItem ? root.panelItem.hexRed : ""
    var _ = [_g, _y, _o, _r]
    if (!rows || rows.length === 0) return ""
    var lines = []
    var anyColor = false
    for (var i = 0; i < rows.length; i++) {
      var row = rows[i] || {}
      var body = root.escapeHtml(row.text || "")
      if (body === "") continue
      var hex = ""
      var sev = String(row.severity || "")
      if (colorOn && sev && root.panelItem && typeof root.panelItem.severityHexOf === "function")
        hex = String(root.panelItem.severityHexOf(sev) || "")
      else if (!colorOn && sev === "critical")
        // Classic mode: only the depleted pool is red, so the tip explains
        // why the tray mark is lit without turning on full RAG.
        hex = String(_r || "")
      if (hex !== "") {
        anyColor = true
        lines.push("<span style=\"color:" + hex + "\">" + body + "</span>")
      } else {
        lines.push(body)
      }
    }
    if (lines.length === 0) return ""
    if (!colorOn && !anyColor) return ""
    return lines.join("<br/>")
  }

  // Colored tip when we have per-line severity HTML and the bar window exists.
  // Independent of the usage panel popup (`opened`) for *hover*, but never
  // while a click is opening the panel or the panel is already open.
  readonly property bool useColorTip: root.barWindow !== null && root.tipHtml !== ""
  readonly property bool tipWanted: button.tooltipHovered && !root.opened
    && !root.clickLock && root.useColorTip
  property bool tipShown: false
  property bool clickLock: false

  onOpenedChanged: {
    if (opened) {
      tipDelay.stop()
      tipShown = false
      clickLock = true
      clickLockClear.restart()
    }
  }

  onTipWantedChanged: {
    if (tipWanted) {
      tipDelay.restart()
    } else {
      tipDelay.stop()
      tipShown = false
    }
  }

  Timer {
    id: tipDelay
    interval: 400
    repeat: false
    onTriggered: {
      if (!root.tipWanted) return
      if (root.bar) root.bar.hideTooltip(button)
      root.tipShown = true
    }
  }

  Timer {
    id: clickLockClear
    interval: 500
    repeat: false
    onTriggered: root.clickLock = false
  }

  function armClickLock() {
    tipDelay.stop()
    tipShown = false
    clickLock = true
    clickLockClear.restart()
    if (root.bar) root.bar.hideTooltip(button)
  }

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  // Full-width open-panel underline (Bar.qml openPanelIndicator). Without
  // this hint the bar only paints ~55% of the slot, which looks short once
  // Cursor shows several percentage chips.
  readonly property real openPanelIndicatorWidth: Math.max(1, Math.round(button.implicitWidth))
  readonly property real openPanelIndicatorHeight: Math.max(1, Math.round(button.implicitHeight))

  onBarChanged: {
    injectPanel()
    // The bar is injected after the widget completes, and the button's own
    // registration rides the same change; re-assert the chips once both are
    // done so the bar scans them ahead of the button.
    Qt.callLater(root.syncChipTargets)
  }
  onSettingsChanged: injectPanel()

  Loader {
    id: panelLoader
    active: true
    source: Qt.resolvedUrl("Panel.qml")
    visible: false
    onLoaded: {
      root.injectPanel()
      Qt.callLater(root.injectPanel)
    }
  }

  WidgetButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    text: " "
    labelVisible: false
    hasVisualContent: true
    fontSize: Style.font.bodySmall
    // Classic alarm chrome when colour-coding is off; with colour-coding on,
    // per-pool RAG colours carry the signal instead (#278 / #292).
    active: !root.colorCodeUsage && root.alarming
    // Empty while the colored PopupWindow owns the tip; otherwise plain text
    // for the bar's native tooltip (startup / no RAG rows).
    tooltipText: root.useColorTip ? "" : root.tipPlain
    horizontalMargin: 8.5
    // The row's outer chips carry the edge padding, so it is not added here.
    fixedWidth: root.bar && root.bar.vertical ? -1 : chipRow.implicitWidth

    onPressed: function(buttonCode) {
      root.armClickLock()
      if (buttonCode === Qt.RightButton) root.launchDashboard()
      else if (buttonCode === Qt.MiddleButton) root.nextEntry()
      else root.toggle()
    }

    onWheelMoved: function(delta) {
      if (delta !== 0 && root.panelItem)
        root.panelItem.selectAdjacentBarEntry(delta < 0 ? 1 : -1)
    }

    Row {
      id: chipRow
      anchors.centerIn: parent
      // Every chip carries the gaps beside it, so the row adds none of its own.
      spacing: 0
      visible: !(root.bar && root.bar.vertical)

      Repeater {
        id: chipRepeater
        model: root.panelItem ? root.panelItem.barChips : []
        // The model is rebuilt when the report changes, which replaces every
        // delegate and the click targets that point at them.
        onModelChanged: Qt.callLater(root.syncChipTargets)

        // The bar presses a slot's widget by geometry, so the registered target
        // is the chip's whole column of the slot rather than the glyph inside
        // it: a press on the padding above, below or beside the glyph would
        // otherwise reach the button and toggle whichever entry was already
        // selected. The column owns half of every gap beside it, split at the
        // midpoint with its neighbour, and the outer columns own the button's
        // padding at either end, which leaves the widget's width and each
        // chip's place in it exactly as the plain spacing and padding drew them.
        Item {
          id: chipHit
          readonly property var chip: modelData
          readonly property var hitGaps: Model.chipHitGaps(index, chipRepeater.count, Style.space(10), Style.spaceReal(17) / 2)
          height: button.height
          width: chipContent.implicitWidth + hitGaps.left + hitGaps.right

          // One chip per provider, and the one the pointer is on is the one
          // the bar presses: left opens that provider's page, while the other
          // buttons keep their panel-wide meaning.
          function triggerPress(buttonCode) {
            root.armClickLock()
            if (buttonCode === Qt.RightButton) root.launchDashboard()
            else if (buttonCode === Qt.MiddleButton) root.nextEntry()
            else if (root.panelItem) root.panelItem.openEntry(chipHit.chip.id || "")
          }

          Row {
            id: chipContent
            x: chipHit.hitGaps.left
            anchors.verticalCenter: parent.verticalCenter
            spacing: Style.space(4)
            // While the panel is open, the chips it is not showing step back.
            opacity: root.opened && root.panelItem && modelData.id
              && modelData.id !== root.panelItem.selectedEntryId ? 0.45 : 1

            Behavior on opacity {
              NumberAnimation { duration: 140; easing.type: Easing.OutCubic }
            }

            BrandMark {
              visible: chipHit.chip.labelOnly !== true
              anchors.verticalCenter: parent.verticalCenter
              brand: chipHit.chip.brand || ""
              fallback: chipHit.chip.icon || "󰚩"
              // Colour-coding: full RAG from worst pool. Off: same aggregate,
              // binary foreground vs red when worst is critical.
              foreground: root.chipIconColor(chipHit.chip)
              fontFamily: button.fontFamily
              fontSize: button.fontSize
            }

            Text {
              visible: !!(chipHit.chip.segments && chipHit.chip.segments.length > 0
                && chipHit.chip.providerPrefix)
              anchors.verticalCenter: parent.verticalCenter
              textFormat: Text.PlainText
              text: chipHit.chip.providerPrefix || ""
              color: root.colorCodeUsage
                ? button.foreground
                : root.alarmColor(!!chipHit.chip.alarming)
              font.family: button.fontFamily
              font.pixelSize: button.fontSize
            }

            Row {
              anchors.verticalCenter: parent.verticalCenter
              spacing: 0
              visible: !!(chipHit.chip.segments && chipHit.chip.segments.length > 0)

              Repeater {
                model: chipHit.chip.segments || []

                Text {
                  anchors.verticalCenter: parent.verticalCenter
                  textFormat: Text.PlainText
                  text: modelData.text || ""
                  color: root.segmentColor(modelData.severity || "")
                  font.family: button.fontFamily
                  font.pixelSize: button.fontSize
                }
              }
            }

            Text {
              visible: !(chipHit.chip.segments && chipHit.chip.segments.length > 0)
                && chipHit.chip.label !== ""
              anchors.verticalCenter: parent.verticalCenter
              textFormat: Text.PlainText
              text: chipHit.chip.label || ""
              color: root.colorCodeUsage
                ? root.chipValueColor(chipHit.chip)
                : root.alarmColor(!!chipHit.chip.alarming)
              font.family: button.fontFamily
              font.pixelSize: button.fontSize
            }
          }
        }
      }
    }

    Text {
      visible: root.bar && root.bar.vertical
      anchors.centerIn: parent
      textFormat: Text.PlainText
      text: root.alarming ? "󰅙" : "󰚩"
      color: button.active && button.useActiveColor ? button.activeColor : button.foreground
      font.family: button.fontFamily
      font.pixelSize: button.fontSize
      rotation: button.textRotation
    }
  }

  // Same PopupWindow + anchor pattern as omarchy bar tooltips (Bar.qml).
  // grabFocus must stay false — otherwise the tip steals input and the
  // click that should open the usage panel only dismisses/shows the tip.
  PopupWindow {
    id: colorTip
    visible: root.tipShown && root.tipWanted
    color: "transparent"
    grabFocus: false
    implicitWidth: Math.max(1, Math.ceil(tipBubble.implicitWidth))
    implicitHeight: Math.max(1, Math.ceil(tipBubble.implicitHeight))

    anchor {
      id: tipAnchor
      window: root.barWindow
      adjustment: PopupAdjustment.Slide
      edges: Edges.Top | Edges.Left
      gravity: Edges.Bottom | Edges.Right
      rect.width: 1
      rect.height: 1

      onAnchoring: {
        var win = root.barWindow
        if (!win || !button) return
        var popupWidth = colorTip.implicitWidth
        var popupHeight = colorTip.implicitHeight
        var localX = button.width / 2 - popupWidth / 2
        var localY = button.height + 6
        if (root.bar && root.bar.position === "bottom") {
          localY = -popupHeight - 6
        } else if (root.bar && root.bar.position === "left") {
          localX = button.width + 6
          localY = button.height / 2 - popupHeight / 2
        } else if (root.bar && root.bar.position === "right") {
          localX = -popupWidth - 6
          localY = button.height / 2 - popupHeight / 2
        }
        var point = win.contentItem.mapFromItem(button, localX, localY)
        tipAnchor.rect.x = Math.round(point.x)
        tipAnchor.rect.y = Math.round(point.y)
      }
    }

    BorderSurface {
      id: tipBubble
      implicitWidth: Math.max(tipLabel.implicitWidth + Style.spacing.controlPaddingX * 2, Style.space(40))
      implicitHeight: Math.max(tipLabel.implicitHeight + Style.spacing.controlPaddingY * 2, Style.space(24))
      color: Color.tooltip.background
      borderSpec: Border.surfaceSpec("tooltip", "border", Color.tooltip.border, 1)
      radius: Style.cornerRadius

      Text {
        id: tipLabel
        anchors.centerIn: parent
        textFormat: Text.RichText
        text: root.tipHtml
        color: Color.tooltip.text
        font.family: button.fontFamily
        font.pixelSize: Style.font.body
        horizontalAlignment: Text.AlignLeft
      }
    }
  }
}
