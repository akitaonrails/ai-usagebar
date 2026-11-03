import QtQuick
import QtQuick.Window
import Quickshell
import qs.Commons as Commons
import "Usage"
import "Usage/Notice.js" as Notice
import "Usage/I18n.js" as I18n
import "Usage/Priority.js" as Priority

// Deterministic, offscreen visual scenarios for the real Quattro settings
// component, the usage view's status notices and the priority cards.
// scripts/omarchy-visual-test.sh supplies the shell modules and
// output directory, then launches this config with Qt's offscreen backend.
ShellRoot {
  id: root

  readonly property string outputDir: Quickshell.env("AI_USAGEBAR_VISUAL_OUTPUT_DIR")
  property int scenario: 0
  property color scenarioBackground: "#1e1e2e"
  readonly property var scenarioNames: [
    "settings-display-dark.png", "settings-credentials-light.png",
    "notices-dark.png", "notices-light-pt-BR.png",
    "settings-priorities-dark.png", "priorities-dark.png", "priorities-light-pt-BR.png"
  ]
  readonly property bool showNotices: scenario === 2 || scenario === 3
  readonly property bool showPriorities: scenario >= 5
  readonly property bool showSettings: !showNotices && !showPriorities
  property string priorityLocale: "en"
  // One row per state the ranking can report, as `usage --json --priorities`
  // emits them, against a fixed clock.
  readonly property double priorityNow: Date.parse("2026-10-21T12:00:00Z")
  readonly property var priorityFixtures: [
    { entry_id: "openai", name: "Codex", status: "priority", reason_code: "priority", urgent: true,
      window: "Weekly (7d)", reset_at: "2026-10-22T03:00:00Z", unused_percent: 58, remaining_percent: 71,
      pace_percent_per_hour: 0.9, recent_percent_per_hour: 0.4, recent_hours: 3, cycle_percent_per_hour: 1.1,
      required_percent_per_hour: 4.7, alert_unused_percent: 50, alert_hours: 24 },
    { entry_id: "anthropic", name: "Claude", status: "priority", reason_code: "priority", urgent: false,
      window: "Weekly (7d)", reset_at: "2026-10-24T09:00:00Z", unused_percent: 22, remaining_percent: 64,
      pace_percent_per_hour: 0.6, recent_percent_per_hour: 0.8, recent_hours: 3, cycle_percent_per_hour: 0.5,
      required_percent_per_hour: 0.9, alert_unused_percent: 50, alert_hours: 24, session_exhausted: true },
    { entry_id: "zai", name: "Z.AI", status: "on_track", reason_code: "on_track", urgent: false,
      window: "Weekly", reset_at: "2026-10-25T00:00:00Z", unused_percent: 0, remaining_percent: 38,
      pace_percent_per_hour: 0.5, recent_percent_per_hour: null, recent_hours: 0, cycle_percent_per_hour: 0.5,
      required_percent_per_hour: 0.45, alert_unused_percent: 50, alert_hours: 24 },
    { entry_id: "kimi", name: "Kimi", status: "blocked", reason_code: "blocked", urgent: false },
    { entry_id: "opencode-go", name: "OpenCode Go", status: "unavailable", reason_code: "unavailable", urgent: false }
  ]
  function priorityTr(key, params) { return I18n.t(priorityLocale, key, params) }
  property string noticeLocale: "en"
  // The same messages the report produces: a sign-in error naming its fix, a
  // transient failure that offers a retry, and a cached-data note.
  readonly property var noticeFixtures: [
    Notice.build("credentials error: OpenCode Go is not signed in. Run `opencode auth login` and choose OpenCode, or set OPENCODE_GO_API_KEY.", "error"),
    Notice.build("HTTP error: 503 Service Unavailable", "error"),
    Notice.build(I18n.t(noticeLocale, "status.cached"), "info", "cached")
  ]

  function applyTheme(foreground, background, accent, urgent, muted) {
    // FileView may finish loading the user's current theme after startup.
    // Reapply the fixture immediately before each capture so output is stable
    // and the test never depends on or mutates personal theme files.
    Commons.Color.themeShellValues = ({})
    Commons.Color.userShellValues = ({})
    Commons.Color.mergeShell()
    Commons.Color.foreground = foreground
    Commons.Color.background = background
    Commons.Color.accent = accent
    Commons.Color.urgent = urgent
    Commons.Color.muted = muted
    Commons.Style.cornerRadius = 8
  }

  function focusFirst(item) {
    if (!item) return false
    if (item.activeFocusOnTab === true && item.visible && item.enabled) {
      item.forceActiveFocus()
      return true
    }
    var children = item.children || []
    for (var i = 0; i < children.length; i++) {
      if (focusFirst(children[i])) return true
    }
    return false
  }

  function prepareScenario() {
    if (scenario === 0) {
      applyTheme("#cdd6f4", "#1e1e2e", "#89b4fa", "#f38ba8", "#6c7086")
      scenarioBackground = "#1e1e2e"
      settings.foreground = "#cdd6f4"
      settings.urgent = "#f38ba8"
      settings.openSection = "display"
      viewport.contentY = 0
    } else if (scenario === 1) {
      applyTheme("#1f2328", "#f6f8fa", "#0969da", "#cf222e", "#656d76")
      scenarioBackground = "#f6f8fa"
      settings.foreground = "#1f2328"
      settings.urgent = "#cf222e"
      settings.openSection = "credentials"
      viewport.contentY = 0
    } else if (scenario === 2) {
      applyTheme("#cdd6f4", "#1e1e2e", "#89b4fa", "#f38ba8", "#6c7086")
      scenarioBackground = "#1e1e2e"
      noticeLocale = "en"
      viewport.contentY = 0
    } else if (scenario === 3) {
      applyTheme("#1f2328", "#f6f8fa", "#0969da", "#cf222e", "#656d76")
      scenarioBackground = "#f6f8fa"
      noticeLocale = "pt-BR"
      viewport.contentY = 0
    } else if (scenario === 4) {
      applyTheme("#cdd6f4", "#1e1e2e", "#89b4fa", "#f38ba8", "#6c7086")
      scenarioBackground = "#1e1e2e"
      settings.foreground = "#cdd6f4"
      settings.urgent = "#f38ba8"
      settings.showPriorities = true
      settings.openSection = "priorities"
      viewport.contentY = 0
    } else if (scenario === 5) {
      applyTheme("#cdd6f4", "#1e1e2e", "#89b4fa", "#f38ba8", "#6c7086")
      scenarioBackground = "#1e1e2e"
      priorityLocale = "en"
      viewport.contentY = 0
    } else {
      applyTheme("#1f2328", "#f6f8fa", "#0969da", "#cf222e", "#656d76")
      scenarioBackground = "#f6f8fa"
      priorityLocale = "pt-BR"
      viewport.contentY = 0
    }
    if (showSettings) {
      settings.forceActiveFocus()
      Qt.callLater(function() { root.focusFirst(settings) })
    }
    captureDelay.restart()
  }

  function capture() {
    var name = scenarioNames[scenario]
    window.contentItem.grabToImage(function(result) {
      if (!result.saveToFile(outputDir + "/" + name)) {
        console.error("VISUAL SCENARIO FAIL: could not write " + name)
        Qt.exit(1)
        return
      }
      console.log("VISUAL SCENARIO OK: " + name)
      scenario++
      if (scenario < scenarioNames.length) prepareScenario()
      else Qt.quit()
    }, Qt.size(window.width, window.height))
  }

  Window {
    id: window
    width: 560
    height: 760
    visible: true
    title: "AI Usage visual scenarios"

    Rectangle {
      anchors.fill: parent
      color: root.scenarioBackground
    }

    Flickable {
      id: viewport
      anchors.fill: parent
      anchors.margins: 24
      clip: true
      contentWidth: width
      contentHeight: root.showNotices ? notices.implicitHeight
        : root.showPriorities ? priorities.implicitHeight : settings.implicitHeight

      Column {
        id: notices
        visible: root.showNotices
        width: viewport.width
        spacing: 12

        Repeater {
          model: root.noticeFixtures

          NoticeCard {
            required property var modelData
            width: notices.width
            notice: modelData
            title: I18n.t(root.noticeLocale, "notice." + modelData.category)
            copyText: I18n.t(root.noticeLocale, "notice.copy")
            copiedText: I18n.t(root.noticeLocale, "notice.copied")
            retryText: I18n.t(root.noticeLocale, "notice.retry")
            foreground: Commons.Color.foreground
            urgentColor: Commons.Color.urgent
            fontFamily: "monospace"
          }
        }
      }

      // The first card is pinned open, as a click leaves it; the others show
      // their compact form.
      Column {
        id: priorities
        visible: root.showPriorities
        width: viewport.width
        spacing: 8

        Repeater {
          model: root.priorityFixtures

          PriorityCard {
            required property var modelData
            required property int index
            readonly property var shown: Object.assign({}, modelData,
              { window: I18n.displayLabel(root.priorityLocale, modelData.window) })
            width: priorities.width
            row: modelData
            rank: index + 1
            pinned: index === 0
            summary: Priority.summary(modelData, root.priorityNow, root.priorityTr)
            facts: Priority.facts(shown, root.priorityNow, true, root.priorityTr)
            notes: Priority.notes(modelData, root.priorityNow, root.priorityTr)
            statusText: root.priorityTr("priority." + Priority.stateKey(modelData))
            limitsText: root.priorityTr("priority.view_limits")
            urgent: Priority.stateKey(modelData) === "urgent"
            urgentColor: Commons.Color.urgent
            foreground: Commons.Color.foreground
            fontFamily: "monospace"
          }
        }
      }

      SettingsView {
        id: settings
        visible: root.showSettings
        width: viewport.width
        fontFamily: "monospace"
        showValue: true
        showProvider: true
        showAll: false
        colorCodeUsage: true
        brandIcons: true
        selectedPrimary: "anthropic"
        barProviderChoices: [
          { id: "anthropic", name: "Claude", checked: true },
          { id: "openai", name: "Codex", checked: true },
          { id: "openrouter", name: "OpenRouter", checked: false }
        ]
        metricEntries: [
          { id: "anthropic", name: "Claude", rows: [
            { key: "window:session", label: "Session", labelKey: "", group: "", checked: true, canToggle: true },
            { key: "window:weekly", label: "Weekly", labelKey: "", group: "", checked: true, canToggle: true }
          ] }
        ]
        snapshot: ({
          primary_choices: [
            { id: "anthropic", value: "anthropic", label: "Claude" },
            { id: "openai", value: "openai", label: "Codex" },
            { id: "openrouter", value: "openrouter", label: "OpenRouter" }
          ],
          vendors: [
            { id: "anthropic", label: "Claude", enabled: true },
            { id: "openai", label: "Codex", enabled: true },
            { id: "openrouter", label: "OpenRouter", enabled: false }
          ],
          keys: [
            { id: "openrouter", label: "OpenRouter", environment: "OPENROUTER_API_KEY",
              secret_label: "API key", note: "Optional explicit credential",
              configured: true, inline_configured: true, environment_configured: false },
            { id: "zai", label: "Z.AI", environment: "ZAI_API_KEY",
              secret_label: "API key", note: "Usage API access",
              configured: false, inline_configured: false, environment_configured: false }
          ]
        })
      }
    }
  }

  Timer {
    id: startup
    interval: 150
    running: true
    repeat: false
    onTriggered: root.prepareScenario()
  }

  Timer {
    id: captureDelay
    interval: 180
    repeat: false
    onTriggered: root.capture()
  }

  Timer {
    interval: 10000
    running: true
    repeat: false
    onTriggered: {
      console.error("VISUAL SCENARIO FAIL: timed out")
      Qt.exit(1)
    }
  }
}
