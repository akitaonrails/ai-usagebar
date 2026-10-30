// Brand mark from contents/icons/providers/<slug>.svg — same catalog as the
// Win/mac tray (src/tray/marks.rs). isMask + Theme.textColor keeps the glyph
// readable on light and dark Plasma schemes; currentColor SVGs mask cleanly.
import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

Kirigami.Icon {
    id: mark

    property string slug: ""
    property int pixelSize: Kirigami.Units.iconSizes.small

    visible: mark.slug !== ""
    source: mark.slug !== ""
        ? Qt.resolvedUrl("../icons/providers/" + mark.slug + ".svg")
        : ""
    isMask: true
    color: Kirigami.Theme.textColor
    implicitWidth: mark.pixelSize
    implicitHeight: mark.pixelSize
    Layout.preferredWidth: mark.pixelSize
    Layout.preferredHeight: mark.pixelSize
}
