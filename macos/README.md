# AI Usage Bar — macOS menu bar app

A native macOS menu bar app for [`ai-usagebar`](../README.md). It shows the
**5-hour (session)** and **weekly** usage bars — plus an optional
**extra-usage (cost)** bar — in the menu bar next to the clock, with a native
dropdown. It's the macOS counterpart to the [GNOME Shell
extension](../gnome-extension/): same binary, same One Dark colors and
severity thresholds.

A single Swift file (`NSStatusItem` + `NSAttributedString`); no Xcode project.

> **Installing?** Follow the step-by-step in **[INSTALL.md](INSTALL.md)**.

## Requirements

- macOS with the **Command Line Tools** (`xcode-select --install`) for `swiftc`.
- The `ai-usagebar` binary on the Mac. Install it with `cargo install ai-usagebar`
  (lands in `~/.cargo/bin`) — see the [main README](../README.md).
- Run `claude` once on the Mac so its OAuth creds are in the login **Keychain**;
  ai-usagebar reads them there automatically (no env vars).

## Build & run

```bash
cd macos
./build.sh                 # swiftc -O → ./ai-usagebar-menubar
./ai-usagebar-menubar &    # appears in the menu bar (no Dock icon)
```

Start at login:

```bash
./install-agent.sh         # installs a LaunchAgent (RunAtLoad)
```

> Not code-signed. It's a local binary you built yourself, so Gatekeeper
> doesn't block it when launched from the terminal / LaunchAgent. If macOS ever
> complains, right-click the binary in Finder → **Open** once.

## Configuration

Open **Preferences** from the dropdown (or press **⌘,**) — a native window
with toggles, color pickers, vendor, interval, bar width, and binary path.
Settings persist in `UserDefaults` and apply **live, no rebuild**.

| Setting | Default | Notes |
|---|---|---|
| Show 5h / weekly / extra | on / on / off | which bars appear |
| Show percentage/value | on | numeric value next to each bar |
| Show bars | on | off = numbers only |
| Bar width | 8 | cells per menu-bar bar (4–20) |
| Colors (low/mid/high/critical/empty) | One Dark | bar color per severity (≥90 / ≥75 / ≥50 / else) |
| Refresh interval | 30 s | 5–3600 |
| Vendor | anthropic | only Anthropic has the 5h + weekly windows |
| Binary path | auto | empty = `~/.cargo/bin`, Homebrew, then `PATH` |

The Preferences window needs **macOS 12+** (the menu bar itself works on
10.15+). Tags/labels use the system label colors, so they adapt to a light or
dark menu bar; only the bar fill/empty colors are configurable.

## How it works

Runs `ai-usagebar --vendor <v> --format '{plan};;{session_pct};;…'`, parses the
Waybar JSON (`{text, …}`), and draws the bars as colored `NSAttributedString`s
in the status item and the dropdown. The subprocess runs **off the main thread**
(`DispatchQueue.global` → back to `.main` for UI), so the UI never blocks.
