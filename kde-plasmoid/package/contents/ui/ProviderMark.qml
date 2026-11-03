// Brand mark from contents/icons/providers/<slug>.svg (symlink/copy of
// windows/popover/src/icons/providers — see sync-provider-icons.sh). isMask +
// Theme.textColor keeps the glyph readable on light and dark Plasma schemes.
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
