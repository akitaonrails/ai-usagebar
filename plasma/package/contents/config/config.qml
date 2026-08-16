import QtQuick
import org.kde.plasma.configuration

ConfigModel {
    ConfigCategory {
        name: i18nc("@title configuration page", "General")
        icon: "configure"
        source: "configGeneral.qml"
    }
}
