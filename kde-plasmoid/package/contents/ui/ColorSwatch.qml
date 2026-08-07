import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as QQC2
import QtQuick.Dialogs as Dialogs
import org.kde.kirigami as Kirigami

// A colour picker that reads and writes a "#rrggbb" string, matching how the
// GNOME extension (Gtk.ColorDialogButton → rgbaToHex) and the macOS app
// (SwiftUI ColorPicker → String(format:"#%02x%02x%02x")) store their five
// colours. Alpha is deliberately not offered: the shared severity palette is
// opaque hex everywhere.
RowLayout {
    id: swatch

    property string hex: "#000000"

    spacing: Kirigami.Units.smallSpacing

    QQC2.Button {
        implicitWidth: Kirigami.Units.gridUnit * 3
        onClicked: dialog.open()

        contentItem: Rectangle {
            radius: Kirigami.Units.cornerRadius ?? 3
            color: swatch.hex
            border.width: 1
            border.color: Kirigami.Theme.disabledTextColor
            implicitHeight: Kirigami.Units.gridUnit
        }
    }

    QQC2.Label {
        text: swatch.hex
        font: Kirigami.Theme.fixedWidthFont
        opacity: 0.8
    }

    Dialogs.ColorDialog {
        id: dialog
        selectedColor: swatch.hex
        options: Dialogs.ColorDialog.NoEyeDropperButton
        onAccepted: {
            // toString() on a QML color yields "#aarrggbb" when alpha is not 1
            // and "#rrggbb" otherwise; force the 6-digit form the other two
            // frontends read back.
            const c = dialog.selectedColor;
            const to255 = v => Math.round(v * 255);
            const hh = n => ("0" + n.toString(16)).slice(-2);
            swatch.hex = "#" + hh(to255(c.r)) + hh(to255(c.g)) + hh(to255(c.b));
        }
    }
}
