import QtQuick
import QtQuick.Layouts
import org.kde.plasma.components as PlasmaComponents
import org.kde.kirigami as Kirigami

// Sizing note: the popup takes its size from this item's IMPLICIT size, so the
// implicit height has to be a real number derived from the content. An earlier
// version set only Layout.preferredHeight and anchored the column with
// anchors.fill, which left the root at height 0 — the rows still rendered but
// the action buttons were clipped away entirely. It showed up on Antigravity
// first because a grouped vendor adds two headings and two extra rows.
Item {
    id: full

    required property var applet

    readonly property int contentHeight: column.implicitHeight + Kirigami.Units.largeSpacing * 2

    implicitWidth: Kirigami.Units.gridUnit * 22
    implicitHeight: full.contentHeight

    Layout.minimumWidth: Kirigami.Units.gridUnit * 20
    Layout.preferredWidth: full.implicitWidth
    // minimumHeight, not just preferred: the buttons must survive the tallest
    // vendor, not just the two-row case.
    Layout.minimumHeight: full.contentHeight
    Layout.preferredHeight: full.contentHeight
    // maximumHeight is what makes the popup SHRINK again. Plasma grows the popup
    // window for a taller vendor (Antigravity: 2 headings + 4 rows) and does not
    // reduce it when the hints get smaller, so switching back to a 3-row vendor
    // left the content pinned to the top of an over-tall popup. Bounding it above
    // forces the window back down to the content.
    Layout.maximumHeight: full.contentHeight


    ColumnLayout {
        id: column
        // Deliberately NOT anchors.fill: the column's natural height is what
        // drives the popup, so it must not be pinned to the parent's height.
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.margins: Kirigami.Units.largeSpacing
        spacing: Kirigami.Units.smallSpacing

        UsageRows {
            applet: full.applet
            Layout.fillWidth: true
        }

        RowLayout {
            Layout.fillWidth: true
            Layout.topMargin: Kirigami.Units.smallSpacing
            spacing: Kirigami.Units.smallSpacing

            // Same wording as the GNOME dropdown and the macOS menu.
            PlasmaComponents.Button {
                text: i18n("Atualizar agora")
                icon.name: "view-refresh"
                onClicked: full.applet.refresh()
            }

            Item { Layout.fillWidth: true }

            PlasmaComponents.Button {
                text: i18n("Abrir TUI")
                icon.name: "utilities-terminal"
                onClicked: {
                    full.applet.launchTui();
                    full.applet.expanded = false;
                }
            }
        }
    }
}
