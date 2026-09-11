import QtQuick
import QtQuick.Controls
import Quickshell.Io
import qs.Commons
import qs.Ui
import "Model.js" as Model

// Native Quattro settings form. Rust remains the sole config owner: this view
// receives only non-secret key-presence metadata and sends changed keys over
// stdin, never argv or the environment.
Column {
  id: root

  property color foreground: Color.foreground
  property color urgent: Color.urgent
  property string fontFamily: Style.font.family
  property bool showValue: true
  property bool showProvider: false
  property bool showAll: false
  readonly property color dim: Qt.darker(foreground, 1.45)

  property var snapshot: ({ primary_choices: [], keys: [] })
  property string selectedPrimary: ""
  property string stateStdout: ""
  property string stateStderr: ""
  property string applyStdout: ""
  property string applyStderr: ""
  property string errorText: ""
  property string statusText: ""
  property string pendingPayload: ""
  property int stateExitCode: -1
  property int applyExitCode: -1
  property bool loading: false
  property bool saving: false
  readonly property bool canSave: !loading && !saving
    && (selectedPrimary !== "" || snapshot.primary_choices.length === 0)

  signal saved()
  signal fallbackRequested()
  signal nousLoginRequested()
  signal copilotLoginRequested()
  signal showValueRequested(bool enabled)
  signal showProviderRequested(bool enabled)
  signal showAllRequested(bool enabled)
  signal closeRequested()

  spacing: Style.space(12)
  focus: visible
  Keys.onEscapePressed: closeRequested()

  function safe(value) { return Model.autoTextSafe(value) }

  function load() {
    if (stateProcess.running || applyProcess.running) return
    loading = true
    errorText = ""
    statusText = ""
    stateStdout = ""
    stateStderr = ""
    stateExitCode = -1
    stateProcess.running = true
  }

  function finishLoad() {
    loading = false
    if (stateExitCode !== 0) {
      var detail = Model.errorMessage(stateStderr)
      errorText = detail.indexOf("unrecognized subcommand") >= 0
        ? "This installed ai-usagebar binary predates native settings. Update the package, or use the terminal settings fallback."
        : detail
      snapshot = ({ primary_choices: [], keys: [] })
      selectedPrimary = ""
      return
    }
    var parsed = Model.parseSettingsSnapshot(stateStdout)
    if (!parsed.ok) {
      errorText = parsed.error
      snapshot = ({ primary_choices: [], keys: [] })
      selectedPrimary = ""
      return
    }
    snapshot = parsed
    selectedPrimary = parsed.primary
  }

  function collectChanges() {
    var changes = []
    for (var i = 0; i < keyRepeater.count; i++) {
      var row = keyRepeater.itemAt(i)
      if (!row || row.pendingAction === "unchanged") continue
      changes.push({
        id: row.vendorId,
        action: row.pendingAction,
        value: row.pendingAction === "set" ? row.secretText : ""
      })
    }
    return changes
  }

  function save() {
    if (!canSave) return
    var built = Model.buildSettingsPatch(selectedPrimary, collectChanges())
    if (!built.ok) {
      errorText = built.error
      return
    }
    saving = true
    errorText = ""
    statusText = ""
    applyStdout = ""
    applyStderr = ""
    applyExitCode = -1
    pendingPayload = built.payload
    applyProcess.running = true
  }

  function scrubSecrets() {
    pendingPayload = ""
    for (var i = 0; i < keyRepeater.count; i++) {
      var row = keyRepeater.itemAt(i)
      if (row) row.scrub()
    }
  }

  function finishApply() {
    saving = false
    // The Rust process has consumed the stdin patch. Do not retain credentials
    // in this long-lived shell, even if the save failed.
    scrubSecrets()
    if (applyExitCode !== 0 || !Model.parseSettingsApplyResult(applyStdout)) {
      errorText = Model.errorMessage(applyStderr || "The settings command did not confirm the save.")
      return
    }
    saved()
    load()
    // load() clears stale status before refreshing the snapshot, so set the
    // confirmation afterwards and keep it visible while the refresh runs.
    statusText = "Settings saved. Usage is refreshing."
  }

  onVisibleChanged: {
    if (visible) {
      load()
      Qt.callLater(function() { root.forceActiveFocus() })
    }
    else scrubSecrets()
  }

  Process {
    id: stateProcess
    running: false
    command: ["ai-usagebar", "settings", "show"]

    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: root.stateStdout = text
    }
    stderr: StdioCollector {
      waitForEnd: true
      onStreamFinished: root.stateStderr = text
    }
    onExited: function(exitCode, exitStatus) {
      root.stateExitCode = exitCode
      Qt.callLater(root.finishLoad)
    }
  }

  Process {
    id: applyProcess
    running: false
    command: ["ai-usagebar", "settings", "apply"]
    stdinEnabled: true

    onStarted: {
      write(root.pendingPayload + "\n")
      root.pendingPayload = ""
    }
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: root.applyStdout = text
    }
    stderr: StdioCollector {
      waitForEnd: true
      onStreamFinished: root.applyStderr = text
    }
    onExited: function(exitCode, exitStatus) {
      root.applyExitCode = exitCode
      Qt.callLater(root.finishApply)
    }
  }

  Column {
    visible: root.loading
    width: parent.width
    spacing: Style.space(8)

    PanelSectionHeader {
      text: "SETTINGS"
      foreground: root.foreground
      fontFamily: root.fontFamily
    }
    Text {
      width: parent.width
      text: "Loading configuration…"
      color: root.dim
      font.family: root.fontFamily
      font.pixelSize: Style.font.body
      horizontalAlignment: Text.AlignHCenter
    }
  }

  Column {
    visible: !root.loading
    width: parent.width
    spacing: Style.space(8)

    PanelSectionHeader {
      text: "DISPLAY"
      foreground: root.foreground
      fontFamily: root.fontFamily
    }
    Toggle {
      width: parent.width
      label: "Show usage value in the top bar"
      description: "Turn this off for an icon-only bar entry. The panel and tooltip still show full usage details. Applies immediately."
      checked: root.showValue
      foreground: root.foreground
      fontFamily: root.fontFamily
      enabled: !root.saving
      onClicked: root.showValueRequested(!root.showValue)
    }
    Toggle {
      width: parent.width
      label: "Show provider name in the top bar"
      description: "Turn this on to prefix the bar entry with the provider's short code — cld, gpt, zai, agy — the way Waybar's {vendor_short} does. Off by default. Applies immediately."
      checked: root.showProvider
      foreground: root.foreground
      fontFamily: root.fontFamily
      enabled: !root.saving
      onClicked: root.showProviderRequested(!root.showProvider)
    }
    Toggle {
      width: parent.width
      label: "Show all providers in the top bar"
      description: "Turn this on to show every configured provider's icon and usage in the top bar at once, instead of cycling one at a time. Click still opens the panel; the wheel still selects which details you see. Off by default. Applies immediately."
      checked: root.showAll
      foreground: root.foreground
      fontFamily: root.fontFamily
      enabled: !root.saving
      onClicked: root.showAllRequested(!root.showAll)
    }
  }

  BorderSurface {
    visible: root.errorText !== ""
    width: parent.width
    implicitHeight: errorColumn.implicitHeight + Style.spacing.xl * 2
    color: Qt.rgba(root.urgent.r, root.urgent.g, root.urgent.b, 0.09)
    borderSpec: Border.flat(Qt.rgba(root.urgent.r, root.urgent.g, root.urgent.b, 0.35), 1)
    radius: Style.cornerRadius

    Column {
      id: errorColumn
      anchors.left: parent.left
      anchors.right: parent.right
      anchors.verticalCenter: parent.verticalCenter
      anchors.leftMargin: Style.space(12)
      anchors.rightMargin: Style.space(12)
      spacing: Style.space(8)

      Text {
        width: parent.width
        text: root.safe(root.errorText)
        textFormat: Text.PlainText
        color: root.dim
        font.family: root.fontFamily
        font.pixelSize: Style.font.caption
        wrapMode: Text.WordWrap
      }
      Row {
        spacing: Style.space(8)
        Button {
          text: "Retry"
          bordered: true
          focusable: true
          foreground: root.foreground
          fontFamily: root.fontFamily
          onClicked: root.load()
        }
        Button {
          text: "Open terminal settings"
          bordered: true
          focusable: true
          foreground: root.foreground
          fontFamily: root.fontFamily
          onClicked: root.fallbackRequested()
        }
      }
    }
  }

  Column {
    visible: !root.loading && root.snapshot.primary_choices.length > 0
    width: parent.width
    spacing: Style.space(8)

    PanelSectionHeader {
      text: "PRIMARY PROVIDER"
      foreground: root.foreground
      fontFamily: root.fontFamily
    }
    Text {
      width: parent.width
      text: "Used by the CLI, Waybar, TUI, and as this panel's preferred provider."
      textFormat: Text.PlainText
      color: root.dim
      font.family: root.fontFamily
      font.pixelSize: Style.font.caption
      wrapMode: Text.WordWrap
    }
    Dropdown {
      id: primaryDropdown
      width: parent.width
      showLabel: false
      value: root.selectedPrimary
      options: root.snapshot.primary_choices
      foreground: root.foreground
      fontFamily: root.fontFamily
      enabled: !root.saving
      onChanged: function(value) { root.selectedPrimary = value }
    }
  }

  Column {
    visible: !root.loading
    width: parent.width
    spacing: Style.space(10)

    PanelSeparator {
      width: parent.width
      foreground: root.foreground
    }
    PanelSectionHeader {
      text: "AUTHENTICATION"
      foreground: root.foreground
      fontFamily: root.fontFamily
    }
    Text {
      width: parent.width
      text: "OAuth login opens in a terminal. Complete it, then return here, choose the provider as primary, save, and press Refresh."
      textFormat: Text.PlainText
      color: root.dim
      font.family: root.fontFamily
      font.pixelSize: Style.font.caption
      wrapMode: Text.WordWrap
    }
    Button {
      width: parent.width
      text: "Log in with Nous Research"
      iconText: "󰍂"
      bordered: true
      focusable: true
      foreground: root.foreground
      fontFamily: root.fontFamily
      enabled: !root.saving
      onClicked: {
        root.statusText = "Nous Research login is opening in a terminal."
        root.nousLoginRequested()
      }
    }
    Button {
      width: parent.width
      text: "Log in with GitHub Copilot"
      iconText: "󰊤"
      bordered: true
      focusable: true
      foreground: root.foreground
      fontFamily: root.fontFamily
      enabled: !root.saving
      onClicked: {
        root.statusText = "GitHub sign-in is opening in a terminal. Complete it, then choose GitHub Copilot as primary and save."
        root.copilotLoginRequested()
      }
    }
  }

  Column {
    visible: !root.loading && root.snapshot.keys.length > 0
    width: parent.width
    spacing: Style.space(10)

    PanelSeparator {
      width: parent.width
      foreground: root.foreground
    }
    PanelSectionHeader {
      text: "CREDENTIALS"
      foreground: root.foreground
      fontFamily: root.fontFamily
    }
    Text {
      width: parent.width
      text: "Stored values are never loaded into the shell. Leave a field blank to keep its current value, or use the clear button to remove an inline credential. Environment variables take precedence."
      textFormat: Text.PlainText
      color: root.dim
      font.family: root.fontFamily
      font.pixelSize: Style.font.caption
      wrapMode: Text.WordWrap
    }

    Repeater {
      id: keyRepeater
      model: root.snapshot.keys

      BorderSurface {
        id: keyCard
        required property var modelData
        readonly property string vendorId: String(modelData.id || "")
        property string pendingAction: "unchanged"
        property alias secretText: keyField.text

        function scrub() {
          keyField.text = ""
          pendingAction = "unchanged"
        }

        width: keyRepeater.parent.width
        implicitHeight: keyColumn.implicitHeight + Style.spacing.xl * 2
        color: Qt.rgba(root.foreground.r, root.foreground.g, root.foreground.b, 0.035)
        borderSpec: Border.flat(Qt.rgba(root.foreground.r, root.foreground.g, root.foreground.b, 0.10), 1)
        radius: Style.cornerRadius

        Column {
          id: keyColumn
          anchors.left: parent.left
          anchors.right: parent.right
          anchors.verticalCenter: parent.verticalCenter
          anchors.leftMargin: Style.space(12)
          anchors.rightMargin: Style.space(12)
          spacing: Style.space(6)

          Item {
            width: parent.width
            implicitHeight: Math.max(keyLabel.implicitHeight, keyStatus.implicitHeight)

            Text {
              id: keyLabel
              anchors.left: parent.left
              anchors.right: keyStatus.left
              anchors.rightMargin: Style.spacing.md
              text: root.safe(keyCard.modelData.label)
              textFormat: Text.PlainText
              color: root.foreground
              font.family: root.fontFamily
              font.pixelSize: Style.font.bodySmall
              font.bold: true
              elide: Text.ElideRight
            }
            Text {
              id: keyStatus
              anchors.right: parent.right
              text: keyCard.pendingAction === "clear" ? "will clear"
                : keyCard.pendingAction === "set" ? "new key"
                : keyCard.modelData.environment_configured ? "environment override"
                : keyCard.modelData.inline_configured ? "stored"
                : "not configured"
              textFormat: Text.PlainText
              color: keyCard.pendingAction === "clear" ? root.urgent : root.dim
              font.family: root.fontFamily
              font.pixelSize: Style.font.caption
            }
          }

          Text {
            visible: text !== ""
            width: parent.width
            text: {
              var parts = []
              if (keyCard.modelData.environment) parts.push(root.safe(keyCard.modelData.environment))
              if (keyCard.modelData.secret_label) parts.push(root.safe(keyCard.modelData.secret_label))
              if (keyCard.modelData.note) parts.push(root.safe(keyCard.modelData.note))
              return parts.join(" · ")
            }
            textFormat: Text.PlainText
            color: root.dim
            font.family: root.fontFamily
            font.pixelSize: Style.font.caption
            elide: Text.ElideRight
          }

          Row {
            width: parent.width
            spacing: Style.space(8)

            TextField {
              id: keyField
              width: parent.width - clearButton.width - parent.spacing
              password: true
              enabled: !root.saving && keyCard.pendingAction !== "clear"
              placeholderText: keyCard.modelData.configured
                ? "Leave blank to keep current credential"
                : "Paste " + (keyCard.modelData.secret_label || "credential")
              foreground: root.foreground
              onTextEdited: keyCard.pendingAction = text.length > 0 ? "set" : "unchanged"
              Keys.onEscapePressed: focus = false
              onAccepted: root.save()
            }

            PanelActionButton {
              id: clearButton
              anchors.verticalCenter: keyField.verticalCenter
              iconText: keyCard.pendingAction === "clear" ? "󰕌" : "󰆴"
              tooltipText: keyCard.pendingAction === "clear"
                ? "Keep the stored key" : "Clear the stored inline key"
              foreground: root.foreground
              hoverColor: keyCard.pendingAction === "clear" ? root.foreground : root.urgent
              fontFamily: root.fontFamily
              focusable: true
              enabled: !root.saving && (keyCard.modelData.inline_configured || keyCard.pendingAction === "clear")
              onClicked: {
                if (keyCard.pendingAction === "clear") {
                  keyCard.pendingAction = "unchanged"
                } else {
                  keyField.text = ""
                  keyCard.pendingAction = "clear"
                }
              }
            }
          }
        }
      }
    }
  }

  BorderSurface {
    visible: root.statusText !== ""
    width: parent.width
    implicitHeight: savedText.implicitHeight + Style.spacing.lg * 2
    color: Qt.rgba(root.foreground.r, root.foreground.g, root.foreground.b, 0.06)
    borderSpec: Border.flat(Qt.rgba(root.foreground.r, root.foreground.g, root.foreground.b, 0.18), 1)
    radius: Style.cornerRadius

    Text {
      id: savedText
      anchors.left: parent.left
      anchors.right: parent.right
      anchors.verticalCenter: parent.verticalCenter
      anchors.leftMargin: Style.space(12)
      anchors.rightMargin: Style.space(12)
      text: root.safe(root.statusText)
      textFormat: Text.PlainText
      color: root.dim
      font.family: root.fontFamily
      font.pixelSize: Style.font.caption
      horizontalAlignment: Text.AlignHCenter
    }
  }

  Button {
    visible: !root.loading
      && (root.snapshot.primary_choices.length > 0 || root.snapshot.keys.length > 0)
    width: parent.width
    text: root.saving ? "Saving…" : "Save settings"
    iconText: root.saving ? "󰑐" : "󰄬"
    iconSpinning: root.saving
    bordered: true
    focusable: true
    foreground: root.foreground
    fontFamily: root.fontFamily
    enabled: root.canSave
    onClicked: root.save()
  }
}
