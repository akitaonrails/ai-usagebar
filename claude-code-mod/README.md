# Claude Desktop accounts — a Claude Code mod

Switch which account the **Claude Desktop** app is signed in as, from a Claude
Code session in the app's Code tab (or a terminal). It shows each account's
quota beside it and drives `ai-usagebar account switch --desktop`.

## Requirements

- macOS with the Claude Desktop app.
- Claude Code **2.1.287+**, the first version that loads mods.
- `ai-usagebar` installed, as AI Usage.app in `/Applications` or the CLI on
  `PATH` / `~/.cargo/bin`.
- At least one Desktop account saved:

  ```bash
  ai-usagebar account add work --desktop       # the account the app is signed in as now
  ai-usagebar account add personal --desktop   # then each other one (the app reopens at its sign-in)
  ```

## Install

1. Get this folder on your Mac, for example with `git clone https://github.com/akitaonrails/ai-usagebar ~/ai-usagebar`.
2. Point Claude Code at it in `~/.claude/settings.json`. Several folders are separated with `:`.

   ```json
   {
     "env": {
       "CLAUDE_CODE_PLUGIN_DIRS": "/Users/you/ai-usagebar/claude-code-mod",
       "CLAUDE_CODE_PLUGIN_DIR_WATCH": "1"
     }
   }
   ```

   `CLAUDE_CODE_PLUGIN_DIR_WATCH=1` is optional. It makes Desktop sessions
   reload the mod when the folder changes, for example after a `git pull`.
3. Quit and reopen the Claude app. Both variables are read when a session starts.

In a terminal, `claude --plugin-dir ~/ai-usagebar/claude-code-mod` loads it for one session.

## Use

`/claude-account` opens the **Claude accounts** pane:

- **The list:** every saved Desktop account, with its e-mail, the active one marked, and a **Switch** button on each of the others.
- **Quota:** under each account, its quota windows (`5h`, `7d`, model weeklies) as a bar and a percentage, colored by how much is used, with the time to reset. They come from `ai-usagebar usage --json` and load after the list.
- **Switching:**
  1. **Switch** first runs `account switch --desktop --dry-run` and shows the plan.
  2. **Switch and reopen** then does it. Claude quits, the saved login and local history move to that account, a rollback archive is written, and the app reopens.
  3. Every open session restarts, the one that pressed the button included.

The status line shows the active Desktop account (`Desktop: work`).

## How the switch outlives Claude

Sessions in the desktop app are children of Claude.app, so anything they start
dies when the app quits, and a switch run that way would never reopen it. The
mod hands the switch to launchd instead:

```
/bin/launchctl submit -l com.akitaonrails.ai-usagebar.desktop-switch.<ms> \
  -o ~/Library/Logs/ai-usagebar-desktop-switch.log -e <same> -- \
  /bin/sh -c '<JOB_SCRIPT>' sh <job> <ai-usagebar> account switch --desktop --yes -- <label>
```

- **Outside Claude:** the job belongs to launchd in your login session, outside Claude's process tree and process group.
- **The label is safe:** it is its own argument, never spliced into shell text, and only labels from `account status --json` are offered.
- **It runs once:** `launchctl submit` would restart a job that fails, so the wrapper removes its own job when the switch ends.
- **Logs:** output goes to `~/Library/Logs/ai-usagebar-desktop-switch.log`, with an `== exit N` line after each switch.
- **First run:** macOS may ask once to let the switch control Claude. If you deny it, `ai-usagebar` quits the app with signals instead.

`ai-usagebar` is looked up in `$PATH`, then `~/.cargo/bin`, `/opt/homebrew/bin`,
`/usr/local/bin`, then `/Applications/AI Usage.app/Contents/MacOS` and
`~/Applications/AI Usage.app/Contents/MacOS`. It tries `ai-usagebar` before
`ai-usagebar-tray` and passes the absolute path, since launchd's `PATH` is minimal.

## Limits

- A switch runs without a terminal. When an account deleted a routine that
  another account still has, `ai-usagebar` keeps every copy rather than asking.
  Run `ai-usagebar account switch <label> --desktop` in a terminal to decide.
- Adding an account is interactive, so it stays a terminal command
  (`ai-usagebar account add <label> --desktop`).

## Develop

```bash
claude plugin validate claude-code-mod
claude plugin test claude-code-mod
npx -p typescript@5 tsc -p claude-code-mod --noEmit   # once Claude has laid .claude-plugin/types/
```

The tests mock `$.process`. Nothing in them runs `ai-usagebar` or `launchctl`.
