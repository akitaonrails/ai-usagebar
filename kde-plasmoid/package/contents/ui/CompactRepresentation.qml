pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Layouts
import org.kde.plasma.plasmoid
import org.kde.plasma.core as PlasmaCore
import org.kde.plasma.components as PlasmaComponents
import org.kde.kirigami as Kirigami
import "../code/marker-logic.mjs" as Marker
import "../code/plasmoid-logic.mjs" as Logic

// Providing our own compactRepresentation means we own click behaviour
// entirely: Plasma's "click expands the popup" is not framework magic, it is
// plain QML in DefaultCompactRepresentation.qml which is used only when an
// applet supplies nothing. So there is nothing to suppress — we simply choose.
MouseArea {
    id: root

    required property var applet

    // Right button is deliberately NOT accepted, so it falls through to
    // Plasma's own context menu (Configure, Remove, …).
    acceptedButtons: Qt.LeftButton | Qt.MiddleButton
    hoverEnabled: true

    readonly property bool vertical: Plasmoid.formFactor === PlasmaCore.Types.Vertical

    // Delegated so the tested rule is the one that actually runs.
    readonly property var parts: Logic.cellParts(Plasmoid.configuration.showPercent,
                                                 Plasmoid.configuration.showBars)

    Layout.minimumWidth: root.vertical ? 0 : content.implicitWidth
    Layout.preferredWidth: root.vertical ? 0 : content.implicitWidth
    Layout.minimumHeight: root.vertical ? content.implicitHeight : 0
    Layout.preferredHeight: root.vertical ? content.implicitHeight : 0

    // --- click -------------------------------------------------------------
    // Captured on press: clicking outside an open popup dismisses it *before*
    // the click lands, so without this a click-to-close immediately reopens.
    property bool wasExpanded: false
    onPressed: root.wasExpanded = root.applet.expanded
    onClicked: mouse => {
        if (mouse.button === Qt.MiddleButton || Plasmoid.configuration.leftClickAction === 1) {
            root.applet.launchTui();
            return;
        }
        root.applet.expanded = !root.wasExpanded;
    }

    // --- wheel: cycle the vendor ring --------------------------------------
    // angleDelta is in eighths of a degree; 120 units == 15deg == one detent.
    // `inverted` honours natural scrolling, the angleDelta.x fallback covers
    // horizontal wheels and touchpads, and the loops (not ifs) matter because a
    // fast flick delivers a delta well over 120 in a single event.
    property int wheelAccumulator: 0
    onWheel: wheel => {
        const d = wheel.angleDelta.y !== 0 ? wheel.angleDelta.y : wheel.angleDelta.x;
        root.wheelAccumulator += (wheel.inverted ? -1 : 1) * d;
        while (root.wheelAccumulator >= 120) {
            root.wheelAccumulator -= 120;
            root.applet.cycleVendor(1);   // scroll up == --cycle-next, as in Waybar
        }
        while (root.wheelAccumulator <= -120) {
            root.wheelAccumulator += 120;
            root.applet.cycleVendor(-1);
        }
    }

    // Keyboard/global-shortcut parity. Plasmoid.activated() is NOT emitted by
    // mouse clicks — only by shortcut, Enter/Space and accessibility.
    Keys.onPressed: event => {
        switch (event.key) {
        case Qt.Key_Space:
        case Qt.Key_Enter:
        case Qt.Key_Return:
        case Qt.Key_Select:
            Plasmoid.activated();
            event.accepted = true;
            break;
        }
    }

    GridLayout {
        id: content
        anchors.centerIn: parent
        flow: root.vertical ? GridLayout.TopToBottom : GridLayout.LeftToRight
        columnSpacing: Kirigami.Units.smallSpacing * 2
        rowSpacing: Kirigami.Units.smallSpacing

        // Error / message state: show the binary's own words rather than
        // inventing our own, and never render a blank panel item.
        // "⚠ ai" and "5h …" are the exact strings the GNOME and macOS panels
        // use for these states.
        PlasmaComponents.Label {
            visible: !root.applet.usage
            text: root.applet.failure ? "⚠ ai" : (root.applet.notice || "5h …")
            color: root.applet.failure ? Kirigami.Theme.negativeTextColor : Kirigami.Theme.textColor
            textFormat: Text.PlainText
        }

        Repeater {
            model: root.applet.usage ? root.applet.compactCells : []

            delegate: RowLayout {
                id: cell
                required property var modelData
                spacing: Kirigami.Units.smallSpacing

                PlasmaComponents.Label {
                    text: cell.modelData.tag
                    textFormat: Text.PlainText
                    opacity: 0.7
                    font: Kirigami.Theme.smallFont
                }

                PlasmaComponents.Label {
                    visible: root.parts.value
                    text: cell.modelData.text || (cell.modelData.pct + "%")
                    textFormat: Text.PlainText
                    color: Marker.colorForPct(cell.modelData.pct, root.applet.colors)
                }

                UsageBar {
                    visible: root.parts.bar
                    pct: cell.modelData.pct
                    elapsed: cell.modelData.elapsed
                    colors: root.applet.colors
                    implicitWidth: Kirigami.Units.gridUnit * Plasmoid.configuration.barWidth / 4
                }
            }
        }
    }
}
