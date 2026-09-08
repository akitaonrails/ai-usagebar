# Omarchy Quattro plugin

This is the native Omarchy 4 frontend for ai-usagebar. It runs inside
Quattro's long-lived Quickshell process and uses the shared Omarchy UI kit for
the bar button, keyboard-aware panel, hero, controls, typography, spacing,
colors, borders, and popup placement.

The plugin is deliberately a frontend. It executes fixed `ai-usagebar`
commands; the Rust binary remains the only code that reads or writes
configuration, talks to providers, manages refresh locks, and writes caches.

## Install

The plugin does not install its executable dependency. Install `ai-usagebar`
first, then install this repository as the plugin:

```bash
omarchy pkg aur add ai-usagebar-bin
omarchy plugin add https://github.com/akitaonrails/ai-usagebar.git --enable
```

To use AI Usage in place of Quattro's default Agents widget, disable the stock
widget:

```bash
omarchy plugin disable omarchy.agents
```

Omarchy clones plugin repositories into `~/.config/omarchy/plugins/`. The root
[manifest](../manifest.json) loads `omarchy/BarWidget.qml`, which owns the bar
button and loads `Panel.qml` inside the same plugin. Update or remove it with
the normal plugin commands:

```bash
omarchy plugin update akitaonrails.ai-usagebar
omarchy plugin remove akitaonrails.ai-usagebar
```

## Controls

- Bar: left-click opens the native Quattro usage panel; right-click
  intentionally launches `ai-usagebar-tui` in a terminal; middle-click or the
  mouse wheel switches provider. The exact provider or named account is saved
  in the widget's inline `shell.json` settings and restored after shell reloads
  and sleep/unlock cycles. Right-click is not the settings shortcut.
- Panel: click the gear or press `s` to open the native QML settings page.
  Its **Show usage value in the top bar** toggle switches between the normal
  icon-and-value label and a compact icon-only label without hiding panel or
  tooltip details. Its **Show provider name in the top bar** toggle adds the
  provider's three-letter code in front of that value — the same code Waybar's
  `{vendor_short}` prints — and is off by default.
  `h`/`l` or Left/Right switches provider, `j`/`k` or Up/Down scrolls, `r`,
  Enter, or Space refreshes, Tab moves to the neighboring bar panel, and Esc
  closes.
- Shell: `omarchy-shell shell summon akitaonrails.ai-usagebar '{}'` opens the
  panel and `omarchy-shell shell hide akitaonrails.ai-usagebar` closes it.

The panel keeps the last successful report visible when a refresh fails and
labels it accordingly. Provider-level stale cache responses and hard errors
are shown inline. Absolute reset timestamps are rendered as live countdowns,
so an open panel stays accurate between network refreshes.

## Settings

Open the panel and select the gear, or press `s`, for the native QML settings
form. It changes the same primary provider and API keys as the terminal
Settings overlay; both write the existing ai-usagebar config in place, preserve
comments and unrelated fields, and retain the platform-specific config path.
Stored key values are never sent to Quattro. The shell receives presence
booleans only, and changed keys travel to the Rust config owner over stdin
rather than argv or the environment. Leave a field blank to keep its current
value, or use its clear button to remove an inline key. Saving a new key also
enables that provider, matching the terminal overlay.

Not every provider has a credential field, and a missing one is not an omission.
Claude, Codex, GitHub Copilot, Cursor, Kiro, Antigravity, and Command Code
authenticate through an existing official or local login, so they never appear
in the key list. For GitHub Copilot, click **Log in with GitHub Copilot** to
run `gh auth login --web` in a terminal. Complete the login, then choose
**GitHub Copilot** under **Primary Provider** and save. That explicitly enables
`[copilot]` and makes it the app-wide default. The fetcher obtains OAuth only
through the fixed `gh auth token` command; it never parses GitHub CLI, editor,
or browser credential stores and never saves a token. A non-empty
`GITHUB_COPILOT_TOKEN` is an optional explicit override.

Existing installations need no migration: `config.toml`, environment-variable
precedence, the TUI, Waybar, macOS, and Windows behavior are unchanged. If the
plugin is updated before the `ai-usagebar` package, the form offers the terminal
settings fallback until the binary has the native settings bridge.

The plugin's display-only options remain in `~/.config/omarchy/shell.json` and
can be changed through Omarchy's bar UI or CLI:

```bash
# Show only one entry. Use an id printed by `ai-usagebar usage --json`.
omarchy bar set akitaonrails.ai-usagebar provider openai
omarchy bar set akitaonrails.ai-usagebar provider anthropic@work

# Empty means all configured entries, with switching in the panel.
omarchy bar set akitaonrails.ai-usagebar provider ''

# Numeric values need --json so shell.json stores a number.
omarchy bar set akitaonrails.ai-usagebar refreshIntervalSec 300 --json

# Booleans also need --json. The default is true for drop-in compatibility.
omarchy bar set akitaonrails.ai-usagebar showValue false --json

# Opt in to the Waybar-style provider tag. The default is false.
omarchy bar set akitaonrails.ai-usagebar showProvider true --json

# Show every configured provider's icon and usage at once. The default is false.
omarchy bar set akitaonrails.ai-usagebar showAll true --json
```

The refresh interval is clamped to 30–3600 seconds. The `provider` setting
prefers an exact entry id; if there is no exact match, a base id such as
`anthropic` selects all accounts for that provider. `showValue`,
`showProvider`, and `showAll` change only the top-bar label; none hide report
details or change provider fetching.

`showProvider` draws the `short_name` the Rust report ships for the selected
entry, so the codes never fork from Waybar's `{vendor_short}`: `cld 29%`,
`gpt 95%`, `agy 81%`. Every account of one provider shares that provider's
code — the panel and tooltip remain the place that tells `Claude · work` from
`Claude · personal`. With both toggles on the bar reads icon + `cld 29%`; with
`showValue` off it is the icon and `cld`. `showAll` draws every visible
entry as its own chip with a brand SVG (see [`icons/README.md`](icons/README.md)
for source and licence). Grok and SuperGrok share a mark; Command Code has
none and falls back to its three-letter code. A vertical bar has room for
none of this and keeps showing a single icon.
Against an `ai-usagebar` older than the `short_name` field the tag falls
back to the entry id's provider half (`anthropic 29%`) until the binary is
updated.

## Development checks

On an Omarchy 4 machine:

```bash
omarchy plugin validate .
node omarchy/model.test.mjs
```

`qmllint` cannot resolve the `qs.*` modules that Omarchy injects at shell
runtime, so it is not a reliable standalone check for plugin entry points.

Saving files under an installed user plugin triggers Quattro's plugin hot
reload. In a source checkout, rerun `omarchy plugin validate .` after changing
the manifest or entry points.
