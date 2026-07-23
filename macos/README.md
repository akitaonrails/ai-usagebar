# AI Usage Bar — macOS menu bar app

A native macOS menu bar app for [`ai-usagebar`](../README.md). It shows the
**5-hour (session)** and **weekly** usage bars — plus an optional
dynamic **model-scoped** bar (for example, Fable) and **extra-usage (cost)**
bar — in the menu bar next to the clock, with a native dropdown. It's the macOS counterpart to the [GNOME Shell
extension](https://github.com/akitaonrails/ai-usagebar/tree/main/gnome-extension): same binary, same One Dark colors and
severity thresholds.

A single Swift file (`NSStatusItem` + `NSAttributedString`); no Xcode project.

> **Installing?** Follow the step-by-step in **[INSTALL.md](INSTALL.md)**.

## Vendor scope

The selector supports **eleven vendors** that ship in the binary:

- **Rate-limit windows (5h / weekly):** Anthropic (Claude), OpenAI (Codex), and
  Z.AI (GLM).
- **Balance-only:** OpenRouter, DeepSeek, Kimi, Kilo, Novita, Moonshot, Grok
  (xAI), and Anthropic (API). These have no 5h/weekly quota windows, so the app
  shows their balance/credits in the header (`cr <amount>`) and suppresses the
  session/weekly rows. Anthropic (API) additionally renders a spend-vs-limit
  bar when a monthly limit is configured.

Only **enabled** vendors appear in the selector. The opt-in balance vendors
(DeepSeek, Kimi, Kilo, Novita, Moonshot, Grok, Anthropic API) default to
disabled in the Rust config, matching `src/config.rs`; set
`[vendor].enabled = true` (or save an API key via the TUI) to turn one on.

Google Antigravity is **not** supported in the macOS app: the binary only
discovers its local language server on Linux, so on macOS it has no reachable
quota source.

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
./run-tests.sh             # optional: pure-logic test harness
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
| Show pace marker | on | persisted `showMeta`; draws the elapsed-time marker only when the window has reset and elapsed output |
| Bar width | 8 | cells per menu-bar bar (4–20) |
| Colors (low/mid/high/critical/empty) | One Dark | bar color per severity (≥90 / ≥75 / ≥50 / else) |
| Refresh interval | 30 s | 5–3600 |
| Vendor | anthropic | selectors: only enabled vendors (see [Vendor scope](#vendor-scope)). Anthropic, OpenAI, and Z.AI expose session/weekly windows; balance-only vendors show a credit balance instead. |
| Binary path | auto | empty = `~/.cargo/bin`, Homebrew, then `PATH` |

The Preferences window needs **macOS 12+** (the menu bar itself works on
10.15+). Tags/labels use the system label colors, so they adapt to a light or
dark menu bar; only the bar fill/empty colors are configurable.

Pace markers require both a real reset and elapsed-time output. Currently only
Anthropic's elapsed placeholder family supplies that pair, so other vendors can
render their generic windows without a pace marker. When available, the fixed
blue `│` pace marker is placed at elapsed time. Fill past the marker follows the
point-delta colors used by the Rust widget: at
least 10 points ahead is critical/red, 1–9 ahead is high/orange, -10 through
on-pace is mid/yellow, and more than 10 under is low/green. Windows without a
reset (including a displayed `—`) retain their row but do not draw a marker.

## Indicator style

The "Estilo do indicador" preference chooses between **block bars** (`░█`, the
default) and a **ring** (`○`) drawn with Core Graphics. The ring paints the
usage fraction as a severity-colored arc over a faint track, with the same pace
marker as the block bar: calm fill from 12 o'clock to the elapsed tick, then
warning-colored overshoot from the tick to the current percentage. Both the menu
bar and the dropdown rows honor the choice. The track adapts to the effective
appearance — faint white on dark menu bars (where the dark `COLOR_EMPTY` would
be invisible) and `COLOR_EMPTY` on light ones.

## Quick vendor switch

A **"Trocar vendor"** submenu in the dropdown (between the usage rows and
Preferences) lists only configured vendors, with a checkmark on the active one.
Selecting one switches immediately, without opening Preferences.

## How it works

Runs `ai-usagebar --vendor <v> --format '{plan};;{session_pct};;…'`, parses the
Waybar JSON (`{text, …}`), and draws the bars as colored `NSAttributedString`s
in the status item and the dropdown. The subprocess runs **off the main thread**
(`DispatchQueue.global` → back to `.main` for UI), so the UI never blocks.
