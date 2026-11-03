# Pop!_OS / COSMIC tray (experimental)

A COSMIC status-area frontend using the same GTK dashboard, report model,
local single-instance socket and five-minute refresh as `linux-mint/`.
It consumes `ai-usagebar usage --json`; provider authentication, fetching,
cache and quota interpretation remain in the Rust CLI.

## Install

Install the official `ai-usagebar` and `ai-usagebar-tui` Linux binaries first
(see the root README). On Pop!_OS 24.04 with COSMIC, install the bindings:

```bash
sudo apt install python3-gi gir1.2-gtk-3.0 gir1.2-ayatanaappindicator3-0.1
bash pop-os/install.sh
~/.local/bin/ai-usagebar-cosmic-tray
```

COSMIC's panel must have its system-tray/status-area applet enabled. Unlike
Cinnamon, it does not use the XApp Status Applet. The launcher selects Ayatana
AppIndicator even when started from an XWayland terminal. It installs a white
icon for COSMIC's dark status area; the dashboard and application-menu icon
keep their original artwork. This fixed white asset is intended for dark panels.

Open the indicator menu and choose **Open dashboard**. Use **Refresh now** to
update, **Quit** to stop. Open the dashboard directly with:

```bash
~/.local/bin/ai-usagebar-cosmic-tray --window
```

The installer writes the shared frontend/model to `~/.local/bin/`, the
COSMIC launcher there, the icon under `~/.local/share/ai-usagebar/tray/`, the
application-menu entry under `~/.local/share/applications/`, and the login
entry under `~/.config/autostart/`. A second launch reuses the same instance.
The Mint and COSMIC installers manage the same application and autostart
entries: run the installer for your current desktop. Quit an existing instance
before switching frontends or reinstalling.

Custom absolute binary locations work through the shared installer's overrides:

```bash
AI_USAGEBAR_BIN=/absolute/path/ai-usagebar \
AI_USAGEBAR_TUI_BIN=/absolute/path/ai-usagebar-tui bash pop-os/install.sh
```

No `sudo` is used by the installer, and it does not change provider settings
or credentials. It fails before writing if GTK/Ayatana or the CLI is missing.
Restart the tray after an update, or log out and back in.

## Testing and limitations

```bash
make cosmic-test mint-test
make mint-runtime-test   # optional, requires GTK and XApp introspection
```

The pure tests cover COSMIC versus Cinnamon/X11 selection, launcher argument
forwarding, white-icon selection and installation under a temporary home.
The installer tests stub dependency detection; they do not validate a real
Ayatana/GTK session. Visual testing on a live COSMIC panel is still needed
for this packaged integration. A local adaptation of this shared dashboard
has been seen in the COSMIC tray, but that is not a test of this installer.

This adds desktop integration only. It does not fix provider-specific quota
schema changes, including Cursor's newer weekly plans. Those belong in the
shared Rust provider so every frontend receives the same data.
