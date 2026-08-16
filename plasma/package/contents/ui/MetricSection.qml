import QtQuick
import QtQuick.Layouts
import org.kde.plasma.components as PlasmaComponents
import org.kde.kirigami as Kirigami
import "Model.js" as Model

// One quota window: label, value, bar and the two detail lines.
ColumnLayout {
    id: metric

    property var section: null
    // Re-evaluated on every tick so the countdown keeps running while the
    // popup stays open.
    property date now: new Date()

    readonly property var detail: Model.parseDetail(section ? section.detail : "")
    readonly property color accent: pal.forSeverity(section ? section.severity : "")

    // Absolute reset metadata wins; the rendered fragment is the fallback for
    // metrics (or binaries) that carry no timestamp.
    readonly property string resetText: {
        var remaining = Model.resetRemainingMs(metric.section ? metric.section.reset_at : "",
                                               metric.now.getTime())
        if (remaining === null) {
            return metric.detail.reset
        }
        return remaining > 0
            ? i18nc("@label time until the quota window resets", "Resets in %1", Model.formatDuration(remaining))
            : i18nc("@label the quota window is past its reset time", "Reset due")
    }

    readonly property string paceText: {
        var parts = []
        if (metric.detail.elapsed !== null) {
            parts.push(i18nc("@label share of the quota window that has passed",
                             "%1% elapsed", metric.detail.elapsed))
        }
        if (metric.detail.onPace) {
            parts.push(i18nc("@label usage matches the elapsed time", "on pace"))
        } else if (metric.detail.pacePoints !== null) {
            parts.push(metric.detail.paceDirection === "ahead"
                ? i18nc("@label using quota faster than the clock, in percentage points",
                        "%1 pts ahead", metric.detail.pacePoints)
                : i18nc("@label using quota slower than the clock, in percentage points",
                        "%1 pts under", metric.detail.pacePoints))
        }
        return parts.concat(metric.detail.extras).join(" · ")
    }

    spacing: Math.round(Kirigami.Units.smallSpacing / 2)

    UsagePalette { id: pal }

    RowLayout {
        Layout.fillWidth: true
        spacing: Kirigami.Units.smallSpacing

        PlasmaComponents.Label {
            Layout.fillWidth: true
            text: metric.section ? metric.section.label : ""
            textFormat: Text.PlainText
            font.weight: Font.DemiBold
            elide: Text.ElideRight
        }

        PlasmaComponents.Label {
            text: metric.section ? metric.section.value : ""
            textFormat: Text.PlainText
            font.weight: Font.DemiBold
            color: metric.accent
        }
    }

    UsageMeter {
        Layout.fillWidth: true
        Layout.topMargin: Math.round(Kirigami.Units.smallSpacing / 2)
        Layout.bottomMargin: Math.round(Kirigami.Units.smallSpacing / 2)
        percent: metric.section ? metric.section.percent : 0
        elapsed: metric.detail.elapsed === null ? -1 : metric.detail.elapsed
        fillColor: metric.accent
    }

    RowLayout {
        Layout.fillWidth: true
        spacing: Kirigami.Units.smallSpacing
        visible: paceLabel.text !== "" || resetLabel.text !== ""

        PlasmaComponents.Label {
            id: paceLabel
            Layout.fillWidth: true
            text: metric.paceText
            textFormat: Text.PlainText
            font: Kirigami.Theme.smallFont
            opacity: 0.7
            elide: Text.ElideRight
        }

        PlasmaComponents.Label {
            id: resetLabel
            text: metric.resetText
            textFormat: Text.PlainText
            font: Kirigami.Theme.smallFont
            opacity: 0.7
        }
    }
}
