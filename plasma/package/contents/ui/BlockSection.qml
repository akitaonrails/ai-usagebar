import QtQuick
import QtQuick.Layouts
import org.kde.plasma.components as PlasmaComponents
import org.kde.kirigami as Kirigami
import "Model.js" as Model

// Free-form block (the Codex credit balance, for instance): a label plus
// "key: value" lines aligned on both edges of the card.
ColumnLayout {
    id: block

    property var section: null

    spacing: Math.round(Kirigami.Units.smallSpacing / 2)

    PlasmaComponents.Label {
        Layout.fillWidth: true
        text: block.section ? block.section.label : ""
        textFormat: Text.PlainText
        font.weight: Font.DemiBold
        elide: Text.ElideRight
    }

    Repeater {
        model: block.section && block.section.body ? block.section.body : []

        delegate: RowLayout {
            id: bodyRow

            required property string modelData

            readonly property var pair: Model.splitPair(modelData)

            Layout.fillWidth: true
            spacing: Kirigami.Units.smallSpacing

            PlasmaComponents.Label {
                Layout.fillWidth: true
                text: bodyRow.pair.key
                textFormat: Text.PlainText
                font.pointSize: Kirigami.Theme.smallFont.pointSize
                opacity: 0.7
                elide: Text.ElideRight
            }

            PlasmaComponents.Label {
                visible: text !== ""
                text: bodyRow.pair.value
                textFormat: Text.PlainText
                font.pointSize: Kirigami.Theme.smallFont.pointSize
                font.weight: Font.DemiBold
            }
        }
    }
}
