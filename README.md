# ai-usagebar

Waybar widget + tabbed TUI for AI plan usage across **Anthropic Claude**, **OpenAI Codex/ChatGPT**, **Z.AI (GLM)**, and **OpenRouter**.

Rust port of [`claudebar`](https://github.com/mryll/claudebar) (drop-in compatible) extended to four vendors. Same minimalist Pango-bordered tooltip design, same Omarchy theme auto-detection, same flock-protected OAuth refresh — but properly tested, modular, and reliable instead of 865 lines of bash.

## Features

- **Per-vendor Waybar modules** — one widget per vendor; same JSON output shape as claudebar (`{text, tooltip, class}`).
- **Tabbed TUI** (`ai-usagebar-tui`) — interactive view with Tab/h/l switching, per-tab refresh, auto-refresh every 60s.
- **Local-testing UX** — `--pretty` mode renders ANSI-colored terminal output; `--watch N` re-renders every N seconds. No need to install into Waybar to iterate.
- **Drop-in claudebar compatibility** — all the same flags (`--icon`, `--format`, `--tooltip-format`, `--pace-tolerance`, `--format-pace-color`, `--tooltip-pace-pts`, `--color-*`), all the same `{placeholders}`, byte-identical output.
- **Always exits 0** — Waybar hides modules that don't, so the widget catches every error into a fallback ⚠ JSON.
- **Atomic cache writes + flock** — multi-monitor Waybar instances coexist without API stampedes.
- **Transient-vs-hard error split** — DNS/timeout failures show a quiet `Loading…`; HTTP 4xx/5xx show actionable HTTP codes in the tooltip.
- **Live API smoke test suite** — `make smoke` hits the real (undocumented) endpoints and asserts the contract we depend on, surfacing schema drift early.

## Install

```bash
cargo build --release
sudo make install                  # → /usr/local/bin
# or
make install PREFIX=$HOME/.local   # → ~/.local/bin
```

## Quick start

```bash
# Local testing — auto-detects TTY and renders human-readable output.
ai-usagebar --vendor anthropic
ai-usagebar --vendor openai
ai-usagebar --vendor zai
ai-usagebar --vendor openrouter

# Force Waybar JSON (e.g. piping into jq).
ai-usagebar --vendor anthropic --json

# Live preview while iterating on --format / --tooltip-format.
ai-usagebar --vendor openrouter --watch 5

# Interactive TUI with tabs.
ai-usagebar-tui
```

## Waybar config

Each vendor is its own module — gives you per-vendor icon, colors, and on-click action:

```jsonc
"modules-right": [
    "custom/claude",
    "custom/openai",
    "custom/openrouter",
    "custom/zai"
],

"custom/claude": {
    "exec": "ai-usagebar --vendor anthropic --icon '󰚩'",
    "return-type": "json",
    "interval": 300,
    "signal": 13,
    "tooltip": true,
    "on-click": "ai-usagebar-tui"
},
"custom/openai": {
    "exec": "ai-usagebar --vendor openai --icon '󱢆'",
    "return-type": "json",
    "interval": 300,
    "tooltip": true,
    "on-click": "ai-usagebar-tui"
},
"custom/openrouter": {
    "exec": "ai-usagebar --vendor openrouter --icon '󱙺' --format '{or_balance} · {or_used_today}'",
    "return-type": "json",
    "interval": 600,
    "tooltip": true
},
"custom/zai": {
    "exec": "ai-usagebar --vendor zai --icon '󰚩'",
    "return-type": "json",
    "interval": 300,
    "tooltip": true
}
```

> Why 300s? The Anthropic and OpenAI Codex endpoints are undocumented and rate-limit aggressively below ~300s. The cache TTL is 60s so multi-monitor instances coexist, but Waybar's polling interval should stay at 300s.

## Configuration

`~/.config/ai-usagebar/config.toml` (optional — defaults enable all four vendors):

```toml
[anthropic]
enabled = true
# credentials_path = "/home/you/.claude/.credentials.json"  # override

[openai]
enabled = true
# codex_auth_path = "/home/you/.codex/auth.json"  # override
admin_key_env = "OPENAI_ADMIN_KEY"

[zai]
enabled = true
api_key_env = "ZAI_API_KEY"
# plan_tier = "lite"   # lite | pro | max — display-only

[openrouter]
enabled = true
api_key_env = "OPENROUTER_API_KEY"
```

API keys are read from env vars (export them in `~/.config/zsh/secrets`, `~/.bashrc`, or similar). The Claude and OpenAI vendors use OAuth from disk (`~/.claude/.credentials.json` and `~/.codex/auth.json`) — no env var needed; just `claude` / `codex login` once.

## Vendor support matrix

| Vendor | Endpoint | Auth | What you see |
|---|---|---|---|
| **Anthropic** | `api.anthropic.com/api/oauth/usage` (undocumented) | OAuth from `~/.claude/.credentials.json`, auto-refreshes | Session (5h), Weekly (7d), Sonnet (7d), Extra usage $ |
| **OpenAI** | `chatgpt.com/backend-api/wham/usage` (undocumented; used by official `codex` CLI) | OAuth from `~/.codex/auth.json`, auto-refreshes via `auth.openai.com/oauth/token` | Codex 5h, Codex weekly, Code-review weekly, Credits |
| **Z.AI** | `api.z.ai/api/monitor/usage/quota/limit` (undocumented) | `ZAI_API_KEY` env var **without `Bearer` prefix** | Session 5h, Weekly 7d, MCP tools monthly |
| **OpenRouter** | `openrouter.ai/api/v1/{credits,key}` (documented) | `OPENROUTER_API_KEY` env var | Balance, today/week/month spend, free vs paid tier |

### Endpoint stability

Three of the four endpoints are undocumented. The Anthropic and OpenAI endpoints are used by their respective official CLIs (`claude` and `codex`) — disappearing them would break those tools too, so they're more durable than scraped web endpoints. Z.AI's monitor endpoint is reverse-engineered from a third-party plugin; treat it as the most fragile.

When an endpoint drifts, **run `make smoke`** — the live API tests check the exact fields we depend on and produce a precise failure pointing at what changed. Paste the failure back into Claude Code and we can update the affected `types.rs` mechanically.

## Format placeholders

### Shared / Anthropic (claudebar-compatible)

| Placeholder | Example |
|---|---|
| `{plan}` | `Max 5x` |
| `{session_pct}`, `{session_reset}`, `{session_bar}`, `{session_elapsed}` | `62`, `1h 30m`, `█████████████░░░░░░░`, `58` |
| `{session_pace}`, `{session_pace_indicator}`, `{session_pace_pct}`, `{session_pace_pts}`, `{session_pace_delta}`, `{session_pace_abs_delta}` | `↑`, `↑`, `12% ahead`, `4pts ahead`, `4`, `4` |
| `{weekly_*}` | same family for the 7d window |
| `{sonnet_*}` | same family for the 7d Sonnet window (empty when absent) |
| `{extra_spent}`, `{extra_limit}`, `{extra_pct}`, `{extra_bar}` | `$2.50`, `$50.00`, `5`, `█░░░░░░░░░░░░░░░░░░░` |

### OpenAI (Codex OAuth)

`{oai_plan}`, `{oai_session_pct}`, `{oai_session_reset}`, `{oai_session_elapsed}`, `{oai_session_pace}`, `{oai_session_pace_indicator}`, `{oai_weekly_*}` (same family), `{oai_code_review_pct}`, `{oai_credit_balance}`, `{oai_local_msgs}`, `{oai_cloud_msgs}`

### Z.AI

`{zai_plan}`, `{zai_session_pct}`, `{zai_session_reset}`, `{zai_weekly_pct}`, `{zai_weekly_reset}`, `{zai_mcp_pct}`, `{zai_mcp_reset}`

### OpenRouter

`{or_label}`, `{or_balance}`, `{or_total}`, `{or_used}`, `{or_used_today}`, `{or_used_week}`, `{or_used_month}`, `{or_consumed_pct}`, `{or_free_tier}`, `{or_limit}`, `{or_limit_remaining}`, `{or_balance_bar}`

## Local development

```bash
# Iteration loop
ai-usagebar --vendor anthropic --watch 5

# Custom format preview
ai-usagebar --vendor openrouter --format '{or_balance} · today {or_used_today}'

# Custom tooltip
ai-usagebar --vendor zai --tooltip-format 'session: {zai_session_pct}% / weekly: {zai_weekly_pct}%'

# Unit + integration tests
make test

# Live API smoke (uses real creds — needs creds sourced)
source ~/.config/zsh/secrets
make smoke

# Lint
make clippy
```

## Theming

- One Dark palette by default.
- Auto-merges with the active Omarchy theme at `~/.config/omarchy/current/theme/colors.toml`.
- Per-color overrides: `--color-low`, `--color-mid`, `--color-high`, `--color-critical` (claudebar-compatible).

## Acknowledgements

Direct reverse-engineering reference for the OpenAI and Anthropic OAuth endpoints came from [`claudebar`](https://github.com/mryll/claudebar) and [`codexbar`](https://github.com/mryll/codexbar) (both by mryll). The visual design — bordered Pango tooltip, severity colors, pacing math — is theirs; this project is a faithful Rust port plus multi-vendor extension.

## License

MIT.
