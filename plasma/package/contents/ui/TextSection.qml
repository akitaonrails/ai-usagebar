import QtQuick
import QtQuick.Layouts
import org.kde.plasma.components as PlasmaComponents
import org.kde.kirigami as Kirigami

// Single label/value row, for report rows that carry no percentage
// (prepaid balances and similar).
RowLayout {
    id: row

    property var section: null

    spacing: Kirigami.Units.smallSpacing

    PlasmaComponents.Label {
        Layout.fillWidth: true
        text: row.section ? row.section.label : ""
        textFormat: Text.PlainText
        font.weight: Font.DemiBold
        elide: Text.ElideRight
    }

    PlasmaComponents.Label {
        text: row.section ? row.section.value : ""
        textFormat: Text.PlainText
        font.weight: Font.DemiBold
    }
}
