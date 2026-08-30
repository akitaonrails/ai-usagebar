import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as Controls
import org.kde.kirigami as Kirigami
import org.kde.plasma.plasmoid
import org.kde.plasma.plasma5support as Plasma5Support

PlasmoidItem {
    id: root

    preferredRepresentation: fullRepresentation

    property string binaryPath: ""
    property int refreshIntervalMs: 300000
    property var commandIndex: ({})

    // Since v0.17.0 ai-usagebar registers the cross-vendor aliases for every
    // binary vendor, so one template covers all cards — no more {oai_*}/{zai_*}.
    readonly property string usageFormat: "{session_pct}% · {session_reset}|{weekly_pct}% · {weekly_reset}"

    // A card counts as stale once it has missed two refresh cycles.
    readonly property int staleAfterMs: refreshIntervalMs * 2

    property color panelColorTop: Qt.rgba(0.12, 0.13, 0.16, 0.96)
    property color panelColorBottom: Qt.rgba(0.08, 0.09, 0.11, 0.96)
    property color cardColorTop: Qt.rgba(0.10, 0.11, 0.135, 0.92)
    property color cardColorBottom: Qt.rgba(0.065, 0.07, 0.09, 0.92)
    property color cardHoverColorTop: Qt.rgba(0.14, 0.15, 0.18, 0.96)
    property color cardHoverColorBottom: Qt.rgba(0.09, 0.10, 0.125, 0.96)
    property color trackColor: Qt.rgba(1.0, 1.0, 1.0, 0.09)
    property color mutedColor: Qt.rgba(1.0, 1.0, 1.0, 0.28)

    function shellQuote(value) {
        return "'" + String(value).replace(/'/g, "'\"'\"'") + "'"
    }

    function commandFor(index) {
        const item = vendorModel.get(index)
        const commandArguments = " --json --vendor " + item.vendor
            + " --format " + shellQuote(root.usageFormat)

        if (binaryPath.trim().length > 0) {
            return shellQuote(binaryPath) + commandArguments
        }

        const candidates = [
            "$HOME/.local/bin/ai-usagebar",
            "$HOME/.cargo/bin/ai-usagebar",
            "/usr/local/bin/ai-usagebar",
            "/usr/bin/ai-usagebar"
        ]
        const notFound = JSON.stringify({
            text: "ai-usagebar not found",
            tooltip: "Install ai-usagebar, then refresh this widget.",
            class: "critical"
        })

        return "AI_USAGEBAR=$(command -v ai-usagebar 2>/dev/null || true); "
            + "if [ -z \"$AI_USAGEBAR\" ]; then "
            + "for candidate in " + candidates.join(" ") + "; do "
            + "if [ -x \"$candidate\" ]; then AI_USAGEBAR=\"$candidate\"; break; fi; "
            + "done; fi; "
            + "if [ -z \"$AI_USAGEBAR\" ]; then printf '%s\\n' " + shellQuote(notFound)
            + "; else \"$AI_USAGEBAR\"" + commandArguments + "; fi"
    }

    function stripMarkup(value) {
        return String(value || "")
        .replace(/<[^>]*>/g, "")
        .replace(/&nbsp;/g, " ")
        .replace(/&lt;/g, "<")
        .replace(/&gt;/g, ">")
        .replace(/&amp;/g, "&")
        .replace(/\s+/g, " ")
        .trim()
    }

    // A window the vendor did not report comes back as an empty segment — e.g.
    // OpenAI dropped the 5h window in July 2026 and now sends the 7d window
    // alone. Returning "" (rather than a fabricated 0%) lets the card render a
    // dash, and the row lights up again by itself once the window returns.
    function parseMetric(value) {
        const cleaned = stripMarkup(value)

        if (cleaned.length === 0 || !/[0-9]/.test(cleaned)) {
            return ""
        }

        return cleaned
    }

    function segment(value, index) {
        const parts = String(value || "").split("|")
        return parts.length > index ? parts[index] : ""
    }

    function percentFrom(value) {
        const match = String(value || "").match(/(\d+(?:\.\d+)?)\s*%/)

        if (!match) {
            return 0
        }

        const parsed = Number(match[1])

        if (isNaN(parsed)) {
            return 0
        }

        return Math.max(0, Math.min(100, parsed))
    }

    function percentText(value) {
        const match = String(value || "").match(/(\d+(?:\.\d+)?)\s*%/)
        return match ? match[1] + "%" : "—"
    }

    function resetText(value) {
        const cleaned = stripMarkup(value)
        const reset = cleaned.replace(/^\d+(?:\.\d+)?\s*%\s*(?:·\s*)?/, "")
        return reset.length > 0 ? reset : cleaned
    }

    function strongestPercent(sessionText, weeklyText) {
        return Math.max(percentFrom(sessionText), percentFrom(weeklyText))
    }

    function colorForPercent(percent) {
        if (percent >= 90) {
            return "#ff5f6d"
        }

        if (percent >= 70) {
            return "#e5c07b"
        }

        if (percent >= 45) {
            return "#d8c86a"
        }

        return "#98c379"
    }

    function lighten(hex, amount) {
        const c = Qt.color(hex)
        return Qt.rgba(
            Math.min(1, c.r + amount),
            Math.min(1, c.g + amount),
            Math.min(1, c.b + amount),
            c.a
        )
    }

    function refreshVendor(index) {
        const command = commandFor(index)

        commandIndex[command] = index
        vendorModel.setProperty(index, "loading", true)
        executable.exec(command)
    }

    function refreshAll() {
        for (let i = 0; i < vendorModel.count; i++) {
            refreshVendor(i)
        }
    }

    // A vendor with no credentials configured is a setup step, not a failure —
    // it gets a muted card instead of a red one.
    function looksUnconfigured(tooltipText) {
        return /no api key|credentials error|no credentials|not configured|missing (api )?key|not signed in|no cached credentials/i.test(tooltipText)
    }

    ListModel {
        id: vendorModel

        ListElement {
            vendor: "anthropic"
            shortName: "CLD"
            label: "Claude"
            accentColor: "#d97757"
            sessionLabel: "5h"
            weeklyLabel: "7d"
            sessionText: ""
            weeklyText: ""
            detail: ""
            updatedAt: ""
            updatedAtMs: 0.0
            loading: false
            cardState: "loading"
        }

        ListElement {
            vendor: "openai"
            shortName: "GPT"
            label: "OpenAI"
            accentColor: "#74aa9c"
            sessionLabel: "5h"
            weeklyLabel: "7d"
            sessionText: ""
            weeklyText: ""
            detail: ""
            updatedAt: ""
            updatedAtMs: 0.0
            loading: false
            cardState: "loading"
        }

        ListElement {
            vendor: "zai"
            shortName: "ZAI"
            label: "Z.AI"
            accentColor: "#9c7cf4"
            sessionLabel: "5h"
            weeklyLabel: "7d"
            sessionText: ""
            weeklyText: ""
            detail: ""
            updatedAt: ""
            updatedAtMs: 0.0
            loading: false
            cardState: "loading"
        }

        ListElement {
            vendor: "kimi"
            shortName: "KMI"
            label: "Kimi"
            accentColor: "#4d6bfe"
            sessionLabel: "5h"
            weeklyLabel: "7d"
            sessionText: ""
            weeklyText: ""
            detail: ""
            updatedAt: ""
            updatedAtMs: 0.0
            loading: false
            cardState: "loading"
        }

        ListElement {
            vendor: "supergrok"
            shortName: "SGR"
            label: "SuperGrok"
            accentColor: "#9aa0a6"
            sessionLabel: "5h"
            weeklyLabel: "7d"
            sessionText: ""
            weeklyText: ""
            detail: ""
            updatedAt: ""
            updatedAtMs: 0.0
            loading: false
            cardState: "loading"
        }
    }

    Plasma5Support.DataSource {
        id: executable
        engine: "executable"
        connectedSources: []

        function exec(command) {
            disconnectSource(command)
            connectSource(command)
        }

        onNewData: function(sourceName, data) {
            const index = root.commandIndex[sourceName]

            disconnectSource(sourceName)

            if (index === undefined) {
                return
            }

            const stdout = data["stdout"] || ""
            const stderr = data["stderr"] || ""
            const now = new Date()

            vendorModel.setProperty(index, "loading", false)
            vendorModel.setProperty(index, "updatedAt", now.toLocaleTimeString(Qt.locale(), "HH:mm"))
            vendorModel.setProperty(index, "updatedAtMs", now.getTime())

            let parsed = null

            try {
                parsed = JSON.parse(stdout.trim())
            } catch (error) {
                vendorModel.setProperty(index, "sessionText", "")
                vendorModel.setProperty(index, "weeklyText", "")
                vendorModel.setProperty(index, "detail", root.stripMarkup(stderr || stdout) || String(error))
                vendorModel.setProperty(index, "cardState", "error")
                return
            }

            const text = root.stripMarkup(parsed.text || "")
            const tooltip = root.stripMarkup(parsed.tooltip || "")
            const severity = String(parsed["class"] || "").toLowerCase()

            // The binary always exits 0 and signals failure in-band: text
            // collapses to a warning glyph with class=critical, and the tooltip
            // carries the actionable message.
            if (severity === "critical" && !/[0-9]/.test(text)) {
                vendorModel.setProperty(index, "sessionText", "")
                vendorModel.setProperty(index, "weeklyText", "")
                vendorModel.setProperty(index, "detail", tooltip || text)
                vendorModel.setProperty(index, "cardState", root.looksUnconfigured(tooltip) ? "unconfigured" : "error")
                return
            }

            vendorModel.setProperty(index, "sessionText", root.parseMetric(root.segment(text, 0)))
            vendorModel.setProperty(index, "weeklyText", root.parseMetric(root.segment(text, 1)))
            vendorModel.setProperty(index, "detail", tooltip)
            vendorModel.setProperty(index, "cardState", "ok")
        }
    }

    Timer {
        id: refreshTimer
        interval: root.refreshIntervalMs
        repeat: true
        running: true
        triggeredOnStart: true
        onTriggered: root.refreshAll()
    }

    // Ticks the clock the staleness badge reads, so an ageing card visibly
    // greys out even while no fetch is running.
    Timer {
        id: clock
        property double nowMs: Date.now()
        interval: 30000
        repeat: true
        running: true
        onTriggered: nowMs = Date.now()
    }

    // Opening the popup after a suspend/resume should not show minutes-old
    // numbers while waiting for the next tick.
    // Qualified as root.expanded: bare `expanded` resolves to the signal's
    // injected parameter, which Qt 6 deprecates.
    onExpandedChanged: {
        if (root.expanded) {
            root.refreshAll()
        }
    }

    fullRepresentation: Rectangle {
        Layout.minimumWidth: 400
        Layout.minimumHeight: 380
        Layout.preferredWidth: 460
        Layout.preferredHeight: 470

        radius: 18
        border.width: 1
        border.color: Qt.rgba(1.0, 1.0, 1.0, 0.14)

        gradient: Gradient {
            orientation: Gradient.Vertical
            GradientStop { position: 0.0; color: root.panelColorTop }
            GradientStop { position: 1.0; color: root.panelColorBottom }
        }

        ColumnLayout {
            anchors.fill: parent
            anchors.margins: Kirigami.Units.largeSpacing + 2
            spacing: Kirigami.Units.largeSpacing

            RowLayout {
                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing

                Rectangle {
                    Layout.preferredWidth: 34
                    Layout.preferredHeight: 34
                    radius: 10
                    gradient: Gradient {
                        GradientStop { position: 0.0; color: "#8a7cf4" }
                        GradientStop { position: 1.0; color: "#5d5bd6" }
                    }

                    Kirigami.Icon {
                        anchors.centerIn: parent
                        width: 18
                        height: 18
                        source: "view-statistics"
                        color: "white"
                        isMask: true
                    }
                }

                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: 1

                    Kirigami.Heading {
                        text: "AI Usage"
                        level: 2
                        Layout.fillWidth: true
                    }

                    Controls.Label {
                        text: "Session and weekly rate limits"
                        opacity: 0.60
                        font.pixelSize: 12
                    }
                }

                Controls.ToolButton {
                    icon.name: "view-refresh"
                    text: "Refresh"
                    display: Controls.AbstractButton.IconOnly
                    onClicked: root.refreshAll()
                }
            }

            Repeater {
                model: vendorModel

                delegate: Rectangle {
                    id: card

                    readonly property bool isOk: cardState === "ok"
                    readonly property bool isUnconfigured: cardState === "unconfigured"
                    readonly property bool isError: cardState === "error"
                    readonly property bool hasSession: isOk && String(sessionText).length > 0
                    readonly property bool hasWeekly: isOk && String(weeklyText).length > 0
                    readonly property bool isStale: isOk && updatedAtMs > 0
                        && (clock.nowMs - updatedAtMs) > root.staleAfterMs

                    readonly property color severityColor: {
                        if (isError) {
                            return "#ff5f6d"
                        }

                        if (isUnconfigured || !isOk) {
                            return root.mutedColor
                        }

                        return root.colorForPercent(root.strongestPercent(sessionText, weeklyText))
                    }

                    Layout.fillWidth: true
                    Layout.preferredHeight: content.implicitHeight + (Kirigami.Units.smallSpacing + 1) * 2

                    radius: 16
                    border.width: 1
                    border.color: Qt.rgba(severityColor.r, severityColor.g, severityColor.b, isOk || isError ? 0.55 : 0.30)
                    opacity: isOk ? 1.0 : 0.85

                    gradient: Gradient {
                        orientation: Gradient.Vertical
                        GradientStop { position: 0.0; color: hoverArea.containsMouse ? root.cardHoverColorTop : root.cardColorTop }
                        GradientStop { position: 1.0; color: hoverArea.containsMouse ? root.cardHoverColorBottom : root.cardColorBottom }
                    }

                    Behavior on border.color {
                        ColorAnimation { duration: 250 }
                    }

                    Controls.ToolTip.visible: hoverArea.containsMouse && String(detail).length > 0
                    Controls.ToolTip.text: detail
                    Controls.ToolTip.delay: 450

                    ColumnLayout {
                        id: content

                        // Deliberately not anchors.fill: the card's height is
                        // derived from this layout, so filling the parent would
                        // close a binding loop.
                        anchors.left: parent.left
                        anchors.right: parent.right
                        anchors.top: parent.top
                        anchors.leftMargin: Kirigami.Units.largeSpacing + 2
                        anchors.rightMargin: Kirigami.Units.smallSpacing + 2
                        anchors.topMargin: Kirigami.Units.smallSpacing + 1
                        spacing: Kirigami.Units.smallSpacing - 1

                        RowLayout {
                            Layout.fillWidth: true
                            spacing: Kirigami.Units.smallSpacing

                            Rectangle {
                                Layout.preferredWidth: 42
                                Layout.preferredHeight: 24
                                radius: 8
                                color: Qt.rgba(Qt.color(accentColor).r, Qt.color(accentColor).g, Qt.color(accentColor).b, card.isOk ? 0.22 : 0.10)
                                border.width: 1
                                border.color: Qt.rgba(Qt.color(accentColor).r, Qt.color(accentColor).g, Qt.color(accentColor).b, card.isOk ? 0.55 : 0.28)

                                Controls.Label {
                                    anchors.centerIn: parent
                                    text: shortName
                                    font.bold: true
                                    font.pixelSize: 11
                                    color: root.lighten(accentColor, 0.18)
                                    opacity: card.isOk ? 1.0 : 0.65
                                }
                            }

                            Controls.Label {
                                text: label
                                font.bold: true
                                font.pixelSize: 13
                                Layout.fillWidth: true
                            }

                            Controls.Label {
                                text: {
                                    if (loading) {
                                        return "updating…"
                                    }

                                    if (card.isStale) {
                                        return updatedAt + " · stale"
                                    }

                                    return updatedAt
                                }
                                color: card.isStale ? "#e5c07b" : Kirigami.Theme.textColor
                                opacity: loading ? 0.90 : (card.isStale ? 0.85 : 0.50)
                                font.pixelSize: 11
                                font.italic: loading
                            }
                        }

                        // Credential and fetch failures replace the gauges
                        // entirely — an empty bar would read as "0% used".
                        Controls.Label {
                            visible: !card.isOk
                            Layout.fillWidth: true
                            Layout.topMargin: 2
                            text: {
                                if (card.isUnconfigured) {
                                    return "Not configured — add credentials for " + label + "."
                                }

                                if (card.isError) {
                                    return String(detail).length > 0 ? detail : "Fetch failed."
                                }

                                return "Loading…"
                            }
                            color: card.isError ? "#ff8f97" : Kirigami.Theme.textColor
                            opacity: card.isError ? 0.95 : 0.55
                            font.pixelSize: 11
                            wrapMode: Text.WordWrap
                            maximumLineCount: 2
                            elide: Text.ElideRight
                        }

                        RowLayout {
                            visible: card.isOk
                            Layout.fillWidth: true
                            spacing: Kirigami.Units.smallSpacing

                            Controls.Label {
                                text: sessionLabel
                                Layout.preferredWidth: 26
                                opacity: card.hasSession ? 0.62 : 0.35
                                font.pixelSize: 11
                                font.bold: true
                            }

                            Rectangle {
                                Layout.fillWidth: true
                                Layout.preferredHeight: 8
                                radius: 4
                                color: root.trackColor
                                clip: true

                                Rectangle {
                                    visible: card.hasSession
                                    anchors.left: parent.left
                                    anchors.top: parent.top
                                    anchors.bottom: parent.bottom
                                    width: parent.width * root.percentFrom(sessionText) / 100
                                    radius: 4

                                    gradient: Gradient {
                                        orientation: Gradient.Horizontal
                                        GradientStop { position: 0.0; color: root.colorForPercent(root.percentFrom(sessionText)) }
                                        GradientStop { position: 1.0; color: root.lighten(root.colorForPercent(root.percentFrom(sessionText)), 0.10) }
                                    }

                                    Behavior on width {
                                        NumberAnimation {
                                            duration: 350
                                            easing.type: Easing.OutCubic
                                        }
                                    }
                                }
                            }

                            Controls.Label {
                                text: card.hasSession ? root.percentText(sessionText) : "—"
                                Layout.preferredWidth: 42
                                horizontalAlignment: Text.AlignRight
                                font.pixelSize: 11
                                font.bold: true
                                opacity: card.hasSession ? 1.0 : 0.40
                            }

                            Controls.Label {
                                text: card.hasSession ? root.resetText(sessionText) : "not reported"
                                Layout.preferredWidth: 72
                                horizontalAlignment: Text.AlignRight
                                elide: Text.ElideRight
                                font.pixelSize: 11
                                opacity: card.hasSession ? 0.62 : 0.35
                            }
                        }

                        RowLayout {
                            visible: card.isOk
                            Layout.fillWidth: true
                            spacing: Kirigami.Units.smallSpacing

                            Controls.Label {
                                text: weeklyLabel
                                Layout.preferredWidth: 26
                                opacity: card.hasWeekly ? 0.62 : 0.35
                                font.pixelSize: 11
                                font.bold: true
                            }

                            Rectangle {
                                Layout.fillWidth: true
                                Layout.preferredHeight: 8
                                radius: 4
                                color: root.trackColor
                                clip: true

                                Rectangle {
                                    visible: card.hasWeekly
                                    anchors.left: parent.left
                                    anchors.top: parent.top
                                    anchors.bottom: parent.bottom
                                    width: parent.width * root.percentFrom(weeklyText) / 100
                                    radius: 4

                                    gradient: Gradient {
                                        orientation: Gradient.Horizontal
                                        GradientStop { position: 0.0; color: root.colorForPercent(root.percentFrom(weeklyText)) }
                                        GradientStop { position: 1.0; color: root.lighten(root.colorForPercent(root.percentFrom(weeklyText)), 0.10) }
                                    }

                                    Behavior on width {
                                        NumberAnimation {
                                            duration: 350
                                            easing.type: Easing.OutCubic
                                        }
                                    }
                                }
                            }

                            Controls.Label {
                                text: card.hasWeekly ? root.percentText(weeklyText) : "—"
                                Layout.preferredWidth: 42
                                horizontalAlignment: Text.AlignRight
                                font.pixelSize: 11
                                font.bold: true
                                opacity: card.hasWeekly ? 1.0 : 0.40
                            }

                            Controls.Label {
                                text: card.hasWeekly ? root.resetText(weeklyText) : "not reported"
                                Layout.preferredWidth: 72
                                horizontalAlignment: Text.AlignRight
                                elide: Text.ElideRight
                                font.pixelSize: 11
                                opacity: card.hasWeekly ? 0.62 : 0.35
                            }
                        }
                    }

                    MouseArea {
                        id: hoverArea
                        anchors.fill: parent
                        hoverEnabled: true
                        onClicked: root.refreshVendor(index)
                    }
                }
            }

            Item {
                Layout.fillHeight: true
            }
        }
    }
}
