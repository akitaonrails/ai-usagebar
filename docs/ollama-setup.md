# Ollama Cloud

Native provider for the cloud quota behind [ollama.com/settings](https://ollama.com/settings)
(session + weekly windows, per-model request counts, rolling 4-week activity
cost). **Not** the local daemon at `127.0.0.1:11434` — that process has no
quota route.

- [Credential](#credential)
- [Config](#config)
- [Run](#run)
- [What the bar shows](#what-the-bar-shows)
- [Troubleshooting](#troubleshooting)

## Credential

Mint a key at [ollama.com/settings/keys](https://ollama.com/settings/keys).
Put it in the environment, never in git:

```bash
export OLLAMA_API_KEY="…"          # Linux / macOS
```

```powershell
$env:OLLAMA_API_KEY = "…"          # Windows (session)
```

The Ed25519 key in `~/.ollama/id_ed25519` is a **registry** credential
(`pull` / `push`). `GET /api/usage` refuses it with 401. A browser cookie
from the settings page is also out of scope — see CONTRIBUTING.

## Config

Ollama Cloud is opt-in, like DeepSeek. Enable it in
`~/.config/ai-usagebar/config.toml` (or `%APPDATA%\ai-usagebar\config.toml`):

```toml
[ui]
primary = "ollama"

[ollama]
enabled = true
api_key_env = "OLLAMA_API_KEY"
# Label only — /api/usage does not send a plan field.
plan = "pro"
```

The `[ollama]` section is documented in [`config.example.toml`](../config.example.toml).
Copy it into your config and set `enabled = true`. `api_key_env` is the
**name of the variable**, not the token.

An inline `api_key` works (`chmod 600` the file) but the environment is
preferred.

Saving a key (or picking Ollama as primary) in the TUI Settings overlay
writes `enabled = true` for you.

## Run

```bash
# widget
ai-usagebar --vendor ollama

# TUI (Overview + an Ollama tab)
ai-usagebar-tui
```

```powershell
.\target\release\ai-usagebar.exe --vendor ollama
.\target\release\ai-usagebar-tui.exe
```

Default bar format: `{oll_session_pct}% · {oll_weekly_pct}%w`.

## What the bar shows

`GET https://ollama.com/api/usage` with `Authorization: Bearer <key>`:

| Field | Meaning |
|---|---|
| `limits.session.usage` | Fraction `[0, 1]` of the session window (rendered as %) |
| `limits.weekly.usage` | Fraction of the weekly window |
| `limits.*.models[]` | `{name, request_count}` per model in that window |
| `activity.cost` | Rolling cost as a **string** of dollars (`"0.00000"`) |
| `activity.period.type` | Always `"last_4_weeks"` today |

The JSON does **not** carry reset timestamps or a plan name (those exist
only in the HTML UI). The tooltip therefore shows `Resets in —` and uses
the `plan` string from config.

Live-verified shape (numbers redacted):

```json
{
  "limits": {
    "session": {
      "usage": 0.82,
      "models": [{"name": "kimi-k3", "request_count": 180}]
    },
    "weekly": {
      "usage": 0.23,
      "models": [{"name": "kimi-k3", "request_count": 180}]
    }
  },
  "activity": {
    "cost": "0.00000",
    "period": {"type": "last_4_weeks"}
  }
}
```

## Troubleshooting

**Vendor in Settings, missing as a tab.** `[ollama] enabled` is still
`false`. Enable it in TOML or pick Ollama Cloud as primary and Save.

**`HTTP 401` / `invalid credentials`.** Key missing, revoked, or you sent
the CLI Ed25519 session. Mint a new one at `/settings/keys`.

**`HTTP 404` against `127.0.0.1:11434/api/usage`.** That is the local
daemon. Quota lives only on `https://ollama.com/api/usage`.

**Probe the endpoint yourself:**

```bash
curl -sS -H "Authorization: Bearer $OLLAMA_API_KEY" \
  https://ollama.com/api/usage
```

```powershell
Invoke-RestMethod `
  -Uri "https://ollama.com/api/usage" `
  -Headers @{ Authorization = "Bearer $env:OLLAMA_API_KEY" }
```

See also: [configuration.md](./configuration.md),
[vendor-endpoints.md](./vendor-endpoints.md),
[DEVELOPMENT.md](../DEVELOPMENT.md).
