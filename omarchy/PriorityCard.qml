import QtQuick
import qs.Commons
import qs.Commons as Commons
import qs.Ui

// Shared native controls and theme tokens. Hover previews; click/keyboard pins
// the rationale. No calculation or provider-specific rules live in this view.
BorderSurface {
  id: root
  property var row: ({})
  property int rank: 1
  property string summary: ""
  // Expanded detail: label/value facts, then the conclusion and its notes.
  property var facts: []
  property var notes: []
  property string rationale: ""
  property string statusText: ""
  property string limitsText: ""
  property color foreground: Commons.Color.foreground
  property color urgentColor: Commons.Color.urgent
  property string fontFamily: Style.font.family
  property bool urgent: false
  property bool pinned: false
  property bool hoverReady: false
  property bool hoverSuppressed: false
  readonly property bool expanded: pinned || (hoverReady && !hoverSuppressed)
  readonly property color tint: urgent ? urgentColor : foreground
  signal limitsRequested()

  implicitHeight: content.implicitHeight + Style.space(16)
  radius: Style.cornerRadius
  color: Qt.rgba(tint.r, tint.g, tint.b, urgent ? 0.10 : 0.035)
  borderSpec: Border.flat(Qt.rgba(tint.r, tint.g, tint.b, urgent ? 0.55 : 0.15), 1)

  HoverHandler {
    id: hover
    onHoveredChanged: {
      if (hovered) hoverDelay.restart()
      else { hoverDelay.stop(); root.hoverReady = false; root.hoverSuppressed = false }
    }
  }
  Timer {
    id: hoverDelay
    interval: 300
    onTriggered: root.hoverReady = true
  }
  Column {
    id: content
    anchors.left: parent.left
    anchors.right: parent.right
    anchors.top: parent.top
    anchors.margins: Style.space(8)
    spacing: Style.space(6)
    Button {
      id: header
      width: parent.width
      text: root.rank + ". " + String(root.row.name || "")
      iconText: root.expanded ? "󰅀" : "󰅂"
      leftAlign: true
      focusable: true
      foreground: root.tint
      fontFamily: root.fontFamily
      fontSize: Style.font.bodySmall
      Accessible.role: Accessible.Button
      Accessible.name: text + ". " + root.statusText + ". " + root.summary + (root.expanded ? ". " + root.rationale : "")
      onClicked: {
        root.hoverSuppressed = root.pinned
        root.pinned = !root.pinned
      }

      // Status as a tag on the right, so the name reads cleanly on the left.
      BorderSurface {
        anchors.right: parent.right
        anchors.rightMargin: Style.space(6)
        anchors.verticalCenter: parent.verticalCenter
        implicitWidth: statusLabel.implicitWidth + Style.space(12)
        implicitHeight: statusLabel.implicitHeight + Style.space(4)
        radius: Style.cornerRadius
        color: Qt.rgba(root.tint.r, root.tint.g, root.tint.b, root.urgent ? 0.18 : 0.08)
        borderSpec: Border.flat(Qt.rgba(root.tint.r, root.tint.g, root.tint.b, root.urgent ? 0.6 : 0.25), 1)

        Text {
          id: statusLabel
          anchors.centerIn: parent
          textFormat: Text.PlainText
          text: root.statusText
          color: root.tint
          font.family: root.fontFamily
          font.pixelSize: Style.font.caption
          font.bold: true
        }
      }
    }
    Text {
      width: parent.width
      leftPadding: Style.space(4)
      text: root.summary
      textFormat: Text.PlainText
      wrapMode: Text.WordWrap
      color: root.tint
      opacity: root.urgent ? 1 : 0.85
      font.family: root.fontFamily
      font.pixelSize: Style.font.caption
    }
    Rectangle {
      visible: root.expanded && root.facts.length > 0
      width: parent.width
      height: 1
      color: Qt.rgba(root.foreground.r, root.foreground.g, root.foreground.b, 0.12)
    }
    Column {
      visible: root.expanded && root.facts.length > 0
      width: parent.width
      spacing: Style.space(3)
      Repeater {
        model: root.facts
        Item {
          required property var modelData
          width: parent.width
          implicitHeight: Math.max(factLabel.implicitHeight, factValue.implicitHeight)
          Text {
            id: factLabel
            anchors.left: parent.left
            anchors.leftMargin: Style.space(4)
            anchors.right: factValue.left
            anchors.rightMargin: Style.space(8)
            textFormat: Text.PlainText
            text: modelData.label
            elide: Text.ElideRight
            color: root.foreground
            opacity: 0.65
            font.family: root.fontFamily
            font.pixelSize: Style.font.caption
          }
          Text {
            id: factValue
            anchors.right: parent.right
            anchors.rightMargin: Style.space(4)
            textFormat: Text.PlainText
            text: modelData.value
            color: modelData.strong ? root.tint : root.foreground
            font.family: root.fontFamily
            font.pixelSize: Style.font.caption
            font.bold: modelData.strong === true
          }
        }
      }
    }
    Column {
      visible: root.expanded && root.notes.length > 0
      width: parent.width
      spacing: Style.space(4)
      Repeater {
        model: root.notes
        Text {
          required property var modelData
          required property int index
          width: parent.width
          leftPadding: Style.space(4)
          textFormat: Text.PlainText
          text: modelData
          wrapMode: Text.WordWrap
          color: index === 0 ? root.foreground : root.tint
          opacity: index === 0 ? 0.95 : 0.8
          font.family: root.fontFamily
          font.pixelSize: Style.font.caption
          font.italic: index > 0
        }
      }
    }
    Button {
      visible: root.expanded
      text: root.limitsText
      bordered: true
      focusable: true
      foreground: root.foreground
      fontFamily: root.fontFamily
      fontSize: Style.font.bodySmall
      onClicked: root.limitsRequested()
    }
  }
}
