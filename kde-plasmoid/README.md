# AI Usage Bar — KDE Plasma 6 plasmoid

A native Plasma panel widget for [`ai-usagebar`](../README.md). It puts up to
two usage values in the panel, with a click popup listing every quota window,
model-scoped rows, and the two-pool layout that Google Antigravity needs.

This is the KDE counterpart to the project's Waybar widget and its
[GNOME extension](../gnome-extension/README.md): Waybar is Wayland-bar-specific
and can't dock into a Plasma panel, so this shells out to the same
`ai-usagebar` binary and draws with native Plasma/Kirigami components.

![The plasmoid in a Plasma 6 panel showing `5h 2%` and `7d 44%` with their bars,
and its popup open below: a Claude / Claude Max 20x header with refresh and TUI
buttons, a provider tab strip reading Claude, Codex, Z.AI and OpenRouter with the
last two marked by a warning icon, then USAGE & BALANCE over Session (5h) 2%,
Weekly (7d) 44% and Fable (7d) 7% — each a full-width bar with its pace text and a
live "Resets in" countdown — and an "Updated just now" footer](../screenshots/kde-plasmoid.png)

## Vendor scope

Whatever `ai-usagebar usage --json` reports. The widget keeps no vendor list of
its own: every entry the binary reports for your `config.toml` becomes a tab in
the popup, with its canonical name and its plan or its error. Adding a vendor to
the CLI makes it appear here with no change to this package.

One caveat worth knowing before you put a vendor in the scroll ring:

- **Antigravity has no credential and no remote endpoint.** Quota is served
  only while Antigravity itself is running (the app, the IDE, or an interactive
  `agy` session). With all of them closed the entry reports an error and its tab
  is marked. That is correct, not a failure of the widget.

## Requirements

- Plasma 6 (developed against 6.6, `X-Plasma-API-Minimum-Version` is `6.0`)
- `ai-usagebar` on `PATH`, or its full path set in the widget settings
- `plasma5support` (ships with Plasma; provides the executable data engine)
- GNU coreutils `timeout` (standard on Plasma Linux distributions)

> **plasmashell does not inherit your shell's `PATH`.** A `cargo install` into
> `~/.cargo/bin` is typically invisible to the widget. Either install to a
> session-visible prefix (`make install PREFIX=~/.local`) or set the full path
> under *Configure → Binary path*.

## Install (dev)

```sh
cd kde-plasmoid
./install.sh
# then: right-click the panel → "Add or Manage Widgets…" → search "AI Usage"
```

It symlinks `package/` into `~/.local/share/plasma/plasmoids/` and restarts
plasmashell, so editing the QML only needs
`systemctl --user restart plasma-plasmashell` afterwards.

The copy-based alternative, if you prefer it:

```sh
kpackagetool6 --type Plasma/Applet --install ./package
kpackagetool6 --type Plasma/Applet --upgrade ./package   # on re-runs
```

## Install (system)

```sh
sudo make install install-plasmoid PREFIX=/usr
```

`PREFIX=/usr` is not optional for a system install: KPackage only scans
`$XDG_DATA_DIRS`, and `/usr/local/share` is normally **not** in it on a stock
Plasma session — installing under the default prefix produces a widget that
never appears in the chooser.

## Configuration

Right-click the widget → *Configure*. One page: which vendors are in the scroll
ring (each listed with its plan or its error, straight from the report), the
current vendor, the popup layout, refresh interval, command timeout, left-click
action, compact percentage and bar toggles, bar width, and the five severity
colours.

The GNOME prefs window has a second "Vendors" page listing per-vendor login
status. There is no equivalent page here because the report already carries
`status` and `error` for every configured vendor: the popup's tab strip marks a
failing one where you are already looking, and the ring checkboxes show the same
thing while you choose.

**Each panel instance keeps its own vendor.** One report covers every vendor, so
switching is a client-side re-pick rather than a refetch, and the widget never
reads `~/.cache/ai-usagebar/active_vendor` — that file belongs to Waybar's
`--cycle-next`. Two instances on one panel track two vendors, and scrolling one
never moves the other.

## How it renders

Scrolling the widget cycles the ring (up = next, as in the Waybar module's
`--cycle-next`). Left click opens the popup; middle click opens the TUI. Right
click is left to Plasma's own menu.

The popup follows the native Omarchy panel (`omarchy/Panel.qml`) so the two
native frontends read the same: header with the provider and its plan, a
provider tab strip, a tinted surface when something is wrong, the usage rows
under one heading, and a footer saying how old the data is. Only the structure
is ported — every size comes from `Kirigami.Units` and every colour from
`Kirigami.Theme`, so the widget follows whatever Plasma colour scheme you run.

Each row puts its label and value on one line, the bar under them, then the pace
text and a live countdown. The countdown is recomputed locally from the
report's absolute `reset_at`, which is why the refresh interval can default to
300s without the popup looking frozen.

Colours follow the Plasma scheme by default. Turning that off exposes the same
five One Dark colours the GNOME extension and the macOS app use; severity comes
from the report rather than from thresholds re-implemented here, so all four
frontends agree about what counts as critical.

Bars are native rectangles rather than the `█`/`░` glyphs the other frontends
use: those come from `barMarkup()`, which emits Pango markup — a GTK format Qt
does not render.

There is no pace marker. `elapsed` exists only inside the report's
human-readable `detail` string, and parsing prose to place a marker is not a
contract worth depending on; the pace text itself is rendered under each row, so
the information is kept. Restoring the marker properly means adding an
`elapsed_pct` field to the report.

### The cards layout

*Popup layout* offers an alternative view, ported from the closed #142 card
design after its review: **One card per vendor** lays every entry the report
returned out as a card, each window (session, weekly, …) rendered as its own
gauge row with the report's own severity colouring. Per-card properties are all
sourced from the same single aggregate report as the tab layout — labels,
windows, percentages, severities, staleness and error text come from the report,
never from a hardcoded provider table, so a newly added vendor gets a card with
no widget change:

- a window a vendor does not report shows *— not reported*, never a fabricated
  0% bar (a metric arriving with `percent: null` stays null — see
  `finitePercent()` in the shared logic)
- an errored vendor replaces its gauges with the failure message instead of
  showing empty bars, which would read as "0% used"
- a stale card is flagged *cached* next to the label, keeping the severity of
  its last good numbers rather than flipping red
- clicking a card selects that vendor, so the compact panel tracks it just like
  the tab strip does; the per-window pace details live in the hover tooltip

Both layouts are projections of one report, so switching the layout never
refetches.

## Shared logic

Everything that can be pure lives in `package/contents/code/plasmoid-logic.mjs`
and is table tested in Node: the report parse, entry selection, the tab strip,
row and cell projection, the severity fallback, duration arithmetic, shell
quoting and argv construction. The QML holds only what needs Qt.

It is an ECMAScript module imported by QML (`import "../code/x.mjs" as X`).
That is officially supported by Qt and used by other Plasma 6 widgets, but it is
a minority pattern in the KDE ecosystem, so two engine differences are guarded
by tests rather than left to review:

- QML's V4 engine **rejects** the ES2019 optional catch binding (`catch {`);
- V4 **silently evaluates Unicode property escapes (`\p{L}`) to false** instead
  of throwing, which once made every pool tag render empty.

`make mjs-probe` loads `probe/` in a real applet host and asserts both, along
with the one thing Node cannot check: that the fully single-quoted
`timeout -k 5 …` command survives `KShell::splitArgs(AbortOnMeta)` and runs.

## Testing

```sh
make desktop-test   # Node: GNOME marker logic + plasmoid logic (no Qt needed)
make qml-lint       # qmllint over the applet QML (needs qt6-declarative-dev-tools)
make qml-test       # instantiates UsageBar.qml offscreen and asserts what it paints
make mjs-probe      # engine contract, in a real Plasma applet host (needs plasma-sdk)
```

`qmltestrunner` can only reach the components that never touch the `Plasmoid`
attached property: the applet host injects that at runtime and KDE documents no
way to mock it, so `main.qml` genuinely cannot be instantiated in a test.
`UsageBar.qml` does not touch it, so `make qml-test` renders it offscreen for
real and asserts the fill geometry and that the colour follows the report's
severity rather than a local threshold.

The rest of the QML — the popup layout, the panel representation, the settings
page — is still exercised by hand against the checklist below.

**Manual smoke checklist**

1. Panel text matches the matching entry of `ai-usagebar usage --json`.
2. Scroll advances the ring and wraps; scrolling back reverses it. Clicking a
   provider tab switches without a refetch.
3. Two instances pinned to different vendors keep their own across a
   `systemctl --user restart plasma-plasmashell` *and* a logout.
4. Running `ai-usagebar --cycle-next` in a terminal does **not** move the
   widget — the proof it is independent of the shared state file.
5. Hover shows every quota row, and the countdowns tick between fetches.
6. Click opens the popup; the refresh and TUI buttons are visible and work
   (they were once clipped by a zero-height root).
7. Breeze Light ↔ Dark ↔ a third-party scheme recolours without a restart, and
   no `#abb2bf` / `#5c6370` leaks through.
8. Binary moved off `PATH` → the widget shows `⚠ ai`, and recovers when restored.
9. Vertical panel and a 24px panel: nothing clipped.
10. A vendor with no credential → its tab is marked and the status surface shows
    the binary's own error (the binary still exits 0).

## Troubleshooting

```sh
journalctl --user -f -u plasma-plasmashell        # QML errors
QT_LOGGING_RULES="qml.debug=true" plasmawindowed io.github.akitaonrails.ai-usagebar
kpackagetool6 --type Plasma/Applet --list | grep usagebar
```

Use `plasmawindowed` rather than `plasmoidviewer` when testing anything in the
popup: `plasmoidviewer` does not instantiate the full representation, so popup
bugs do not reproduce under it.

Plasma caches applet metadata — restart plasmashell (or run `kbuildsycoca6`)
after editing `metadata.json`.

Opening the settings logs a burst of `Setting initial properties failed: ...
does not have a property called cfg_<key>`. That is Plasma, not the widget:
`AppletConfiguration.qml` pushes every config key onto every config page, and
also pushes `cfg_<key>Default`, which no applet declares. Harmless.
