// The popup. Layout follows the native Omarchy panel (omarchy/Panel.qml) so the
// two native frontends read the same: header, provider tabs, a status surface
// when something is wrong, the usage rows under one heading, and a footer
// saying how old the data is.
//
// Only the STRUCTURE is ported. Omarchy draws with its shell's own theme
// tokens; everything here sizes off Kirigami.Units and colours off
// Kirigami.Theme, so the widget follows whatever Plasma colour scheme the user
// runs.
import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PlasmaComponents
import "../code/plasmoid-logic.mjs" as Logic

Item {
    id: full

    required property var applet

    readonly property var entry: full.applet.entry
    readonly property string status: full.applet.statusMessage()
    readonly property var rows: Logic.detailRows(full.entry)

    // An Item defaults to implicitHeight 0 and the popup sizes itself from the
    // implicit size, so without this the buttons render off-canvas. The
    // maximum is what makes the popup SHRINK again when a smaller vendor is
    // selected rather than keeping the tallest height it ever had.
    readonly property int contentHeight: column.implicitHeight + Kirigami.Units.largeSpacing * 2
    implicitWidth: Kirigami.Units.gridUnit * 22
    implicitHeight: full.contentHeight
    Layout.minimumHeight: full.contentHeight
    Layout.preferredHeight: full.contentHeight
    Layout.maximumHeight: full.contentHeight

    ColumnLayout {
        id: column
        // Deliberately not anchors.fill: the column must drive the height, not
        // be stretched by it, or contentHeight feeds back on itself.
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.margins: Kirigami.Units.largeSpacing
        spacing: Kirigami.Units.smallSpacing

        // --- header --------------------------------------------------------
        RowLayout {
            Layout.fillWidth: true
            spacing: Kirigami.Units.smallSpacing

            // isMask is explicit on purpose: Kirigami only infers it for some
            // icons, and without it `color` is ignored and the full-colour
            // artwork renders — a little monitor with a chart painted on it,
            // which reads as a picture rather than as this widget's mark and
            // ignores the colour scheme entirely.
            Kirigami.Icon {
                source: "speedometer-symbolic"
                isMask: true
                implicitWidth: Kirigami.Units.iconSizes.medium
                implicitHeight: Kirigami.Units.iconSizes.medium
                color: full.applet.statusIsUrgent()
                    ? Kirigami.Theme.negativeTextColor : Kirigami.Theme.textColor
            }

            ColumnLayout {
                Layout.fillWidth: true
                spacing: 0

                Kirigami.Heading {
                    Layout.fillWidth: true
                    level: 3
                    elide: Text.ElideRight
                    text: full.entry ? full.entry.label : i18n("AI Usage Bar")
                    textFormat: Text.PlainText
                }

                PlasmaComponents.Label {
                    Layout.fillWidth: true
                    elide: Text.ElideRight
                    font: Kirigami.Theme.smallFont
                    opacity: 0.7
                    visible: text !== ""
                    text: {
                        if (!full.entry)
                            return "";
                        const plan = full.entry.plan;
                        return full.entry.stale ? i18n("%1 · cached", plan) : plan;
                    }
                    textFormat: Text.PlainText
                }
            }

            PlasmaComponents.ToolButton {
                icon.name: "view-refresh-symbolic"
                display: PlasmaComponents.AbstractButton.IconOnly
                enabled: full.applet.pendingCommand === ""
                text: i18n("Refresh now")
                PlasmaComponents.ToolTip.text: text
                PlasmaComponents.ToolTip.visible: hovered
                PlasmaComponents.ToolTip.delay: Kirigami.Units.toolTipDelay
                onClicked: full.applet.refresh()
            }

            PlasmaComponents.ToolButton {
                icon.name: "utilities-terminal-symbolic"
                display: PlasmaComponents.AbstractButton.IconOnly
                text: i18n("Open TUI")
                PlasmaComponents.ToolTip.text: text
                PlasmaComponents.ToolTip.visible: hovered
                PlasmaComponents.ToolTip.delay: Kirigami.Units.toolTipDelay
                onClicked: full.applet.launchTui()
            }
        }

        // --- provider tabs -------------------------------------------------
        // Every configured vendor, including the ones currently failing. Hiding
        // a broken vendor is what made "not configured" indistinguishable from
        // "configured and erroring". Only in the tab layout; the cards layout
        // shows every entry side by side instead.
        Flow {
            Layout.fillWidth: true
            Layout.topMargin: Kirigami.Units.smallSpacing
            visible: full.applet.viewMode === 0 && full.applet.tabs.length > 1
            spacing: Kirigami.Units.smallSpacing

            Repeater {
                model: full.applet.tabs

                PlasmaComponents.Button {
                    required property var modelData

                    text: modelData.label
                    checkable: true
                    checked: modelData.active
                    icon.name: modelData.failing ? "dialog-warning" : ""
                    onClicked: full.applet.selectVendor(modelData.id)
                }
            }
        }

        // --- status surface -------------------------------------------------
        Rectangle {
            Layout.fillWidth: true
            Layout.topMargin: Kirigami.Units.smallSpacing
            visible: full.status !== ""
            implicitHeight: visible ? statusLabel.implicitHeight + Kirigami.Units.largeSpacing : 0
            radius: Kirigami.Units.cornerRadius
            color: Qt.alpha(full.applet.statusIsUrgent()
                ? Kirigami.Theme.negativeTextColor : Kirigami.Theme.textColor, 0.09)
            border.width: 1
            border.color: Qt.alpha(full.applet.statusIsUrgent()
                ? Kirigami.Theme.negativeTextColor : Kirigami.Theme.textColor, 0.35)

            PlasmaComponents.Label {
                id: statusLabel
                anchors.fill: parent
                anchors.margins: Kirigami.Units.smallSpacing
                wrapMode: Text.WordWrap
                font: Kirigami.Theme.smallFont
                text: full.status
                textFormat: Text.PlainText
            }
        }

        // --- usage rows (tab layout) ----------------------------------------
        Kirigami.Separator {
            Layout.fillWidth: true
            Layout.topMargin: Kirigami.Units.smallSpacing
            visible: full.applet.viewMode === 0 && full.rows.length > 0
        }

        PlasmaComponents.Label {
            Layout.fillWidth: true
            visible: full.applet.viewMode === 0 && full.rows.length > 0
            font: Kirigami.Theme.smallFont
            opacity: 0.6
            text: i18n("USAGE & BALANCE")
            textFormat: Text.PlainText
        }

        Repeater {
            model: full.applet.viewMode === 0 ? full.rows : []

            UsageRow {
                required property var modelData

                Layout.fillWidth: true
                row: modelData
                colors: full.applet.colors
                resetText: full.applet.resetText(modelData.resetAt)
                showBar: full.applet.showBars
            }
        }

        PlasmaComponents.Label {
            Layout.fillWidth: true
            Layout.topMargin: Kirigami.Units.smallSpacing
            visible: !full.entry && full.status === "" && full.applet.viewMode === 0
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.WordWrap
            opacity: 0.6
            text: i18n("No configured provider reported usage.")
            textFormat: Text.PlainText
        }

        // --- cards (alternative layout) -------------------------------------
        // The #142 card design, reworked: one card per entry the report
        // returned, gauges per window, failures and staleness inline. Sourced
        // from the same aggregate report as the tab view, so switching the
        // layout never refetches.
        VendorCards {
            Layout.fillWidth: true
            visible: full.applet.viewMode === 1
            applet: full.applet
        }

        // --- footer ---------------------------------------------------------
        PlasmaComponents.Label {
            Layout.fillWidth: true
            Layout.topMargin: Kirigami.Units.smallSpacing
            visible: text !== ""
            horizontalAlignment: Text.AlignHCenter
            font: Kirigami.Theme.smallFont
            opacity: 0.6
            text: full.applet.updatedText()
            textFormat: Text.PlainText
        }
    }
}
