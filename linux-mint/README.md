# Linux Mint / Cinnamon tray

This frontend uses the official `ai-usagebar usage --json` report. It does not
implement provider authentication, quota retrieval or caching. On Cinnamon/X11,
the GTK status icon opens the dashboard with a left click and the options menu
with a right click. It uses Mint's XApp StatusIcon so the icon appears in the
XApp Status Applet. The runtime also has AyatanaAppIndicator and GTK fallbacks,
but this installer currently targets Mint and requires XApp.
The dashboard groups quota bars under each provider name and uses the provider
marks already shipped in [`omarchy/icons`](../omarchy/icons/README.md). Unknown
providers get a two-letter fallback. The marks are copied locally by the
installer; no icons are fetched from the network.
Bars follow the report's severity when no pacing data is available. With a
reset time and window duration, blue means usage is projected to leave at
least 10% spare, yellow means the projection is close to the limit, and red
with 🔥 means the limit is projected to be reached before reset. Providers
without credentials show **Não conectado** with expandable setup guidance.

## Install

Install the official `ai-usagebar` and `ai-usagebar-tui` binaries in
`~/.local/bin/` using the [project instructions](../README.md). On Linux Mint,
install Python 3, `python3-gi`, GTK3 and XApp, then run:

```bash
./linux-mint/install.sh
```

The installer writes the tray script, presentation model, launcher, autostart
entry and icons under your home directory. It does not modify the official Rust
binaries or your `~/.config/ai-usagebar/config.toml` credentials and provider
settings.
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
make mint-test
ai-usagebar usage --json
```

The Python frontend follows the report's `sections` list and the metric
`headline` contract (`percent` or `value`). It uses the `metrics` list when
`sections` is absent.
