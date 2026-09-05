# Vendor endpoints and live tests

Some providers do not publish a stable usage API. ai-usagebar keeps its parsers
defensive and includes opt-in live tests for catching response changes.

## Support matrix

| Vendor | Endpoint | What you see | Native desktop selector (v0.13) |
|---|---|---|---|
| **Claude** | `api.anthropic.com/api/oauth/usage` (undocumented) | Session (5h), Weekly (7d), model-scoped weekly (e.g. Fable), Extra usage $ | Yes |
| **Codex** | `chatgpt.com/backend-api/wham/usage`, plus `…/wham/rate-limit-reset-credits` when any are banked (undocumented; both used by the official `codex` CLI) | Codex 5h and/or weekly, Code-review weekly, named extra limits with their own windows, models currently at capacity, Credits, banked reset credits + expiry | Yes |
| **GitHub Copilot** | `api.github.com/copilot_internal/user` (private; used by VS Code) | Premium requests, Chat, and Completions quota %, counts when supplied, plan, reset | Yes |
| **Z.AI** | `api.z.ai/api/monitor/usage/quota/limit` (undocumented) | Session 5h, Weekly 7d, MCP tools monthly | Yes |
| **OpenRouter** | `openrouter.ai/api/v1/{credits,key}` (documented) | Balance, today/week/month spend, free vs paid tier | Yes |
| **DeepSeek** | `api.deepseek.com/user/balance` (documented) | Balance, granted, topped-up credits | Yes |
| **Kimi** | `api.kimi.com\|.ai/coding/v1/usages` (undocumented; community-confirmed), plus `auth.kimi.com\|.ai/api/oauth/token` to refresh a Kimi Code CLI login | Weekly subscription quota + 5h rolling rate-limit window | No — widget/TUI only; desktop protocol and marker parity are future work |
| **MiniMax** | `api.minimax.io/v1/token_plan/remains` (official Token Plan quota route) | Token Plan rolling interval window + weekly, per model bucket (text, video) | No — widget/TUI only |
| **Kilo** | `api.kilo.ai/api/profile/balance` (undocumented; extension-internal) | Remaining credit balance ($) | No — widget/TUI only |
| **Novita** | `api.novita.ai/openapi/v1/billing/balance/detail` (documented) | Remaining credit balance ($) | No — widget/TUI only |
| **Moonshot** | `api.moonshot.ai\|.cn/v1/users/me/balance` (documented) | Account balance ($ on `.ai`, ¥ on `.cn`) | No — widget/TUI only |
| **Grok (xAI)** | `management-api.x.ai/v1/billing/teams/{team}/prepaid/balance` (Management API; documented) | Prepaid credit balance ($) | No — widget/TUI only |
| **SuperGrok** | `cli-chat-proxy.grok.com/v1/billing` with the Grok Build login's key, falling back to its `x.ai/billing` ACP extension; `grok.com` `ConsumerUiSvc/GetRemainingResets` for banked resets | Current weekly/monthly included-credit %, prepaid API balance, reset, banked resets + expiry | No — widget/TUI only |
| **Anthropic API** | `api.anthropic.com/v1/organizations/cost_report` (Admin API; documented) | Month-to-date spend ($, excludes Priority Tier), optional spend-vs-limit % | No — widget/TUI only |
| **Google Antigravity** | A loopback RPC on the local Antigravity product's own port, discovered from `/proc` (Linux), `lsof` (macOS), or the process/TCP tables (Windows); no remote endpoint and no credential | Whichever quota windows the running product reports — Gemini and Claude/GPT pools, 5-hour and weekly | Yes |
| **Cursor** | `cursor.com/api/usage-summary` (undocumented; the dashboard's own frontend) | Two included-usage pools this billing cycle — Cursor Models (Auto/Composer) % and Other Models (named/API) % — plus plan, reset, on-demand | Yes |
| **Kiro CLI** | `codewhisperer.<region>.amazonaws.com` `GetUsageLimits` (undocumented; the same call kiro-cli's own `/usage` slash command makes) | Single credit pool this cycle — used/limit/%, plan, reset | No — widget/TUI only |
| **Nous Research** | `portal.nousresearch.com/api/oauth/account` (OAuth-authenticated Portal account response) | Subscription usage %, subscription credits, top-up/purchased credits, total usable credits, renewal | Yes |
| **OpenCode Go** | `opencode.ai/zen/go/v1/usage` | Rolling, weekly, and monthly `percent` windows with absolute reset timestamps | Yes |
| **Command Code** | `api.commandcode.ai` `/alpha/billing/credits` + `/alpha/billing/subscriptions` (undocumented; the same calls the official `commandcode` CLI's `/usage` makes) | 5-hour and weekly rolling spend windows ($ used of $ cap), plan, and remaining monthly credits | No — widget/TUI only |


## Providers evaluated and not added

Requests for a new provider come down to one question: **is the quota reachable
with a credential the user already has, obtained the way this project obtains
credentials?** Every supported vendor uses one of three: an API key the user
holds, an OAuth file an official CLI wrote (`~/.codex/auth.json`, kiro-cli's
`data.sqlite3`, Cursor's `state.vscdb`), or an official CLI invoked for a token
(`gh auth token`). CLI, editor and browser credentials are never parsed, copied,
or stored, and no vendor asks the user to paste a session cookie.

| Provider | Status | Why |
|---|---|---|
| **Xiaomi MiMo** (Token Plan) | Not implementable | The quota routes (`platform.xiaomimimo.com/api/v1/tokenPlan/{usage,detail}`) authenticate with a Xiaomi Account **web SSO session**, not the plan's API key. The API key reaches only the inference gateway, which exposes no quota surface and returns no rate-limit headers. The effective session credential is an HttpOnly cookie, so there is no CLI-written file to read — only a browser profile. Waiting on Xiaomi to expose quota to API keys. (#146) |
| **Alibaba Cloud Model Studio** (Token Plan) | Viable, wanted | 5-hour and weekly percentage windows with epoch-ms resets — the Codex/Kimi/Z.AI shape. Usage needs the console credential rather than the `sk-sp-` inference key, but the official `bl` CLI stores that credential locally after `bl auth login --console`, which is the same pattern as Kiro CLI and Command Code. Blocked only on evidence: the credential's on-disk shape and a real response capture. (#147) |


## Stability notes

| Provider | Status |
|---|---|
| Claude | Undocumented usage endpoint, but used by the official `claude` CLI. Less fragile than a scraped web page. |
| Codex | Undocumented ChatGPT usage endpoint used by the official `codex` CLI. Windows are identified by duration instead of response position. |
| GitHub Copilot | Private endpoint used by VS Code. It requires a GitHub OAuth token and VS Code-compatible client headers; ai-usagebar gets it from the official `gh auth token` command after `gh auth login --web`. A non-empty `GITHUB_COPILOT_TOKEN` is an optional explicit override. GitHub CLI/editor/browser credentials are never parsed, copied, or stored. |
| Z.AI | Reverse-engineered from a third-party plugin. Treat this as the most fragile integration. |
| Kimi | Community-confirmed `/coding/v1/usages` route used by third-party quota tools. Drift is possible. The refresh grant is the Kimi Code CLI's own documented-by-behaviour device-flow token endpoint, using the CLI's public client id. |
| Cursor | Undocumented endpoint called by Cursor's dashboard. Its shape may change with Cursor pricing. |
| MiniMax | The Token Plan route is official, but no formal response schema is published. |
| Kiro CLI | `GetUsageLimits` is the same undocumented CodeWhisperer operation used by kiro-cli's `/usage` command. AWS SSO OIDC `CreateToken`, used for refresh, is documented. |
| Command Code | Undocumented `/alpha/*` routes called by the official `commandcode` CLI. The `alpha` path segment is the vendor's own signal that these may move. Windows are read by name (`fiveHour`, `weekly`) rather than by position, and `windowLimits` is accepted both at the top level and beside the ledger, so the most likely reshuffles are already tolerated. |

Codex's known five-hour and seven-day windows are matched by their reported
duration, not by `primary_window` or `secondary_window` position. This handles
both the normal response and the temporary
[weekly-only response](https://github.com/openai/codex/issues/32707) without a
config switch.

### Banked resets

Codex and SuperGrok both let you *earn* quota resets and redeem them by hand,
which is a different thing from the window rollover in the table above. Both
report them behind a second endpoint, and both are read-only here:
ai-usagebar shows what you have and when it lapses, and never redeems one. The
redemption identifier each provider returns beside the expiry
(`credits[].id`, `tokens[].token_id`) is skipped during parsing rather than
parsed and dropped, so it reaches neither the cache nor the screen.

| Provider | Endpoint | Notes |
|---|---|---|
| Codex | `GET chatgpt.com/backend-api/wham/rate-limit-reset-credits` | Called only when the usage response's `rate_limit_reset_credits.available_count` is non-zero. The count always comes from the usage response — that is the one consistent with the quota figures beside it. A failure of this call costs the expiry date and nothing else. |
| SuperGrok | `POST grok.com/prod_mc_billing.ConsumerUiSvc/GetRemainingResets` | gRPC-Web (`application/grpc-web+proto`) with the Grok Build login's own bearer key — the same key `direct.rs` uses, in one outgoing header, never copied or cached. The response is a `repeated ConsumerResetToken` whose `validity_end` is the expiry; a hand-written bounded protobuf reader takes the count and that field, skipping everything else by wire type. gRPC reports failure in a trailer *behind* HTTP 200, so the trailer's `grpc-status` is checked before any count is believed. |

Neither is documented, and both are more fragile than the usage endpoints they
accompany. Both fail quietly by design: a broken reset call leaves the rest of
the vendor's snapshot exactly as it was.

## Run the live tests

```bash
make smoke
```

Claude, Codex, Z.AI, and OpenRouter tests require their normal credentials or
API keys. Command Code needs no key of its own — it reuses whichever local
agent harness is signed in, and skips when none is. Kimi is optional: its test
prints a skip reason when `KIMI_API_KEY` is unset (the smoke test covers the
API-key path; a subscription login is exercised by `ai-usagebar --vendor kimi`).

To test only Kimi:

```bash
cargo test --test live kimi_live -- --ignored --nocapture
```

The tests validate the fields used by ai-usagebar and report which part of a
response changed.
