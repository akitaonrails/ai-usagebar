import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

// A custom toolTipItem rather than toolTipMainText/toolTipSubText: the default
// tooltip hardcodes textFormat: Text.PlainText on the main text and caps the
// sub text at 8 lines in a single wrapped Label, which cannot express one row
// per quota window with a bar.
Item {
    id: tip

    required property var applet

    // Tooltips use the Window colour set. Copying what DefaultToolTip.qml does
    // matters: without `inherit: false` the set is overwritten by the parent and
    // the text picks up panel colours, which on some themes is invisible.
    Kirigami.Theme.colorSet: Kirigami.Theme.Window
    Kirigami.Theme.inherit: false

    implicitWidth: content.implicitWidth
    implicitHeight: content.implicitHeight

    UsageRows {
        id: content
        applet: tip.applet
        anchors.fill: parent
        // The tooltip does not size a custom item for us.
        Layout.minimumWidth: Kirigami.Units.gridUnit * 20
    }
}
