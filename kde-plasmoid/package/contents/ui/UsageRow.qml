import QtQuick
import QtQuick.Layouts
import org.kde.plasma.components as PlasmaComponents
import org.kde.kirigami as Kirigami
import "../code/marker-logic.mjs" as Marker

// One quota row: name, bar, value, and when we have it, the reset countdown.
RowLayout {
    id: row

    required property string label
    required property int pct
    property string valueText: ""
    property string reset: ""
    property int elapsed: -1
    property var colors: ({})

    spacing: Kirigami.Units.smallSpacing * 2

    PlasmaComponents.Label {
        text: row.label
        textFormat: Text.PlainText
        elide: Text.ElideRight
        Layout.minimumWidth: Kirigami.Units.gridUnit * 6
        Layout.maximumWidth: Kirigami.Units.gridUnit * 9
    }

    UsageBar {
        pct: row.pct
        elapsed: row.elapsed
        colors: row.colors
        Layout.fillWidth: true
        Layout.minimumWidth: Kirigami.Units.gridUnit * 5
    }

    PlasmaComponents.Label {
        text: row.valueText || (row.pct + "%")
        textFormat: Text.PlainText
        color: Marker.colorForPct(row.pct, row.colors)
        horizontalAlignment: Text.AlignRight
        Layout.minimumWidth: Kirigami.Units.gridUnit * 2.5
    }

    PlasmaComponents.Label {
        visible: row.reset !== "" && row.reset !== "—"
        // Same phrasing as the GNOME dropdown ("↺ resets in 1h 30m"); macOS
        // drops the words for menu-bar width, which a panel popup doesn't need.
        text: i18nc("time until a quota window resets", "↺ reinicia em %1", row.reset)
        textFormat: Text.PlainText
        opacity: 0.7
        font: Kirigami.Theme.smallFont
        Layout.minimumWidth: Kirigami.Units.gridUnit * 5
    }
}
