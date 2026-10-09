# Ollama Cloud

Native provider for cloud credits and legacy quotas behind
[ollama.com/settings](https://ollama.com/settings). **Not** the local daemon
at `127.0.0.1:11434` — that process has no quota route.

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
(`pull` / `push`), not an API key. Browser cookies are out of scope.

## Config

Ollama Cloud is opt-in. Enable it in
`~/.config/ai-usagebar/config.toml` (or `%APPDATA%/ai-usagebar/config/config.toml`):

```toml
[ui]
primary = "ollama"

[ollama]
enabled = true
api_key_env = "OLLAMA_API_KEY"
# Label only — the API does not send a plan name.
plan = "pro"
```

`api_key_env` is the **name of the variable**, not the token. An inline
`api_key` also works; `chmod 600` the file if you use it. Saving a key or
picking Ollama as primary in TUI Settings enables the provider.

**No plan-mode switch is needed.** The response automatically selects the
credit-based monthly plan or legacy session/weekly quotas. The $20 Pro
subscription's $60 usage allowance is read from the API, not hard-coded;
the subscription price is not the usage allowance.

## Run

```bash
ai-usagebar --vendor ollama
ai-usagebar-tui
```

```powershell
.\target\release\ai-usagebar.exe --vendor ollama
.\target\release\ai-usagebar-tui.exe
```

## What the bar shows

The provider queries the documented
[`GET https://ollama.com/api/balance`](https://docs.ollama.com/api/balance)
with `Authorization: Bearer <key>`.

### Credit-based plans

Example (synthetic values):

```json
{
  "included": {
    "balance_usd": 45,
    "allowance_usd": 60,
    "period": {
      "from": "2026-10-08T08:00:00Z",
      "until": "2026-11-08T08:00:00Z"
    }
  },
  "purchased": {"balance_usd": 12.5}
}
```

The default bar displays **$45.00 / $60.00** (remaining / included
allowance). The tooltip and TUI also show purchased credits separately.
Monthly utilization is **25% used**, calculated from the included balance
and allowance; purchased credits do not change that percentage. The reset
comes from `period.until`, and pacing uses the actual billing-period length,
not a fixed 30 days. A zero allowance has no percentage window.

Custom formats can use `{oll_balance}`, `{oll_allowance}`, `{oll_purchased}`
(all USD-formatted), `{oll_monthly_pct}`, and `{oll_monthly_reset}`.

### Legacy plans

The same endpoint returns remaining percentages and reset timestamps:

```json
{
  "included": {
    "session": {"remaining_percent": 75, "resets_at": "2026-10-01T07:00:00Z"},
    "weekly": {"remaining_percent": 40, "resets_at": "2026-10-05T00:00:00Z"}
  },
  "purchased": {"balance_usd": 25}
}
```

The default bar remains `{oll_session_pct}% · {oll_weekly_pct}%w`:
**25% · 60%w** in this example. These percentages are **used**, converted
from the API's remaining percentages. Reset timestamps are preserved.
Absent windows have empty placeholders, never fabricated zero percentages.

### Older caches and usage history

Historical `/api/usage` quota payloads (`limits.session`, `limits.weekly`,
or `limits.monthly`, plus activity/model counts) remain readable as cache
fallbacks. Their reset timestamps are unavailable.

The current [`/api/usage`](https://docs.ollama.com/api/cloud-usage) endpoint
returns request/token/cost history, **not remaining quota**. That shape is
rejected instead of silently turning into an empty snapshot. An existing
usage-history cache triggers a live balance fetch even if it is fresh.
The balance endpoint does not supply model counts or activity cost, so live
snapshots no longer populate those historical fields (`{oll_cost}` is `—`).

## Troubleshooting

**Vendor in Settings, missing as a tab.** Enable `[ollama] enabled = true`.

**HTTP 401.** Key missing, revoked, or a registry credential instead of an
API key. Mint a new key at `/settings/keys`.

**HTTP 404 from localhost.** The local daemon has no cloud quota route.

**HTTP 429.** Ollama limits the balance endpoint to 10 requests/minute per
user, shared across devices and keys; it recommends polling once per minute.

Probe the endpoint:

```bash
curl -sS -H "Authorization: Bearer $OLLAMA_API_KEY" \
  https://ollama.com/api/balance
```

```powershell
Invoke-RestMethod `
  -Uri "https://ollama.com/api/balance" `
  -Headers @{ Authorization = "Bearer $env:OLLAMA_API_KEY" }
```

See also [configuration.md](./configuration.md),
[format-placeholders.md](./format-placeholders.md), and
[vendor-endpoints.md](./vendor-endpoints.md).
