import QtQuick
import qs.Commons
import qs.Commons as Commons

// Offscreen stand-in for qs.Ui's KeyboardPanel. The real one is a layer-shell
// window and needs a compositor; this draws the same card (BorderSurface,
// padding, radius, popup colours) as a plain item, so panel.qml can render
// the plugin's whole Panel.qml and grab the card. Only the panel scenarios
// load it; everything inside the card is the plugin's own code.
Item {
  id: root

  readonly property bool visualStandIn: true
  property Item anchorItem: null
  property QtObject bar: null
  property var owner: null
  property int margin: Style.gapsOut
  property int padding: Style.spacing.popupPadding
  property int contentWidth: Style.space(280)
  property int contentHeight: Style.space(200)
  property var borderSpec: Border.surfaceSpec("popups", "border", Commons.Color.popups.border, Math.max(1, Style.space(2)))
  property bool centerOnBar: false
  property bool open: false
  property int gap: Style.gapsOut
  property bool popoutSwitching: false
  property bool popoutSwitchClosing: false
  property bool focusPrimed: false
  property Item focusTarget: null
  default property alias contentItem: contentHolder.children
  readonly property real verticalContentInset: padding * 2 + Border.top(borderSpec) + Border.bottom(borderSpec)

  function close() {
    if (owner && "close" in owner) owner.close()
    else root.open = false
  }

  // No screen to fit: the requested size, capped like the real panel.
  function fittedContentWidth(width, cap) {
    var desired = Math.max(1, Number(width) || 1)
    return Math.round(cap !== undefined && Number(cap) > 0 ? Math.min(desired, Number(cap)) : desired)
  }

  function fittedContentHeight(implicitHeight, cap) {
    var desired = Math.max(verticalContentInset, (Number(implicitHeight) || 0) + verticalContentInset)
    return Math.round(cap !== undefined && Number(cap) > 0 ? Math.min(desired, Number(cap)) : desired)
  }

  function cappedContentHeight(height) {
    return Math.round(Math.max(padding * 2, Number(height) || padding * 2))
  }

  width: contentWidth
  height: contentHeight

  BorderSurface {
    id: card
    anchors.fill: parent
    color: Commons.Color.popups.background
    borderSpec: root.borderSpec
    padding: root.padding
    radius: Style.cornerRadius

    Item {
      id: contentHolder
      anchors.fill: parent
      anchors.topMargin: card.contentTopInset
      anchors.rightMargin: card.contentRightInset
      anchors.bottomMargin: card.contentBottomInset
      anchors.leftMargin: card.contentLeftInset
    }
  }
}
