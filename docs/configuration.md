# Configuration reference

The config file is `~/.config/ai-usagebar/config.toml`. All fields are optional.
Claude, Codex, Z.AI, and OpenRouter are enabled by default; other providers are
opt-in. The commented example shows the defaults and provider-specific
settings.

Both binaries accept `--config <PATH>` to use an alternate file instead of the
default location (`%APPDATA%\ai-usagebar\config.toml` on Windows). The file
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
# primary = "anthropic"   # anthropic | anthropic_api | openai | copilot | ollama
#                         # | zai | openrouter | deepseek | kimi | kilo | novita
#                         # | moonshot | grok | supergrok | grokbot | antigravity | cursor
#                         # | minimax | kiro | nous | opencode-go | commandcode
#                         # | orcarouter | modelstudio

[context]
enabled = false           # opt in, then press c in ai-usagebar-tui
# projects_path = "~/.claude/projects"
# context_window_tokens = 200000  # optional fallback denominator
# [context.model_context_window_tokens]
# "claude-opus-4-6" = 1000000    # exact model id overrides the fallback

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

[deepseek]
enabled = true             # disabled by default; enable once you add an API key
api_key_env = "DEEPSEEK_API_KEY"
# api_key = "sk-..."       # used if DEEPSEEK_API_KEY is unset; chmod 600 the file!
# display_limit = 200      # tank size in USD; see "Balance tanks" below
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
# ai-usagebar uses the saved Google session and the Cloud Code API instead.
# The session is read-only from either the OS keyring or the CLI file:
# ~/.gemini/antigravity-cli/antigravity-oauth-token
# oauth_client_id = "<public installed-app client id>"
# oauth_client_secret = "<public installed-app client secret>"
# The OAuth client is needed only to refresh an expired saved session.
#
# Set ANTIGRAVITY_LS_ADDRESS=host:port only when automatic loopback discovery
# fails; discovered ports are still tried after this address.

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

[modelstudio]
enabled = false            # disabled by default; enable after `bl auth login --console`
# Alibaba Cloud Model Studio (Bailian) Token Plan. No API key: the credential
# is the official `bl` CLI's own console login in ~/.bailian/config.json,
# read-only. The region×site pair recorded there picks the console gateway
# (cn-beijing/ap-southeast-1 × domestic/international).
# config_dir = "/home/you/.bailian"   # or set BAILIAN_CONFIG_DIR at runtime
```

For more than one OpenRouter key, see the
[OpenRouter account guide](openrouter-accounts.md). The existing singular
`[openrouter]` key remains the default account and needs no migration.

### Notifications

`[notifications]` controls the quota-threshold desktop alerts. They are on by
default at 97%: after a **fresh** fetch (never a cached or failed one), any
window at or above the threshold raises one notification — normal urgency
between the threshold and 99%, critical at 100% (exhausted). Linux delivers
via `notify-send` (`-a ai-usagebar -c quota`); macOS delivers through
Notification Center. The macOS popover also exposes the enable switch and
threshold in Preferences. Windows delivery is planned for a later release.

One crossing is one notification. A key re-arms only when usage drops 7
percentage points below the threshold (97 → below 90) or when the window's
reset moves to a later instant, and the dedupe state lives in
`~/.cache/ai-usagebar/notifications.json` behind the same file locking as the
vendor caches. Banked reset credits (Codex, SuperGrok) also notify once, 48
hours before each credit expires. Bodies carry only vendor-reported absolute
resets — never a burn-rate estimate.

Delivery is best-effort: a missing or failing notifier, an unwritable state
file, or lock contention is a silent skip that never affects the bar, the
report, or any exit code. Both fields are also editable in the TUI Settings
overlay (`s`).

### Balance tanks

DeepSeek, Kilo, Novita, Moonshot and prepaid Grok report how much money is
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

### Explicitly enable a provider

Run `ai-usagebar settings enable anthropic` to set `[anthropic].enabled = true`,
including when it was explicitly false. This is an explicit opt-in; automatic
detection continues to respect disabled providers. The command preserves other
settings, comments, and credentials, and supports `--config PATH` to edit an
existing alternate configuration. It does not sign in or select a primary
provider. Successful writes return `{"ok":true}`; failures exit nonzero.
