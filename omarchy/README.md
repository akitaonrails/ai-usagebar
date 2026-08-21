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

## Copilot Setup

To display GitHub Copilot quota in the Omarchy bar and panel:

1. **Authenticate with GitHub CLI:**
   - For github.com: `gh auth login`
   - For GitHub Enterprise Cloud (GHE): `gh auth login --hostname <your-ghe-host>`

2. **Enable Copilot in config:**
   Add to `~/.config/ai-usagebar/config.toml`:
   ```toml
   [copilot]
   enabled = true
   # For github.com (omit if using public GitHub):
   # hostname = "your-ghe-host.ghe.com"
   # If using mise or another shim that pollutes stdout, point to the real binary:
   # gh_binary = "/usr/bin/gh"
   ```

3. **Provider switching:**
   The Copilot vendor appears in the Omarchy panel provider list (middle-click the
   bar button to cycle, or `h`/`l` in the panel). Select it to view your current
   Copilot quota and reset date.

See the main [`config.example.toml`](../config.example.toml) for all available
Copilot options (multiple accounts, custom API tokens, etc.).

## Controls

- Bar: left-click opens the native Quattro usage panel; right-click
  intentionally launches `ai-usagebar-tui` in a terminal; middle-click or the
  mouse wheel switches provider. Right-click is not the settings shortcut.
- Panel: click the gear or press `s` to open the native QML settings page.
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
```

The refresh interval is clamped to 30–3600 seconds. The `provider` setting
prefers an exact entry id; if there is no exact match, a base id such as
`anthropic` selects all accounts for that provider.

## Development checks

On an Omarchy 4 machine:

```bash
omarchy plugin validate .
qmllint -U -I /usr/share/omarchy/shell omarchy/BarWidget.qml omarchy/Panel.qml omarchy/SettingsView.qml
node omarchy/model.test.mjs
```

Saving files under an installed user plugin triggers Quattro's plugin hot
reload. In a source checkout, rerun `omarchy plugin validate .` after changing
the manifest or entry points.
