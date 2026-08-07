import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as QQC2
import org.kde.plasma.plasma5support as Plasma5Support
import org.kde.kirigami as Kirigami
import org.kde.kcmutils as KCM
import "../code/plasmoid-logic.mjs" as Logic

// The cfg_<key> properties ARE the mechanism: Plasma copies each config value
// onto a property of that exact name here, and reads them back on Apply. A typo
// in the suffix fails silently — the setting simply never persists.
KCM.SimpleKCM {
    id: page

    property alias cfg_interval: intervalSpin.value
    property alias cfg_binaryPath: binaryField.text
    property alias cfg_terminalCommand: terminalField.text
    property alias cfg_useThemeColors: themeColorsCheck.checked
    property alias cfg_showBars: showBarsCheck.checked
    property alias cfg_barWidth: barWidthSpin.value
    property alias cfg_showSession: showSessionCheck.checked
    property alias cfg_showWeekly: showWeeklyCheck.checked
    property alias cfg_showExtra: showExtraCheck.checked
    property alias cfg_showPercent: showPercentCheck.checked
    property alias cfg_panelAutoThreshold: thresholdSpin.value
    property string cfg_panelPools: "both"
    property alias cfg_colorLow: lowSwatch.hex
    property alias cfg_colorMid: midSwatch.hex
    property alias cfg_colorHigh: highSwatch.hex
    property alias cfg_colorCritical: criticalSwatch.hex
    property alias cfg_colorEmpty: emptySwatch.hex
    // A ComboBox cannot use `property alias` to currentIndex: the alias would be
    // write-only from Plasma's side at load time, so the saved value never shows.
    property int cfg_leftClickAction: 0
    property string cfg_vendor: ""
    property var cfg_vendorRing: []

    // Which vendors exist and whether they currently work. Sourced from
    // `ai-usagebar usage --json`, which reports every entry enabled in
    // config.toml plus its plan or its error — so you can see that OpenAI has no
    // credentials *before* putting it in the scroll ring.
    property var vendorList: Logic.vendorChoices(null)
    property bool probing: true

    Plasma5Support.DataSource {
        id: prober
        engine: "executable"
        connectedSources: []
        onNewData: (sourceName, data) => {
            disconnectSource(sourceName);
            page.probing = false;
            const report = Logic.parseUsageReport(data["stdout"] || "");
            page.vendorList = Logic.vendorChoices(report.entries);
        }
    }

    Component.onCompleted: {
        const bin = Logic.shellQuote(page.cfg_binaryPath || Logic.DEFAULT_BINARY);
        prober.connectSource(bin + " usage --json");
    }

    function ringHas(id) {
        return Array.from(page.cfg_vendorRing || []).indexOf(id) !== -1;
    }

    function setRing(id, on) {
        const next = Array.from(page.cfg_vendorRing || []).filter(v => v !== id);
        if (on)
            next.push(id);
        page.cfg_vendorRing = next;
        // Never leave the applet pointing at a vendor no longer in the ring.
        if (next.length > 0 && next.indexOf(page.cfg_vendor) === -1)
            page.cfg_vendor = next[0];
    }

    Kirigami.FormLayout {
        anchors.fill: parent

        QQC2.Label {
            Kirigami.FormData.label: i18n("Vendors:")
            text: page.probing
                ? i18n("Verificando quais vendors estão configurados…")
                : i18n("Marque os que entram no ciclo do scroll. O status vem do seu config.toml.")
            font: Kirigami.Theme.smallFont
            opacity: 0.7
            wrapMode: Text.WordWrap
            Layout.maximumWidth: Kirigami.Units.gridUnit * 24
        }

        Repeater {
            model: page.vendorList

            delegate: RowLayout {
                id: choice
                required property var modelData
                spacing: Kirigami.Units.smallSpacing

                QQC2.CheckBox {
                    text: choice.modelData.label || choice.modelData.id
                    checked: page.ringHas(choice.modelData.id)
                    onToggled: page.setRing(choice.modelData.id, checked)
                }

                QQC2.Label {
                    text: choice.modelData.status === "ok"
                            ? "✓ " + choice.modelData.detail
                            : choice.modelData.status === "error"
                                ? "✗ " + choice.modelData.detail
                                : i18n("fora do config.toml")
                    color: choice.modelData.status === "ok"
                            ? Kirigami.Theme.positiveTextColor
                            : choice.modelData.status === "error"
                                ? Kirigami.Theme.negativeTextColor
                                : Kirigami.Theme.disabledTextColor
                    font: Kirigami.Theme.smallFont
                    elide: Text.ElideRight
                    Layout.fillWidth: true
                    Layout.maximumWidth: Kirigami.Units.gridUnit * 20
                }
            }
        }

        QQC2.ComboBox {
            id: currentVendorCombo
            Kirigami.FormData.label: i18n("Vendor atual:")
            model: Array.from(page.cfg_vendorRing || [])
            textRole: ""
            displayText: Logic.vendorLabel(page.cfg_vendor)
            onActivated: page.cfg_vendor = model[currentIndex]
            delegate: QQC2.ItemDelegate {
                required property var modelData
                width: currentVendorCombo.width
                text: Logic.vendorLabel(modelData)
                highlighted: currentVendorCombo.highlightedIndex === index
            }
        }

        Item { Kirigami.FormData.isSection: true }

        QQC2.SpinBox {
            id: intervalSpin
            Kirigami.FormData.label: i18n("Intervalo de atualização (s):")
            from: 5
            to: 3600
            stepSize: 5
        }

        QQC2.ComboBox {
            id: clickCombo
            Kirigami.FormData.label: i18n("Clique esquerdo:")
            model: [i18n("Abrir o popup do painel"), i18n("Abrir a TUI")]
            currentIndex: page.cfg_leftClickAction
            onActivated: page.cfg_leftClickAction = currentIndex
        }

        QQC2.TextField {
            id: terminalField
            Kirigami.FormData.label: i18n("Terminal:")
            placeholderText: i18n("Automático (konsole, x-terminal-emulator…)")
        }

        Item { Kirigami.FormData.isSection: true }

        QQC2.CheckBox {
            id: showSessionCheck
            Kirigami.FormData.label: i18n("Exibição:")
            text: i18n("Mostrar barra de 5h (sessão)")
        }

        QQC2.CheckBox {
            id: showWeeklyCheck
            text: i18n("Mostrar barra semanal")
        }

        QQC2.CheckBox {
            id: showExtraCheck
            text: i18n("Mostrar barra de uso extra ($)")
        }

        QQC2.CheckBox {
            id: showPercentCheck
            text: i18n("Mostrar porcentagem/valor")
        }

        QQC2.CheckBox {
            id: showBarsCheck
            text: i18n("Mostrar barras (off = só os números)")
        }

        QQC2.CheckBox {
            id: themeColorsCheck
            text: i18n("Seguir o esquema de cores do Plasma")
        }

        QQC2.SpinBox {
            id: barWidthSpin
            Kirigami.FormData.label: i18n("Largura de cada barra (células):")
            from: 4
            to: 20
            enabled: showBarsCheck.checked
        }

        QQC2.ComboBox {
            id: poolsCombo
            Kirigami.FormData.label: i18n("Pools no painel:")
            model: [i18n("Ambos"), i18n("Só o primeiro"), i18n("Só o segundo"), i18n("Automático")]
            property var ids: ["both", "primary", "secondary", "auto"]
            currentIndex: Math.max(0, ids.indexOf(page.cfg_panelPools))
            onActivated: page.cfg_panelPools = ids[currentIndex]
        }

        QQC2.Label {
            text: i18n("para vendors com dois pools independentes (ex.: Antigravity)")
            font: Kirigami.Theme.smallFont
            opacity: 0.7
        }

        QQC2.SpinBox {
            id: thresholdSpin
            Kirigami.FormData.label: i18n("Limiar do modo automático (%):")
            from: 50
            to: 100
            enabled: page.cfg_panelPools === "auto"
        }

        Item { Kirigami.FormData.isSection: true }

        ColorSwatch {
            id: lowSwatch
            Kirigami.FormData.label: i18n("Baixo (<50%):")
            enabled: !themeColorsCheck.checked
        }

        ColorSwatch {
            id: midSwatch
            Kirigami.FormData.label: i18n("Médio (50-74%):")
            enabled: !themeColorsCheck.checked
        }

        ColorSwatch {
            id: highSwatch
            Kirigami.FormData.label: i18n("Alto (75-89%):")
            enabled: !themeColorsCheck.checked
        }

        ColorSwatch {
            id: criticalSwatch
            Kirigami.FormData.label: i18n("Crítico (>=90%):")
            enabled: !themeColorsCheck.checked
        }

        ColorSwatch {
            id: emptySwatch
            Kirigami.FormData.label: i18n("Vazio (fundo da barra):")
            enabled: !themeColorsCheck.checked
        }

        QQC2.Label {
            text: i18n("usadas quando \"Seguir o esquema de cores do Plasma\" está desligado")
            font: Kirigami.Theme.smallFont
            opacity: 0.7
        }

        Item { Kirigami.FormData.isSection: true }

        QQC2.TextField {
            id: binaryField
            Kirigami.FormData.label: i18n("Caminho do binário (vazio = auto):")
            placeholderText: i18n("vazio = procurar no PATH")
        }

        QQC2.Label {
            text: i18n("O plasmashell não herda o PATH do seu shell, então uma\ninstalação via cargo pode exigir o caminho completo aqui.")
            font: Kirigami.Theme.smallFont
            opacity: 0.7
        }
    }
}
