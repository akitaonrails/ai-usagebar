pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Layouts
import org.kde.plasma.components as PlasmaComponents
import org.kde.kirigami as Kirigami
import "../code/plasmoid-logic.mjs" as Logic

// Shared body of the popup and the tooltip: plan headline plus one row per
// quota window. Both surfaces show the same thing, so they share the component
// rather than drifting apart.
ColumnLayout {
    id: body

    required property var applet

    spacing: Kirigami.Units.smallSpacing

    Kirigami.Heading {
        level: 4
        text: body.applet.usage && body.applet.usage.plan
            ? body.applet.usage.plan
            : i18n("AI Usage Bar")
        textFormat: Text.PlainText
        elide: Text.ElideRight
        Layout.fillWidth: true
    }

    PlasmaComponents.Label {
        text: body.applet.vendor
        textFormat: Text.PlainText
        opacity: 0.7
        font: Kirigami.Theme.smallFont
        Layout.fillWidth: true
    }

    // Failure and "the binary is still talking" states show the binary's own
    // words. Never a blank popup.
    PlasmaComponents.Label {
        visible: body.applet.failure !== ""
        text: body.applet.failure
        color: Kirigami.Theme.negativeTextColor
        textFormat: Text.PlainText
        wrapMode: Text.WordWrap
        Layout.fillWidth: true
        Layout.maximumWidth: Kirigami.Units.gridUnit * 22
    }

    PlasmaComponents.Label {
        visible: body.applet.failure === "" && body.applet.notice !== ""
        text: body.applet.notice
        textFormat: Text.PlainText
        Layout.fillWidth: true
    }

    Repeater {
        model: Logic.detailRows(body.applet.usage)

        // A grouped vendor (Antigravity) has two independent pools, so the rows
        // are grouped under Session / Weekly headings exactly as the GNOME
        // dropdown does — four flat rows give no clue which pool is which.
        delegate: Loader {
            required property var modelData
            Layout.fillWidth: true
            Layout.topMargin: modelData.kind === "heading" ? Kirigami.Units.smallSpacing : 0
            sourceComponent: modelData.kind === "heading" ? headingRow : usageRow

            Component {
                id: headingRow
                Kirigami.Heading {
                    level: 5
                    text: modelData.label
                    textFormat: Text.PlainText
                    opacity: 0.8
                }
            }

            Component {
                id: usageRow
                UsageRow {
                    label: modelData.label
                    pct: modelData.pct
                    valueText: modelData.valueText
                    reset: modelData.reset
                    elapsed: modelData.elapsed
                    colors: body.applet.colors
                }
            }
        }
    }
}
