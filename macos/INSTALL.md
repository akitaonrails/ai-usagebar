# Installing the AI Usage Bar menu bar app (macOS)

A step-by-step guide to get the 5h / weekly usage bars into your macOS menu
bar. For configuration and how it works, see [README.md](README.md).

## Prerequisites

| Need | How |
|---|---|
| **Command Line Tools** (for `swiftc`) | `xcode-select --install` |
| **`ai-usagebar` binary** | `cargo install ai-usagebar` (lands in `~/.cargo/bin`) |
| **Claude logged in once** | run `claude` once — its OAuth creds go to the login **Keychain**, which ai-usagebar reads automatically |

## Step by step

### 1. Get the code

```bash
git clone git@github.com:samirhvbr/ai-usagebar.git
cd ai-usagebar/macos
# (already cloned? just `git pull` and cd into macos/)
```

### 2. Install the binary (skip if you already have it)

```bash
cargo install ai-usagebar
ai-usagebar --vendor anthropic --pretty   # quick smoke test — should print bars
```

### 3. Log in to Claude once (if you haven't)

```bash
claude        # authenticates; creds land in the login Keychain
```

### 4. Build the app

```bash
./build.sh    # runs: swiftc -O ai-usagebar-menubar.swift -o ai-usagebar-menubar
```

### 5. Run it

```bash
./ai-usagebar-menubar &
```

It appears in the menu bar next to the clock (no Dock icon). Click it for the
per-window dropdown (Session / Weekly / Sonnet / Extra).

### 6. Start automatically at login

```bash
./install-agent.sh
```

This installs a LaunchAgent at
`~/Library/LaunchAgents/com.akitaonrails.ai-usagebar-menubar.plist` with
`RunAtLoad`, so the app comes back on every login (and is restarted if it
quits).

### 7. Verify it's running

```bash
launchctl list | grep ai-usagebar          # shows the agent
pgrep -lf ai-usagebar-menubar               # shows the process
```

## Managing it

**Update to a newer version**

```bash
git pull
cd macos && ./build.sh
launchctl unload ~/Library/LaunchAgents/com.akitaonrails.ai-usagebar-menubar.plist
launchctl load   ~/Library/LaunchAgents/com.akitaonrails.ai-usagebar-menubar.plist
```

**Stop / uninstall the auto-start**

```bash
launchctl unload ~/Library/LaunchAgents/com.akitaonrails.ai-usagebar-menubar.plist
rm ~/Library/LaunchAgents/com.akitaonrails.ai-usagebar-menubar.plist
```

**Change settings** — open **Preferences** from the menu bar dropdown (or
press **⌘,**): toggles for which bars, color pickers, vendor, interval, bar
width, and binary path. Changes apply live and persist (no rebuild). The
Preferences window needs macOS 12+.

## Troubleshooting

| Symptom | Fix |
|---|---|
| `swiftc: command not found` | `xcode-select --install`, then re-run `./build.sh` |
| Menu bar shows `⚠ ai` | the binary wasn't found — `cargo install ai-usagebar`, or set its path; it's searched in `~/.cargo/bin`, `/opt/homebrew/bin`, `/usr/local/bin`, then `PATH` |
| Menu bar shows `Loading…` and never updates | run `claude` once so creds exist; test with `ai-usagebar --vendor anthropic --pretty` |
| macOS blocks the binary (Gatekeeper) | it's your own local build — launching from Terminal / LaunchAgent is fine; if Finder blocks it, right-click → **Open** once |
| Bars look dim on a light menu bar | bar colors are tuned for dark mode; tweak `COLOR_*` constants and rebuild |
