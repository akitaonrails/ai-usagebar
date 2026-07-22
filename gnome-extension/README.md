# AI Usage Bar — GNOME Shell extension

A native GNOME top-panel indicator for [`ai-usagebar`](../README.md). It puts
the **5-hour session** and **weekly** usage bars next to the clock/network,
with optional dynamic model-scoped (for example, Fable) and extra-usage rows
in a native click dropdown.

This is the GNOME counterpart to the project's Waybar widget: Waybar is
Wayland-only (Sway/Hyprland) and can't dock into the GNOME top bar, so this
extension bridges the gap by shelling out to the same `ai-usagebar` binary and
drawing the bars with native `St` widgets. The panel is rendered by GNOME; no
GNOME screenshot is currently bundled.

## Vendor scope

The selector supports **Anthropic, OpenAI, Z.AI, OpenRouter, DeepSeek, and
Google Antigravity**. **Kimi is widget/TUI-only in this release**; desktop
protocol and marker parity for Kimi is dedicated future work. DeepSeek is
balance-only, so the extension shows its balance in the header and suppresses
the 5h/weekly quota rows.

Antigravity is the first vendor with **two independent quota pools** (Gemini,
and Claude & GPT OSS), each carrying its own 5-hour and weekly window. The
dropdown groups them under `Session` and `Weekly` headings, and the panel draws
one segment per pool per window — see [Two-pool vendors](#two-pool-vendors).
Quota comes from whichever Antigravity product is running locally (the app, the
IDE, or an interactive `agy` session); with all of them closed the extension
shows the last cached figures, then an error once those age out.

## Requirements

- GNOME Shell **45–50** (ESM extensions).
- The `ai-usagebar` binary on `PATH` (or `~/.cargo/bin`, or set an explicit
  path in preferences). Install it with `cargo install ai-usagebar` or from
  the AUR — see the [main README](../README.md).
- For the colored bars to be even, the panel uses a monospace font. For the
  dropdown's Nerd Font glyphs to render, set a Nerd Font as your monospace
  font; without one the icons show as tofu but the bars/numbers are fine.

## Install (dev)

```bash
./install.sh
# then reload the shell:
#   X11      → Alt+F2, type 'r', Enter
#   Wayland  → log out / in
gnome-extensions enable ai-usagebar@akitaonrails.github.io
```

Manual equivalent:

```bash
UUID=ai-usagebar@akitaonrails.github.io
DEST=~/.local/share/gnome-shell/extensions/$UUID
glib-compile-schemas schemas/
mkdir -p "$DEST" && cp -r * "$DEST"/      # or: ln -s "$PWD" "$DEST"
```

## Preferences

`gnome-extensions prefs ai-usagebar@akitaonrails.github.io`

| Setting | Default | Notes |
|---|---|---|
| Show 5h / weekly bar | on / on | toggle either window |
| Show percentage | on | numeric `%` next to each bar |
| Bar width | 8 | cells per bar (4–20) |
| Refresh interval | 30 s | 5–3600 |
| Vendor | `anthropic` | selectors: Anthropic, OpenAI, Z.AI, OpenRouter, DeepSeek, Antigravity (not Kimi). Anthropic, OpenAI, Z.AI and Antigravity expose generic session/weekly windows. |
| Panel pools | `both` | two-pool vendors only: `both`, first pool, second pool, or `auto` |
| Auto threshold | 95 % | `auto` switches pools once the shown one reaches this usage |
| Binary path | auto | empty = `PATH` then `~/.cargo/bin` |
| Panel area | `right` | `right` = next to network/clock; also `center`/`left` |
| Panel index | 0 | order within the area (0 = leftmost) |

## How it renders

It runs:

```
ai-usagebar --vendor <vendor> --format '{plan};;{session_pct};;{session_reset};;{weekly_pct};;{weekly_reset};;{sonnet_pct};;{sonnet_reset};;{extra_pct};;{extra_spent};;{extra_limit};;{scoped_model};;{scoped_pct};;{scoped_reset};;{session_elapsed};;{weekly_elapsed};;{scoped_elapsed};;{vendor_short};;{extra_model};;{extra_reset};;{extra_elapsed};;{session_model};;{weekly_model};;__aiub_end__'
```

parses the Waybar JSON (`{text, tooltip, class}`), extracts the formatted
fields from `text`, and draws the plan, session, weekly, optional dynamic
model-scoped (for example, Fable), and optional extra-usage values with native `St`
widgets. Colors mirror the
binary's default One Dark theme and `severity_for()` thresholds (≥90 red · ≥75
orange · ≥50 yellow · else green), so it matches the Waybar widget. The
dropdown is a native aligned menu, not the tooltip markup rendered verbatim.

Pace markers require both a real reset and elapsed-time output. Anthropic and
Antigravity supply that pair, so other vendors can render their generic windows
without a pace marker. When available, the bar
draws a fixed blue `│` marker at the elapsed-time position. The fill after that
point uses Rust's point-delta
severity bands: at least 10 points ahead is red, 1–9 ahead is orange, -10
through on-pace is yellow, and more than 10 under is green. A missing reset
(including `—`) keeps its row visible but suppresses the marker, even if an
older binary reports elapsed `0`. `{vendor_short}` distinguishes balance-only
DeepSeek from quota vendors. The final `__aiub_end__` literal is ignored;
it receives a stale `⏸` suffix so the last elapsed field remains numeric.

The subprocess is spawned **asynchronously** (`Gio.Subprocess` +
`communicate_utf8_async`) so it never blocks the shell, and all timers /
signal handlers are torn down in `disable()`.

### Model-scoped weekly window

When Anthropic reports a model-scoped weekly limit, `{scoped_model}` provides
the dynamic row label (for example, `Fable`) and `{scoped_pct}` provides its
usage. The model name is the presence signal: if `{scoped_reset}` is missing,
the extension displays `—` instead of falling back to a potentially unrelated
legacy model-specific window. Older binaries or accounts without a scoped limit
leave the model field empty and omit the dynamic row.

### Two-pool vendors

A vendor whose windows come in two independent pools names its primary rows via
`{session_model}` / `{weekly_model}`; that name is the presence signal for the
grouped layout. Antigravity is the first such vendor. When present, the dropdown
groups the four rows under `Session` and `Weekly` headings, each holding one bar
per pool (`Gemini`, `Claude & GPT OSS`), and the panel prefixes each segment
with the pool's initial (`G 5h`, `C 7d`).

The slot mapping is unchanged: the primary pool still fills the generic
`session`/`weekly` fields, so the panel toggles and the pace markers keep
working exactly as they do for single-pool vendors. Only the labels and the
visual order differ. Binaries that predate these placeholders echo them back
literally, the extension discards them, and the flat four-row layout is used —
so an older binary keeps working.

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
