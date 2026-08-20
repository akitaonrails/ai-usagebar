# Configuration reference

The config file is `~/.config/ai-usagebar/config.toml`. All fields are optional.
Claude, Codex, Z.AI, and OpenRouter are enabled by default; other providers are
opt-in. The sync feature is opt-in. The commented example shows the defaults and
provider-specific settings.

```toml
[ui]
# Which vendor the widget shows when --vendor is omitted, AND which tab
# is selected when the TUI opens. Defaults to anthropic when not set.
# Only a vendor that is enabled can be primary.
# primary = "anthropic"   # anthropic | anthropic_api | openai | zai
#                         # | openrouter | deepseek | kimi | kilo | novita
#                         # | moonshot | grok | supergrok | antigravity | cursor
#                         # | minimax | kiro

[context]
enabled = false           # opt in, then press c in ai-usagebar-tui
# projects_path = "~/.claude/projects"
# context_window_tokens = 200000  # optional fallback denominator
# [context.model_context_window_tokens]
# "claude-opus-4-6" = 1000000    # exact model id overrides the fallback

[anthropic]
enabled = true
# credentials_path = "/home/you/.claude/.credentials.json"

[anthropic_api]
enabled = true             # disabled by default; requires an organization Admin key
api_key_env = "ANTHROPIC_ADMIN_KEY"
# api_key = "sk-ant-admin01-..."  # not an inference key; chmod 600 if inline
# monthly_limit = 1000     # optional positive, finite USD display limit

[openai]
enabled = true
# codex_auth_path = "/home/you/.codex/auth.json"

[zai]
enabled = true
api_key_env = "ZAI_API_KEY"
# api_key = "..."          # used if ZAI_API_KEY is unset; chmod 600 the file!
# plan_tier = "lite"       # lite | pro | max — display-only

[openrouter]
enabled = true
api_key_env = "OPENROUTER_API_KEY"
# api_key = "sk-or-v1-..."
# show_default_account = false  # hide default when named accounts exist

# [[openrouter.accounts]]
# label = "work"
# api_key_env = "OPENROUTER_WORK_API_KEY"
# api_key = "sk-or-v1-..."      # optional fallback; chmod 600 if inline

[deepseek]
enabled = true             # disabled by default; enable once you add an API key
api_key_env = "DEEPSEEK_API_KEY"
# api_key = "sk-..."       # used if DEEPSEEK_API_KEY is unset; chmod 600 the file!

[kimi]
enabled = true             # disabled by default; enable once you add an API key
api_key_env = "KIMI_API_KEY"
# api_key = "sk-..."       # used if KIMI_API_KEY is unset; chmod 600 the file!

[minimax]
enabled = true             # disabled by default; enable once you add an API key
api_key_env = "MINIMAX_API_KEY"
# api_key = "..."          # used if MINIMAX_API_KEY is unset; chmod 600 the file!
# region = "global"        # global -> api.minimax.io | cn -> api.minimaxi.com

# --- Account-balance vendors (all opt-in) ---

[kilo]
enabled = true             # disabled by default; enable once you add an API key
api_key_env = "KILO_API_KEY"
# api_key = "..."          # used if KILO_API_KEY is unset; chmod 600 the file!
# organization_id = "org_..."   # team balance; omit for the personal balance

[novita]
enabled = true             # disabled by default; enable once you add an API key
api_key_env = "NOVITA_API_KEY"
# api_key = "..."          # used if NOVITA_API_KEY is unset; chmod 600 the file!

[moonshot]
enabled = true             # disabled by default; enable once you add an API key
api_key_env = "MOONSHOT_API_KEY"
# api_key = "sk-..."       # used if MOONSHOT_API_KEY is unset; chmod 600 the file!
# region = "global"        # global → api.moonshot.ai (USD) | cn → api.moonshot.cn (CNY)

[grok]
enabled = true             # disabled by default; enable once you add an API key
# The xAI *Management* key, NOT the inference key.
api_key_env = "XAI_MANAGEMENT_KEY"
# api_key = "..."          # used if XAI_MANAGEMENT_KEY is unset; chmod 600 the file!
# Required for organization-scoped keys; auto-resolved for team-scoped ones.
# team_id = "..."

[supergrok]
enabled = true             # disabled by default; enable once you've run `grok login`
# No API key: billing comes from the official Grok Build ACP process.
# Defaults to $GROK_HOME/bin/grok or ~/.grok/bin/grok. Override only when the
# trusted official binary was installed elsewhere.
# grok_binary = "/opt/grok/bin/grok"
# Opaque cache-scope fingerprint inputs; neither file is parsed or copied.
# auth_path = "/home/you/.grok/auth.json"
# config_path = "/home/you/.grok/config.toml"

[cursor]
enabled = true             # disabled by default; enable once you've signed in to Cursor
# No API key: reads the session token the Cursor IDE already wrote to its own
# state.vscdb after you signed in there. No desktop IDE (headless machine)?
# Sign in to the cursor-agent CLI once instead — its own auth.json is the
# fallback when the IDE database is absent.
# db_path = "/home/you/.config/Cursor/User/globalStorage/state.vscdb"
# agent_auth_path = "/home/you/.config/cursor/auth.json"

[kiro]
enabled = true             # disabled by default; enable once you've run `kiro-cli login`
# No API key: reads the AWS SSO OIDC session kiro-cli already wrote to its own
# data.sqlite3 after you logged in there.
# db_path = "/home/you/.local/share/kiro-cli/data.sqlite3"

[sync]
# repo = "owner/name"      # required for backup; no default. See docs/sync-github.md
# categories = ["config", "credentials", "routines", "chat_index"]
# transcript_days = 30            # newest-first age bound, when transcripts is on
# transcript_max_bytes = 2147483648   # 2 GiB backstop, when transcripts is on
# keep_snapshots = 10             # must be >= 1; 0 is refused at load
```

For more than one OpenRouter key, see the
[OpenRouter account guide](openrouter-accounts.md). The existing singular
`[openrouter]` key remains the default account and needs no migration.

## Sync configuration

### `[sync] keep_snapshots`

Value: a positive integer. Default `10`.

How many snapshots the remote pointer keeps. Old snapshots share chunks, so ten
costs little more than three and covers roughly a week of daily syncs; a user
syncing hourly wants a different number than one syncing weekly, which is why it
is config rather than a constant.

**Zero is refused when the config is loaded**, with a message saying why: at 0
the flip that publishes a snapshot would also drop it, so every push would leave
the remote with nothing to restore from. `sync prune` additionally clamps
whatever it is handed to at least 1 — the pointer it truncates gets *published*,
so a zero arriving by any other route must not empty the snapshot list.

### `[sync] repo`

Value: `owner/name` (e.g. `alice/ai-usagebar-backup`)

Required if you use the sync feature. There is no default. A missing or unset value is an error, not something the tool resolves for you, because the tool holds no permission to create repositories. Naming it is a one-time, explicit act.

For token setup and repository requirements, see [GitHub sync setup and authentication](sync-github.md).

The token itself is never a config key. It lives in one of four places in this order:

1. `AI_USAGEBAR_SYNC_TOKEN` environment variable (useful for CI and headless restores)
2. macOS Keychain item (macOS only)
3. `~/.config/ai-usagebar/sync-token` file, mode 0600 (Linux and other platforms)
4. `gh auth token` (GitHub CLI, if installed and logged in)

## `ai-usagebar sync pull` — restoring onto a second machine

```
ai-usagebar sync pull [--apply | --dry-run] [--force [--force-credentials]]
                      [--allow-rollback] [-y|--yes] [--rebuild-index]
```

**A pull writes nothing by default.** Without `--apply` it downloads the
bundle's metadata, prints exactly what would land, and stops — it does not even
fetch a byte of your file content. That inverts the usual convenience default on
purpose: a wrong push costs a re-push, a wrong restore costs the credentials and
history on the machine in front of you.

You do not need a local keyfile to pull. A second machine has none — that is the
situation a restore exists for — so the wrapped key comes off the remote and the
sync password unwraps it. The password is read from stdin only: never from a
flag, never from an environment variable.

| Flag | What it does | What it can cost you |
|---|---|---|
| `--apply` | the only flag that lets a byte reach the disk | — |
| `--dry-run` | already the default; accepted for symmetry with `push --dry-run`, and refused alongside `--apply` | — |
| `--force` | restores items whose **local copy is newer** than the snapshot, which are skipped without it | **the newer local version of every non-credential item it names.** It prints them before it acts, and the pre-restore archive is your undo |
| `--force-credentials` | the second, separate consent for a locally-newer **credential**. Requires `--force`, and `--force` alone never grants it | **a live OAuth token.** It writes the snapshot's older token over the one this machine is using now; if that token has since rotated, the live one is gone and everything authenticated with it stops working until you log in again |
| `--allow-rollback` | opens a snapshot **older** than the newest one this machine has already seen | you get the older snapshot's contents. It never waives the bundle-identity check: a counter borrowed from a *different* bundle is refused with this flag exactly as without it |
| `-y`, `--yes` | answers the one confirmation. It does **not** answer the credential question, which has its own flag | — |
| `--rebuild-index` | throws away the local change-detection index and starts it empty | one slow sync. It changes nothing about what a restore writes: a restore hashes what is on disk and never asks the index |

There is no `--force-rehash` on `sync pull`, deliberately. It exists on
`sync push`, where it changes what the planner reads. Nothing on the restore
path would read it, and a flag with no reader is worse than no flag.

### When you are asked, and when you are not

The sync password is read from stdin, and so are the two confirmations, so a
piped run has one stream and no way to tell them apart. **The password wins the
stream** — it is the one input that cannot be supplied any other way:

- **On a terminal:** you are asked for the password, then offered the apply
  confirmation, then the credential confirmation if any credential needs one.
  The credential one is not a `[y/N]`; it wants the word `overwrite` typed out.
- **Piped** (`echo "$PASSWORD" | ai-usagebar sync pull …`): you are never asked
  anything. The password takes the first line, and the answers come from flags —
  `--apply`, `--yes`, and `--force-credentials`. A piped run with none of them
  prints the plan, names `--apply`, and exits 0.

### Undo

Before the first byte is written, everything the restore is about to overwrite
is tarred into `~/.claude-acc/backups/sync-restore-<timestamp>.tar.gz` — the same
directory the account switcher uses, so there is one place to look for "undo".
The exact `tar -xzf … -C …` that reverses the whole restore is printed when the
run ends, and printed again as the last line if the run stopped part way. The
archive is taken even under `--force`, and even for a partial restore.

A restore that only *creates* files has nothing to archive and says so rather
than promising an archive.

### What a pull will not do

- **It never deletes.** A file you have that the snapshot does not mention is
  left alone, including under `--force`. Pulling onto a machine that already has
  files gives you the union.
- **It never restores machine-bound state** — device registries, bridge state,
  caches, lock files — even if a bundle names them.
- **It never writes through a symlink.** A symlink, directory, or anything other
  than a regular file at a destination is refused and reported, and no flag
  promotes that refusal.
- **It never advances the rollback anchor on a failure.** Every refusal leaves
  the anchor exactly as it was, so a forged snapshot cannot lock you out of your
  own bundle.
- **Re-running is safe.** Applying the same snapshot twice writes nothing the
  second time: already-correct files are recognised by content, not by timestamp.

For the format, the read chain, and the rules a restore enforces, see
[Encrypted sync bundle format](sync-format.md) §11.
