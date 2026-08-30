# KDE Plasma widget

The widget requires Plasma 6 and an installed `ai-usagebar` binary. It searches
`PATH`, `~/.local/bin`, `~/.cargo/bin`, `/usr/local/bin`, and `/usr/bin`.

Install or update the widget from the repository root:

```bash
kpackagetool6 --type Plasma/Applet --upgrade plasmoid/package \
  || kpackagetool6 --type Plasma/Applet --install plasmoid/package
```

Then right-click the desktop, choose **Enter Edit Mode**, select **Add Widgets**,
search for **AI Usage**, and drag it onto the desktop. If Plasma still shows an
old copy after an upgrade, log out and back in before adding it again.

The widget reads the same credentials and configuration as the CLI.

## Cards

One card per vendor: Claude, OpenAI, Z.AI, Kimi, SuperGrok. Each shells out to
`ai-usagebar --json --vendor <v> --format …` using the cross-vendor
`{session_pct}` / `{weekly_pct}` aliases, which every binary vendor has
registered since ai-usagebar 0.17.0.

Adding another vendor is one `ListElement` in `vendorModel` — set `vendor` to a
value the CLI's `--vendor` accepts.

## SuperGrok

SuperGrok delegates auth and billing to the official Grok Build CLI — install
it, run `grok login`, or point `[supergrok] grok_binary` / `[supergrok]
auth_path` at it in `~/.config/ai-usagebar/config.toml`. Since grok CLI
1.0.13 dropped the billing ACP extension, the CLI's documented billing
endpoint is called directly with the login's stored key.

## Auto-update

`scripts/auto-update.sh` rebuilds the binary, installs it into `~/.local`,
and upgrades the widget. It is wired up as systemd user units in
`packaging/systemd/user/`:

- `ai-usagebar-update.timer` — runs every 12 h after boot
- `ai-usagebar-update.path` — triggers immediately whenever `src/`,
  `plasmoid/`, or `Cargo.toml` change in the repo

Enable both with:

```bash
systemctl --user enable --now ai-usagebar-update.timer ai-usagebar-update.path
```

Logs: `~/.local/state/ai-usagebar/auto-update.log`. plasmashell is restarted
only when the widget content actually changed.

## Card states

- **ok** — gauges render. A window the vendor does not report shows `—` and
  *not reported* rather than a fabricated 0% bar. This matters for OpenAI,
  which dropped the 5-hour window in July 2026 and currently sends only the
  7-day one; the row lights up again on its own once the window returns.
- **unconfigured** — muted card reading "Not configured". Credentials for that
  vendor are missing; the CLI's actionable message is in the hover tooltip.
- **error** — red card with the failure text inline.
- **stale** — data older than two refresh cycles is flagged next to the
  timestamp, so a silently failing fetch cannot pass as current.

## Refreshing

Auto-refresh every 5 minutes, on popup expand (so numbers are not minutes old
after a resume), via the toolbar button, or by clicking a single card to
refresh just that vendor.

## Kimi

Kimi needs an API key that the other cards do not. Either export `KIMI_API_KEY`,
or add it to `~/.config/ai-usagebar/config.toml`:

```toml
[kimi]
api_key = "…"
```

Until then the Kimi card shows the muted "Not configured" state.
