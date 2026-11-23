# AI Usage Bar — GNOME Shell extension (42–44)

A native GNOME top-panel indicator for [`ai-usagebar`](../README.md). It puts
each chosen provider's **5-hour session** and **weekly** usage next to the
clock/network. Its native click menu lists every provider that is both
configured and enabled, with the windows and details the binary reports.

This is the **legacy-imports frontend** for GNOME Shell 42, 43 and 44. GNOME
45 and newer use [`../gnome-extension`](../gnome-extension), which is an ES
module extension; 42–44 load extensions through the old importer and cannot
run it. Install only one of the two trees: both use the same UUID and settings
schema.

This is the GNOME counterpart to the project's Waybar widget: Waybar is
Wayland-only (Sway/Hyprland) and can't dock into the GNOME top bar, so this
extension calls the same `ai-usagebar` binary and draws the bars with GNOME's
native `St` widgets.

## Vendor scope

The **top bar** selector supports **Claude, Codex, OpenCode Go, Z.AI, OpenRouter,
DeepSeek, and Google Antigravity**. The **click menu** does not use that list.
It draws every entry `ai-usagebar usage --json` returns, including named
accounts, enabled custom providers and providers the bar selector does not
offer.

DeepSeek is balance-only, so a DeepSeek block shows its balance and suppresses
the 5h/weekly quota rows when the report has no percentage window. An entry
that carries an error shows that error instead of a 0% bar. A window the
report omits is left out rather than drawn as empty.

Antigravity is the first vendor with **two independent quota pools** (Gemini,
and Claude & GPT OSS), each carrying its own 5-hour and weekly window. The menu
shows the report's `Session` and `Weekly` headings with one row per pool, and
the **top bar** draws one segment per pool per window — see [Two-pool vendors](#two-pool-vendors).
Quota is read locally first, from the app, IDE or an interactive `agy`
session. If there is no local server or its `agy` session cannot be queried,
the binary can use the saved sign-in via `agy` or the Cloud Code API. A
running product that is signed out reports that error instead. See the
[modern GNOME extension notes](../gnome-extension/README.md#vendor-scope)
for the shared fallback behavior.

## Requirements

- GNOME Shell **42–44** (legacy imports). For 45–51 use
  [`../gnome-extension`](../gnome-extension).
- The `ai-usagebar` binary on `PATH` (or `~/.cargo/bin`, or set an explicit
  path in preferences). Install it with `cargo install ai-usagebar` or from
  the AUR — see the [main README](../README.md).
- The top bar uses a monospace font to align its text bars. The menu uses
  native widgets and does not require a Nerd Font.

## Install (dev)

```bash
./install.sh
# then reload the shell:
#   X11      → Alt+F2, type 'r', Enter
#   Wayland  → log out / in
gnome-extensions enable ai-usagebar@akitaonrails.github.io
```

The installer won't replace another extension at that UUID. If switching
from the modern extension or a copied install, disable it and move its
installed directory or symlink aside before running this script.

## Preferences

`gnome-extensions prefs ai-usagebar@akitaonrails.github.io`

| Setting | Default | Notes |
|---|---|---|
| Show 5h / weekly bar | on / on | toggle either window; with nothing left to draw, the top bar shows the vendor's icon |
| Show percentage | on | numeric `%` next to each bar |
| Pace arrows | off | with bars off, colors each value by pace (usage minus elapsed) and adds `↓` under, `→` on, `↑` ahead; off keeps plain numbers |
| Bar width | 8 | cells per bar (4–20) |
| Refresh interval | 30 s | 5–3600 |
| Top bar vendors | `anthropic` | Top bar only. One switch per provider (Claude, Codex, OpenCode Go, Z.AI, OpenRouter, DeepSeek, Antigravity, SuperGrok); each one on gets its own indicator, marked by its brand icon. Stored as a comma-separated list in `vendor`. The click menu shows the entries from `usage --json`. |
| Panel pools | `both` | two-pool vendors only: `both`, first pool, second pool, or `auto` |
| Auto threshold | 95 % | `auto` switches pools once the shown one reaches this usage |
| Binary path | auto | empty = `PATH` then `~/.cargo/bin` |
| Panel area | `right` | `right` = next to network/clock; also `center`/`left` |
| Panel index | 0 | order within the area (0 = leftmost) |
| Provider summary | bars and values | Menu only. Choose values only to hide the metric bars. |
| Show provider icons | on | Menu only. Controls the selected provider's symbolic mark; providers without an asset omit it. |
| Compact spacing | off | Menu only. Reduces panel and metric spacing without hiding data. |

## Legacy shell notes

GNOME 42's `St.ScrollView` does not register a `child` passed to its
constructor, and removing a widget with that unregistered child aborts the
shell (`st_bin_destroy: assertion failed`). The tabs scroll view is therefore
built with `add_actor()`, and each menu refresh builds a new panel off-stage
and swaps it into the host item instead of destroying live children. The
parity test (`legacy-parity.test.mjs`) guards both constraints, the export
surface and the value computations against the ESM frontend.

## How it renders

The **top bar** runs:

```
ai-usagebar --vendor <vendor> --format '{plan};;{session_pct};;{session_reset};;{weekly_pct};;{weekly_reset};;{sonnet_pct};;{sonnet_reset};;{extra_pct};;{extra_spent};;{extra_limit};;{scoped_model};;{scoped_pct};;{scoped_reset};;{session_elapsed};;{weekly_elapsed};;{scoped_elapsed};;{vendor_short};;{extra_model};;{extra_reset};;{extra_elapsed};;{session_model};;{weekly_model};;__aiub_end__'
```

parses the Waybar JSON (`{text, tooltip, class}`), extracts the formatted
fields from `text`, and draws that one provider's session and weekly values
with native `St` widgets. Colors mirror the
binary's default One Dark theme and `severity_for()` thresholds (≥90 red · ≥75
orange · ≥50 yellow · else green), so it matches the Waybar widget.

The **click menu** runs `ai-usagebar usage --json` when opened and at the
configured interval while it is open. It shows one provider at a time in a
tabbed panel, preserving the selected tab while the Shell process remains
running. Tabs follow report order and include named accounts and custom
providers without frontend registration. A horizontally scrolling tab strip
keeps large provider lists usable.

The selected tab shows the provider's plan, metric labels and values, bars,
reset countdowns, text and blocks. The report supplies bar severity. Absent
metrics stay absent; a balance keeps its formatted value, including a negative
sign. Errors remain messages rather than becoming zero usage. The header has
**Refresh now** and **Settings** buttons, and **Open TUI** remains below the
panel. The footer records when the current report was received.

**Menu appearance** controls bars, the selected provider icon, and spacing
independently of the top bar. Changes apply immediately and preserve the
selected tab. Bar fills reuse the existing severity colours;
backgrounds, text and selection follow the Shell theme. Provider artwork is
reused from the Omarchy integration; see [sources and licences](icons/README.md).
Assets are optional: a new provider works without adding an icon or editing a
frontend registry.

Pace markers need a real reset and the elapsed share of the window. The top
bar uses the `{*_reset}` / `{*_elapsed}` fields and its existing point-delta
colors. The menu computes marker positions from `reset_at` and `window_secs`,
which Rust supplies for windows of an exact length. Without both, it draws no
marker. With bars off and **Pace arrows** on, the top bar moves the pace into
the value instead: the percentage is colored by usage minus elapsed, with the same point-delta bands,
and followed by `↓` (more than 10 points under), `→` (within 10 under), or
`↑` (ahead). Menu fills use the report's severity; countdowns use the absolute
reset time. The panel's final `__aiub_end__` literal receives any stale suffix
so that the preceding elapsed field remains numeric.

For a fresh, active quota with an exact window length, the menu can show
`on pace for X%`. The trend line appears after three recent samples.
The extension requests `usage --json --trend` only when refreshing an open
menu; it does not poll for history while closed. Older binaries still show
the normal report menu.

Both subprocesses are spawned **asynchronously** (`Gio.Subprocess` +
`communicate_utf8_async`) so it never blocks the shell, and all timers /
signal handlers are torn down in `disable()`.

### Model-scoped weekly window

When Anthropic reports a model-scoped weekly limit (for example, `Fable`), the
menu shows it as one more metric row of the Claude block, straight from the
report. The top bar draws only the session and weekly windows.

### Two-pool vendors

A vendor whose windows come in two independent pools names its primary rows via
`{session_model}` / `{weekly_model}`; that name is the presence signal for the
top bar's grouped layout. Antigravity is the first such vendor. When present,
the panel prefixes each segment with the pool's initial (`G 5h`, `C 7d`). The
menu needs no such signal: the report itself lists one row per pool under
`Session` and `Weekly`.

The slot mapping is unchanged: the primary pool still fills the generic
`session`/`weekly` fields, so the panel toggles and the pace markers keep
working exactly as they do for single-pool vendors. Binaries that predate these
placeholders echo them back literally, the extension discards them, and the
panel draws single-pool segments — so an older binary keeps working.

Each pool keeps **its own countdown**. The two 5-hour windows can look
synchronised while both are untouched — an unused bucket's reset slides with the
clock and only anchors on first use — but they diverge as soon as either pool is
used, so they are never merged into one row.

`Panel pools` selects which pools the panel draws: `both` (default, four
segments), either pool alone, or `auto`, which shows the preferred pool and
falls back to the other once **either** of its windows reaches `Auto threshold`.
An unavailable pool is omitted; selecting it explicitly falls back to the pool
that has data instead of leaving the panel blank.
It composes with the 5h/weekly toggles — turning the weekly bar off leaves two
segments, one per pool, at the same width as a single-pool vendor.
