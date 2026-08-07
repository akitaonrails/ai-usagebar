import QtQuick
import org.kde.plasma.configuration

// `source` resolves relative to contents/ui/, NOT contents/config/.
// Two categories, mirroring the GNOME prefs window's two pages.
ConfigModel {
    ConfigCategory {
        name: i18n("Geral")
        icon: "configure"
        source: "configGeneral.qml"
    }

    ConfigCategory {
        name: i18n("Vendors")
        icon: "dialog-password"
        source: "configVendors.qml"
    }
}
