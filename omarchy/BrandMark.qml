import QtQuick
import QtQuick.Effects
import qs.Commons

Item {
  id: root
  property string brand: ""
  property string fallback: "󰚩"
  property color foreground: Color.foreground
  property string fontFamily: Style.font.family
  property real fontSize: Style.font.body

  width: fontSize
  height: fontSize

  Image {
    id: artwork
    anchors.fill: parent
    visible: false
    source: root.brand !== "" ? Qt.resolvedUrl("icons/" + root.brand) : ""
    fillMode: Image.PreserveAspectFit
    sourceSize.width: width * 2
    sourceSize.height: height * 2
    cache: true
  }

  MultiEffect {
    anchors.fill: artwork
    source: artwork
    visible: root.brand !== "" && artwork.status === Image.Ready
    colorization: 1
    colorizationColor: root.foreground
  }

  Text {
    visible: root.brand === "" || artwork.status !== Image.Ready
    anchors.centerIn: parent
    textFormat: Text.PlainText
    text: root.fallback
    color: root.foreground
    font.family: root.fontFamily
    font.pixelSize: root.fontSize
  }
}
