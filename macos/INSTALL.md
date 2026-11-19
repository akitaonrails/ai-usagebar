# Installing the AI Usage Bar menu bar app (macOS)

A step-by-step guide to get the 5h / weekly usage bars into your macOS menu
bar. For configuration and how it works, see [README.md](README.md).

## Install a release

See the [Homebrew install and upgrade instructions](../README.md#macos-menu-bar-and-windows-tray)
to install the app and both command-line tools with Homebrew.

Download the macOS archive for your Mac from [Releases](https://github.com/akitaonrails/ai-usagebar/releases)
(`aarch64` for Apple Silicon, `x86_64` for Intel). Extract it, move **AI Usage.app**
to `/Applications`, and open the app. No Rust or Node is
needed for a release. Claude and Codex reuse their existing CLI sign-ins.

Use the **.app**, rather than the bare `ai-usagebar-tray` executable: its bundle
identity lets macOS and menu-bar managers recognize it. In particular, Hidden
Bar's macOS 27 native visibility mode inventories application bundles; a bare
tray is omitted and can disappear even on the visible side of its arrow.
The archive still includes standalone executables for CLI use.

Use `/Applications` when running Hidden Bar on macOS 27. Hidden Bar also has
a [reported issue with apps in `~/Applications`](https://github.com/dwarvesf/hidden/issues/446#issuecomment-5931939658),
even when they have a bundle identity and sit on the visible side of its arrow.

When moving from a bare tray, quit it first. In the app, turn **Launch at Login**
off and back on so the LaunchAgent points inside the installed bundle. Then
expand Hidden Bar, place AI Usage on the desired side, and collapse again.

## Build from source

### Prerequisites

| Need | How |
|---|---|
| **Rust** (`rustc` 1.90+) | `rustup` |
| **Node.js 20+** | first tray build runs `npm ci` in `windows/popover/` |
| **Python 3** | creates the application bundle |
| **Claude logged in once** | run `claude` once — its OAuth creds go to the login **Keychain**, which ai-usagebar reads automatically |

## Step by step

### 1. Get the code

```bash
git clone git@github.com:akitaonrails/ai-usagebar.git
cd ai-usagebar
```

### 2. Log in to Claude once (if you haven't)

```bash
claude        # authenticates; creds land in the login Keychain
```

### 3. Build and run the tray

```bash
cargo build --release
python3 macos/package-app.py target/release "dist/AI Usage.app" \
  "$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)"
# Quit an existing copy first; use Finder to replace it if already installed.
mv "dist/AI Usage.app" /Applications/
open "/Applications/AI Usage.app"
```

It appears in the menu bar next to the clock (no Dock icon). Left-click opens
the dashboard; right-click opens the same panel. Refresh and Settings are in
the panel header; Detect Providers, Open TUI, Start at Login, and Quit are in
Options or Settings.

Quit the old Swift `ai-usagebar-menubar` first if it is still running, or you
will see two status items.

### 4. Start automatically at login

Popover **Settings → Launch at Login**. That
writes `~/Library/LaunchAgents/com.akitaonrails.ai-usagebar-tray.plist`.

### 5. Verify it's running

```bash
pgrep -lf ai-usagebar-tray
```

## Managing it

**Update**

A tray in a writable application bundle (such as `/Applications/AI Usage.app`)
or copied somewhere of your own (such as `~/.local/bin`) updates
itself: it checks GitHub once an hour, and Options → Check for Updates asks
right away. When a release ships a macOS binary for your Mac, Install
downloads it, verifies its SHA-256, swaps it in place and restarts the tray;
nothing is compiled. The CLI and TUI beside the tray are replaced too, but only
if they are already there as plain files. Settings → Updates chooses
Automatic, Notify me or Off.

The tray leaves itself alone, and offers the release page instead, when
another tool owns the file — a Homebrew formula or Nix install, a link into place, or
a copy running straight from cargo's `target/` directory — when it may not
write its directory, and when a release has no macOS binary. Run from the
source tree, it follows the tree; rebuild:

```bash
git pull
cargo build --release
# Quit the running tray, package a new .app as above into a fresh output
# directory, then replace your installed copy and open it again.
```

**Stop / uninstall auto-start**

Turn **Launch at Login** off, or:

```bash
rm ~/Library/LaunchAgents/com.akitaonrails.ai-usagebar-tray.plist
```

**Change settings** from the popover: Options → Settings. Use the General,
Providers, Menu, Preferences, and Alerts tabs for refresh and shortcut,
provider order and stars, menu-bar display, language and appearance, and
system notifications.

## Troubleshooting

| Symptom | Fix |
|---|---|
| `npm` missing during `cargo build` | install Node.js 20+ |
| Popover is empty / stub page | `npm ci && npm run build` in `windows/popover/`, then rebuild the tray |
| Clicking the icon opens a menu instead of the popover | the WKWebView could not be built, so the status item falls back to a bare Refresh / Quit menu (#249). Quit it from that menu, rebuild, and check the error printed at launch |
| Two status items | quit `ai-usagebar-menubar` (legacy Swift dropdown) |
| Hidden Bar hides the tray even outside its hidden section on macOS 27 | launch **AI Usage.app** from `/Applications`, update Launch at Login as above, then expand Hidden Bar, Command-drag AI Usage to the right of its arrow, and collapse it again |
| No usage in the glyph | star metrics in Settings → Providers (max two per provider); turn on Chart Icon Only in the Menu tab |
| macOS blocks the binary (Gatekeeper) | local build — launch from Terminal; if Finder blocks it, right-click → **Open** once |
