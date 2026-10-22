# Grok and SuperGrok

Two different bills, two providers. They do not share a number.

| What you want | Provider | Credential |
|---|---|---|
| SuperGrok / X subscription weekly pool (grok.com, Grok apps, Grok CLI) | `[supergrok]` | An existing `grok login` |
| xAI API team prepaid credit (console.x.ai) | `[grok]` | An xAI **management** key |

## SuperGrok (the consumer subscription)

xAI's docs describe the included pool — a weekly allowance shared across Grok
surfaces, plus additional usage after that pool is spent — and tell you to
read it in the account's usage view. They do not publish a Management API, or
any other docs.x.ai route, that returns that percentage.

The official Grok CLI does. `grok login` (browser OAuth at `auth.x.ai`, or
`grok login --device-auth` on a machine without a browser) stores the session
in `~/.grok/auth.json`, the same way `claude` and `codex login` store theirs.
The CLI's `/usage` command reads the pool from
`GET https://cli-chat-proxy.grok.com/v1/billing?format=credits` with that
login's key. That call lives in the open-source Grok CLI
(`xai-org/grok-build`); it is not a docs.x.ai contract, and it can change when
the CLI changes.

ai-usagebar reuses the login the CLI already wrote. It reads `auth.json` for
the key, sends that key only in the `Authorization` header of the billing
request, and does not copy, cache, refresh, or write the key back. Token
refresh stays with the Grok CLI: if the bar says the login was rejected, run
`grok` or `grok login` again. If that HTTPS call fails, the widget falls back
to the CLI's `x.ai/billing` ACP extension (`grok agent stdio`), which asks the
CLI itself for the same document.

### What the bar shows

- Included usage for the current period (weekly, or monthly on older
  accounts), as a percentage, with the reset countdown.
- Per-product slices of that same pool when the billing document sends them
  (Grok Build, Grok Chat, Grok Imagine, and any later product name).
- **Additional usage** when the document reports on-demand spend or a cap:
  dollars used, and `$used / $cap` when a cap is set. A zero pair is omitted.
  Placeholder: `{sgk_on_demand}`.
- Prepaid top-up balance, only when it is greater than zero. A unified
  account's API dollars live on the `[grok]` row instead.

Banked reset credits, when shown, come from a separate `grok.com` call. That
call is not part of `/usage` and is not a documented API. If it fails, the
usage percentage above is unchanged.

### Setup

Install the official Grok CLI, then:

```bash
grok login
```

```toml
[supergrok]
enabled = true
# Only if the binary is not at $GROK_HOME/bin/grok or ~/.grok/bin/grok:
# grok_binary = "/opt/grok/bin/grok"
```

ai-usagebar does not search `PATH` for `grok`. An inference API key
(`XAI_API_KEY`) does not authenticate this provider.

## xAI API credits (`[grok]`)

The documented place to read API money is the Management API
([billing reference](https://docs.x.ai/developers/rest-api-reference/management/billing)):

`GET https://management-api.x.ai/v1/billing/teams/{team}/prepaid/balance`

with a management key from the xAI Console (Settings → Management Keys). An
inference key is rejected. A team-scoped key supplies its own team id; an
organization-scoped key needs `team_id` under `[grok]`.

The figure is prepaid USD left on that API team. It does not move when you
chat on grok.com, and the SuperGrok percentage does not move when you spend
API credit. The published API rate limits are per-model request and token
ceilings for that team; a `429` from `api.x.ai` is not a reading of the
weekly subscription pool, so the widget does not call the inference API to
harvest headers.

```toml
[grok]
enabled = true
api_key_env = "XAI_MANAGEMENT_KEY"
# team_id = "your-team-id"   # organization-scoped keys only
```

## What this widget does not do

It does not sign into grok.com, read browser cookies, or call the web app's
private usage RPCs to compute the weekly pool. The subscription percentage
comes from the Grok CLI login above. There is no key to paste for SuperGrok.
