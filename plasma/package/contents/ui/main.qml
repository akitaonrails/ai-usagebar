import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as QQC2
import org.kde.plasma.plasmoid
import org.kde.plasma.core as PlasmaCore
import org.kde.plasma.components as PlasmaComponents
import org.kde.plasma.extras as PlasmaExtras
import org.kde.plasma.plasma5support as P5Support
import org.kde.kirigami as Kirigami
import "Model.js" as Model

PlasmoidItem {
    id: root

    // Panel text, straight from `ai-usagebar --json`
    property string barHtml: ""
    property string statusClass: ""
    property string lastError: ""
    property bool loading: true

    // Full report (`ai-usagebar usage --json`), used by the tooltip and popup
    property var usageEntries: []
    property string reportError: ""
    property bool reportLoaded: false
    property double reportFetchedAt: 0

    property date now: new Date()
    property bool hovered: false

    // The panel text follows the theme font, like every other applet.
    readonly property string barFont: Kirigami.Theme.defaultFont.family

    // Provider shown in the panel. `vendor` pins one through the configuration;
    // without it, the provider cycled by the wheel/buttons wins.
    readonly property bool vendorPinned: (plasmoid.configuration.vendor || "").trim() !== ""
    readonly property string effectiveVendor: vendorPinned
        ? (plasmoid.configuration.vendor || "").trim()
        : (plasmoid.configuration.activeVendor || "").trim()

    // Report entry matching the panel's provider — what the tooltip renders.
    readonly property var activeEntry: Model.entryById(usageEntries, effectiveVendor)

    preferredRepresentation: compactRepresentation

    // ------------------------------------------------------------------
    // Running the binary
    // ------------------------------------------------------------------
    function binary() {
        var bin = (plasmoid.configuration.binaryPath || "").trim()
        return bin === "" ? "ai-usagebar" : bin
    }

    function buildCmd(args) {
        var vendor = root.effectiveVendor
        var extra = (plasmoid.configuration.extraArgs || "").trim()
        var cmd = 'PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH" ' + root.binary()
        if (vendor !== "") {
            cmd += ' --vendor ' + vendor
        }
        if (extra !== "") {
            cmd += ' ' + extra
        }
        return cmd + ' ' + args
    }

    // `usage` and `settings` are subcommands: clap rejects `--vendor` and the
    // other global options next to them.
    function buildSubCmd(sub) {
        return 'PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH" ' + root.binary() + ' ' + sub
    }

    P5Support.DataSource {
        id: executable
        engine: "executable"
        connectedSources: []

        signal exited(string cmd, int exitCode, string stdout, string stderr)

        onNewData: function(sourceName, data) {
            disconnectSource(sourceName)
            exited(sourceName,
                   data["exit code"],
                   data["stdout"],
                   data["stderr"])
        }

        function run(cmd) {
            // reconnecting is what forces a new run
            disconnectSource(cmd)
            connectSource(cmd)
        }
    }

    Connections {
        target: executable
        function onExited(cmd, exitCode, stdout, stderr) {
            if (cmd.indexOf(" usage --json") !== -1) {
                root.applyReport(exitCode, stdout, stderr)
            } else if (cmd.indexOf(" settings show") !== -1) {
                root.applySettings(stdout)
            } else {
                root.loading = false
                root.applyBar(exitCode, stdout, stderr)
            }
        }
    }

    function applyBar(exitCode, stdout, stderr) {
        var raw = (stdout || "").trim()
        if (raw === "") {
            root.lastError = (stderr || "").trim()
                || i18nc("@info:status %1 is the process exit code",
                         "No output (exit code %1). Is the ai-usagebar binary installed?", exitCode)
            root.barHtml = errorBarText()
            root.statusClass = "error"
            return
        }
        try {
            var obj = JSON.parse(raw)
            root.lastError = ""
            root.barHtml = Model.pangoToHtml(obj.text || "")
            root.statusClass = String(obj.class || "")
        } catch (e) {
            root.lastError = i18nc("@info:status %1 is the raw command output",
                                   "Unexpected answer: %1", raw)
            root.barHtml = errorBarText()
            root.statusClass = "error"
        }
    }

    function errorBarText() {
        return '<span style="color:' + Kirigami.Theme.negativeTextColor + '">⚠ ai-usagebar</span>'
    }

    function applyReport(exitCode, stdout, stderr) {
        root.reportLoaded = true
        root.reportFetchedAt = Date.now()
        var raw = (stdout || "").trim()
        if (raw === "") {
            root.usageEntries = []
            root.reportError = (stderr || "").trim()
                || i18nc("@info:status %1 is the process exit code",
                         "`ai-usagebar usage --json` produced no output (exit code %1).", exitCode)
            return
        }
        var report = Model.parseReport(raw)
        if (!report.ok) {
            root.usageEntries = []
            root.reportError = reportFailure(report.reason)
            return
        }
        root.usageEntries = report.entries
        root.reportError = ""
    }

    function reportFailure(reason) {
        switch (reason) {
        case "invalid-json":
            return i18nc("@info:status", "The usage command returned invalid JSON.")
        case "unsupported":
            return i18nc("@info:status", "The usage command returned an unsupported report.")
        default:
            return i18nc("@info:status", "The usage report contained no valid provider entry.")
        }
    }

    // `settings show` only reads configuration (no provider call): it tells us
    // which provider the binary would use by default, which is then pinned to
    // the applet so every later command is explicit about it.
    function applySettings(stdout) {
        if (root.effectiveVendor !== "") {
            return
        }
        var settings = Model.parseSettings(stdout)
        if (settings.ok && settings.primary !== "") {
            plasmoid.configuration.activeVendor = settings.primary
            root.refresh()
        }
    }

    function refresh() {
        executable.run(buildCmd("--json"))
    }

    function refreshReport() {
        executable.run(buildSubCmd("usage --json"))
    }

    // Only refetch when the last report has some age: hovering the panel must
    // not turn into a query against every provider on each pass.
    function refreshReportThrottled() {
        if (Date.now() - root.reportFetchedAt > 20000) {
            root.refreshReport()
        }
    }

    // Providers worth a turn of the wheel: by default only the ones holding
    // credentials, since switching to a keyless provider only yields a "⚠".
    function cycleCandidates() {
        return Model.cycleIds(root.usageEntries, plasmoid.configuration.hideUnconfigured)
    }

    function cycle(forward) {
        if (root.vendorPinned) {
            return
        }
        var ids = root.cycleCandidates()
        if (ids.length === 0) {
            // No report yet (or no provider configured at all): fetch and stop.
            root.refreshReport()
            return
        }
        root.setVendor(Model.stepId(ids, root.effectiveVendor, forward))
    }

    function setVendor(id) {
        if (root.vendorPinned || !id || id === root.effectiveVendor) {
            return
        }
        plasmoid.configuration.activeVendor = id
        root.refresh()
    }

    // ------------------------------------------------------------------
    // Refresh cadence
    // ------------------------------------------------------------------
    Timer {
        id: refreshTimer
        interval: Math.max(5, plasmoid.configuration.interval) * 1000
        running: true
        repeat: true
        triggeredOnStart: true
        onTriggered: root.refresh()
    }

    // While the popup is open, keep the report fresh too.
    Timer {
        id: popupTimer
        interval: Math.max(15, plasmoid.configuration.interval) * 1000
        running: root.expanded
        repeat: true
        onTriggered: root.refreshReport()
    }

    // Countdowns and "updated x ago" only need to tick while something is visible.
    Timer {
        id: clockTimer
        interval: 30000
        running: root.expanded || root.hovered
        repeat: true
        onTriggered: root.now = new Date()
    }

    onExpandedChanged: {
        if (root.expanded) {
            root.now = new Date()
            root.refreshReport()
        }
    }

    Component.onCompleted: {
        // The tooltip needs the report on the very first hover.
        root.refreshReport()
        if (root.effectiveVendor === "") {
            executable.run(buildSubCmd("settings show"))
        }
    }

    // ------------------------------------------------------------------
    // Panel representation
    // ------------------------------------------------------------------
    compactRepresentation: MouseArea {
        id: compactRoot

        readonly property bool vertical: Plasmoid.formFactor === PlasmaCore.Types.Vertical

        Layout.minimumWidth: vertical ? 0 : barLabel.implicitWidth + Kirigami.Units.smallSpacing * 2
        Layout.preferredWidth: Layout.minimumWidth
        Layout.minimumHeight: vertical ? barLabel.implicitHeight + Kirigami.Units.smallSpacing * 2 : 0
        Layout.preferredHeight: Layout.minimumHeight

        acceptedButtons: Qt.LeftButton | Qt.MiddleButton
        hoverEnabled: true

        // The tooltip reads the same report as the popup: renew it on hover.
        onEntered: {
            root.hovered = true
            root.now = new Date()
            root.refreshReportThrottled()
        }
        onExited: root.hovered = false

        onClicked: function(mouse) {
            if (mouse.button === Qt.MiddleButton) {
                root.refresh()
            } else {
                root.expanded = !root.expanded
            }
        }

        onWheel: function(wheel) {
            if (!plasmoid.configuration.scrollCycles) {
                return
            }
            if (wheel.angleDelta.y > 0) {
                root.cycle(true)
            } else if (wheel.angleDelta.y < 0) {
                root.cycle(false)
            }
        }

        PlasmaComponents.Label {
            id: barLabel
            anchors.centerIn: parent
            textFormat: Text.RichText
            text: root.loading && root.barHtml === "" ? "…" : root.barHtml
            font.family: root.barFont
            font.pointSize: Kirigami.Theme.defaultFont.pointSize
            rotation: compactRoot.vertical ? 90 : 0
        }
    }

    // Tooltip: the popup's card, for the panel's provider only.
    toolTipItem: Item {
        implicitWidth: Kirigami.Units.gridUnit * 20
        implicitHeight: root.activeEntry
            ? tooltipCard.implicitHeight
            : tooltipFallback.implicitHeight + Kirigami.Units.largeSpacing * 2

        VendorCard {
            id: tooltipCard
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            visible: root.activeEntry !== null
            flat: true
            entry: root.activeEntry
            now: root.now
        }

        PlasmaComponents.Label {
            id: tooltipFallback
            anchors.centerIn: parent
            width: parent.width - Kirigami.Units.largeSpacing * 2
            visible: root.activeEntry === null
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.WordWrap
            textFormat: Text.PlainText
            text: root.reportError !== "" ? root.reportError
                : root.lastError !== "" ? root.lastError
                : i18nc("@info:status", "Loading…")
        }
    }

    // ------------------------------------------------------------------
    // Popup: every provider, as cards
    // ------------------------------------------------------------------
    fullRepresentation: PlasmaExtras.Representation {
        id: fullRep

        // Providers with no credentials become a quiet footnote instead.
        readonly property var shownEntries: {
            var out = []
            for (var i = 0; i < root.usageEntries.length; ++i) {
                var entry = root.usageEntries[i]
                if (plasmoid.configuration.hideUnconfigured && Model.isUnconfigured(entry)) {
                    continue
                }
                out.push(entry)
            }
            return out
        }
        readonly property int hiddenCount: root.usageEntries.length - shownEntries.length

        Layout.minimumWidth: Kirigami.Units.gridUnit * 20
        Layout.minimumHeight: Kirigami.Units.gridUnit * 16
        Layout.preferredWidth: Kirigami.Units.gridUnit * 24
        Layout.preferredHeight: Kirigami.Units.gridUnit * 26

        collapseMarginsHint: true

        header: PlasmaExtras.PlasmoidHeading {
            RowLayout {
                anchors.fill: parent
                spacing: Kirigami.Units.smallSpacing

                PlasmaExtras.Heading {
                    level: 4
                    text: i18nc("@title popup header", "AI usage")
                    Layout.fillWidth: true
                    elide: Text.ElideRight
                }
                PlasmaComponents.ToolButton {
                    icon.name: "go-previous"
                    text: i18nc("@action:button", "Previous provider")
                    display: PlasmaComponents.AbstractButton.IconOnly
                    visible: !root.vendorPinned && root.cycleCandidates().length > 1
                    onClicked: root.cycle(false)
                    PlasmaComponents.ToolTip.text: text
                    PlasmaComponents.ToolTip.visible: hovered
                    PlasmaComponents.ToolTip.delay: Kirigami.Units.toolTipDelay
                }
                PlasmaComponents.ToolButton {
                    icon.name: "go-next"
                    text: i18nc("@action:button", "Next provider")
                    display: PlasmaComponents.AbstractButton.IconOnly
                    visible: !root.vendorPinned && root.cycleCandidates().length > 1
                    onClicked: root.cycle(true)
                    PlasmaComponents.ToolTip.text: text
                    PlasmaComponents.ToolTip.visible: hovered
                    PlasmaComponents.ToolTip.delay: Kirigami.Units.toolTipDelay
                }
                PlasmaComponents.ToolButton {
                    icon.name: "view-refresh"
                    text: i18nc("@action:button", "Refresh")
                    display: PlasmaComponents.AbstractButton.IconOnly
                    onClicked: {
                        root.refresh()
                        root.refreshReport()
                    }
                    PlasmaComponents.ToolTip.text: text
                    PlasmaComponents.ToolTip.visible: hovered
                    PlasmaComponents.ToolTip.delay: Kirigami.Units.toolTipDelay
                }
            }
        }

        PlasmaComponents.ScrollView {
            id: scroll
            anchors.fill: parent
            contentWidth: availableWidth
            QQC2.ScrollBar.horizontal.policy: QQC2.ScrollBar.AlwaysOff

            ColumnLayout {
                id: cards
                width: scroll.availableWidth
                spacing: Kirigami.Units.smallSpacing

                // The ScrollView reparents this item; margins stand in for padding.
                Item {
                    Layout.preferredHeight: Kirigami.Units.smallSpacing
                }

                Repeater {
                    model: fullRep.shownEntries

                    delegate: VendorCard {
                        id: vendorCard

                        required property var modelData

                        Layout.fillWidth: true
                        Layout.leftMargin: Kirigami.Units.smallSpacing
                        Layout.rightMargin: Kirigami.Units.smallSpacing
                        entry: modelData
                        now: root.now
                        active: root.activeEntry === modelData
                        interactive: !root.vendorPinned
                            && root.cycleCandidates().indexOf(modelData.id) !== -1
                        onActivated: root.setVendor(vendorCard.modelData.id)
                    }
                }

                PlasmaComponents.Label {
                    Layout.fillWidth: true
                    Layout.margins: Kirigami.Units.largeSpacing
                    visible: fullRep.hiddenCount > 0
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.WordWrap
                    textFormat: Text.PlainText
                    opacity: 0.6
                    font: Kirigami.Theme.smallFont
                    text: i18ncp("@info:status %1 is a number of providers",
                                 "%1 provider without credentials is hidden",
                                 "%1 providers without credentials are hidden",
                                 fullRep.hiddenCount)
                }

                PlasmaExtras.PlaceholderMessage {
                    Layout.fillWidth: true
                    Layout.margins: Kirigami.Units.largeSpacing
                    visible: root.reportError !== ""
                    iconName: "dialog-error"
                    text: i18nc("@info:placeholder", "Could not read the usage report")
                    explanation: root.reportError
                }

                PlasmaComponents.Label {
                    Layout.fillWidth: true
                    Layout.margins: Kirigami.Units.largeSpacing
                    visible: !root.reportLoaded
                    horizontalAlignment: Text.AlignHCenter
                    textFormat: Text.PlainText
                    opacity: 0.6
                    text: i18nc("@info:status", "Loading…")
                }

                Item {
                    Layout.preferredHeight: Kirigami.Units.smallSpacing
                }
            }
        }

        Component.onCompleted: root.refreshReport()
    }

    Plasmoid.contextualActions: [
        PlasmaCore.Action {
            text: i18nc("@action", "Refresh now")
            icon.name: "view-refresh"
            onTriggered: {
                root.refresh()
                root.refreshReport()
            }
        },
        PlasmaCore.Action {
            text: i18nc("@action", "Next provider")
            icon.name: "go-next"
            onTriggered: root.cycle(true)
        },
        PlasmaCore.Action {
            text: i18nc("@action", "Previous provider")
            icon.name: "go-previous"
            onTriggered: root.cycle(false)
        }
    ]
}
