// The alternative popup layout (config "viewMode": VendorCards): one card per
// entry the aggregate `usage --json` report returned, every window rendered as
// its own gauge row.
//
// Ported from the closed #142 card design, reworked the way the maintainer
// asked: no second plasmoid, no per-vendor subprocess plumbing, and above all
// no hardcoded provider table — every label, window, percentage, severity and
// error text comes from the same report the tab view consumes, projected by
// Logic.cardModel (table-tested in plasmoid-logic.test.mjs). Colours follow
// the applet's severity palette, so this view tracks the same theme tokens as
// everything else instead of carrying its own hexes.
//
// A window the vendor does not report (percent === null) renders as a dash
// with "not reported", never as a fabricated 0% bar — an absent 5h window must
// not look like an empty one.
import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PlasmaComponents
import "../code/plasmoid-logic.mjs" as Logic

ColumnLayout {
    id: cards

    required property var applet

    spacing: Kirigami.Units.smallSpacing

    Repeater {
        model: cards.applet.cards

        delegate: Rectangle {
            id: card

            required property var modelData

            readonly property bool isError: modelData.state === "error"
            readonly property color accent: Logic.severityColor(
                modelData.accent, cards.applet.colors) ?? Kirigami.Theme.textColor

            Layout.fillWidth: true
            implicitHeight: content.implicitHeight + Kirigami.Units.largeSpacing
            radius: Kirigami.Units.cornerRadius
            color: Qt.alpha(card.accent, 0.07)
            border.width: 1
            border.color: Qt.alpha(card.accent, card.isError ? 0.55 : 0.30)

            TapHandler {
                onTapped: cards.applet.selectVendor(card.modelData.id)
            }

            // The detail lines (pace, resets) live in the hover tooltip rather
            // than inline, which is what keeps a five-provider report from
            // becoming an unscrollable wall.
            PlasmaComponents.ToolTip {
                visible: cardHover.hovered && tooltipText !== ""
                delay: Kirigami.Units.toolTipDelay
                timeout: 5000
                text: card.tooltipText
            }

            readonly property string tooltipText: {
                const lines = [];
                for (const w of card.modelData.windows)
                    if (w.detail !== "")
                        lines.push(w.label + " · " + w.detail);
                return lines.join("\n");
            }

            HoverHandler {
                id: cardHover
                cursorShape: Qt.PointingHandCursor
            }

            ColumnLayout {
                id: content

                anchors.left: parent.left
                anchors.right: parent.right
                anchors.top: parent.top
                anchors.margins: Kirigami.Units.smallSpacing
                spacing: Kirigami.Units.smallSpacing / 2

                // --- header -------------------------------------------------
                RowLayout {
                    Layout.fillWidth: true
                    spacing: Kirigami.Units.smallSpacing

                    PlasmaComponents.Label {
                        Layout.fillWidth: true
                        elide: Text.ElideRight
                        font.bold: true
                        text: card.modelData.label
                        textFormat: Text.PlainText
                    }

                    // Same "cached" wording the tab view puts after the plan.
                    PlasmaComponents.Label {
                        visible: card.modelData.state === "stale"
                        font: Kirigami.Theme.smallFont
                        opacity: 0.7
                        text: i18n("cached")
                        textFormat: Text.PlainText
                    }

                    // The selected card is the entry the compact panel and the
                    // tooltip show, matching the tab view's checked button.
                    Kirigami.Icon {
                        source: "emblem-ok-symbolic"
                        isMask: true
                        visible: cards.applet.vendor === card.modelData.id && !card.isError
                        implicitWidth: Kirigami.Units.iconSizes.small
                        implicitHeight: Kirigami.Units.iconSizes.small
                        color: Kirigami.Theme.positiveTextColor
                    }

                    Kirigami.Icon {
                        source: "dialog-warning"
                        visible: card.isError
                        implicitWidth: Kirigami.Units.iconSizes.small
                        implicitHeight: Kirigami.Units.iconSizes.small
                    }
                }

                PlasmaComponents.Label {
                    Layout.fillWidth: true
                    visible: text !== ""
                    elide: Text.ElideRight
                    font: Kirigami.Theme.smallFont
                    opacity: 0.7
                    text: card.modelData.plan
                    textFormat: Text.PlainText
                }

                // --- gauges, or the failure that replaces them ---------------
                // Credential and fetch failures replace the gauges entirely:
                // an empty bar would read as "0% used", which is exactly the
                // silently-failing fetch the cards view exists to expose.
                Repeater {
                    model: card.isError ? [] : card.modelData.windows

                    ColumnLayout {
                        id: gauge

                        required property var modelData

                        Layout.fillWidth: true
                        spacing: Math.round(Kirigami.Units.smallSpacing / 2)

                        RowLayout {
                            Layout.fillWidth: true
                            spacing: Kirigami.Units.smallSpacing

                            PlasmaComponents.Label {
                                Layout.fillWidth: true
                                elide: Text.ElideRight
                                text: gauge.modelData.label
                                textFormat: Text.PlainText
                            }

                            // A window the vendor does not report shows a dash
                            // and the words, never a bar and never a bare 0%.
                            PlasmaComponents.Label {
                                visible: gauge.modelData.percent === null
                                font: Kirigami.Theme.smallFont
                                opacity: 0.6
                                text: i18n("— not reported")
                                textFormat: Text.PlainText
                            }

                            PlasmaComponents.Label {
                                visible: gauge.modelData.percent !== null
                                font.bold: gauge.modelData.severity === "critical"
                                color: gauge.modelData.severity === "critical"
                                    ? Kirigami.Theme.negativeTextColor
                                    : Kirigami.Theme.textColor
                                text: gauge.modelData.value
                                textFormat: Text.PlainText
                            }
                        }

                        UsageBar {
                            Layout.fillWidth: true
                            visible: gauge.modelData.percent !== null
                            pct: gauge.modelData.percent ?? 0
                            severity: gauge.modelData.severity
                            colors: cards.applet.colors
                        }
                    }
                }

                PlasmaComponents.Label {
                    Layout.fillWidth: true
                    visible: card.isError
                    wrapMode: Text.WordWrap
                    font: Kirigami.Theme.smallFont
                    color: Kirigami.Theme.negativeTextColor
                    text: card.isError ? card.modelData.error : ""
                    textFormat: Text.PlainText
                }

                // --- block sections (balances and notes) ---------------------
                Repeater {
                    model: card.isError ? [] : card.modelData.blocks

                    ColumnLayout {
                        required property var modelData

                        Layout.fillWidth: true
                        spacing: 0

                        PlasmaComponents.Label {
                            Layout.fillWidth: true
                            visible: text !== ""
                            elide: Text.ElideRight
                            font: Kirigami.Theme.smallFont
                            opacity: 0.7
                            text: modelData.label || ""
                            textFormat: Text.PlainText
                        }

                        Repeater {
                            model: modelData.body || []

                            PlasmaComponents.Label {
                                required property string modelData

                                Layout.fillWidth: true
                                visible: text !== ""
                                wrapMode: Text.WordWrap
                                font: Kirigami.Theme.smallFont
                                opacity: 0.7
                                text: modelData
                                textFormat: Text.PlainText
                            }
                        }
                    }
                }
            }
        }
    }
}
