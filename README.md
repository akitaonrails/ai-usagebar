# ai-usagebar

Waybar widget and tabbed TUI for AI plan usage across **Anthropic Claude**, **OpenAI Codex/ChatGPT**, **Z.AI (GLM)**, **OpenRouter**, and **DeepSeek**.

This started as a Rust port of [`claudebar`](https://github.com/mryll/claudebar) and stays drop-in compatible with it. It keeps the minimalist Pango-bordered tooltip, Omarchy theme auto-detection, and flock-protected OAuth refresh, then adds three more vendors and a proper testable codebase instead of one long shell script.

![Waybar widget showing `cld 29% · 1h 12m` in the top-right, with the hover tooltip showing Claude Max 20x session/weekly/sonnet/extra-usage progress bars](screenshot.png)

## Features

- **Per-vendor Waybar modules** with the same JSON shape as claudebar.
- **Tabbed TUI** (`ai-usagebar-tui`) with Tab/h/l switching, per-tab refresh, and 60-second auto-refresh. Native ratatui widgets fill the available terminal width and keep the vendor tabs visually consistent.
- **Scroll-to-cycle on the bar**: wire `on-scroll-up` / `on-scroll-down`, and one bar item cycles through your enabled vendors.
- **Config-driven primary vendor**: set `[ui] primary` once; the widget shows that vendor by default and the TUI opens on its tab.
- **Local testing tools**: `--pretty` renders ANSI-colored terminal output (auto-detects TTY), and `--watch N` re-renders every N seconds.
- **Drop-in claudebar compatibility** with the same flags (`--icon`, `--format`, `--tooltip-format`, `--pace-tolerance`, `--format-pace-color`, `--tooltip-pace-pts`, `--color-*`) and `{placeholders}`.
- **Always exits 0**, because Waybar hides modules that don't.
- **Atomic cache writes + flock**, so multi-monitor Waybar instances can coexist without API stampedes.
- **Separate transient and hard errors**: DNS/timeout failures show a quiet `Loading…`; HTTP 4xx/5xx errors put the code in the tooltip.
- **Live API smoke tests**: `make smoke` hits the real undocumented endpoints and catches schema drift early.

## Install

### Arch (AUR)

Two packages. Pick one:

```bash
yay -S ai-usagebar-bin    # prebuilt binary from GitHub Releases (fast, ~5s install)
yay -S ai-usagebar        # compiles from source (~30-60s, hermetic)
```

The `-bin` variant downloads the same x86_64 ELF that CI built and tested. The source variant compiles locally with your toolchain. Both install identical binaries to `/usr/bin/`. If you already have one installed, switch with `yay -S` the other package; pacman handles the swap through `conflicts`/`provides`.

### Other Linux / macOS (crates.io)

```bash
cargo install ai-usagebar                # compile from source (needs rustup)
cargo binstall ai-usagebar               # download prebuilt binary (needs cargo-binstall, no rustup)
```

`cargo binstall` fetches the same x86_64 / aarch64 Linux tarball the AUR `-bin` package uses. Both install `ai-usagebar` + `ai-usagebar-tui` to `~/.cargo/bin/`.

### From source

```bash
cargo build --release
sudo make install                  # → /usr/local/bin
# or
make install PREFIX=$HOME/.local   # → ~/.local/bin
```

### Windows

The **Waybar widget is Wayland-only and does not apply to Windows.** The
**`ai-usagebar-tui`** binary, however, runs natively, and `ai-usagebar --json`
/ `--pretty` work too (handy for feeding a custom tray/widget). Build with a
standard Rust toolchain:

```powershell
cargo build --release
# binaries land in target\release\ai-usagebar.exe and ai-usagebar-tui.exe
```

Credentials are read from the Windows user profile rather than `$HOME`:
`%USERPROFILE%\.claude\.credentials.json` (Anthropic) and
`%USERPROFILE%\.codex\auth.json` (OpenAI Codex). Run the official `claude` /
`codex` CLI once on Windows to populate them, exactly as on Linux/macOS.
API-key vendors (Z.AI, OpenRouter, DeepSeek) work unchanged via environment
variables or `config.toml`.

## Authentication

Each vendor authenticates a little differently. Anthropic and OpenAI use OAuth credentials that their official CLIs already wrote to disk, so **no env vars are needed.** Z.AI and OpenRouter use API keys. You can pass those through env vars or, if you don't source secrets in your shell, put them inline in `config.toml`.

| Vendor | Method | Action required |
|---|---|---|
| Anthropic | OAuth, read from `~/.claude/.credentials.json` (or the macOS login Keychain — see below) | Run `claude` once to log in. Token auto-refreshes. |
| OpenAI | OAuth, read from `~/.codex/auth.json` | Run `codex login` once. Token auto-refreshes. |
| Z.AI | API key (`ZAI_API_KEY` env or `[zai] api_key` in config) | Set either. |
| OpenRouter | API key (`OPENROUTER_API_KEY` env or `[openrouter] api_key` in config) | Set either. |
| DeepSeek | API key (`DEEPSEEK_API_KEY` env or `[deepseek] api_key` in config) | Set either. Disabled by default — add `[deepseek] enabled = true` to config. |

### Credential resolution order (for API-key vendors)

For each API-key vendor, ai-usagebar checks in this order:

1. **Env var named by `api_key_env`** in config (defaults: `ZAI_API_KEY`, `OPENROUTER_API_KEY`). If set + non-empty, used.
2. **Inline `api_key`** in the same config section.
3. Otherwise, **error** with a message naming both options.

### Security

- If you put inline `api_key` values in config, `chmod 600 ~/.config/ai-usagebar/config.toml`. The default behavior reads only env vars, which is safer when your config might be world-readable.
- Don't commit your config dir if you check it into dotfiles unless you've redacted `api_key` lines.
- OAuth credential files (`~/.claude/.credentials.json`, `~/.codex/auth.json`) are managed by their respective CLIs and already chmod-protected.

#### macOS: Anthropic credentials in the Keychain

On macOS, recent Claude Code builds don't write `~/.claude/.credentials.json` — they keep the same OAuth JSON in the **login Keychain** under the generic-password service `Claude Code-credentials`. ai-usagebar detects the missing file and transparently reads (and writes refreshed tokens back to) that Keychain item via the built-in `security` tool, so no manual step is needed. If the file *does* exist it still takes precedence, matching Linux.

## Configuration

`~/.config/ai-usagebar/config.toml` (optional — defaults enable Anthropic, OpenAI, Z.AI, and OpenRouter; DeepSeek is opt-in). Full example:

```toml
[ui]
# Which vendor the widget shows when --vendor is omitted, AND which tab
# is selected when the TUI opens. Defaults to anthropic when not set.
# primary = "anthropic"   # anthropic | openai | zai | openrouter | deepseek

[anthropic]
enabled = true
# credentials_path = "/home/you/.claude/.credentials.json"

[openai]
enabled = true
# codex_auth_path = "/home/you/.codex/auth.json"

[zai]
enabled = true
api_key_env = "ZAI_API_KEY"
# api_key = "..."          # used if ZAI_API_KEY is unset; chmod 600 the file!
# plan_tier = "lite"       # lite | pro | max — display-only

[openrouter]
enabled = true
api_key_env = "OPENROUTER_API_KEY"
# api_key = "sk-or-v1-..."

[deepseek]
enabled = true             # disabled by default; enable once you add an API key
api_key_env = "DEEPSEEK_API_KEY"
# api_key = "sk-..."       # used if DEEPSEEK_API_KEY is unset; chmod 600 the file!
```

## Quick start

```bash
# Local testing — auto-detects TTY and renders human-readable output.
ai-usagebar                        # uses [ui] primary (defaults to anthropic)
ai-usagebar --vendor openai
ai-usagebar --vendor zai
ai-usagebar --vendor openrouter
ai-usagebar --vendor deepseek

# Force Waybar JSON (e.g. piping into jq).
ai-usagebar --json

# Live preview while iterating on --format / --tooltip-format.
ai-usagebar --vendor openrouter --watch 5

# Interactive TUI with tabs.
ai-usagebar-tui
```

## Standalone TUI — no Waybar required

The two binaries are independent. If you don't run Waybar (or just want to check usage occasionally rather than have it on your bar permanently), `ai-usagebar-tui` works as a fully standalone terminal app:

```bash
ai-usagebar-tui                    # opens in your current terminal
```

It runs in any terminal emulator (Kitty, Alacritty, Foot, Ghostty, etc.), works in plain SSH sessions, and doesn't need a compositor or window manager integration. All controls and the Settings overlay work the same way. Use it as:

- An ad-hoc check ("am I close to my Claude weekly limit before I start a long session?")
- A foreground monitor on a secondary screen or tmux pane while you code
- A shell-only tool on remote machines (just install the binary; no Waybar/Hyprland dependencies)

The Waybar widget is optional. The TUI is the best way to see every enabled vendor at once, even if you never set up the widget.

## Waybar config

### Single module, scroll-to-cycle (recommended)

Use one bar item and scroll through your vendors. The TUI on-click still shows them all:

```jsonc
"modules-right": ["custom/aibar", ...],

"custom/aibar": {
    "exec": "ai-usagebar --format '{vendor_short} {session_pct}% · {session_reset}'",
    "return-type": "json",
    "interval": 300,
    "signal": 13,
    "tooltip": true,
    "on-click": "ai-usagebar-tui",
    "on-scroll-up":   "ai-usagebar --cycle-next",
    "on-scroll-down": "ai-usagebar --cycle-prev"
}
```

The `{vendor_short}` placeholder always expands to a 3-letter vendor ID (`cld` / `gpt` / `zai` / `opr` / `dsk`), so the bar text tells you which vendor is active. The other usage placeholders (`{session_pct}` for Anthropic, `{oai_session_pct}` for OpenAI, etc.) are vendor-specific. If you want one format string for every cycled vendor, prefer the generic placeholders where available. For now, `{session_pct}` works for Anthropic only; the other vendors expose their own `{oai_*}` / `{zai_*}` / `{or_*}` / `{ds_*}` families, which expand to empty strings for vendors that don't define them.

`signal: 13` lets the scroll-cycle commands refresh the bar instantly (via `SIGRTMIN+13`) instead of waiting for the next 300s interval.

If your Waybar theme puts a tray expander immediately after `custom/aibar`, such as Omarchy's `group/tray-expander` with `custom/expand-icon`, the usage text can sit very close to the expand icon. Add right padding for the module in your Waybar CSS if you want extra spacing:

```css
#custom-aibar {
    padding-right: 18px;
}
```

### Per-vendor modules

If you'd rather see them all at once:

```jsonc
"modules-right": ["custom/claude", "custom/openai", "custom/openrouter", "custom/zai", "custom/deepseek"],

"custom/claude": {
    "exec": "ai-usagebar --vendor anthropic --icon '󰚩'",
    "return-type": "json",
    "interval": 300,
    "tooltip": true,
    "on-click": "ai-usagebar-tui"
},
"custom/openai": {
    "exec": "ai-usagebar --vendor openai --icon '󱢆'",
    "return-type": "json",
    "interval": 300,
    "tooltip": true
},
"custom/openrouter": {
    "exec": "ai-usagebar --vendor openrouter --icon '󱙺' --format '{or_balance} · {or_used_today}'",
    "return-type": "json",
    "interval": 600,
    "tooltip": true
},
"custom/zai": {
    "exec": "ai-usagebar --vendor zai --icon '󰚩'",
    "return-type": "json",
    "interval": 300,
    "tooltip": true
},
"custom/deepseek": {
    "exec": "ai-usagebar --vendor deepseek --icon '󰧑'",
    "return-type": "json",
    "interval": 600,
    "tooltip": true
}
```

> Why 300s? The Anthropic and OpenAI Codex endpoints are undocumented and rate-limit aggressively below ~300s. The cache TTL is 60s so multi-monitor instances coexist, but Waybar's polling interval should stay at 300s.

### Multiple accounts (advanced)

To watch **more than one account of the same vendor** — say a personal and a
work Claude subscription — run one module per account, giving each its own
credentials file and its own cache directory:

```jsonc
"modules-right": ["custom/claude-personal", "custom/claude-work", ...],

"custom/claude-personal": {
    "exec": "ai-usagebar --vendor anthropic --icon '󰚩' --format 'p {session_pct}% · {session_reset}'",
    "return-type": "json",
    "interval": 300,
    "tooltip": true
},
"custom/claude-work": {
    "exec": "ai-usagebar --vendor anthropic --icon '󰚩' --format 'w {session_pct}% · {session_reset}' --creds-path ~/.config/ai-usagebar/accounts/work.credentials.json --cache-dir ~/.cache/ai-usagebar/anthropic-work",
    "return-type": "json",
    "interval": 300,
    "tooltip": true
}
```

- `--creds-path` points the module at a different OAuth credentials file
  (same JSON shape Claude Code writes). To capture a second account's file,
  log in with `claude` under that account and copy
  `~/.claude/.credentials.json` somewhere stable — token refreshes are
  written back to whatever file the flag names, so each account keeps
  itself alive independently. `chmod 600` the copies.
- `--cache-dir` gives the module a private cache so the two accounts don't
  overwrite each other's 60-second cache window. Any directory works; the
  per-vendor default is `~/.cache/ai-usagebar/<vendor>`.
- `--creds-path` currently applies to the **Anthropic vendor only**. For
  API-key vendors (Z.AI, OpenRouter, DeepSeek) point each module at a
  different key via a wrapper script that sets the env var, plus its own
  `--cache-dir`.
- The TUI still shows one tab per vendor (not per account); per-account TUI
  tabs are tracked in
  [#14](https://github.com/akitaonrails/ai-usagebar/issues/14).
- On macOS, the login Keychain can hold only one Claude credential per OS
  user, so additional accounts must be file-based as shown above.

#### Config-driven accounts (`--account`)

Instead of repeating `--creds-path`/`--cache-dir` on every module, name your
extra Anthropic accounts once in config and select them with `--account
<label>`:

```toml
[anthropic]
# The default account. `--vendor anthropic` with no `--account` uses this,
# exactly as before. Optional — falls back to ~/.claude/.credentials.json.
# credentials_path = "~/.claude/.credentials.json"

[[anthropic.accounts]]
label = "work"
credentials_path = "~/.config/ai-usagebar/accounts/work.json"

[[anthropic.accounts]]
label = "personal"
credentials_path = "~/.config/ai-usagebar/accounts/personal.json"
```

```jsonc
"custom/claude-work": {
    "exec": "ai-usagebar --vendor anthropic --account work --format 'w {session_pct}% · {session_reset}'",
    "return-type": "json",
    "interval": 300,
    "tooltip": true
}
```

- The **default account** is the singular `[anthropic] credentials_path` (or the
  platform default file). `--vendor anthropic` without `--account` uses it, with
  the same output and the same `~/.cache/ai-usagebar/anthropic/` cache as today.
- Each `--account <label>` gets an isolated cache at
  `~/.cache/ai-usagebar/anthropic/<label>/` automatically — no `--cache-dir`
  needed. Only *extra* accounts get a subdir; the default never moves.
- `--account` is Anthropic-only and can't be combined with `--creds-path` (both
  name a credentials file). A typo'd label fails loudly, listing the known ones.

## Hyprland: float the TUI window

By default Hyprland tiles the TUI. To make `ai-usagebar-tui` open as a centered floating window, the same way Omarchy floats its own settings TUIs (Wi-Fi/`impala`, audio/`wiremix`, Bluetooth/`bluetui`), add this to `~/.config/hypr/hyprland.conf` or any sourced `.conf`, such as `looknfeel.conf`:

```ini
# ai-usagebar TUI — float + center + fixed size. omarchy-launch-tui sets the
# app-id from the binary basename, so the class is org.omarchy.ai-usagebar-tui.
# 875x600 matches the size Omarchy gives its own `floating-window`-tagged TUIs.
windowrule = float on, match:class ^(org\.omarchy\.ai-usagebar-tui)$
windowrule = center on, match:class ^(org\.omarchy\.ai-usagebar-tui)$
windowrule = size 875 600, match:class ^(org\.omarchy\.ai-usagebar-tui)$
```

Then `hyprctl reload` (no logout needed).

> Omarchy tags a hardcoded list of TUI app-ids with `floating-window` in `~/.local/share/omarchy/default/hypr/apps/system.conf`, which then applies `float + center + size 875 600`. The rules above set those values directly, so the size is deterministic regardless of which config is sourced first. If you launch the TUI differently (e.g. `kitty -e ai-usagebar-tui`), replace the class regex with whatever `hyprctl clients` reports for your terminal.

> Hyprland 0.46+ uses the unified `windowrule` keyword with `match:…` filters. The older `windowrulev2 = …, class:…` syntax still works on legacy Hyprland but is deprecated — use the form above on current Omarchy / Hyprland releases.

## Vendor support matrix

| Vendor | Endpoint | What you see |
|---|---|---|
| **Anthropic** | `api.anthropic.com/api/oauth/usage` (undocumented) | Session (5h), Weekly (7d), Sonnet (7d), Extra usage $ |
| **OpenAI** | `chatgpt.com/backend-api/wham/usage` (undocumented; used by official `codex` CLI) | Codex 5h, Codex weekly, Code-review weekly, Credits |
| **Z.AI** | `api.z.ai/api/monitor/usage/quota/limit` (undocumented) | Session 5h, Weekly 7d, MCP tools monthly |
| **OpenRouter** | `openrouter.ai/api/v1/{credits,key}` (documented) | Balance, today/week/month spend, free vs paid tier |

### Endpoint stability

Three of the four endpoints are undocumented. The Anthropic and OpenAI endpoints are used by their official CLIs (`claude` and `codex`), so removing them would break those tools too. That makes them less shaky than scraped web endpoints. Z.AI's monitor endpoint is reverse-engineered from a third-party plugin; treat it as the most fragile one.

When an endpoint drifts, **run `make smoke`**. The live API tests check the exact fields this project depends on and produce a precise failure pointing at what changed. Paste the failure back into Claude Code and the affected `types.rs` can usually be updated mechanically.

## Format placeholders

### Shared / Anthropic (claudebar-compatible)

| Placeholder | Example |
|---|---|
| `{plan}` | `Max 5x` |
| `{session_pct}`, `{session_reset}`, `{session_bar}`, `{session_elapsed}` | `62`, `1h 30m`, `█████████████░░░░░░░`, `58` |
| `{session_pace}`, `{session_pace_indicator}`, `{session_pace_pct}`, `{session_pace_pts}`, `{session_pace_delta}`, `{session_pace_abs_delta}` | `↑`, `↑`, `12% ahead`, `4pts ahead`, `4`, `4` |
| `{weekly_*}` | same family for the 7d window |
| `{sonnet_*}` | same family for the 7d Sonnet window (empty when absent) |
| `{extra_spent}`, `{extra_limit}`, `{extra_pct}`, `{extra_bar}` | `$2.50`, `$50.00`, `5`, `█░░░░░░░░░░░░░░░░░░░` |

### OpenAI (Codex OAuth)

`{oai_plan}`, `{oai_session_pct}`, `{oai_session_reset}`, `{oai_session_elapsed}`, `{oai_session_pace}`, `{oai_session_pace_indicator}`, `{oai_weekly_*}` (same family), `{oai_code_review_pct}`, `{oai_credit_balance}`, `{oai_local_msgs}`, `{oai_cloud_msgs}`

### Z.AI

`{zai_plan}`, `{zai_session_pct}`, `{zai_session_reset}`, `{zai_weekly_pct}`, `{zai_weekly_reset}`, `{zai_mcp_pct}`, `{zai_mcp_reset}`

### OpenRouter

`{or_label}`, `{or_balance}`, `{or_total}`, `{or_used}`, `{or_used_today}`, `{or_used_week}`, `{or_used_month}`, `{or_consumed_pct}`, `{or_free_tier}`, `{or_limit}`, `{or_limit_remaining}`, `{or_balance_bar}`

### DeepSeek

`{ds_balance}`, `{ds_granted}`, `{ds_topped_up}`, `{ds_available}` — credit balance from `/user/balance`. USD is preferred when both currencies are present; falls back to CNY otherwise.

## Local development

```bash
ai-usagebar --watch 5                              # iterate on --format live
ai-usagebar --vendor openrouter --format '{or_balance} · today {or_used_today}'

make test                                          # unit + integration
source ~/.config/zsh/secrets                       # only needed for live smoke
make smoke                                         # live API drift detection
make clippy                                        # cargo clippy -D warnings
```

## TUI controls

![ai-usagebar-tui showing the OpenAI tab — Codex 5h and weekly gauges, Credits block with message-count ranges, tabs at top, key hints in the footer](screenshots/tui-openai.png)

- `Tab` / `l` / `→` — next tab
- `Shift+Tab` / `h` / `←` — previous tab
- `r` — refresh active tab
- `R` — refresh all tabs
- `s` — open Settings overlay (primary vendor + API keys)
- `q` / `Esc` / `Ctrl-C` — quit

Auto-refresh runs every 60 seconds in the background. Vendors use the same layout. Here's OpenRouter showing the credit balance gauge (red because 98% is consumed), usage-by-period totals, and tier:

![ai-usagebar-tui showing the OpenRouter tab — Credit balance gauge at 98% in red ($13.67 left of $900), Usage by period with today/week/month, paid tier](screenshots/tui-openrouter.png)

### Settings overlay

![Settings overlay floating over the TUI — Primary vendor radio (Anthropic selected), masked Z.AI API key (•••), masked OpenRouter API key (•••), Save button, key hints at bottom](screenshots/tui-settings.png)

Press `s` while the TUI is open. The overlay lets you:

- Pick the **primary vendor** that the widget defaults to and that the TUI selects on startup. Use `←` / `→` to cycle.
- Enter your **Z.AI API key** and **OpenRouter API key** inline. Keys are masked as you type; press `Ctrl-V` to reveal or hide them. Env vars (`ZAI_API_KEY`, `OPENROUTER_API_KEY`) still win at runtime if they're set; the inline key is the fallback.

Key bindings inside the overlay:

- `Tab` / `↑↓` — move between fields
- `←` / `→` — cycle primary-vendor selection (only on the vendor field)
- `Ctrl-V` — toggle key visibility on the focused key field
- `Ctrl-S` — save and close
- `Esc` — discard and close

Save writes to `~/.config/ai-usagebar/config.toml` via `toml_edit` so your existing comments and unrelated fields are preserved. The file is automatically `chmod 600`ed on save, so inline keys aren't world-readable.

After save, the Settings overlay fires `SIGRTMIN+13` so any Waybar module configured with `signal: 13` refreshes immediately. You don't need to wait for the next 300-second interval or kick the bar by hand. The TUI's own tabs also re-fetch right away, so a freshly set API key takes effect on the spot.

If your module doesn't use `signal: 13`, the signal is a no-op and the bar will refresh on its next normal tick (up to `interval` seconds away). To force-refresh manually: `pkill -SIGUSR2 waybar` (full reload).

## Theming

- One Dark palette by default.
- Auto-merges with the active Omarchy theme at `~/.config/omarchy/current/theme/colors.toml`.
- Per-color overrides: `--color-low`, `--color-mid`, `--color-high`, `--color-critical` (claudebar-compatible).

## Changelog

See [CHANGELOG.md](CHANGELOG.md) for the release history. Each release also has its own page at <https://github.com/akitaonrails/ai-usagebar/releases> with the auto-generated install snippet and checksum.

## Acknowledgements

The OpenAI and Anthropic OAuth endpoint references came from [`claudebar`](https://github.com/mryll/claudebar) and [`codexbar`](https://github.com/mryll/codexbar), both by mryll. The visual design, including the bordered Pango tooltip, severity colors, and pacing math, is theirs. This project is a Rust port with multi-vendor support.

## License

MIT.
