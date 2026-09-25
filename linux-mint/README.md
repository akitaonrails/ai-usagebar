# Linux Mint / Cinnamon tray

This frontend uses the official `ai-usagebar usage --json` report. It does not
implement provider authentication, quota retrieval or caching. On Cinnamon/X11,
the GTK status icon opens the dashboard with a left click and the options menu
with a right click. It uses Mint's XApp StatusIcon so the icon appears in the
XApp Status Applet. Other desktops use AyatanaAppIndicator or a GTK fallback.

## Install

Install the official `ai-usagebar` and `ai-usagebar-tui` binaries in
`~/.local/bin/` using the [project instructions](../README.md). On Linux Mint,
install Python 3, `python3-gi`, GTK3 and XApp, then run:

```bash
./linux-mint/install.sh
```

The installer writes the tray script, launcher, autostart entry and icon under
your home directory. It does not modify the official Rust binaries or your
`~/.config/ai-usagebar/config.toml` credentials and provider settings.
The launcher uses a local socket to show the existing window, so starting it
twice does not create duplicate tray icons.

If a Cinnamon panel applet hides other applets, keep **XApp Status Applet** on
the visible side of that applet. Otherwise the tray icon will be hidden with
the rest of the status area even while this process is running.

Run `~/.local/bin/ai-usagebar-tray --window` to open the dashboard directly.
Use the tray menu to refresh usage, launch the TUI or quit.

## Development

```bash
python3 -m py_compile linux-mint/ai-usagebar-tray
ai-usagebar usage --json
```

The Python frontend follows the report's `sections` list and the metric
`headline` contract (`percent` or `value`). It uses the `metrics` list when
`sections` is absent.
