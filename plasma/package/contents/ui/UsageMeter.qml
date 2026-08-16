import QtQuick
import org.kde.kirigami as Kirigami

// Rounded usage bar with a thin marker at the elapsed position of the window,
// so consumption running ahead of the clock is visible at a glance.
Item {
    id: meter

    property real percent: 0      // 0..100 consumed
    property real elapsed: -1     // 0..100 of the window; negative hides the marker
    property color fillColor: Kirigami.Theme.highlightColor

    readonly property real clampedPercent: Math.max(0, Math.min(100, percent))

    implicitHeight: Math.round(Kirigami.Units.gridUnit * 0.4)

    Rectangle {
        id: track
        anchors.fill: parent
        radius: height / 2
        color: Qt.rgba(Kirigami.Theme.textColor.r,
                       Kirigami.Theme.textColor.g,
                       Kirigami.Theme.textColor.b,
                       0.15)

        Rectangle {
            id: fill
            height: parent.height
            width: meter.clampedPercent <= 0
                ? 0
                : Math.max(parent.height, parent.width * meter.clampedPercent / 100)
            radius: parent.radius
            color: meter.fillColor

            Behavior on width {
                NumberAnimation { duration: Kirigami.Units.longDuration; easing.type: Easing.OutCubic }
            }
        }

        Rectangle {
            id: marker
            visible: meter.elapsed >= 0 && meter.elapsed <= 100
            width: 2
            height: parent.height
            radius: width / 2
            x: Math.round(parent.width * Math.max(0, Math.min(100, meter.elapsed)) / 100)
               - width / 2
            // Over the filled part the marker has to read against the bar
            // color; over the empty track, against the background.
            color: meter.elapsed <= meter.clampedPercent
                ? Qt.rgba(Kirigami.Theme.backgroundColor.r,
                          Kirigami.Theme.backgroundColor.g,
                          Kirigami.Theme.backgroundColor.b,
                          0.8)
                : Qt.rgba(Kirigami.Theme.textColor.r,
                          Kirigami.Theme.textColor.g,
                          Kirigami.Theme.textColor.b,
                          0.55)
        }
    }
}
