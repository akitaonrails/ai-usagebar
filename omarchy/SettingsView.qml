import QtQuick
import QtQuick.Controls
import Quickshell.Io
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "Model.js" as Model
import "I18n.js" as I18n
import "Priority.js" as Priority

// Native Quattro settings form. Rust remains the sole config owner: this view
// receives only non-secret key-presence metadata and sends changed keys over
// stdin, never argv or the environment.
Column {
  id: root

  property color foreground: Commons.Color.foreground
  property color urgent: Commons.Color.urgent
  property string fontFamily: Style.font.family
  property int recentHours: 3
  signal recentHoursRequested(int hours)
  property bool showPriorities: false
  property bool showPriorityShortcut: false
  property string binaryPath: "ai-usagebar"
  signal showPrioritiesRequested(bool enabled)
  signal showPriorityShortcutRequested(bool enabled)
  signal openPrioritiesRequested()
  property bool priorityAlert: true
  property int alertUnusedPercent: 50
  property int alertHours: 24
  signal priorityAlertRequested(bool enabled)
  signal alertUnusedPercentRequested(int value)
  signal alertHoursRequested(int value)
  property bool showValue: true
  property bool showProvider: false
  property bool showAll: false
  property bool colorCodeUsage: false
  property string uiLocale: "en"
  property string uiLocaleSetting: "auto"
  property string barWindow: "auto"
  property string showAs: "used"
  property bool brandIcons: true
  property var barProviderChoices: []
  property bool tabNames: true
  signal tabNamesRequested(bool enabled)
  property var metricEntries: []
  property string openMetricEntry: ""
  property string openSection: "display"
  property bool fetchingOpen: false
  property int keyPendingCount: 0
  readonly property color dim: Qt.darker(foreground, 1.45)

  property var snapshot: ({ primary_choices: [], keys: [], vendors: [] })
  property string selectedPrimary: ""
  // Pending provider on/off overrides (#244), keyed by provider id. Rebuilt
  // (never mutated) so the Toggle bindings re-evaluate.
  property var vendorOverrides: ({})
  property int vendorPendingCount: 0
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
    && (selectedPrimary !== "" || snapshot.primary_choices.length === 0
        || vendorPendingCount > 0)

  signal saved()
  signal fallbackRequested()
  signal nousLoginRequested()
  signal copilotLoginRequested()
  signal showValueRequested(bool enabled)
  signal showProviderRequested(bool enabled)
  signal showAllRequested(bool enabled)
  signal colorCodeUsageRequested(bool enabled)
  signal uiLocaleRequested(string value)
  signal barWindowRequested(string value)
  signal showAsRequested(string value)
  signal brandIconsRequested(bool enabled)
  signal barProviderToggleRequested(string entryId)
  signal metricToggleRequested(string entryId, string key)
  signal closeRequested()

  spacing: Style.space(12)
  focus: visible
  Keys.onEscapePressed: closeRequested()

  function safe(value) { return Model.autoTextSafe(value) }
  function tr(key, params) { return I18n.t(uiLocale, key, params || null) }

  function load() {
    if (stateProcess.running || applyProcess.running) return
    loading = true
    errorText = ""
    statusText = ""
    stateStdout = ""
    stateStderr = ""
    stateExitCode = -1
    resetVendorOverrides()
    stateProcess.running = true
  }

  function finishLoad() {
    loading = false
    if (stateExitCode !== 0) {
      var detail = Model.errorMessage(stateStderr)
      errorText = detail.indexOf("unrecognized subcommand") >= 0
        ? root.tr("error.binary_old")
        : detail
      snapshot = ({ primary_choices: [], keys: [], vendors: [] })
      selectedPrimary = ""
      return
    }
    var parsed = Model.parseSettingsSnapshot(stateStdout)
    if (!parsed.ok) {
      errorText = parsed.error
      snapshot = ({ primary_choices: [], keys: [], vendors: [] })
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

  function collectVendorToggles() {
    var toggles = []
    for (var id in root.vendorOverrides) {
      if (Object.prototype.hasOwnProperty.call(root.vendorOverrides, id))
        toggles.push({ id: id, enabled: root.vendorOverrides[id] })
    }
    return toggles
  }

  // Record one pending provider switch (#244). A value equal to the snapshot
  // drops the override, so toggling twice returns the row to "unchanged".
  function setVendorOverride(id, enabled) {
    var next = {}
    var base = snapshot.vendors || []
    var current = null
    for (var i = 0; i < base.length; i++) {
      if (base[i].id === id) current = base[i].enabled
    }
    for (var key in root.vendorOverrides) {
      if (Object.prototype.hasOwnProperty.call(root.vendorOverrides, key) && key !== id)
        next[key] = root.vendorOverrides[key]
    }
    if (enabled !== current) next[id] = enabled
    vendorOverrides = next
    vendorPendingCount = Object.keys(next).length
  }

  function vendorSummary() {
    var list = snapshot.vendors || []
    var on = 0
    for (var i = 0; i < list.length; i++) {
      var pending = vendorOverrides[list[i].id]
      var effective = pending === true || pending === false ? pending : list[i].enabled
      if (effective) on++
    }
    return (vendorPendingCount > 0 ? "● " : "") + on + "/" + list.length
  }

  function metricShownCount(rows) {
    var count = 0
    for (var i = 0; i < rows.length; i++)
      if (rows[i].checked) count++
    return count
  }

  function toggleMetricEntry(id) {
    openMetricEntry = openMetricEntry === id ? "" : id
  }

  function toggleSection(id) {
    openSection = openSection === id ? "" : id
  }

  function primaryLabel() {
    var choices = snapshot.primary_choices || []
    for (var i = 0; i < choices.length; i++)
      if (choices[i].id === selectedPrimary) return safe(choices[i].label)
    return ""
  }

  function recountKeys() {
    var count = 0
    for (var i = 0; i < keyRepeater.count; i++) {
      var row = keyRepeater.itemAt(i)
      if (row && row.pendingAction !== "unchanged") count++
    }
    keyPendingCount = count
  }

  function resetVendorOverrides() {
    vendorOverrides = ({})
    vendorPendingCount = 0
  }

  function save() {
    if (!canSave) return
    var built = Model.buildSettingsPatch(selectedPrimary, collectChanges(), collectVendorToggles())
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
      errorText = Model.errorMessage(applyStderr || root.tr("error.apply"))
      return
    }
    saved()
    load()
    // load() clears stale status before refreshing the snapshot, so set the
    // confirmation afterwards and keep it visible while the refresh runs.
    statusText = root.tr("status.saved")
  }

  onVisibleChanged: {
    if (visible) {
      openSection = "display"
      openMetricEntry = ""
      load()
      Qt.callLater(function() { root.forceActiveFocus() })
    }
    else scrubSecrets()
  }

  Process {
    id: stateProcess
    running: false
    command: [root.binaryPath, "settings", "show"]

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
    command: [root.binaryPath, "settings", "apply"]
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
      text: root.tr("section.settings")
      foreground: root.foreground
      fontFamily: root.fontFamily
    }
    Text {
      width: parent.width
      text: root.tr("loading.config")
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

    Disclosure {
      text: root.tr("section.display")
      expanded: root.openSection === "display"
      onToggled: root.toggleSection("display")
    }
    Column {
      visible: root.openSection === "display"
      width: parent.width
      spacing: Style.space(8)

      Toggle {
        width: parent.width
        label: root.tr("toggle.show_value")
        description: root.tr("toggle.show_value_desc")
        checked: root.showValue
        foreground: root.foreground
        fontFamily: root.fontFamily
        enabled: !root.saving
        onClicked: root.showValueRequested(!root.showValue)
      }
      Toggle {
        width: parent.width
        label: root.tr("toggle.brand_icons")
        description: root.tr("toggle.brand_icons_desc")
        checked: root.brandIcons
        foreground: root.foreground
        fontFamily: root.fontFamily
        enabled: !root.saving
        onClicked: root.brandIconsRequested(!root.brandIcons)
      }
      Toggle {
        width: parent.width
        label: root.tr("toggle.tab_names")
        description: root.tr("toggle.tab_names_desc")
        checked: root.tabNames || !root.brandIcons
        foreground: root.foreground
        fontFamily: root.fontFamily
        enabled: root.brandIcons && !root.saving
        onClicked: root.tabNamesRequested(!root.tabNames)
      }
      Toggle {
        width: parent.width
        label: root.tr("toggle.show_provider")
        description: root.tr("toggle.show_provider_desc")
        checked: root.showProvider
        foreground: root.foreground
        fontFamily: root.fontFamily
        enabled: !root.saving
        onClicked: root.showProviderRequested(!root.showProvider)
      }
      Toggle {
        width: parent.width
        label: root.tr("toggle.color_code")
        description: root.tr("toggle.color_code_desc")
        checked: root.colorCodeUsage
        foreground: root.foreground
        fontFamily: root.fontFamily
        enabled: !root.saving
        onClicked: root.colorCodeUsageRequested(!root.colorCodeUsage)
      }
      Toggle {
        width: parent.width
        label: root.tr("toggle.show_all")
        description: root.tr("toggle.show_all_desc")
        checked: root.showAll
        foreground: root.foreground
        fontFamily: root.fontFamily
        enabled: !root.saving
        onClicked: root.showAllRequested(!root.showAll)
      }
    }
  }

  Column {
    visible: !root.loading
    width: parent.width
    spacing: Style.space(8)
    Disclosure {
      text: root.tr("priority.section")
      badge: root.tr("priority.hours", { hours: root.recentHours })
      expanded: root.openSection === "priorities"
      onToggled: root.toggleSection("priorities")
    }
    Column {
      visible: root.openSection === "priorities"
      width: parent.width
      spacing: Style.space(8)
      Toggle {
        width: parent.width
        label: root.tr("priority.enable")
        description: root.tr("priority.enable_desc")
        checked: root.showPriorities
        foreground: root.foreground
        fontFamily: root.fontFamily
        enabled: !root.saving
        onClicked: root.showPrioritiesRequested(!root.showPriorities)
      }
      Toggle {
        width: parent.width
        label: root.tr("priority.shortcut")
        description: root.tr("priority.shortcut_desc")
        checked: root.showPriorityShortcut
        enabled: root.showPriorities && !root.saving
        foreground: root.foreground
        fontFamily: root.fontFamily
        onClicked: root.showPriorityShortcutRequested(!root.showPriorityShortcut)
      }
      Dropdown {
        width: parent.width
        label: root.tr("priority.recent_label")
        value: String(root.recentHours)
        options: {
          var hours = []
          for (var h = 1; h <= 24; h++) hours.push({ value: String(h), label: root.tr("priority.hours", {hours: h}) })
          return hours
        }
        enabled: root.showPriorities && !root.saving
        foreground: root.foreground
        fontFamily: root.fontFamily
        onChanged: function(value) { root.recentHoursRequested(Priority.normalizeHours(value)) }
      }
      Text {
        width: parent.width
        text: root.tr("priority.recent_help")
        textFormat: Text.PlainText
        wrapMode: Text.WordWrap
        color: root.dim
        font.family: root.fontFamily
        font.pixelSize: Style.font.caption
      }
      Toggle {
        width: parent.width
        label: root.tr("priority.alert")
        description: root.tr("priority.alert_desc", { unused: root.alertUnusedPercent, hours: root.alertHours })
        checked: root.priorityAlert
        enabled: root.showPriorities && !root.saving
        foreground: root.foreground
        fontFamily: root.fontFamily
        onClicked: root.priorityAlertRequested(!root.priorityAlert)
      }
      Dropdown {
        width: parent.width
        label: root.tr("priority.alert_unused_label")
        value: String(root.alertUnusedPercent)
        options: Priority.choices([10, 20, 25, 30, 40, 50, 60, 70, 75, 80, 90], root.alertUnusedPercent,
          function(v) { return root.tr("priority.percent", { value: v }) })
        enabled: root.showPriorities && !root.saving
        foreground: root.foreground
        fontFamily: root.fontFamily
        onChanged: function(value) { root.alertUnusedPercentRequested(Priority.normalizeAlertPercent(value)) }
      }
      Dropdown {
        width: parent.width
        label: root.tr("priority.alert_hours_label")
        value: String(root.alertHours)
        options: Priority.choices([6, 12, 24, 36, 48, 72, 96, 120], root.alertHours,
          function(v) { return root.tr("priority.hours", { hours: v }) })
        enabled: root.showPriorities && !root.saving
        foreground: root.foreground
        fontFamily: root.fontFamily
        onChanged: function(value) { root.alertHoursRequested(Priority.normalizeAlertHours(value)) }
      }
      Button {
        text: root.tr("priority.open")
        enabled: root.showPriorities && !root.saving
        foreground: root.foreground
        fontFamily: root.fontFamily
        fontSize: Style.font.bodySmall
        bordered: true
        focusable: true
        onClicked: root.openPrioritiesRequested()
      }
    }
  }

  Column {
    visible: !root.loading
    width: parent.width
    spacing: Style.space(8)

    Disclosure {
      text: root.tr("section.language")
      badge: root.tr("language." + (I18n.normalizeLocaleTag(root.uiLocaleSetting) || "auto"))
      expanded: root.openSection === "language"
      onToggled: root.toggleSection("language")
    }
    Column {
      visible: root.openSection === "language"
      width: parent.width
      spacing: Style.space(8)

      Text {
        width: parent.width
        text: root.tr("language.help")
        textFormat: Text.PlainText
        color: root.dim
        font.family: root.fontFamily
        font.pixelSize: Style.font.caption
        wrapMode: Text.WordWrap
      }
      Dropdown {
        width: parent.width
        showLabel: false
        value: {
          var tag = I18n.normalizeLocaleTag(root.uiLocaleSetting)
          return tag === "" ? "auto" : tag
        }
        options: [
          { value: "auto", label: root.tr("language.auto") },
          { value: "en", label: root.tr("language.en") },
          { value: "pt-BR", label: root.tr("language.pt-BR") },
          { value: "ru", label: root.tr("language.ru") },
          { value: "ko", label: root.tr("language.ko") },
          { value: "es", label: root.tr("language.es") }
        ]
        foreground: root.foreground
        fontFamily: root.fontFamily
        enabled: !root.saving
        onChanged: function(value) { root.uiLocaleRequested(value) }
      }
    }
  }

  Column {
    visible: !root.loading
    width: parent.width
    spacing: Style.space(8)

    Disclosure {
      text: root.tr("section.bar_window")
      badge: root.tr("bar_window." + Model.normalizeBarWindow(root.barWindow))
      expanded: root.openSection === "barWindow"
      onToggled: root.toggleSection("barWindow")
    }
    Column {
      visible: root.openSection === "barWindow"
      width: parent.width
      spacing: Style.space(8)

      Text {
        width: parent.width
        text: root.tr("bar_window.help")
        textFormat: Text.PlainText
        color: root.dim
        font.family: root.fontFamily
        font.pixelSize: Style.font.caption
        wrapMode: Text.WordWrap
      }
      Dropdown {
        width: parent.width
        showLabel: false
        value: Model.normalizeBarWindow(root.barWindow)
        options: [
          { value: "auto", label: root.tr("bar_window.auto") },
          { value: "session", label: root.tr("bar_window.session") },
          { value: "weekly", label: root.tr("bar_window.weekly") },
          { value: "monthly", label: root.tr("bar_window.monthly") }
        ]
        foreground: root.foreground
        fontFamily: root.fontFamily
        enabled: !root.saving
        onChanged: function(value) { root.barWindowRequested(value) }
      }
    }
  }

  Column {
    visible: !root.loading
    width: parent.width
    spacing: Style.space(8)

    Disclosure {
      text: root.tr("section.show_as")
      badge: root.tr("show_as." + Model.normalizeShowAs(root.showAs))
      expanded: root.openSection === "showAs"
      onToggled: root.toggleSection("showAs")
    }
    Column {
      visible: root.openSection === "showAs"
      width: parent.width
      spacing: Style.space(8)

      Text {
        width: parent.width
        text: root.tr("show_as.help")
        textFormat: Text.PlainText
        color: root.dim
        font.family: root.fontFamily
        font.pixelSize: Style.font.caption
        wrapMode: Text.WordWrap
      }
      Dropdown {
        width: parent.width
        showLabel: false
        value: Model.normalizeShowAs(root.showAs)
        options: [
          { value: "used", label: root.tr("show_as.used") },
          { value: "left", label: root.tr("show_as.left") }
        ]
        foreground: root.foreground
        fontFamily: root.fontFamily
        enabled: !root.saving
        onChanged: function(value) { root.showAsRequested(value) }
      }
    }
  }

  Column {
    visible: !root.loading && root.metricEntries.length > 0
    width: parent.width
    spacing: Style.space(8)

    Disclosure {
      text: root.tr("section.metrics")
      expanded: root.openSection === "metrics"
      onToggled: root.toggleSection("metrics")
    }
    Column {
      visible: root.openSection === "metrics"
      width: parent.width
      spacing: Style.space(8)

      Text {
        width: parent.width
        text: root.tr("metrics.help")
        textFormat: Text.PlainText
        color: root.dim
        font.family: root.fontFamily
        font.pixelSize: Style.font.caption
        wrapMode: Text.WordWrap
      }
      Repeater {
        model: root.metricEntries

        Column {
          id: metricProvider
          required property var modelData
          width: parent.width
          spacing: Style.space(8)

          Disclosure {
            text: root.safe(metricProvider.modelData.name)
            badge: root.metricShownCount(metricProvider.modelData.rows) + "/" + metricProvider.modelData.rows.length
            expanded: root.openMetricEntry === metricProvider.modelData.id
            onToggled: root.toggleMetricEntry(metricProvider.modelData.id)
          }
          Column {
            visible: root.openMetricEntry === metricProvider.modelData.id
            width: parent.width
            spacing: Style.space(8)

            Repeater {
              model: metricProvider.modelData.rows

              Toggle {
                required property var modelData
                width: parent.width
                label: modelData.labelKey !== ""
                  ? root.tr(modelData.labelKey)
                  : I18n.displayLabel(root.uiLocale, modelData.label)
                description: root.safe(modelData.group)
                checked: modelData.checked
                foreground: root.foreground
                fontFamily: root.fontFamily
                enabled: !root.saving && modelData.canToggle
                opacity: modelData.canToggle ? 1 : 0.45
                onClicked: root.metricToggleRequested(metricProvider.modelData.id, modelData.key)
              }
            }
          }
        }
      }
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
          text: root.tr("action.retry")
          bordered: true
          focusable: true
          foreground: root.foreground
          fontFamily: root.fontFamily
          onClicked: root.load()
        }
        Button {
          text: root.tr("action.terminal_settings")
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

    Disclosure {
      text: root.tr("section.primary")
      badge: root.primaryLabel()
      expanded: root.openSection === "primary"
      onToggled: root.toggleSection("primary")
    }
    Column {
      visible: root.openSection === "primary"
      width: parent.width
      spacing: Style.space(8)

      Text {
        width: parent.width
        text: root.tr("primary.help")
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
  }

  Column {
    visible: !root.loading && (root.snapshot.vendors.length > 0 || root.barProviderChoices.length > 0)
    width: parent.width
    spacing: Style.space(8)

    Disclosure {
      text: root.tr("section.providers")
      badge: Model.providerDisplaySummary(root.barProviderChoices)
      expanded: root.openSection === "providers"
      onToggled: root.toggleSection("providers")
    }
    Column {
      visible: root.openSection === "providers"
      width: parent.width
      spacing: Style.space(8)

      PanelSectionHeader {
        text: root.tr("providers.bar_title")
        foreground: root.foreground
        fontFamily: root.fontFamily
      }
      Text {
        width: parent.width
        text: root.tr("providers.bar_help")
        textFormat: Text.PlainText
        color: root.dim
        font.family: root.fontFamily
        font.pixelSize: Style.font.caption
        wrapMode: Text.WordWrap
      }
      Repeater {
        model: root.barProviderChoices
        Toggle {
          required property var modelData
          width: parent.width
          label: root.safe(modelData.name)
          description: modelData.checked ? root.tr("providers.shown") : root.tr("providers.hidden")
          checked: modelData.checked
          foreground: root.foreground
          fontFamily: root.fontFamily
          onClicked: root.barProviderToggleRequested(modelData.id)
        }
      }
      Disclosure {
        text: root.tr("providers.fetch_title")
        badge: root.vendorSummary()
        expanded: root.fetchingOpen
        onToggled: root.fetchingOpen = !root.fetchingOpen
      }
      Column {
        width: parent.width
        visible: root.fetchingOpen
        spacing: Style.space(8)
        Text {
          width: parent.width
          text: root.tr("providers.help")
          textFormat: Text.PlainText
          color: root.dim
          font.family: root.fontFamily
          font.pixelSize: Style.font.caption
          wrapMode: Text.WordWrap
        }
        Repeater {
          model: root.snapshot.vendors

          Toggle {
            required property var modelData
            width: parent.width
            label: root.safe(modelData.label)
            description: {
              var pending = root.vendorOverrides[modelData.id]
              var effective = pending === true || pending === false ? pending : modelData.enabled
              return effective ? root.tr("status.vendor_on") : root.tr("status.vendor_off")
            }
            checked: {
              var pending = root.vendorOverrides[modelData.id]
              return pending === true || pending === false ? pending : modelData.enabled
            }
            foreground: root.foreground
            fontFamily: root.fontFamily
            enabled: !root.saving
            onClicked: root.setVendorOverride(modelData.id, !checked)
          }
        }
      }
    }
  }

  Column {
    visible: !root.loading
    width: parent.width
    spacing: Style.space(10)

    Disclosure {
      text: root.tr("section.credentials")
      badge: root.keyPendingCount > 0 ? "● " + root.keyPendingCount : ""
      expanded: root.openSection === "credentials"
      onToggled: root.toggleSection("credentials")
    }
    Column {
      visible: root.openSection === "credentials"
      width: parent.width
      spacing: Style.space(10)

      PanelSectionHeader {
        text: root.tr("section.auth")
        foreground: root.foreground
        fontFamily: root.fontFamily
      }
      Text {
        width: parent.width
        text: root.tr("auth.help")
        textFormat: Text.PlainText
        color: root.dim
        font.family: root.fontFamily
        font.pixelSize: Style.font.caption
        wrapMode: Text.WordWrap
      }
      Button {
        width: parent.width
        text: root.tr("auth.nous")
        iconText: "󰍂"
        bordered: true
        focusable: true
        foreground: root.foreground
        fontFamily: root.fontFamily
        enabled: !root.saving
        onClicked: {
          root.statusText = root.tr("status.nous_login")
          root.nousLoginRequested()
        }
      }
      Button {
        width: parent.width
        text: root.tr("auth.copilot")
        iconText: "󰊤"
        bordered: true
        focusable: true
        foreground: root.foreground
        fontFamily: root.fontFamily
        enabled: !root.saving
        onClicked: {
          root.statusText = root.tr("status.copilot_login")
          root.copilotLoginRequested()
        }
      }

    Column {
      visible: root.snapshot.keys.length > 0
      width: parent.width
      spacing: Style.space(10)

        Text {
          width: parent.width
          text: root.tr("credentials.help")
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
            onPendingActionChanged: root.recountKeys()

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
                  text: keyCard.pendingAction === "clear" ? root.tr("status.will_clear")
                    : keyCard.pendingAction === "set" ? root.tr("credentials.new_key")
                    : keyCard.modelData.environment_configured ? root.tr("credentials.env_override")
                    : keyCard.modelData.inline_configured ? root.tr("credentials.stored")
                    : root.tr("credentials.not_configured")
                  textFormat: Text.PlainText
                  color: keyCard.pendingAction === "clear" ? root.urgent : root.dim
                  font.family: root.fontFamily
                  font.pixelSize: Style.font.caption
                }
              }

              // Env var stays on its own line (identifier, may elide). Role +
              // note wrap below so long hints are readable instead of cutting
              // mid-word as "mont…" / "sp…".
              Text {
                visible: text !== ""
                width: parent.width
                text: keyCard.modelData.environment ? root.safe(keyCard.modelData.environment) : ""
                textFormat: Text.PlainText
                color: root.dim
                font.family: root.fontFamily
                font.pixelSize: Style.font.caption
                elide: Text.ElideRight
              }
              Text {
                visible: text !== ""
                width: parent.width
                text: {
                  var parts = []
                  var secret = I18n.displaySecretLabel(root.uiLocale, keyCard.modelData.secret_label)
                  var note = I18n.displayNote(root.uiLocale, keyCard.modelData.note)
                  if (secret) parts.push(secret)
                  if (note) parts.push(note)
                  return parts.join(" · ")
                }
                textFormat: Text.PlainText
                color: root.dim
                font.family: root.fontFamily
                font.pixelSize: Style.font.caption
                wrapMode: Text.WordWrap
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
                    ? root.tr("credentials.keep_blank")
                    : root.tr("credentials.paste", {
                        label: I18n.displaySecretLabel(root.uiLocale, keyCard.modelData.secret_label)
                          || root.tr("credentials.credential")
                      })
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
                    ? root.tr("credentials.keep_key") : root.tr("credentials.clear_key")
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
      && (root.snapshot.primary_choices.length > 0 || root.snapshot.keys.length > 0
          || root.snapshot.vendors.length > 0)
    width: parent.width
    text: root.saving ? root.tr("action.saving") : root.tr("action.save")
    iconText: root.saving ? "󰑐" : "󰄬"
    iconSpinning: root.saving
    bordered: true
    focusable: true
    foreground: root.foreground
    fontFamily: root.fontFamily
    enabled: root.canSave
    onClicked: root.save()
  }

  component Disclosure: Item {
    id: disclosure
    property string text: ""
    property string badge: ""
    property bool expanded: false
    signal toggled()

    width: parent ? parent.width : 0
    implicitHeight: Style.spacing.controlHeight
    activeFocusOnTab: true
    Keys.onReturnPressed: disclosure.toggled()
    Keys.onEnterPressed: disclosure.toggled()
    Keys.onSpacePressed: disclosure.toggled()

    Rectangle {
      anchors.fill: parent
      radius: Style.cornerRadius
      color: disclosureMouse.containsMouse || disclosure.activeFocus
        ? Qt.rgba(root.foreground.r, root.foreground.g, root.foreground.b, 0.08) : "transparent"
    }
    Text {
      id: disclosureChevron
      anchors.left: parent.left
      anchors.leftMargin: Style.space(8)
      anchors.verticalCenter: parent.verticalCenter
      text: disclosure.expanded ? "󰅀" : "󰅂"
      textFormat: Text.PlainText
      color: root.foreground
      font.family: root.fontFamily
      font.pixelSize: Style.font.body
    }
    Text {
      anchors.left: disclosureChevron.right
      anchors.leftMargin: Style.space(8)
      anchors.right: disclosureBadge.left
      anchors.rightMargin: Style.spacing.md
      anchors.verticalCenter: parent.verticalCenter
      text: disclosure.text
      textFormat: Text.PlainText
      color: root.foreground
      font.family: root.fontFamily
      font.pixelSize: Style.font.bodySmall
      font.bold: true
      elide: Text.ElideRight
    }
    Text {
      id: disclosureBadge
      anchors.right: parent.right
      anchors.rightMargin: Style.space(8)
      anchors.verticalCenter: parent.verticalCenter
      text: disclosure.badge
      textFormat: Text.PlainText
      color: root.dim
      font.family: root.fontFamily
      font.pixelSize: Style.font.caption
    }
    MouseArea {
      id: disclosureMouse
      anchors.fill: parent
      hoverEnabled: true
      cursorShape: Qt.PointingHandCursor
      onClicked: {
        disclosure.forceActiveFocus()
        disclosure.toggled()
      }
    }
  }
}
