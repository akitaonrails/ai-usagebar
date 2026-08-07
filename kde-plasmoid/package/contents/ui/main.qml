pragma ComponentBehavior: Bound

import QtQuick
import org.kde.plasma.plasmoid
import org.kde.plasma.core as PlasmaCore
import org.kde.plasma.plasma5support as Plasma5Support
import org.kde.kirigami as Kirigami
import "../code/marker-logic.mjs" as Marker
import "../code/plasmoid-logic.mjs" as Logic

PlasmoidItem {
    id: root

    // --- state -------------------------------------------------------------
    property var usage: null        // parsed model, or null
    property string notice: ""      // the binary's own Loading… text
    property string failure: ""     // our failure: spawn, non-zero exit, bad JSON

    // The command currently in flight, "" when idle. Also the watchdog's handle
    // on which source to drop.
    property string pendingCommand: ""

    // GNOME uses 60s, macOS 45s. 60 here: the binary's own HTTP timeout is 30s
    // per vendor, so this leaves headroom for a retry without ever being the
    // thing that gives up first.
    readonly property int fetchTimeoutSecs: 60

    // The floor the settings actually offer: config/main.xml declares <min>5</min>
    // and the spinbox steps in fives from 5, so 5 is the first value a user can
    // pick. The GNOME extension clamps to the same 5 against its own gschema
    // range. Clamping higher here silently ran a chosen 5s at 10s.
    readonly property int minIntervalSecs: 5

    readonly property string vendor: Plasmoid.configuration.vendor
    readonly property var compactCells: Logic.compactCells(root.usage, {
        showSession: Plasmoid.configuration.showSession,
        showWeekly: Plasmoid.configuration.showWeekly,
        showExtra: Plasmoid.configuration.showExtra,
        poolMode: Plasmoid.configuration.panelPools,
        threshold: Plasmoid.configuration.panelAutoThreshold,
    })

    // The user's own five colours, defaulting to One Dark exactly as in
    // src/theme.rs, the GNOME extension and the macOS app.
    readonly property var fixedColors: ({
        low: Plasmoid.configuration.colorLow,
        mid: Plasmoid.configuration.colorMid,
        high: Plasmoid.configuration.colorHigh,
        critical: Plasmoid.configuration.colorCritical,
        empty: Plasmoid.configuration.colorEmpty,
    })

    // Referencing the Kirigami roles inside the binding is what makes this
    // reactive: they are notifying properties, so a theme switch recolours with
    // no restart. Assigning imperatively anywhere would freeze the old colours.
    readonly property var colors: Plasmoid.configuration.useThemeColors
        ? Logic.paletteFromTheme({
            textColor: Kirigami.Theme.textColor,
            neutralTextColor: Kirigami.Theme.neutralTextColor,
            negativeTextColor: Kirigami.Theme.negativeTextColor,
            positiveTextColor: Kirigami.Theme.positiveTextColor,
            disabledTextColor: Kirigami.Theme.disabledTextColor,
        })
        : root.fixedColors

    // --- applet plumbing ---------------------------------------------------
    Plasmoid.icon: "utilities-system-monitor"
    Plasmoid.status: root.failure
        ? PlasmaCore.Types.ActiveStatus
        : PlasmaCore.Types.ActiveStatus

    switchWidth: Kirigami.Units.gridUnit * 14
    switchHeight: Kirigami.Units.gridUnit * 10

    // When left click launches the TUI, the global shortcut and Enter/Space
    // must not still open a popup we have decided not to use.
    activationTogglesExpanded: Plasmoid.configuration.leftClickAction === 0
    Plasmoid.onActivated: {
        if (Plasmoid.configuration.leftClickAction === 1)
            root.launchTui();
    }

    compactRepresentation: CompactRepresentation { applet: root }
    fullRepresentation: FullRepresentation { applet: root }

    toolTipMainText: root.usage && root.usage.plan ? root.usage.plan : i18n("AI Usage Bar")
    toolTipSubText: root.failure ? root.failure : (root.notice || root.vendor)
    toolTipItem: UsageToolTip { applet: root }

    // Wording deliberately identical to the GNOME dropdown and the macOS menu.
    Plasmoid.contextualActions: [
        PlasmaCore.Action {
            text: i18n("Atualizar agora")
            icon.name: "view-refresh"
            onTriggered: root.refresh()
        },
        PlasmaCore.Action {
            text: i18n("Abrir TUI")
            icon.name: "utilities-terminal"
            onTriggered: root.launchTui()
        }
    ]

    // --- data --------------------------------------------------------------
    Plasma5Support.DataSource {
        id: reader
        engine: "executable"
        connectedSources: []

        onNewData: (sourceName, data) => {
            // MANDATORY: the source name IS the command string and stays
            // connected, re-running on the engine's own interval. Disconnecting
            // here turns every connectSource into a strict one-shot.
            disconnectSource(sourceName);
            // ...and because the source name is the command, it also carries
            // the vendor it was started for. Scrolling during a slow fetch
            // would otherwise paint the OLD vendor's numbers under the NEW
            // vendor's name. Both siblings guard this (GNOME with a refresh
            // token, macOS by re-checking the selection); here the identity of
            // the command is guard enough.
            if (sourceName !== root.currentCommand())
                return;
            root.pendingCommand = "";
            watchdog.stop();
            root.consume(data);
        }

        function exec(cmd) {
            // Connecting an already-connected source is a no-op, so a slow
            // command cannot pile up duplicate runs.
            if (connectedSources.indexOf(cmd) === -1)
                connectSource(cmd);
        }
    }

    // Separate source for fire-and-forget launches: sharing `reader` would feed
    // the terminal's output into consume() and clobber the usage state.
    Plasma5Support.DataSource {
        id: launcher
        engine: "executable"
        connectedSources: []
        onNewData: sourceName => disconnectSource(sourceName)
        function exec(cmd) {
            if (connectedSources.indexOf(cmd) === -1)
                connectSource(cmd);
        }
    }

    function currentCommand() {
        return Logic.buildCommand(Plasmoid.configuration.binaryPath,
                                  root.vendor, Marker.FORMAT, root.fetchTimeoutSecs);
    }

    function refresh() {
        const cmd = root.currentCommand();
        root.pendingCommand = cmd;
        watchdog.restart();
        reader.exec(cmd);
    }

    // Backstop only. timeout(1) in the spawned command is what bounds and kills
    // a hung binary (see buildArgv); this covers the remaining case where the
    // data engine never reports back at all, leaving the panel on stale numbers
    // with no hint anything is wrong. Both siblings guard the same way (GNOME
    // 60s, macOS 45s) and both replace the display with the error rather than
    // keeping stale data on screen, so this does too.
    //
    // Fires after timeout(1) would have, so the process-level kill is what the
    // user normally sees, not this.
    Timer {
        id: watchdog
        interval: (root.fetchTimeoutSecs + Logic.TIMEOUT_KILL_GRACE_SECS + 5) * 1000
        repeat: false
        onTriggered: {
            if (root.pendingCommand === "")
                return;
            reader.disconnectSource(root.pendingCommand);
            root.pendingCommand = "";
            root.usage = null;
            root.notice = "";
            root.failure = i18n("ai-usagebar demorou demais (>%1s)", root.fetchTimeoutSecs);
        }
    }

    function consume(data) {
        const exitCode = data["exit code"];
        const stdout = data["stdout"] || "";
        const stderr = data["stderr"] || "";

        // timeout(1) killed it. Report that as a timeout rather than letting it
        // fall through to "invalid output", which is what a half-written stdout
        // would otherwise look like.
        if (exitCode === Logic.EXIT_TIMED_OUT || exitCode === Logic.EXIT_KILLED) {
            root.usage = null;
            root.notice = "";
            root.failure = i18n("ai-usagebar demorou demais (>%1s)", root.fetchTimeoutSecs);
            return;
        }

        const res = Logic.parseWidgetJson(stdout);

        if (!res.ok) {
            // Keep the ORIGINAL output as the detail line. A bare "invalid JSON"
            // throws away the only actionable thing the user has — the same
            // mistake the maintainer rejected in #24 ("Preserve the original
            // error"), and the reason _setError in the GNOME extension is called
            // as _setError('saída inválida', stdout). Capped and shown verbatim.
            root.usage = null;
            root.notice = "";
            const headline = exitCode !== 0
                ? (stderr.trim() ? i18n("ai-usagebar falhou") : i18n("ai-usagebar saiu com %1", exitCode))
                : i18n("saída inválida");
            const detail = (exitCode !== 0 ? stderr.trim() : res.raw) || "";
            root.failure = detail ? headline + "\n" + detail.substring(0, 300) : headline;
            return;
        }

        root.failure = "";
        const parsed = Logic.parseFields(res.text);
        if (parsed.kind === "message") {
            root.usage = null;
            root.notice = parsed.text;
        } else {
            root.notice = "";
            root.usage = parsed.data;
        }
    }


    function cycleVendor(delta) {
        const next = Logic.nextVendor(Plasmoid.configuration.vendorRing, root.vendor, delta);
        if (next !== root.vendor)
            Plasmoid.configuration.vendor = next;
    }

    function launchTui() {
        launcher.exec(Logic.buildTuiCommand(Plasmoid.configuration.terminalCommand));
    }

    // A vendor change must repaint immediately, not at the next tick.
    onVendorChanged: {
        root.usage = null;
        root.notice = "";
        root.refresh();
    }

    Timer {
        interval: Math.max(root.minIntervalSecs, Plasmoid.configuration.interval) * 1000
        running: true
        repeat: true
        triggeredOnStart: true
        onTriggered: root.refresh()
    }
}
