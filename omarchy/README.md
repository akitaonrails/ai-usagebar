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
  mouse wheel switches to the next provider the bar is showing. A provider
  hidden from the bar stays in the panel. With **Show all providers** on the
  bar draws one chip per entry, and left-clicking a chip opens the panel on
  the entry that chip stands for (clicking the chip the panel already shows
  closes it).
  While the panel is open the chips it is not showing dim to 45%, so the bar
  says which entry the panel belongs to.
  The exact provider or named account is saved
  in the widget's inline `shell.json` settings and restored after shell reloads
  and sleep/unlock cycles. Right-click is not the settings shortcut.
- Panel: click the gear or press `s` to open the native QML settings page.
  Its **Show usage value in the top bar** toggle switches between the normal
  icon-and-value label and a compact icon-only label without hiding panel or
  tooltip details. Its **Show provider name in the top bar** toggle adds the
  provider's three-letter code in front of that value — the same code Waybar's
  `{vendor_short}` prints — and is off by default. Its **Color-code usage by
  level** toggle paints bar values, panel meters, and the tooltip green →
  yellow → orange → red from the Omarchy theme as usage climbs, and is off by
  default. Its **Show provider logos** toggle, on by default, draws each
  provider's own mark; off, the bar and the panel hero use the generic robot
  icon of the bar before the marks. Its **Show provider names on tabs** toggle,
  off by default, adds each provider's name beside its logo in the panel tabs;
  off, the tabs show logos only and the name on hover, and with logos off the
  names always show. Its **Top bar usage window**
  dropdown pins the bar to auto (highest), 5-hour, weekly, or monthly; the
  tooltip and panel hero echo the pinned value while the panel rows keep
  showing every window and alert state still follows the highest percent of the
  metrics not hidden. Its **Show usage as** dropdown reads percentages as what
  is used (the default) or what is left of the same window, on the bar, tooltip,
  hero and panel meters. The form is an accordion of Display, Priorities,
  Language, Top bar window, Show usage as, Primary provider, Providers and
  Credentials: one
  section is open at a time and the first is open by default. A folded section
  keeps what you typed, so Save still covers it.
  `h`/`l` or Left/Right switches provider, `j`/`k` or Up/Down scrolls, `r`,
  Enter, or Space refreshes, `p` opens usage priorities (when enabled), Tab
  moves to the neighboring bar panel, and Esc closes. Mouse-wheel and touchpad scrolling cover long settings forms faster;
  changes to settings take effect only after the Save button at the bottom.
- Shell: `omarchy-shell shell summon akitaonrails.ai-usagebar '{}'` opens the
  panel and `omarchy-shell shell hide akitaonrails.ai-usagebar` closes it.

The panel keeps the last successful report visible when a refresh fails and
labels it accordingly. Provider-level stale cache responses and hard errors
are shown inline, and unlike the bar's alert state they do not turn that bar
red: only the highest-percent visible window decides whether it is alarming, and a
refresh that yields no report at all still marks it. An inline error reads as a
notice: a translated title for its kind (sign-in needed, connection problem,
rate limited, refresh failed, not installed), the message without its internal
`credentials error:` / `HTTP error:` prefix, and, when the message names a fix
(``Run `codex login` ``, ``sign in with `droid` ``), that command with a Copy
button. Connection problems and rate limits also offer Try again. A command is
only ever copied to the clipboard (`wl-copy`), never run. While a provider is
in error, a red badge with the same icon and a one-word label sits beside
refresh and settings in place of the value pill. Absolute reset timestamps
are rendered as live countdowns, so an open panel stays accurate between
network refreshes.

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

The **Providers** section first chooses which entries appear on the top bar.
It includes custom report entries and individual named accounts. The badge counts
the display switches in this list. All built-ins, including inactive providers,
remain available in the advanced fetching section with their own enabled count.
Display changes take effect immediately and persist as exact entry IDs in the
widget's `hiddenBarEntries` setting in `shell.json`. Hidden entries remain
available in the usage panel and keep refreshing. Hiding every entry leaves a
generic usage icon so the panel and settings remain reachable. In single-entry
mode, a hidden panel selection falls back to the first entry still shown.

To enable an inactive built-in, expand **Advanced: provider fetching**, switch it
on and choose Save; normal credentials are still required. That section retains
the original controls
for enabling or disabling fetching independently from top-bar visibility.

The advanced fetching section lists every known built-in provider with an on/off
switch (the per-vendor `enabled` in `config.toml`). Turning a provider off
removes it from the bar, panel and reports; turning one on takes effect on the
next refresh. The switch only ever names built-in providers, and travels over
the same stdin patch as everything else.

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

# Color-code bar values, panel meters, and the tooltip by usage level
# (green → yellow → orange → red from the Omarchy theme). The default is false.
omarchy bar set akitaonrails.ai-usagebar colorCodeUsage true --json

# Which quota window the top bar shows: auto (highest, the historical
# default), session (5-hour), weekly (7-day), or monthly. The default is auto.
omarchy bar set akitaonrails.ai-usagebar barWindow session

# How percentages read: used (the default) or left, the share that remains of
# the same window. The bar still picks the most-used window; only the number
# drawn for it changes.
omarchy bar set akitaonrails.ai-usagebar showAs left

# Provider logos in the top bar and panel. Off restores the generic robot icon
# (and the short code on each chip with showAll). The default is true.
omarchy bar set akitaonrails.ai-usagebar brandIcons false --json

# Provider names beside the logos in the panel tabs. The default is false
# (logos only, the name on hover); without logos the names always show.
omarchy bar set akitaonrails.ai-usagebar tabNames true --json

# Panel and settings language: auto (follow the system locale, the default),
# en, ru, pt-BR, ko, or es.
omarchy bar set akitaonrails.ai-usagebar uiLocale pt-BR

# Cursor's chip lists Cursor Models, Other Models, on-demand used percent,
# then a credit grant when the account has one. These switches hide a figure
# from the top bar, the tooltip, the panel header and the panel list, like the
# pool buttons on the panel. At least one stays on. Credits is absent when there is no grant.
omarchy bar set akitaonrails.ai-usagebar showCursorModels false --json
omarchy bar set akitaonrails.ai-usagebar showCursorOther false --json
omarchy bar set akitaonrails.ai-usagebar showCursorOnDemand false --json
omarchy bar set akitaonrails.ai-usagebar showCursorCredits false --json

# Antigravity's chip shows one figure per model pool: the most-used Session or
# Weekly window of Gemini and of Claude & GPT OSS.
omarchy bar set akitaonrails.ai-usagebar showAntigravityGemini false --json
omarchy bar set akitaonrails.ai-usagebar showAntigravityClaudeGpt false --json
```

The refresh interval is clamped to 30–3600 seconds. The `provider` setting
prefers an exact entry id; if there is no exact match, a base id such as
`anthropic` selects all accounts for that provider. `showValue`,
`showProvider`, and `showAll` change only the top-bar label; `colorCodeUsage`
also recolors the panel meters and tooltip; `barWindow`
changes the top-bar value and its tooltip/hero echo; none hide report
details or change provider fetching. Cursor is the exception to the window
pin: its two included pools are model categories, so the chip shows Cursor
Models, Other Models, prepaid on-demand used percent, and a credit grant
side by side (`35% · 7% · 0% · 15%`). The Cursor page's switches hide those
figures from
the top bar, the tooltip, the panel header and the panel list, and the Metrics
section of the settings flips the same switches. The last remaining figure
cannot be turned off. A pool
the report does not contain does not count as that last figure. The bar's
urgent color follows the pools still on the chip; each panel row keeps its
own color. Antigravity works the same way: Gemini and Claude & GPT OSS are
independent pools, each represented by its most-used Session or Weekly window,
and the pool buttons above its list hide both of that pool's windows. For every
other provider, panel rows list every window and the alert
state follows the highest percent of the metrics not hidden. `barWindow` falls
back to the highest percent (balance/text where a vendor has no metric) when
a vendor lacks the pinned window (a balance-only provider, a weekly-only
response, or no monthly pool), so the bar never goes blank.

The **Metrics** section of the panel Settings chooses what each provider shows,
the way the tray's Customize does: it expands per provider, with one switch per
metric. A metric switched off disappears from the panel, the top bar and the
tooltip. The choice is saved as `hiddenMetrics` in the widget's `shell.json`
settings, a map from an entry id to the metric labels switched off
(`{"zai": ["MCP tools (monthly)"]}`; a metric under a heading is keyed
`Heading / Label`), so each account is independent. A hidden metric is ignored
when the bar picks the highest percentage and when it decides whether the icon
is alarming, so Z.AI with Session and Weekly at 0% and the monthly MCP window
switched off reads `0%` however full that window is. A row under a heading
(SuperGrok's product slices, Claude's CLI sessions) stands in for the bar only
when no other meter is left, so switching off every plain window lets such a
row set the value and the alert. The last metric still on cannot be switched
off: its switch is disabled, so the bar never goes blank (a `hiddenMetrics`
that would hide every metric, edited by hand, is ignored). Cursor and Antigravity
list their time windows in that section (Session, Weekly, Monthly): a window
switched off hides every model pool of it, and Cursor's single monthly window is
locked. The pools stay on the buttons above the list (`showCursorModels`,
`showAntigravityGemini` and friends).
`showAs` changes only the number: Left shows what remains of the most-used
window (Z.AI's 18% monthly window reads `82%`), and the alert state keeps
following the used share.

`showProvider` draws the `short_name` the Rust report ships for the selected
entry, so the codes never fork from Waybar's `{vendor_short}`: `cld 29%`,
`gpt 95%`, `agy 81%`. Every account of one provider shares that provider's
code — the panel and tooltip remain the place that tells `Claude · work` from
`Claude · personal`. With both toggles on the bar reads icon + `cld 29%`; with
`showValue` off it is the icon and `cld`. `showAll` draws every visible
entry as its own chip with a brand SVG (see [`icons/README.md`](icons/README.md)
for source and licence) — and each chip is a target for its entry across the
whole bar height and its share of the gaps beside it, so a left-click anywhere
in that column opens the panel there. Grok and SuperGrok share a mark; Grok Bot has its
own head-and-eyes logomark. Command Code has
none and falls back to its three-letter code. A `[[custom]]` provider can set
`brand = "<built-in slug>"` to use one of these marks; without it, the custom
entry keeps its three-letter tag. A vertical bar has room for none of this and
keeps showing a single icon.
Against an `ai-usagebar` older than the `short_name` field the tag falls
back to the entry id's provider half (`anthropic 29%`) until the binary is
updated.

### Local builds

Two settings point the plugin at a build other than the binaries on `PATH`.
They are meant for development, so Settings has no control for them on
purpose; set them from a shell, and set them back to the default to undo:

- `reportBinary` (default `ai-usagebar`) runs `usage --json`, `settings
  show`/`settings apply` and the Nous login.
- `tuiBinary` (default `ai-usagebar-tui`) opens the dashboard on right-click
  and from the dashboard action.

```bash
omarchy bar set akitaonrails.ai-usagebar reportBinary ~/src/ai-usagebar/target/release/ai-usagebar
omarchy bar set akitaonrails.ai-usagebar tuiBinary ~/src/ai-usagebar/target/release/ai-usagebar-tui
omarchy bar set akitaonrails.ai-usagebar reportBinary ai-usagebar
```

The report command stays a structured argument list (no shell), and the
dashboard and Nous login commands quote the path with `Util.shellQuote`.

## Usage priorities

The priorities view is off by default. Turn it on in Settings → Priorities.
It is a second view inside the same popup, built from the same native controls
and locale catalog; it opens no other application. While it is on, a
`Usage | Usage priorities` switcher sits at the bottom of the panel, outside the
scrolling content, and `p` opens the view. Esc goes back to usage. Turning it
off removes the switcher, the bar alert and the shortcut.

While the view is on, the plugin runs `usage --json --priorities` with the
settings below. While it is off, the plugin runs the plain `usage --json` and
nothing is recorded. The Rust backend owns the ranking, the rates and the
urgency rule; the plugin only formats them. The history lives in
`~/.cache/ai-usagebar/priority/history.json`: provider and metric ids,
percentages and timestamps for the last 25 hours, at most 16,384 samples.

Each card shows the provider's position, its state (urgent, spare quota, on
pace, exhausted, unavailable or no forecast) and a one-line summary. Hover for
300 ms to preview the numbers behind it, or click (Enter/Space) to pin them:
the weekly window and its reset, what is left now, the projected unused quota,
your pace, the recent and cycle rates, and the rate that would use it all.
**View limits** opens that provider's usage view.

| Setting | Default | Meaning |
|---|---|---|
| `showPriorities` | `false` | Turns the module on. |
| `showPriorityShortcut` | `false` | Always shows the recommended provider in the bar (`󰉻 Provider`). |
| `recentHours` | `3` | Hours of recent use in the estimate, 1 to 24. Changing it asks for a fresh report. |
| `priorityAlert` | `true` | Shows an urgent quota in the bar, in red, even with the shortcut off. |
| `alertUnusedPercent` | `50` | Projected unused share that makes a quota urgent. |
| `alertHours` | `24` | ...when the weekly quota resets within this many hours. |

The urgent chip reads `󰀦 Provider` in the theme's red. On a vertical bar the
chips are hidden, so the bar's single icon turns into the red alert glyph
instead, and only that icon takes clicks. A provider hidden from the bar
neither alerts nor is recommended there; the priorities view still lists it.
The bar's priority chip presses like a provider chip: left opens or closes the
priorities view, middle switches provider, right opens the dashboard.

```bash
omarchy bar set akitaonrails.ai-usagebar showPriorities true --json
omarchy bar set akitaonrails.ai-usagebar recentHours 6 --json
omarchy bar set akitaonrails.ai-usagebar alertUnusedPercent 30 --json
```

## Development checks

On an Omarchy 4 machine:

```bash
omarchy plugin validate .
make plugin-test
make omarchy-visual-test
```

`make plugin-test` runs `omarchy/model.test.mjs`, the priority presentation
tests in `omarchy/priority.test.mjs` and the notice tests in
`omarchy/notice.test.mjs`. The visual test renders the real settings component,
the usage view's notices and the priority cards (English and pt-BR) in dark and light fixture
themes through Quickshell's offscreen backend. It also renders the whole panel
(`panel-*.png`): tabs, header, meters near and at the limit, a balance and the
priorities view, from a synthetic report that `visual-tests/fake-ai-usagebar`
serves, with a stand-in for the layer-shell window. The brand marks need an
OpenGL context; without a GPU, set `AI_USAGEBAR_VISUAL_BACKEND=software` and the
logos render blank. It does not open a window, run the real `ai-usagebar` or
read credentials. Review the generated PNGs under
`target/omarchy-visual-scenarios/`; they are local test artifacts, not golden
pixel snapshots.

`qmllint` cannot resolve the `qs.*` modules that Omarchy injects at shell
runtime, so it is not a reliable standalone check for plugin entry points.

Saving files under an installed user plugin triggers Quattro's plugin hot
reload. If the running shell keeps the old panel after a change, use
`omarchy restart shell` to reload its QML. In a source checkout, rerun
`omarchy plugin validate .` after changing the manifest or entry points.
