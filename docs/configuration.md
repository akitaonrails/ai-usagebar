# Configuration reference

The config file is `~/.config/ai-usagebar/config.toml`. All fields are optional.
Claude, Codex, Z.AI, and OpenRouter are enabled by default; other providers are
opt-in. The commented example shows the defaults and provider-specific
settings.

```toml
[ui]
# Which vendor the widget shows when --vendor is omitted, AND which tab
# is selected when the TUI opens. Defaults to anthropic when not set.
# Only a vendor that is enabled can be primary.
# primary = "anthropic"   # anthropic | anthropic_api | openai | zai
#                         # | openrouter | deepseek | kimi | kilo | novita
#                         # | moonshot | grok | supergrok | antigravity | cursor
#                         # | copilot | minimax | kiro

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

[copilot]
enabled = true             # disabled by default; enable once `gh auth login` or a token is set
# Same live quota endpoint the official Copilot clients poll.
token_env = "GITHUB_TOKEN" # `GH_TOKEN` is also checked automatically
# token = "github_pat_..." # optional inline fallback; chmod 600 the file if inline
# api_key = "github_pat_..." # compatibility alias for `token`
# gh_binary = "/usr/bin/gh" # optional trusted GitHub CLI path override
# hostname = "acme.ghe.com" # GitHub Enterprise Cloud (data residency) or GHES hostname; unset = public github.com

[kiro]
enabled = true             # disabled by default; enable once you've run `kiro-cli login`
# No API key: reads the AWS SSO OIDC session kiro-cli already wrote to its own
# data.sqlite3 after you logged in there.
# db_path = "/home/you/.local/share/kiro-cli/data.sqlite3"
```
