import QtQuick
import QtQuick.Layouts
import org.kde.plasma.components as PlasmaComponents
import org.kde.plasma.extras as PlasmaExtras
import org.kde.kirigami as Kirigami
import "Model.js" as Model

// One provider: header (name and plan), how fresh the reading is, and the
// sections the binary reported — metrics with bars, blocks, or an error.
Rectangle {
    id: card

    property var entry: null
    // Bumped on every tick, only so the relative timestamps re-evaluate.
    property date now: new Date()
    // In the tooltip the card is the whole content: no frame of its own.
    property bool flat: false
    // The provider currently shown in the panel.
    property bool active: false
    // Clickable (in the popup) to become the panel's provider.
    property bool interactive: false

    signal activated()

    readonly property bool hasError: entry && entry.status === "error"
    readonly property var sections: entry && entry.sections ? entry.sections : []

    readonly property string updatedText: {
        var elapsed = Model.elapsedMs(card.entry ? card.entry.fetched_at : "", card.now.getTime())
        if (elapsed === null) {
            return ""
        }
        return elapsed < 60000
            ? i18nc("@info the reading was taken seconds ago", "Updated just now")
            : i18nc("@info how long ago the reading was taken", "Updated %1 ago", Model.formatDuration(elapsed))
    }

    readonly property real padding: Kirigami.Units.largeSpacing

    implicitHeight: content.implicitHeight + padding * 2
    radius: flat ? 0 : Math.round(Kirigami.Units.gridUnit * 0.35)
    color: flat
        ? "transparent"
        : pal.dimmed(cardMouse.containsMouse ? 0.1 : 0.06)
    border.width: flat ? 0 : 1
    border.color: card.active ? Kirigami.Theme.highlightColor : pal.dimmed(0.1)

    UsagePalette { id: pal }

    MouseArea {
        id: cardMouse
        anchors.fill: parent
        enabled: card.interactive
        hoverEnabled: card.interactive
        cursorShape: card.interactive ? Qt.PointingHandCursor : Qt.ArrowCursor
        onClicked: card.activated()
    }

    ColumnLayout {
        id: content

        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.margins: card.padding

        spacing: Kirigami.Units.smallSpacing

        RowLayout {
            Layout.fillWidth: true
            spacing: Kirigami.Units.smallSpacing

            // Marks the provider currently displayed in the panel.
            Rectangle {
                visible: card.active && !card.flat
                Layout.alignment: Qt.AlignVCenter
                implicitWidth: Math.round(Kirigami.Units.gridUnit * 0.35)
                implicitHeight: implicitWidth
                radius: implicitWidth / 2
                color: Kirigami.Theme.highlightColor
            }

            PlasmaExtras.Heading {
                Layout.fillWidth: true
                level: 5
                text: card.entry ? (card.entry.display_name || card.entry.name || card.entry.id) : ""
                textFormat: Text.PlainText
                elide: Text.ElideRight
            }

            PlasmaComponents.Label {
                visible: text !== ""
                text: card.entry ? card.entry.plan : ""
                textFormat: Text.PlainText
                opacity: 0.7
                elide: Text.ElideRight
            }
        }

        RowLayout {
            Layout.fillWidth: true
            Layout.bottomMargin: Kirigami.Units.smallSpacing
            spacing: Kirigami.Units.smallSpacing
            visible: updatedLabel.text !== "" || staleLabel.visible

            PlasmaComponents.Label {
                id: updatedLabel
                Layout.fillWidth: true
                text: card.updatedText
                textFormat: Text.PlainText
                font: Kirigami.Theme.smallFont
                opacity: 0.6
                elide: Text.ElideRight
            }

            PlasmaComponents.Label {
                id: staleLabel
                visible: card.entry ? card.entry.stale === true : false
                text: i18nc("@info the provider answered from a stale cache", "cached")
                textFormat: Text.PlainText
                font: Kirigami.Theme.smallFont
                color: Kirigami.Theme.neutralTextColor
            }
        }

        RowLayout {
            Layout.fillWidth: true
            spacing: Kirigami.Units.smallSpacing
            visible: card.hasError

            Kirigami.Icon {
                source: "dialog-warning"
                Layout.preferredWidth: Kirigami.Units.iconSizes.small
                Layout.preferredHeight: Kirigami.Units.iconSizes.small
                Layout.alignment: Qt.AlignTop
            }

            PlasmaComponents.Label {
                Layout.fillWidth: true
                text: !card.entry ? ""
                    : Model.isUnconfigured(card.entry)
                        ? i18nc("@info provider without an API key or login",
                                "No credentials configured")
                        : card.entry.error
                textFormat: Text.PlainText
                wrapMode: Text.WordWrap
                opacity: 0.8
            }
        }

        Repeater {
            model: card.hasError ? [] : card.sections

            delegate: ColumnLayout {
                id: sectionItem

                required property var modelData
                required property int index

                Layout.fillWidth: true
                Layout.topMargin: sectionItem.index > 0 ? Kirigami.Units.smallSpacing * 2 : 0
                spacing: Kirigami.Units.smallSpacing

                // Text blocks get a rule: it separates them from the bars above.
                Kirigami.Separator {
                    Layout.fillWidth: true
                    Layout.bottomMargin: Kirigami.Units.smallSpacing
                    visible: sectionItem.index > 0 && sectionItem.modelData.type === "block"
                    opacity: 0.4
                }

                // Each row only feeds the component that matches its type; the
                // others stay empty (and invisible) instead of binding against
                // fields that do not exist on that section.
                MetricSection {
                    Layout.fillWidth: true
                    visible: sectionItem.modelData.type === "metric"
                    section: visible ? sectionItem.modelData : null
                    now: card.now
                }

                BlockSection {
                    Layout.fillWidth: true
                    visible: sectionItem.modelData.type === "block"
                    section: visible ? sectionItem.modelData : null
                }

                TextSection {
                    Layout.fillWidth: true
                    visible: sectionItem.modelData.type === "text"
                    section: visible ? sectionItem.modelData : null
                }
            }
        }
    }
}
