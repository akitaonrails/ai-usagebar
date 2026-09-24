# Installing the AI Usage Bar menu bar app (macOS)

A step-by-step guide to get the 5h / weekly usage bars into your macOS menu
bar. For configuration and how it works, see [README.md](README.md).

## Prerequisites

| Need | How |
|---|---|
| **Rust** (`rustc` 1.88+) | `rustup` |
| **Node.js 20+** | first tray build runs `npm ci` in `windows/popover/` |
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
cargo build --release --bin ai-usagebar-tray
./target/release/ai-usagebar-tray
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

```bash
git pull
cargo build --release --bin ai-usagebar-tray
# quit the running tray (popover Options → Quit) and start the new binary
./target/release/ai-usagebar-tray
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
| Two status items | quit `ai-usagebar-menubar` (legacy Swift dropdown) |
| No usage in the glyph | star metrics in Settings → Providers (max two per provider); turn on Chart Icon Only in the Menu tab |
| macOS blocks the binary (Gatekeeper) | local build — launch from Terminal; if Finder blocks it, right-click → **Open** once |
