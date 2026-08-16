import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

Kirigami.FormLayout {
    id: page

    property alias cfg_binaryPath: binaryPathField.text
    property alias cfg_interval: intervalSpin.value
    property alias cfg_vendor: vendorCombo.currentValue
    property alias cfg_extraArgs: extraArgsField.text
    property alias cfg_scrollCycles: scrollCheck.checked
    property alias cfg_hideUnconfigured: hideUnconfiguredCheck.checked

    QQC2.TextField {
        id: binaryPathField
        Kirigami.FormData.label: i18nc("@label:textbox", "Binary path:")
        placeholderText: i18nc("@info:placeholder", "ai-usagebar (looked up in PATH)")
        Layout.preferredWidth: Kirigami.Units.gridUnit * 20
    }

    QQC2.SpinBox {
        id: intervalSpin
        Kirigami.FormData.label: i18nc("@label:spinbox", "Interval (seconds):")
        from: 5
        to: 3600
        stepSize: 5
    }

    QQC2.ComboBox {
        id: vendorCombo
        Kirigami.FormData.label: i18nc("@label:listbox", "Provider:")
        Layout.preferredWidth: Kirigami.Units.gridUnit * 12
        textRole: "label"
        valueRole: "value"
        model: [
            { label: i18nc("@item:inlistbox no provider pinned", "(cycled / config.toml)"), value: "" },
            { label: "anthropic", value: "anthropic" },
            { label: "anthropic_api", value: "anthropic_api" },
            { label: "openai", value: "openai" },
            { label: "zai", value: "zai" },
            { label: "openrouter", value: "openrouter" },
            { label: "deepseek", value: "deepseek" },
            { label: "kimi", value: "kimi" },
            { label: "kilo", value: "kilo" },
            { label: "novita", value: "novita" },
            { label: "moonshot", value: "moonshot" },
            { label: "grok", value: "grok" },
            { label: "supergrok", value: "supergrok" },
            { label: "antigravity", value: "antigravity" },
            { label: "cursor", value: "cursor" },
            { label: "minimax", value: "minimax" },
            { label: "kiro", value: "kiro" }
        ]
        Component.onCompleted: {
            var index = indexOfValue(plasmoid.configuration.vendor)
            currentIndex = index >= 0 ? index : 0
        }
    }

    QQC2.TextField {
        id: extraArgsField
        Kirigami.FormData.label: i18nc("@label:textbox", "Extra arguments:")
        placeholderText: "--icon 󰚩 --format '{session_pct}%'"
        Layout.preferredWidth: Kirigami.Units.gridUnit * 20
    }

    Item {
        Kirigami.FormData.isSection: true
    }

    QQC2.CheckBox {
        id: scrollCheck
        Kirigami.FormData.label: i18nc("@label:checkbox", "Mouse wheel:")
        text: i18nc("@option:check", "switches provider")
    }

    QQC2.CheckBox {
        id: hideUnconfiguredCheck
        Kirigami.FormData.label: i18nc("@label:checkbox providers without an API key", "Without credentials:")
        text: i18nc("@option:check", "hide in the popup and skip when switching")
    }

    QQC2.Label {
        Layout.preferredWidth: Kirigami.Units.gridUnit * 20
        wrapMode: Text.WordWrap
        opacity: 0.7
        font: Kirigami.Theme.smallFont
        text: i18nc("@info:usagetip",
                    "Switching to a provider without a key would only show a warning in the panel; with this option the wheel and the buttons pass over it.")
    }
}
