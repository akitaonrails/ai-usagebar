// A usage bar drawn with real Rectangles rather than the █/░ block characters
// the Waybar and GNOME frontends use.
//
// marker-logic's barMarkup() is deliberately NOT reused here: it emits Pango
// markup (`<span foreground=…>`), which Qt's rich text does not understand — it
// is a GTK format. The *policy* still comes from the shared module (colorForPct
// and colorForDelta decide the colours), only the drawing is native.
import QtQuick
import org.kde.kirigami as Kirigami
import "../code/marker-logic.mjs" as Marker
import "../code/plasmoid-logic.mjs" as Logic

Item {
    id: root

    required property int pct
    /// Fraction of the window already elapsed, or -1 when unknown. Drives the
    /// pace marker: usage to the right of it means burning faster than the clock.
    property int elapsed: -1
    property var colors: ({})

    readonly property int clampedPct: Math.max(0, Math.min(100, root.pct))
    readonly property bool hasMarker: root.elapsed >= 0 && root.elapsed <= 100

    implicitHeight: Math.round(Kirigami.Units.gridUnit * 0.55)
    implicitWidth: Kirigami.Units.gridUnit * 4

    Rectangle {
        id: track
        anchors.fill: parent
        radius: height / 2
        color: root.colors.empty ?? Kirigami.Theme.disabledTextColor
        opacity: 0.35

        // Two segments, composed exactly as barMarkup() does. Up to the pace
        // marker the fill is the absolute-usage colour; only the overshoot past
        // it takes the pace colour. A single rectangle would have to pick one,
        // which turned the whole bar critical the moment usage edged past the
        // clock — louder than what GNOME and Waybar show for the same numbers.
        readonly property var seg: Logic.barSegments(root.clampedPct, root.elapsed, width)

        Rectangle {
            width: Math.round(track.seg.preWidth)
            height: parent.height
            radius: parent.radius
            color: Marker.colorForPct(root.clampedPct, root.colors)
        }

        Rectangle {
            visible: track.seg.postWidth > 0
            x: Math.round(track.seg.postX)
            width: Math.round(track.seg.postWidth)
            height: parent.height
            radius: parent.radius
            color: Marker.colorForDelta(root.clampedPct - root.elapsed, root.colors)
        }
    }

    // The pace marker. Same role as the │ glyph in the Pango bar, and the same
    // fixed blue: Marker.MARKER is #61afef, hard-coded identically in the GNOME
    // extension and the macOS app, so all three panels mark pace the same way
    // even when the rest of the palette follows the desktop theme.
    Rectangle {
        visible: root.hasMarker
        width: Math.max(1, Math.round(Kirigami.Units.smallSpacing / 2))
        height: parent.height + Math.round(Kirigami.Units.smallSpacing / 2)
        anchors.verticalCenter: parent.verticalCenter
        x: Math.min(parent.width - width, Math.round(parent.width * root.elapsed / 100))
        color: Marker.MARKER
    }
}
