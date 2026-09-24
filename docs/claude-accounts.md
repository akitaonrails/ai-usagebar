# Claude account guide

ai-usagebar can report several Claude accounts at once. On macOS, it can also
switch the active login used by Claude Desktop and the `claude` CLI.

## Choose a setup

| Need | Recommended setup |
|---|---|
| One Claude account | Use the default Claude Code login. No extra config is needed. |
| A few named accounts | Run `ai-usagebar account add <label>`. |
| Accounts already organized by `CLAUDE_CONFIG_DIR` | Set `[anthropic] accounts_dir`. |
| Separate Waybar modules backed by files you already manage | Use `--creds-path` and `--cache-dir`. |
| Switch Claude Desktop or the CLI on macOS | Use `ai-usagebar account switch`. |

## Add a named account

```bash
ai-usagebar account add work
```

The command:

- adds a `[[anthropic.accounts]]` entry without disturbing comments or
  formatting;
- creates a credentials directory for the account;
- runs `claude` with that account's own `CLAUDE_CONFIG_DIR`.

The login goes straight to the source ai-usagebar reads: a scoped Keychain item
on macOS or `.credentials.json` on Linux and Windows. The default Claude login
is left alone. Re-run the command to sign in again, or pass `--no-login` to
register the account without opening Claude.

Once signed in, the account appears in the TUI and native integrations without
a restart, provided `[anthropic]` is enabled.

### Configure accounts by hand

```toml
[anthropic]
# Optional default account. Without this, ai-usagebar uses the platform default.
# credentials_path = "~/.claude/.credentials.json"

[[anthropic.accounts]]
label = "work"
credentials_path = "~/.config/ai-usagebar/accounts/work/.credentials.json"

[[anthropic.accounts]]
label = "personal"
credentials_path = "~/.config/ai-usagebar/accounts/personal/.credentials.json"
```

Select one with `--account`:

```bash
ai-usagebar --vendor anthropic --account work
```

Or use it in Waybar:

```jsonc
"custom/claude-work": {
    "exec": "ai-usagebar --vendor anthropic --account work --format 'w {session_pct}% · {session_reset}'",
    "return-type": "json",
    "interval": 300,
    "tooltip": true
}
```

Each named account gets its own cache under
`~/.cache/ai-usagebar/anthropic/<label>/`. The default account keeps the
original `~/.cache/ai-usagebar/anthropic/` path.

For Claude, `--account` cannot be combined with `--creds-path`. (OpenRouter
also supports `--account` through its own account array.) An unknown label
fails with a list of valid labels. The TUI shows the default Claude tab
followed by one tab for each named account.

If a CLI account and a saved Desktop profile share a label, aggregate views
such as the TUI and `usage --json` use the Desktop profile to avoid refreshing
the same rotating token from two stores. Direct widget commands are explicit:
add `--desktop` alongside `--account` when you want the Desktop profile.

## Discover accounts from a directory

Point `accounts_dir` at a directory whose immediate children are Claude Code
config directories:

```toml
[anthropic]
accounts_dir = "~/.config/ai-usagebar/accounts"
```

Populate each account with the official CLI:

```bash
CLAUDE_CONFIG_DIR=~/.config/ai-usagebar/accounts/personal claude
CLAUDE_CONFIG_DIR=~/.config/ai-usagebar/accounts/work claude
```

This is Claude Code's standard
[`CLAUDE_CONFIG_DIR`](https://docs.claude.com/en/docs/claude-code/settings)
layout. Each subdirectory becomes an account named after the directory.

- Linux stores `.credentials.json` inside the account directory.
- macOS stores a config-dir-scoped Keychain item.
- ai-usagebar reads and refreshes each source independently.
- Explicit `[[anthropic.accounts]]` entries override discovered accounts with
  the same label.
- A missing or unreadable `accounts_dir` is ignored.

Any account manager that uses the same directory layout can share these logins
with ai-usagebar.

## Use existing credential files in Waybar

This lower-level setup is for credential files you already manage. Prefer
`account add` for new logins so you never copy an active refresh token.

```jsonc
"modules-right": ["custom/claude-personal", "custom/claude-work", ...],

"custom/claude-personal": {
    "exec": "ai-usagebar --vendor anthropic --icon '󰚩' --format 'p {session_pct}% · {session_reset}'",
    "return-type": "json",
    "interval": 300,
    "tooltip": true
},
"custom/claude-work": {
    "exec": "ai-usagebar --vendor anthropic --icon '󰚩' --format 'w {session_pct}% · {session_reset}' --creds-path ~/.config/ai-usagebar/accounts/work.credentials.json --cache-dir ~/.cache/ai-usagebar/anthropic-work",
    "return-type": "json",
    "interval": 300,
    "tooltip": true
}
```

Keep these rules in mind:

- `--creds-path` must point to an independently managed Claude OAuth file.
  Refreshes are written back to that file.
- Never run two clients against copies of the same refresh token. Token
  rotation will eventually strand one copy.
- Keep credential files at mode `600`.
- Give each module a separate `--cache-dir`.
- `--creds-path` is Claude-only. For API-key providers, use a wrapper that
  exports the account's key and give each module its own cache directory.

On macOS, prefer `accounts_dir`; scoped Keychain items avoid copied credential
files entirely.

## Switch the active account on macOS

Usage reporting and the active login are separate. macOS has two independent
Claude identities:

- Claude Desktop, signed in through its own `config.json`;
- the `claude` CLI, whose default login lives in the login Keychain.

Use the same label for both if they belong to the same account:

```bash
ai-usagebar account add work
ai-usagebar account add work --desktop
ai-usagebar account status
ai-usagebar account status --json
ai-usagebar account switch work --dry-run
ai-usagebar account switch work --desktop
ai-usagebar account switch work --cli
```

Without `--desktop` or `--cli`, `switch` handles both identities. If a label
exists on only one side, the missing side is skipped.

### Capture a Desktop account

The CLI supports isolated logins through `CLAUDE_CONFIG_DIR`. Claude Desktop
has only one login slot, so `account add <label> --desktop` must:

1. save the current Desktop login as a profile;
2. sign out and wait for the new login;
3. capture what Desktop writes;
4. seed the new profile with this machine's existing history.

Ctrl-C or a five-minute timeout restores the original login. CLI and Desktop
use different OAuth clients, so each identity must be captured separately.

### Switch Claude Desktop

Before switching, ai-usagebar merges local history into the target profile.
Session indexes use the newest copy; routines and schedules are merged by id.
It then quits Desktop, swaps the credential and browser state, and reopens the
app.

Every switch creates a rollback archive in `~/.claude-acc/backups/`:

- `--keep-backups N` controls retention (default: 10).
- `--backup-sessions` includes the full session tree.
- On Unix, the directory is mode `0700` and archives are mode `0600` because
  they contain credentials and browser state.

The switch clears `bridge-state.json` because stale cloud-session ids can stop
`/remote-control` from disconnecting. Pass `--keep-bridge` only when testing
that behavior.

### Switch the CLI

The CLI has one default credential slot. A switch first saves the outgoing
credential under its account, then moves the target credential into the
default slot. ai-usagebar reads an active account from that default slot while
its own file is gone — which is exactly what the switch leaves behind — so a
rotating refresh token is never live in two places.

A `CLAUDE_CONFIG_DIR` layout is the other case. There every directory keeps its
own live login, and two of them can hold the *same* account, which is what the
active-account marker records. An account whose own credential file is still
there is therefore read from that file, not from the default slot: otherwise a
perfectly good account reports a re-auth prompt from a slot its owner never
signs into.

If the current CLI login is not managed by ai-usagebar, the switch stops before
discarding it. `--force` overrides that safeguard and removes the unmanaged
login.

### Adopt the login you already use

The account you signed into first usually lives only in the default slot. Rather
than signing it in again (which mints a second, independent grant for the same
account), register it as it is:

```bash
ai-usagebar account add main --adopt-current
```

This writes only the identity marker in the new account's directory. The
credential stays in the default slot, and the first switch away from `main`
saves it into `main`'s own slot like any other outgoing login. With every
account named, `[anthropic] show_default_account = false` drops the extra
unnamed tab.

### Switch Codex

Codex works the same way. The Codex CLI, the Codex desktop app and the IDE
extension all read `~/.codex/auth.json`, so one switch moves all three:

```bash
ai-usagebar account add main --codex --adopt-current  # the login ~/.codex already has
ai-usagebar account add work --codex                  # CODEX_HOME=~/.codex-work codex login
ai-usagebar account switch work --codex
```

The switch saves the outgoing login back into its account's `auth.json`, moves
the target's file into `~/.codex/auth.json`, and records each account's ChatGPT
account id in a small `.ai-usagebar-account.json` marker next to its file (no
token), so an account whose file was moved away is still recognized. Reads for
the active account follow it into `~/.codex/auth.json`. A Codex session that was
already open keeps working until its token expires: before refreshing, Codex
reloads `auth.json` and skips the refresh when the account changed, so it cannot
overwrite the new login. Restart it to use the new account.

### Switch from the macOS menu bar

With named accounts configured, each account's card in the `ai-usagebar-tray`
popover shows a switch control beside Customize and Reset. The active login has
a green check; any other account has a switch button that runs the same
`ai-usagebar account switch` (with `--codex` for a Codex card). A Claude switch
quits and reopens Claude Desktop when that account also has a Desktop profile.
The button spins while the switch runs, and a failed switch turns it red with
the reason in its tooltip.

### Storage and history conflicts

CLI accounts use `[[anthropic.accounts]]` or `accounts_dir`. Desktop profiles
use claude-acc's format under `~/.claude-acc/profiles`; override that path with
`[anthropic] desktop_profiles_dir`. Existing claude-acc profiles work as-is.

History merges can expose deletions that another account has not seen yet.
When that happens, ai-usagebar asks whether to keep every copy, delete the item
from all accounts, or decide one item at a time. Deleting a chat removes only
its index; transcripts under `~/.claude/projects/` are never touched.

Non-interactive switches always keep conflicting items. The macOS menu bar
shows the same choices in a dialog. For scripts, `account status --json` lists
pending `deletion_conflicts`; pass the returned opaque key through
`--delete-conflict <key>`. Keys are scoped by item type, so a routine id cannot
authorize deletion of a chat with the same id.

Chats reconcile by `lastActivityAt`. Routines use a per-task three-way
baseline. Concurrent edits to the same routine keep both local copies and are
reported as a conflict. Edit the preferred copy again to resolve it on the next
switch.

Account removal and chat filters (`only` / `reset`) are not implemented. Remove
a profile directory manually or use claude-acc. Cowork sessions stay with the
account that created them because their transcript path contains the account
UUID.

The Desktop profile format and switching behavior are based on
[claude-acc](https://github.com/ohmaseclaro/claude-acc) (MIT). The Desktop
versions of `add` and `switch` share its profile store.
