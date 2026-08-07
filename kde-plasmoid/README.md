# AI Usage Bar — KDE Plasma 6 plasmoid

A native Plasma panel widget for [`ai-usagebar`](../README.md). It puts the
**5-hour session** and **weekly** usage bars in the panel, with a click popup
listing every quota window, model-scoped rows, and the two-pool layout that
Google Antigravity needs.

This is the KDE counterpart to the project's Waybar widget and its
[GNOME extension](../gnome-extension/README.md): Waybar is Wayland-bar-specific
and can't dock into a Plasma panel, so this shells out to the same
`ai-usagebar` binary and draws with native Plasma/Kirigami components.

![The plasmoid in a Plasma 6 panel showing `5h 11%` and `7d 10%` with their bars,
and its popup open above: Max 20x / anthropic, then Session 11%, Weekly 10% and
Fable 3% as progress bars with a pace marker, each with its reset time, and the
Atualizar agora / Abrir TUI buttons along the bottom](../screenshots/kde-plasmoid.png)

## Vendor scope

Supports the vendors covered by the shared `--format` contract:
**Anthropic, OpenAI, Z.AI, OpenRouter, DeepSeek, and Google Antigravity** —
the same set as the GNOME extension. Kimi, Cursor, MiniMax and the
account-balance vendors are widget/TUI-only; use `ai-usagebar --vendor <id>`
or the TUI for those.

Two caveats worth knowing before you put a vendor in the scroll ring:

- **Antigravity has no credential and no remote endpoint.** Quota is served
  only while Antigravity itself is running (the app, the IDE, or an interactive
  `agy` session). With all of them closed the widget shows `⚠` and the tooltip
  explains why. That is correct, not a failure of the widget.
- **OpenRouter maps the same credit-consumption percentage onto both the
  session and weekly slots** (`src/openrouter/vendor.rs`), so the panel shows
  one number twice, labelled `5h` and `7d`. This is inherited from the shared
  format contract and matches the GNOME extension; fixing it properly means
  extending the contract for balance-style vendors across all frontends.
- **Balance-only vendors can still show a `Sonnet only` row.** `hasUsageWindows`
  suppresses the 5h/7d rows for DeepSeek, but the scoped slot is gated only on
  its own value being present — the same rule the GNOME dropdown uses
  (`extension.js`, the `sonnet` row is gated on `d.sonnet.pct != null`, not on
  `hasUsageWindows`). Kept identical rather than diverging unilaterally, since
  the fix belongs in the shared contract, not in one frontend.

## Requirements

- Plasma 6 (developed against 6.6, `X-Plasma-API-Minimum-Version` is `6.0`)
- `ai-usagebar` on `PATH`, or its full path set in the widget settings
- `plasma5support` (ships with Plasma; provides the executable data engine)

> **plasmashell does not inherit your shell's `PATH`.** A `cargo install` into
> `~/.cargo/bin` is typically invisible to the widget. Either install to a
> session-visible prefix (`make install PREFIX=~/.local`) or set the full path
> under *Configurar → Caminho do binário*.

## Install (dev)

```sh
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
make install install-plasmoid PREFIX=/usr
```

`PREFIX=/usr` is not optional for a system install: KPackage only scans
`$XDG_DATA_DIRS`, and `/usr/local/share` is normally **not** in it on a stock
Plasma session — installing under the default prefix produces a widget that
never appears in the chooser.

## Configuration

Right-click the widget → *Configurar*. Two pages, mirroring the GNOME prefs
window.

**Geral** — which vendors are in the scroll ring (with their live status), the
current vendor, refresh interval, left-click action, the display toggles
(`5h` / weekly / extra / percentage / bars), bar width, panel pools, and the
five severity colours.

**Vendors** — login and configuration status per vendor, with the same actions
the GNOME page offers: `Logar` / `Re-logar` for OAuth vendors, `Instalar +
logar` when the CLI is missing (installs to `~/.local` via `npm --prefix`, no
sudo, and asks first), `Configurar (TUI)` for API-key vendors, and `Abrir agy`
for Antigravity.

**Each panel instance keeps its own vendor.** The widget always passes
`--vendor` explicitly and never reads `~/.cache/ai-usagebar/active_vendor`, so
you can drop two instances on the panel for two vendors, and scrolling one
never moves the other — or a Waybar module running alongside it.

## How it renders

Scrolling the widget cycles the ring (up = next, as in the Waybar module's
`--cycle-next`). Left click opens the popup; middle click opens the TUI. Right
click is left to Plasma's own menu.

Colours follow the Plasma colour scheme by default and re-render on a theme
switch. Turning that off exposes the same five One Dark colours the GNOME
extension and macOS app use, with the same 50/75/90 severity bands. The pace
marker keeps its fixed `#61afef` in both modes, so "am I ahead of the clock"
reads identically across all three panels.

Bars are drawn with native rectangles rather than the `█`/`░` glyphs the other
frontends use — `marker-logic`'s `barMarkup()` emits Pango markup, which is a
GTK format Qt does not render. The *policy* (which colour, where the marker
goes) still comes from the shared module.

### Two-pool vendors

Antigravity reports two independent pools. The popup groups them under
**Session** and **Weekly** headings, and the panel tags each with its pool's
initial (`G 5h`, `C 7d`), widened until the two differ. *Pools no painel*
chooses which pools the panel shows: both, only the first, only the second, or
automatic (switch when the first crosses the threshold).

## Shared logic

`package/contents/code/marker-logic.mjs` is a **byte-identical copy** of
`gnome-extension/marker-logic.js`, which is the canonical file. Edit the
canonical one, then:

```sh
cp gnome-extension/marker-logic.js kde-plasmoid/package/contents/code/marker-logic.mjs
```

`plasmoid-logic.test.mjs` fails the build if the two diverge. Neither directory
can symlink the other: the GNOME extension ships as a zip to
extensions.gnome.org, and KPackage needs the file under `contents/`.

Both modules are ECMAScript modules imported by QML (`import "../code/x.mjs" as
X`). That is officially supported by Qt and used by other Plasma 6 widgets, but
it is a minority pattern in the KDE ecosystem, so two engine differences are
guarded by tests rather than left to review:

- QML's V4 engine **rejects** the ES2019 optional catch binding (`catch {`);
- V4 **silently evaluates Unicode property escapes (`\p{L}`) to false** instead
  of throwing, which once made every pool tag render empty.

`make mjs-probe` loads `probe/` in a real applet host and asserts both.

## Testing

```sh
make desktop-test   # Node: shared marker logic + plasmoid logic (no Qt needed)
make qml-lint       # qmllint over the applet QML (needs qt6-declarative-dev-tools)
make qml-test       # instantiates UsageBar.qml offscreen and asserts what it paints
make mjs-probe      # engine contract, in a real Plasma applet host (needs plasma-sdk)
```

`qmltestrunner` can only reach the components that never touch the `Plasmoid`
attached property: the applet host injects that at runtime and KDE documents no
way to mock it, so `main.qml` genuinely cannot be instantiated in a test.
`UsageBar.qml` does not touch it, so `make qml-test` renders it offscreen for
real and asserts segment widths, positions and colours — including that a bar
ahead of pace paints **two** colours, which is the one thing a pure Node test
cannot confirm. Everything else that can be pure lives in `plasmoid-logic.mjs`.

The rest of the QML — the popup layout, the panel representation, the settings
pages — is still exercised by hand against the checklist below.

**Manual smoke checklist**

1. Panel text matches `ai-usagebar --vendor <v>` in a terminal.
2. Scroll advances the ring and wraps; scrolling back reverses it.
3. Two instances pinned to different vendors keep their own across a
   `systemctl --user restart plasma-plasmashell` *and* a logout.
4. Running `ai-usagebar --cycle-next` in a terminal does **not** move the
   widget — the proof it is independent of the shared state file.
5. Hover shows every quota row, with the `│` pace marker where elapsed is known.
   (The two-colour fill past the marker is asserted by `make qml-test`; what is
   left to eyeball here is the row set and the tooltip layout.)
6. Click opens the popup; the *Atualizar agora* and *Abrir TUI* buttons are
   visible and work (they were once clipped by a zero-height root).
7. Breeze Light ↔ Dark ↔ a third-party scheme recolours without a restart, and
   no `#abb2bf` / `#5c6370` leaks through.
8. Binary moved off `PATH` → the widget shows `⚠ ai`, and recovers when restored.
9. Vertical panel and a 24px panel: nothing clipped.
10. Revoked credentials → `⚠` renders (the binary still exits 0).

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
