// One row of the popup, composed as omarchy/Panel.qml composes it: label and
// value share the first line, the bar sits directly under them, and the
// remaining detail and the live countdown stack below.
//
// Handles every section type the report emits — `metric`, `block` and the
// text fallback — so an unfamiliar section degrades to something readable
// instead of vanishing.
import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PlasmaComponents
import "../code/plasmoid-logic.mjs" as Logic

ColumnLayout {
    id: item

    required property var row
    property var colors: ({})
    property string resetText: ""
    property bool showBar: true

    readonly property bool isMetric: item.row && item.row.type === "metric"
    readonly property string detail: item.isMetric ? Logic.metricDetail(item.row) : ""

    spacing: 0

    RowLayout {
        Layout.fillWidth: true
        Layout.topMargin: Kirigami.Units.smallSpacing
        spacing: Kirigami.Units.smallSpacing

        PlasmaComponents.Label {
            Layout.fillWidth: true
            elide: Text.ElideRight
            text: item.row ? (item.row.label || "") : ""
            textFormat: Text.PlainText
        }

        PlasmaComponents.Label {
            font.bold: true
            visible: text !== ""
            color: item.isMetric && item.row.severity === "critical"
                ? Kirigami.Theme.negativeTextColor : Kirigami.Theme.textColor
            text: {
                if (!item.row)
                    return "";
                if (item.isMetric) {
                    if (item.row.headline === "value" && item.row.value !== "")
                        return item.row.value;
                    return item.row.percent === null ? item.row.value : item.row.percent + "%";
                }
                return item.row.value || "";
            }
            textFormat: Text.PlainText
        }
    }

    UsageBar {
        Layout.fillWidth: true
        Layout.topMargin: Math.round(Kirigami.Units.smallSpacing / 2)
        visible: item.showBar && item.isMetric && item.row.percent !== null
        pct: item.isMetric && item.row.percent !== null ? item.row.percent : 0
        severity: item.isMetric ? item.row.severity : "low"
        colors: item.colors
    }

    // What is left of `detail` once the reset clause is removed: the pace text,
    // e.g. "79% elapsed · 73pts under". The countdown below is rendered live
    // from reset_at instead, so keeping both would duplicate and go stale.
    PlasmaComponents.Label {
        Layout.fillWidth: true
        visible: text !== ""
        wrapMode: Text.WordWrap
        font: Kirigami.Theme.smallFont
        opacity: 0.6
        text: item.detail
        textFormat: Text.PlainText
    }

    PlasmaComponents.Label {
        Layout.fillWidth: true
        visible: text !== ""
        font: Kirigami.Theme.smallFont
        opacity: 0.6
        text: item.resetText
        textFormat: Text.PlainText
    }

    // A `block` section carries free-form lines rather than a percentage.
    Repeater {
        model: item.row && item.row.type === "block" ? item.row.body : []

        PlasmaComponents.Label {
            required property string modelData

            Layout.fillWidth: true
            visible: text !== ""
            wrapMode: Text.WordWrap
            font: Kirigami.Theme.smallFont
            opacity: 0.6
            text: modelData
            textFormat: Text.PlainText
        }
    }
}
