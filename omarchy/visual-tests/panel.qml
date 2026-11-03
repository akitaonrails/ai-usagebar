import QtQuick
import QtQuick.Window
import Quickshell
import qs.Commons as Commons
import "Usage"

// Offscreen scenarios for the whole usage panel: the plugin's Panel.qml, fed
// by fake-ai-usagebar (a synthetic report: made-up providers and figures)
// and drawn inside the KeyboardPanel stand-in. scripts/omarchy-visual-test.sh
// supplies the modules, the stub binaries and the output directory.
ShellRoot {
  id: root

  readonly property string outputDir: Quickshell.env("AI_USAGEBAR_VISUAL_OUTPUT_DIR")
  readonly property string binDir: Quickshell.env("AI_USAGEBAR_VISUAL_BIN_DIR")
  readonly property var dark: ["#cdd6f4", "#1e1e2e", "#89b4fa", "#f38ba8", "#6c7086"]
  readonly property var light: ["#1f2328", "#f6f8fa", "#0969da", "#cf222e", "#656d76"]
  // The usage views keep the bar alert off so the switcher shows its usual
  // icons; the priorities view has an urgent quota. `weekly` is Claude's
  // weekly quota in the report.
  readonly property var scenarios: [
    { name: "panel-dark.png", theme: dark, weekly: 69, entry: "anthropic", settings: { priorityAlert: false } },
    { name: "panel-names-dark.png", theme: dark, weekly: 69, entry: "anthropic", settings: { priorityAlert: false, tabNames: true } },
    { name: "panel-light-pt-BR.png", theme: light, weekly: 69, entry: "openai", settings: { priorityAlert: false, uiLocale: "pt-BR", showAs: "left" } },
    { name: "panel-near-limit-dark.png", theme: dark, weekly: 94, entry: "anthropic", settings: { priorityAlert: false } },
    { name: "panel-exhausted-dark.png", theme: dark, weekly: 100, entry: "anthropic", settings: { priorityAlert: false } },
    { name: "panel-exhausted-colors-dark.png", theme: dark, weekly: 100, entry: "anthropic", settings: { priorityAlert: false, colorCodeUsage: true } },
    { name: "panel-balance-dark.png", theme: dark, weekly: 69, entry: "openrouter", settings: { priorityAlert: false } },
    { name: "panel-priorities-dark.png", theme: dark, weekly: 69, entry: "anthropic", settings: {}, priorities: true }
  ]
  // A fixed colors.toml, so colour-coded rows never depend on the user's theme.
  readonly property string palette: "green = \"#a6e3a1\"\nyellow = \"#f9e2af\"\norange = \"#fab387\"\nred = \"#f38ba8\"\n"
  property int scenario: 0

  function binary(weekly) { return binDir + "/ai-usagebar-" + weekly }

  function applyTheme(theme) {
    Commons.Color.themeShellValues = ({})
    Commons.Color.userShellValues = ({})
    Commons.Color.mergeShell()
    Commons.Color.foreground = theme[0]
    Commons.Color.background = theme[1]
    Commons.Color.accent = theme[2]
    Commons.Color.urgent = theme[3]
    Commons.Color.muted = theme[4]
    Commons.Color.popups.background = theme[1]
    Commons.Color.popups.border = theme[2]
    Commons.Color.popups.text = theme[0]
    Commons.Style.cornerRadius = 8
    fakeBar.foreground = theme[0]
    fakeBar.urgent = theme[3]
  }

  function prepare() {
    var s = scenarios[scenario]
    applyTheme(s.theme)
    var settings = { reportBinary: binary(s.weekly), showPriorities: true, uiLocale: "en", lastSelectedEntryId: s.entry }
    for (var key in s.settings) settings[key] = s.settings[key]
    panel.priorityOpen = false
    panel.settings = settings
    panel.refresh()
    settle.restart()
  }

  function standIn() {
    var kids = panel.children
    for (var i = 0; i < kids.length; i++)
      if (kids[i].visualStandIn === true) return kids[i]
    return null
  }

  function capture() {
    var s = scenarios[scenario]
    if (s.priorities) panel.openPriorities()
    // The panel reloads the theme's colors.toml when colours change; pin the
    // fixture palette last.
    panel.applyThemePalette(palette)
    shoot.restart()
  }

  function grab() {
    var name = scenarios[scenario].name
    var card = standIn()
    if (!card || panel.entries.length === 0 || panel.loadError !== "") {
      console.error("VISUAL SCENARIO FAIL: " + name + " has no report: " + panel.loadError)
      Qt.exit(1)
      return
    }
    card.grabToImage(function(result) {
      if (!result.saveToFile(outputDir + "/" + name)) {
        console.error("VISUAL SCENARIO FAIL: could not write " + name)
        Qt.exit(1)
        return
      }
      console.log("VISUAL SCENARIO OK: " + name)
      scenario++
      if (scenario < scenarios.length) prepare()
      else Qt.quit()
    })
  }

  Timer { id: warmup; interval: 800; onTriggered: root.prepare() }
  Timer { id: settle; interval: 2500; onTriggered: root.capture() }
  Timer { id: shoot; interval: 800; onTriggered: root.grab() }

  QtObject {
    id: fakeBar
    property color foreground: "#cdd6f4"
    property color urgent: "#f38ba8"
    property color barForeground: foreground
    property string fontFamily: Commons.Style.font.family
    property bool vertical: false
    property string position: "top"
    property var shell: null
    function run(command) {}
    function switchPanelFrom(panel, direction) { return false }
  }

  Window {
    id: window
    width: 480
    height: 980
    visible: true
    title: "AI Usage panel scenarios"
    color: "#11111b"

    Panel {
      id: panel
      x: 16
      y: 16
      bar: fakeBar
      moduleName: "akitaonrails.ai-usagebar"
      // Set before the first refresh, so only the stub ever runs.
      settings: ({ reportBinary: root.binary(69), showPriorities: true, uiLocale: "en" })
    }
  }

  Component.onCompleted: {
    applyTheme(dark)
    panel.open()
    warmup.start()
  }
}
