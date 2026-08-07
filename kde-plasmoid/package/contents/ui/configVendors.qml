import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as QQC2
import org.kde.plasma.plasma5support as Plasma5Support
import org.kde.kirigami as Kirigami
import org.kde.kcmutils as KCM
import "../code/plasmoid-logic.mjs" as Logic

// Port of the GNOME prefs "Vendors" page (gnome-extension/prefs.js). Same table,
// same status wording, same buttons and same actions; the difference is that QML
// has no filesystem API, so one shell probe replaces GLib.file_test/getenv.
//
// This page reads nothing but booleans: the api_key check runs inside awk and
// only its result crosses back, so a key never reaches the QML side.
//
// Expect "Setting initial properties failed: ... does not have a property called
// cfg_<key>" in the journal when this page opens. AppletConfiguration.qml walks
// Plasmoid.configuration.keys() and pushes cfg_<key> onto EVERY config page, so
// a page that stores nothing of its own is warned about all of them. The same
// happens for cfg_<key>Default on every page, including single-page applets —
// Plasma filters those in ConfigurationContainmentAppearance.qml (`key.endsWith
// ("Default")`) but not in the applet path. Cosmetic; the page works.
KCM.SimpleKCM {
    id: page

    property var rows: Logic.vendorAuthRows({})
    property string tuiPath: ""
    property string terminalCommand: ""

    Plasma5Support.DataSource {
        id: prober
        engine: "executable"
        connectedSources: []
        onNewData: (sourceName, data) => {
            disconnectSource(sourceName);
            page.rows = Logic.vendorAuthRows(Logic.parseVendorProbe(data["stdout"] || ""));
        }
        function probe() {
            const cmd = Logic.buildVendorProbeCommand();
            if (connectedSources.indexOf(cmd) === -1)
                connectSource(cmd);
        }
    }

    Plasma5Support.DataSource {
        id: runner
        engine: "executable"
        connectedSources: []
        onNewData: sourceName => disconnectSource(sourceName)
        function run(cmd) {
            if (connectedSources.indexOf(cmd) === -1)
                connectSource(cmd);
        }
    }

    Component.onCompleted: prober.probe()

    // Re-check after the terminal has had time to finish a login — the same 4s
    // delay the GNOME page uses.
    Timer {
        id: recheck
        interval: 4000
        repeat: false
        onTriggered: prober.probe()
    }

    // GNOME re-checks when its window regains focus, which is what fixes the
    // "still shows não logado" loop. The KCM equivalent is becoming visible again.
    onVisibleChanged: if (visible) prober.probe()

    Kirigami.FormLayout {
        anchors.fill: parent

        QQC2.Label {
            Kirigami.FormData.label: i18n("Vendors:")
            text: i18n("OAuth abre um terminal com o comando de login; vendors de API key são configurados no TUI. Reabra esta janela para reavaliar o status.")
            font: Kirigami.Theme.smallFont
            opacity: 0.7
            wrapMode: Text.WordWrap
            Layout.maximumWidth: Kirigami.Units.gridUnit * 26
        }

        Repeater {
            model: page.rows

            delegate: RowLayout {
                id: row
                required property var modelData
                spacing: Kirigami.Units.largeSpacing
                Layout.fillWidth: true

                ColumnLayout {
                    spacing: 0
                    Layout.fillWidth: true

                    QQC2.Label {
                        text: row.modelData.name
                        textFormat: Text.PlainText
                    }

                    QQC2.Label {
                        text: row.modelData.subtitle
                        textFormat: Text.PlainText
                        font: Kirigami.Theme.smallFont
                        opacity: 0.75
                        wrapMode: Text.WordWrap
                        Layout.fillWidth: true
                        Layout.maximumWidth: Kirigami.Units.gridUnit * 22
                    }
                }

                QQC2.Button {
                    text: row.modelData.button
                    enabled: row.modelData.enabled && row.modelData.button !== ""
                    onClicked: {
                        const v = Logic.VENDOR_AUTH.filter(x => x.id === row.modelData.id)[0];
                        if (!v)
                            return;
                        runner.run(Logic.vendorActionCommand(v, page.terminalCommand, page.tuiPath));
                        recheck.restart();
                    }
                }
            }
        }
    }
}
