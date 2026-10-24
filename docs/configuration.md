# Configuration reference

The config file is `~/.config/ai-usagebar/config.toml`. All fields are optional.
Claude, Codex, Z.AI, and OpenRouter are enabled by default; other providers are
opt-in. The commented example shows the defaults and provider-specific
settings.

Muse Code subscription quotas use the official CLI's existing file-based
OAuth login. Enable `[muse-code] enabled = true`, optionally with a
`credentials_path`. It remains opt-in during detection. See
[Muse Code setup](muse-code.md) for paths, the five-minute request bound,
and Meta's idle-window missing-data limitation.

Both binaries accept `--config <PATH>` to use an alternate file instead of the
default location (`%APPDATA%\ai-usagebar\config\config.toml` on Windows). The file
must already exist; loads and the Settings overlay then read and write that
file for the whole process, so a test config never touches the real one:

```bash
ai-usagebar --vendor kimi --config ./config.test.toml --watch 5
ai-usagebar-tui --config ./config.test.toml
```

```toml
[ui]
# Which vendor the widget shows when --vendor is omitted, AND which tab
# is selected when the TUI opens. Defaults to anthropic when not set.
# Only a vendor that is enabled can be primary.
# primary = "anthropic"   # any vendor slug, e.g. anthropic | openai | copilot | ollama | cline
#                         # | zai | openrouter | deepseek | deepinfra | kimi | kilo | novita
#                         # | moonshot | grok | supergrok | grokbot | antigravity | cursor
#                         # | minimax | kiro | nous | opencode-go | commandcode
#                         # | orcarouter | modelstudio | lyceum | devin | cline | muse-code | droid
# Which vendors the Overview lists, in this order. The TUI tab bar leads with
# the same list; enabled vendors left off it follow in the canonical order.
# Omit for every enabled vendor. A repeated slug keeps its first occurrence;
# an empty list is set, and is not the same as omitting the key.
# `usage --json` includes `overview_vendors` only when this is set, so a
# frontend can order cards without reading this file.
# overview_vendors = ["anthropic", "openai", "supergrok"]

[context]
enabled = false           # opt in, then press c in ai-usagebar-tui
# projects_path = "~/.claude/projects"
# context_window_tokens = 200000  # optional fallback denominator
# [context.model_context_window_tokens]
# "claude-opus-4-6" = 1000000    # exact model id overrides the fallback
# While enabled, `usage` (and the tray/Omarchy panels built on it) also shows
# the most recent Claude Code sessions on the Claude entry as a "Sessions"
# group: one row per session with its context health on the same severity
# colors as quota meters, plus the model and last-active time. Each Claude
# account whose sessions are working or waiting on you also gets an "Activity"
# row ("2 working · 1 waiting") and an `activity` field, read from the
# `sessions/` directory in that account's config directory.

# Quota-threshold desktop notifications. On by default at 97%: a window that
# crosses the threshold raises one notification per crossing (Linux uses
# notify-send; macOS uses Notification Center; Windows delivery follows). A window re-arms only when
# usage drops 7 points below the threshold or its reset moves later, and
# banked reset credits (Codex, SuperGrok) notify 48h before they expire.
# [notifications]
# enabled = true    # false turns every quota/expiry notification off
# threshold = 97    # 1..=100; at 100 only an exhausted window notifies

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

[copilot]
enabled = false           # opt in after `gh auth login --web`
# Uses `gh auth token`; GITHUB_COPILOT_TOKEN is an optional explicit override.
# [[copilot.accounts]]     # one entry per GitHub login, each with its own plan
# label = "work"           # --account work, and the cache subdir
# user = "my-work-login"   # the login from `gh auth status`, not an email
# show_default_account = true   # false hides the active-gh entry once named

[zai]
enabled = true
api_key_env = "ZAI_API_KEY"
# api_key = "..."          # used if ZAI_API_KEY is unset; chmod 600 the file!
# plan_tier = "lite"       # lite | pro | max — display-only

[openrouter]
enabled = true
api_key_env = "OPENROUTER_API_KEY"
# api_key = "sk-or-v1-..."
# headline = "percent"          # "percent" | "amount"; see "Balance tanks" below
# show_default_account = false  # hide default when named accounts exist

# [[openrouter.accounts]]
# label = "work"
# api_key_env = "OPENROUTER_WORK_API_KEY"
# api_key = "sk-or-v1-..."      # optional fallback; chmod 600 if inline
# One entry per workspace; keys inside one workspace share its billing
# account, so the per-entry split is per login session, not per key within
# a bill. See docs/openrouter-accounts.md.

[deepseek]
enabled = true             # disabled by default; enable once you add an API key
api_key_env = "DEEPSEEK_API_KEY"
# api_key = "sk-..."       # used if DEEPSEEK_API_KEY is unset; chmod 600 the file!
# display_limit = 200      # tank size in USD; see "Balance tanks" below
# headline = "amount"      # "amount" | "percent"

[deepinfra]
enabled = true             # disabled by default; enable once you add an API key
api_key_env = "DEEPINFRA_API_KEY"
# api_key = "..."          # used if DEEPINFRA_API_KEY is unset; chmod 600 the file!
# display_limit = 50       # optional prepaid tank size in USD
# headline = "amount"      # "amount" | "percent"

[kimi]
enabled = true             # disabled by default; a Kimi Code CLI login is enough
# Log in with `kimi` and ai-usagebar reads the OAuth session the CLI already
# stored, refreshing it in place when it expires — no key to create or paste.
# An API key still wins when one is set; a Kimi For Coding subscription can
# issue one at kimi.com/code/console, and a platform key works too.
api_key_env = "KIMI_API_KEY"
# api_key = "sk-..."       # used if KIMI_API_KEY is unset; chmod 600 the file!
# credentials_path = "~/.kimi-code/credentials/kimi-code.json"  # CLI login file
# region = "auto"          # auto follows ~/.kimi-code/region
#                          # cn -> api.kimi.com | global -> api.kimi.ai

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
# display_limit = 200           # tank size in USD; see "Balance tanks" below
# headline = "amount"           # "amount" | "percent"

[novita]
enabled = true             # disabled by default; enable once you add an API key
api_key_env = "NOVITA_API_KEY"
# api_key = "..."          # used if NOVITA_API_KEY is unset; chmod 600 the file!
# display_limit = 200      # tank size in USD; see "Balance tanks" below
# headline = "amount"      # "amount" | "percent"

[lyceum]
enabled = true              # disabled by default; enable once you add an API key
api_key_env = "LYCEUM_API_KEY"
# api_key = "..."           # inline fallback; chmod 600 the file if used
# [[lyceum.accounts]]       # optional named API-key accounts
# label = "work"
# api_key_env = "LYCEUM_WORK_API_KEY"
# api_key = "..."

[orcarouter]
enabled = true             # disabled by default; enable once you add an API key
api_key_env = "ORCAROUTER_API_KEY"
# api_key = "sk-orca-..."  # used if ORCAROUTER_API_KEY is unset; chmod 600 the file!

[ollama]
# Disabled by default; enable after minting a key at
# https://ollama.com/settings/keys (Bearer for https://ollama.com/api/usage).
enabled = true
api_key_env = "OLLAMA_API_KEY"
# api_key = "..."          # used if OLLAMA_API_KEY is unset; chmod 600 the file!

[moonshot]
enabled = true             # disabled by default; enable once you add an API key
api_key_env = "MOONSHOT_API_KEY"
# api_key = "sk-..."       # used if MOONSHOT_API_KEY is unset; chmod 600 the file!
# region = "global"        # global → api.moonshot.ai (USD) | cn → api.moonshot.cn (CNY)
# display_limit = 200      # tank size in the region's currency; see "Balance tanks"
# headline = "amount"      # "amount" | "percent"

[grok]
enabled = true             # disabled by default; enable once you add an API key
# The xAI *Management* key, NOT the inference key.
api_key_env = "XAI_MANAGEMENT_KEY"
# api_key = "..."          # used if XAI_MANAGEMENT_KEY is unset; chmod 600 the file!
# Required for organization-scoped keys; auto-resolved for team-scoped ones.
# team_id = "..."
# display_limit = 200      # tank size in USD; see "Balance tanks" below
# headline = "amount"      # "amount" | "percent"

[supergrok]
enabled = true             # disabled by default; enable once you've run `grok login`
# Included usage from Grok Build billing (overall % plus productUsage slices).
# Distinct from `[grok]`, which is Management API prepaid dollars.
# No API key of its own: billing and banked resets use the `key` already in
# its auth.json (read-only, sent in an Authorization header, never copied or
# rewritten). Billing is Grok Build's documented HTTPS endpoint, or its ACP
# process as fallback; remaining resets are a separate grok.com RPC.
# Defaults to $GROK_HOME/bin/grok or ~/.grok/bin/grok. Override only when the
# trusted official binary was installed elsewhere.
# grok_binary = "/opt/grok/bin/grok"
# Cache-scope fingerprint inputs. config.toml is read as opaque bytes only;
# auth.json is also read for its billing `key`. Neither is copied or written.
# auth_path = "/home/you/.grok/auth.json"
# config_path = "/home/you/.grok/config.toml"

[grokbot]
enabled = false            # disabled by default; enable after signing in to the app
# Grok Bot desktop app's weekly included-usage pool (Linux and macOS).
# Distinct from `[grok]` (Management API prepaid dollars) and `[supergrok]`
# (Grok Build subscription). No API key: the credential is the app's own
# session in sand-secrets.json, read-only. Default:
# ~/.config/Grok Bot/sand-secrets.json (Linux) or
# ~/Library/Application Support/Grok Bot/sand-secrets.json (macOS).
# Refreshed tokens persist only in ai-usagebar's cache, never back to the app's file.
# secrets_path = "~/Library/Application Support/Grok Bot/sand-secrets.json"

[antigravity]
enabled = false            # opt in after signing in with Antigravity
# Antigravity is read locally first: the running desktop product or `agy`
# language server supplies quota over its loopback RPC. When that source is
# unavailable — including `agy` sessions whose CSRF token is not published —
# ai-usagebar asks `agy -p "/quota"` when `agy` is installed (at most every two
# minutes), and otherwise uses the saved Google session and the Cloud Code API.
# By default `agy` is discovered on `PATH` and in `~/.local/bin`. Set
# `agy_binary = "~/.local/bin/agy"` to pin the exact executable.
# The session is read-only from either the OS keyring or the CLI file:
# ~/.gemini/antigravity-cli/antigravity-oauth-token
# oauth_client_id = "<public installed-app client id>"
# oauth_client_secret = "<public installed-app client secret>"
# The OAuth client is needed only to refresh an expired saved session.
#
# Set ANTIGRAVITY_LS_ADDRESS=host:port only when automatic loopback discovery
# fails; discovered ports are still tried after this address.
#
# ide_language_server = false leaves the Antigravity IDE's language server out
# of discovery (default: true). Use it when the IDE is installed but its
# sign-in is managed separately — a signed-out IDE answers 401/403, which
# outranks and masks a working `agy` session (#405). No effect on Windows,
# where the IDE's server cannot be told apart from the 2.0 app's on disk.

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

[opencode-go]
enabled = true             # disabled by default; enable once signed in or configured
# Reads auth.json, then OpenCode 2's opencode.db, under
# $XDG_DATA_HOME/opencode or ~/.local/share/opencode (read-only).
# The database must have an active opencode key or unexpired OAuth access token.
# OpenCode owns token refresh; renew an expired session in the CLI.
# An explicit key wins over both stores. If v1 and v2 coexist, auth.json
# keeps priority; use auth_paths with only opencode.db to select the v2 login.
# api_key = "..."                          # or export OPENCODE_GO_API_KEY
# auth_paths = ["/custom/opencode/opencode.db"]  # replaces the search list

[modelstudio]
enabled = false            # disabled by default; enable after `bl auth login --console`
# Alibaba Cloud Model Studio (Bailian) Token Plan. No API key: the credential
# is the official `bl` CLI's own console login in ~/.bailian/config.json,
# read-only. The region×site pair recorded there picks the console gateway
# (cn-beijing/ap-southeast-1 × domestic/international).
# config_dir = "/home/you/.bailian"   # or set BAILIAN_CONFIG_DIR at runtime

[devin]
enabled = false            # disabled by default; enable after signing in with Devin CLI
# Reuses the official Devin CLI credential file read-only. Defaults to
# %APPDATA%/devin/credentials.toml on Windows, or
# ${XDG_DATA_HOME:-~/.local/share}/devin/credentials.toml on Linux and macOS
# (the CLI's documented paths; macOS is untested here).
# An explicit credentials_path needs no home directory to resolve.
# credentials_path = "/home/you/.local/share/devin/credentials.toml"

[droid]
enabled = false            # disabled by default; enable after signing in with `droid` on Linux
# Factory Droid subscription quota. Reuses the login the official `droid` CLI
# keeps on Linux: `~/.factory/auth.v2.keyring`, opened with the
# `Factory CLI/auth-encryption-key` Secret Service item. ai-usagebar never
# refreshes or writes the token itself. Other `auth.v2.*` stores and non-Linux
# platforms are not supported.
cli_renewal = true         # near expiry, let the `droid` CLI renew its own login
# cli_path = "/home/you/.local/bin/droid"   # default: `droid` on PATH
```

The CLI's access token lasts about a day and the CLI renews it only when it
runs. With `cli_renewal` on, a login within 10 minutes of expiry makes
ai-usagebar run `droid doctor --checks auth --timeout 10000` — the CLI's own
auth check, which renews and saves the session through Factory's sign-in
service without calling a model. It runs with stdin closed, its output
discarded, provider secrets removed from its environment, a 30-second limit,
and at most once every 15 minutes. If it is off or the CLI cannot renew
(signed out, offline, `droid` not on the bar's `PATH`), the bar keeps the last
reading and says to run `droid` once.

Droid remains opt-in when its CLI login is present. First-run detection and
`detect --all` do not enable it; set `enabled = true` explicitly to activate
the provider. It reports the Standard quota's 5-hour, weekly and monthly
windows plus the Droid Core bucket and the extra-usage balance. A window the
account has not started reads "Not started", and a Droid Core bucket that was
never used collapses to one "not used yet" row.

Devin remains opt-in when its CLI login is present. First-run detection and
`detect --all` do not enable it; set `enabled = true` explicitly to activate
the provider.

Devin reports daily and weekly remaining percentages, which ai-usagebar
converts to consumed percentages for consistent meters. A window whose reset
time arrives without a remaining percentage (the encoding omits zero values) is
shown as fully used. Its optional
`overageBalanceMicros` value is displayed as USD to six decimal places based on
the tested account; that currency interpretation is not a verified universal
contract. The existing CLI token is read only for the status request and cache
identity. ai-usagebar does not sign in, refresh, or rewrite Devin credentials.

For more than one OpenRouter key, see the
[OpenRouter account guide](openrouter-accounts.md). The existing singular
`[openrouter]` key remains the default account and needs no migration. Z.AI,
DeepSeek, DeepInfra, Kilo, Novita, Moonshot, Grok, MiniMax, OrcaRouter, and Lyceum take the same
`[[<vendor>.accounts]]` array and `show_default_account` switch — see the
[API-key account guide](api-key-accounts.md).

### Notifications

`[notifications]` controls the quota-threshold desktop alerts. They are on by
default at 97%: after a **fresh** fetch (never a cached or failed one), any
window at or above the threshold raises one notification — normal urgency
between the threshold and 99%, critical at 100% (exhausted). Linux delivers
via `notify-send` (`-a ai-usagebar -c quota`); macOS delivers through
Notification Center. A critical (100%) toast also asks for a 30-second
timeout, so it can expire where the daemon honours one: Omarchy's notification
plugin keeps critical toasts until they are dismissed and ignores the sender's
timeout, and
[omacom/omarchy#12964](https://github.com/omacom/omarchy/pull/12964) is what
makes it honour a requested one, capped at 30s. The macOS popover also exposes
the enable switch and
threshold in Preferences. Windows delivery is planned for a later release.

One crossing is one notification. A key re-arms only when usage drops 7
percentage points below the threshold (97 → below 90) or when the window's
reset moves to a later instant by more than an hour and a half — a smaller move
is the same window reported again, since vendors report the instant with
sub-second drift between fetches and a rolling window slides it forward with
the refresh interval — and the dedupe state lives in
`~/.cache/ai-usagebar/notifications.json` behind the same file locking as the
vendor caches. Banked reset credits (Codex, SuperGrok) also notify once, 48
hours before each credit expires. Bodies carry only vendor-reported absolute
resets — never a burn-rate estimate.

Delivery is best-effort: a missing or failing notifier, an unwritable state
file, or lock contention is a silent skip that never affects the bar, the
report, or any exit code. Both fields are also editable in the TUI Settings
overlay (`s`).

### Balance tanks

DeepSeek, DeepInfra, Kilo, Novita, Moonshot and prepaid Grok report how much money is
**left** and nothing else. There is no denominator in those responses, so
there is nothing to draw a meter against and the row is a plain balance.

`display_limit` supplies that denominator yourself — the size of the tank, in
the currency that vendor already reports:

```toml
[deepseek]
display_limit = 200        # you topped up $200 and want to watch it burn down
```

It must be finite and greater than zero; anything else fails at load with the
offending section named. There is no default and no built-in figure: leave it
out and nothing changes.

DeepInfra also shows the current calendar-month spend and the account's monthly
spending limit when the API supplies one. Its `display_limit` affects only the
prepaid-balance tank; it never replaces the provider's monthly limit.

It is a fallback, never an override: a vendor that states a limit of its own
keeps it. That is why **`[openrouter]` has no `display_limit` at all**. It
reports credits purchased against credits used (and a per-key limit when the key
has one), so there is nothing to fall back to — and in the one case where a tank
would not simply be ignored, a free-tier account that purchased nothing,
honouring it would be actively wrong: that row's percentage comes from the API,
not from the tank, so the bar would read `0%` for an account with money in it.
A free-tier OpenRouter account therefore keeps its dollar figure on the bar even
at the `"percent"` default. `[openrouter]` does take `headline`.

The Anthropic Admin API's `monthly_limit` is a separate, older setting and is
unaffected.

The percentage is **consumed**, matching every other meter in the app:

```
(display_limit - balance) / display_limit, clamped to 0–100
```

A balance above the cap reads as 0% used; the money figure is what says how far
above it sits.

`headline` is a separate choice: which of the two numbers goes on the bar.

| value       | bar        | detail line |
| ----------- | ---------- | ----------- |
| `"amount"`  | `$50.00`   | `75% of $200.00 used ($50.00 left)` |
| `"percent"` | `75%`      | `$50.00 of $200.00 left (75% used)` |

Balance vendors default to `"amount"`; `[openrouter]`, which always has a
denominator of its own, defaults to `"percent"`. Setting `display_limit` does
not switch the headline by itself, and choosing `"percent"` with no limit from
either source leaves the amount on the bar rather than inventing a percentage.

`[nous]` takes `headline` as well, and needs no `display_limit`: its plan's
monthly credits are already the percentage's denominator. `"amount"` puts the
credits still usable on the bar — the Portal's total usable credits, falling
back to top-up credits and then to subscription credits — and leaves the
consumed percentage in the meter, the severity colour and the detail line.

The Omarchy panel, the KDE plasmoid and the tray popover (Windows and macOS)
read the metric's own `headline` field out of `usage --json` rather than
guessing from the row's label. In the popover, `"amount"` puts the money figure
under the meter and moves the percentage and the detail line to its hover text;
`"percent"` keeps the popover's used/left toggle. Waybar and GNOME build their
bar text from the per-vendor formats, so `headline` does not reach them.

### GitHub Copilot

GitHub Copilot uses the OAuth login managed by the official GitHub CLI. Run
`gh auth login --web`, then select **GitHub Copilot** under **Primary Provider**
in the Omarchy settings form and save; this enables `[copilot]` and sets it as
the primary provider. The normal fetch path runs only the fixed structured
command `gh auth token`. ai-usagebar never parses GitHub CLI configuration or
credential stores and never writes the OAuth token to its config or cache.

`GITHUB_COPILOT_TOKEN` is an optional explicit environment override. It takes
precedence over `gh auth token`, which can be useful for a managed runtime that
provides its own short-lived token. Do not put that token in `config.toml`.

For more than one Codex login, add `[[openai.accounts]]` — a label and that
login's own `auth.json`, the same shape `[[anthropic.accounts]]` uses:

```toml
[[openai.accounts]]
label = "work"
codex_auth_path = "~/.config/ai-usagebar/accounts/work-codex/auth.json"
```

Create the second login with `CODEX_HOME=~/.codex-work codex login` and point
`codex_auth_path` at the file it writes. Select it with `--account work`; each
account caches separately under `~/.cache/ai-usagebar/openai/<label>`. The
singular `codex_auth_path` remains the default account and needs no migration.

For more than one GitHub Copilot plan — a personal and a work account, say —
add `[[copilot.accounts]]`. ai-usagebar stores no token of its own here: `gh`
already keeps several logins per host, so an entry only needs that login's
name, as printed by `gh auth status`:

```toml
[[copilot.accounts]]
label = "work"
user = "my-work-login"
```

Each account resolves through `gh auth token --user <user>`, so signing in
stays entirely `gh`'s job (`gh auth login`, once per account). Select one with
`--account work`; each caches separately under
`~/.cache/ai-usagebar/copilot/<label>`. With no `accounts` array the behavior
is unchanged: whichever account `gh` has active. `show_default_account = false`
hides that unnamed entry once every account is named, and is ignored while the
array is empty.

Two deliberate limits. `GITHUB_COPILOT_TOKEN` names no account, so combining it
with `--account` is an error rather than a silent choice between the two — the
alternative is reporting one account's quota under another's label. And there
is no `hostname` field: `gh` can hold a GitHub Enterprise login, but the quota
endpoint (`api.github.com/copilot_internal/user`) is github.com-only, so the
field would promise support the fetch cannot deliver.

`ai-usagebar account add <label> --codex` writes that entry and runs the login
for you; `ai-usagebar account switch <label> --codex` makes a named login the
one the Codex CLI, desktop app and IDE extension use. See "Switch Codex" in
[claude-accounts.md](claude-accounts.md). Once every login is named, set
`[openai] show_default_account = false` so the active account is not listed a
second time as the unnamed default.

### Explicitly enable a provider

Run `ai-usagebar settings enable anthropic` to set `[anthropic].enabled = true`,
including when it was explicitly false. This is an explicit opt-in; automatic
detection continues to respect disabled providers. The command preserves other
settings, comments, and credentials, and supports `--config PATH` to edit an
existing alternate configuration. It does not sign in or select a primary
provider. Successful writes return `{"ok":true}`; failures exit nonzero.
