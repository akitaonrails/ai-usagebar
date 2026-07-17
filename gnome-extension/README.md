# AI Usage Bar — GNOME Shell extension

A native GNOME top-panel indicator for [`ai-usagebar`](../README.md). It puts
the **5-hour session** and **weekly** usage bars next to the clock/network,
with optional Sonnet-only and extra-usage rows in a native click dropdown.

This is the GNOME counterpart to the project's Waybar widget: Waybar is
Wayland-only (Sway/Hyprland) and can't dock into the GNOME top bar, so this
extension bridges the gap by shelling out to the same `ai-usagebar` binary and
drawing the bars with native `St` widgets.

![panel: `5h 2% ███░░░░░  7d 77% ██████░░`](../screenshot.png)

## Requirements

- GNOME Shell **45–48** (ESM extensions).
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
| Vendor | `anthropic` | only Anthropic has the 5h + weekly windows |
| Binary path | auto | empty = `PATH` then `~/.cargo/bin` |
| Panel area | `right` | `right` = next to network/clock; also `center`/`left` |
| Panel index | 0 | order within the area (0 = leftmost) |

## How it renders

It runs:

```
ai-usagebar --vendor <vendor> --format '{plan};;{session_pct};;{session_reset};;{weekly_pct};;{weekly_reset};;{sonnet_pct};;{sonnet_reset};;{extra_pct};;{extra_spent};;{extra_limit};;{scoped_model};;{scoped_pct};;{scoped_reset};;{session_elapsed};;{weekly_elapsed};;{scoped_elapsed};;__aiub_end__'
```

parses the Waybar JSON (`{text, tooltip, class}`), extracts the formatted
fields from `text`, and draws the plan, session, weekly, optional Sonnet-only,
and optional extra-usage values with native `St` widgets. Colors mirror the
binary's default One Dark theme and `severity_for()` thresholds (≥90 red · ≥75
orange · ≥50 yellow · else green), so it matches the Waybar widget. The
dropdown is a native aligned menu, not the tooltip markup rendered verbatim.

For windows with a real reset, the bar also draws a fixed blue `│` marker at
the elapsed-time position. The fill after that point uses Rust's point-delta
severity bands: at least 10 points ahead is red, 1–9 ahead is orange, -10
through on-pace is yellow, and more than 10 under is green. A missing reset
(including `—`) keeps its row visible but suppresses the marker, even if an
older binary reports elapsed `0`. The final `__aiub_end__` literal is ignored;
it receives a stale `⏸` suffix so the last elapsed field remains numeric.

The subprocess is spawned **asynchronously** (`Gio.Subprocess` +
`communicate_utf8_async`) so it never blocks the shell, and all timers /
signal handlers are torn down in `disable()`.

### Model-scoped weekly window

When Anthropic reports a model-scoped weekly limit, `{scoped_model}` provides
the dynamic row label (for example, `Fable`) and `{scoped_pct}` provides its
usage. The model name is the presence signal: if `{scoped_reset}` is missing,
the extension displays `—` instead of falling back to a potentially unrelated
legacy Sonnet window. Older binaries or accounts without a scoped limit leave
the model field empty and gracefully use the legacy “Sonnet only” row.
