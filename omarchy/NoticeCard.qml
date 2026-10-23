import QtQuick
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "Notice.js" as Notice

// Status/error notice for the usage view: an accent edge, icon and title in
// the theme's urgent colour for errors, a readable explanation, and the fix as
// a copyable command. Built from Notice.js; no parsing or actions live here.
BorderSurface {
  id: root
  property var notice: null
  property string title: ""
  property string copyText: ""
  property string copiedText: ""
  property string retryText: ""
  property color foreground: Commons.Color.foreground
  property color urgentColor: Commons.Color.urgent
  property string fontFamily: Style.font.family
  property bool copied: false
  readonly property bool isError: !!(notice && notice.kind === "error")
  readonly property color tint: isError ? urgentColor : foreground
  signal copyRequested(string command)
  signal retryRequested()

  implicitHeight: content.implicitHeight + Style.space(24)
  radius: Style.cornerRadius
  color: Qt.rgba(tint.r, tint.g, tint.b, isError ? 0.08 : 0.04)
  borderSpec: Border.flat(Qt.rgba(tint.r, tint.g, tint.b, isError ? 0.45 : 0.18), 1)

  Timer {
    id: copiedReset
    interval: 1600
    onTriggered: root.copied = false
  }

  Rectangle {
    width: Style.space(3)
    anchors.left: parent.left
    anchors.top: parent.top
    anchors.bottom: parent.bottom
    anchors.margins: 1
    color: root.tint
    opacity: root.isError ? 0.9 : 0.4
  }

  Row {
    id: content
    anchors.left: parent.left
    anchors.right: parent.right
    anchors.top: parent.top
    anchors.leftMargin: Style.space(14)
    anchors.rightMargin: Style.space(12)
    anchors.topMargin: Style.space(12)
    spacing: Style.space(10)

    Text {
      id: glyph
      textFormat: Text.PlainText
      text: Notice.glyph(root.notice)
      color: root.tint
      font.family: root.fontFamily
      font.pixelSize: Style.font.body + Style.space(4)
    }

    Column {
      width: content.width - glyph.width - content.spacing
      spacing: Style.space(6)

      Text {
        width: parent.width
        textFormat: Text.PlainText
        text: root.title
        color: root.tint
        font.family: root.fontFamily
        font.pixelSize: Style.font.bodySmall
        font.bold: true
        wrapMode: Text.WordWrap
      }

      Text {
        width: parent.width
        textFormat: Text.PlainText
        text: root.notice ? root.notice.body : ""
        color: root.foreground
        opacity: 0.85
        font.family: root.fontFamily
        font.pixelSize: Style.font.caption
        wrapMode: Text.WordWrap
        lineHeight: 1.15
      }

      Flow {
        width: parent.width
        spacing: Style.space(8)
        visible: !!(root.notice && (root.notice.command !== "" || root.notice.retry))

        BorderSurface {
          visible: !!(root.notice && root.notice.command !== "")
          implicitWidth: commandText.implicitWidth + Style.space(16)
          implicitHeight: Style.spacing.controlHeight
          radius: Style.cornerRadius
          color: Qt.rgba(root.foreground.r, root.foreground.g, root.foreground.b, 0.06)
          borderSpec: Border.flat(Qt.rgba(root.foreground.r, root.foreground.g, root.foreground.b, 0.15), 1)

          Text {
            id: commandText
            anchors.centerIn: parent
            textFormat: Text.PlainText
            text: root.notice ? "$ " + root.notice.command : ""
            color: root.foreground
            font.family: root.fontFamily
            font.pixelSize: Style.font.caption
          }
        }

        Button {
          visible: !!(root.notice && root.notice.command !== "")
          height: Style.spacing.controlHeight
          text: root.copied ? root.copiedText : root.copyText
          iconText: root.copied ? "󰄬" : "󰆏"
          bordered: true
          foreground: root.foreground
          fontFamily: root.fontFamily
          fontSize: Style.font.caption
          iconSize: Style.font.bodySmall
          onClicked: {
            root.copyRequested(root.notice.command)
            root.copied = true
            copiedReset.restart()
          }
        }

        Button {
          visible: !!(root.notice && root.notice.retry)
          height: Style.spacing.controlHeight
          text: root.retryText
          iconText: "󰑐"
          bordered: true
          foreground: root.foreground
          fontFamily: root.fontFamily
          fontSize: Style.font.caption
          iconSize: Style.font.bodySmall
          onClicked: root.retryRequested()
        }
      }
    }
  }
}
