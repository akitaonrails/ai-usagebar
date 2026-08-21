# Format placeholders

Use placeholders with `--format` and `--tooltip-format`. Unsupported or absent
metrics expand to an empty string unless noted otherwise.

## Cross-provider formats

`{vendor_short}` identifies the active provider:

| Provider | Value | Provider | Value |
|---|---:|---|---:|
| Claude | `cld` | Codex | `gpt` |
| Z.AI | `zai` | OpenRouter | `opr` |
| DeepSeek | `dsk` | Kimi | `kmi` |
| Kilo | `klo` | Novita | `nvt` |
| Moonshot | `msh` | Grok | `grk` |
| SuperGrok | `sgk` | Anthropic API | `aac` |
| Antigravity | `agy` | Cursor | `cur` |
| GitHub Copilot | `ghc` | MiniMax | `mmx` |
| Kiro CLI | `kir` |  |  |

Use `{session_pct}`, `{session_reset}`, `{weekly_pct}`, and `{weekly_reset}`
when one format must work across providers. Providers without matching time
windows return neutral values. Cursor maps Cursor Models to the session slot
and Other Models to the weekly slot; both reset with the billing cycle. Kiro
has one pool, so it maps `kiro_pct` to both percentage slots.

Claude and Codex also provide `*_elapsed`, `*_pace`, and `*_bar` families.
Antigravity provides elapsed values for all four windows plus
`{session_model}`, `{weekly_model}`, `{scoped_model}`, and `{extra_model}`.
Provider-specific families such as `{oai_*}`, `{zai_*}`, and `{or_*}` are empty
for providers that do not define them.

## Shared and Claude placeholders

These are compatible with claudebar.

| Placeholder | Example |
|---|---|
| `{plan}` | `Max 5x` |
| `{session_pct}`, `{session_reset}`, `{session_bar}`, `{session_elapsed}` | `62`, `1h 30m`, `█████████████░░░░░░░`, `58` |
| `{session_pace}`, `{session_pace_indicator}`, `{session_pace_pct}`, `{session_pace_pts}`, `{session_pace_delta}`, `{session_pace_abs_delta}` | `↑`, `↑`, `12% ahead`, `4pts ahead`, `4`, `4` |
| `{weekly_*}` | The same family for the seven-day window. |
| `{sonnet_*}` | The same family for the seven-day Sonnet window. Empty when absent. |
| `{scoped_model}`, `{scoped_pct}`, `{scoped_reset}`, `{scoped_elapsed}`, `{scoped_bar}` | `Fable`, `84`, `5d 2h`, `27`, `█████████████████░░░` |
| `{extra_spent}`, `{extra_limit}`, `{extra_pct}`, `{extra_bar}` | `$2.50`, `$50.00`, `5`, `█░░░░░░░░░░░░░░░░░░░` |

The scoped family describes the first model-specific weekly window. When that
window is absent, it returns neutral empty, `0`, or `—` values as appropriate.

## Codex

`{oai_plan}`, `{oai_session_pct}`, `{oai_session_reset}`,
`{oai_session_elapsed}`, `{oai_session_pace}`,
`{oai_session_pace_indicator}`, `{oai_weekly_*}`,
`{oai_code_review_pct}`, `{oai_credit_balance}`, `{oai_local_msgs}`,
`{oai_cloud_msgs}`

Session and weekly families are empty when the API omits that window. The
default widget automatically uses weekly values for a weekly-only response.

## Z.AI

`{zai_plan}`, `{zai_session_pct}`, `{zai_session_reset}`,
`{zai_weekly_pct}`, `{zai_weekly_reset}`, `{zai_mcp_pct}`,
`{zai_mcp_reset}`

## OpenRouter

`{or_label}`, `{or_balance}`, `{or_total}`, `{or_used}`,
`{or_used_today}`, `{or_used_week}`, `{or_used_month}`,
`{or_consumed_pct}`, `{or_free_tier}`, `{or_limit}`,
`{or_limit_remaining}`, `{or_balance_bar}`

## DeepSeek

`{ds_balance}`, `{ds_granted}`, `{ds_topped_up}`, `{ds_available}`

These report the `/user/balance` credit balance. USD is preferred when both
currencies are present; otherwise they use CNY.

## Kimi

`{kimi_plan}`, `{kimi_weekly_pct}`, `{kimi_weekly_used}`,
`{kimi_weekly_limit}`, `{kimi_weekly_remaining}`, `{kimi_weekly_reset}`,
`{kimi_window_pct}`, `{kimi_window_used}`, `{kimi_window_limit}`,
`{kimi_window_remaining}`, `{kimi_window_reset}`

These cover the subscription quota and rolling five-hour window from
`api.kimi.com/coding/v1/usages`. Generic aliases are `{plan}` for the plan,
`{weekly_pct}` for weekly usage, and `{session_pct}` for the five-hour window.

## Kilo

`{kilo_balance}` is the remaining USD balance from
`api.kilo.ai/api/profile/balance`.

## Novita

`{nv_balance}`, `{nv_cash}`, `{nv_credit_limit}`, `{nv_owed}` report the USD
balance and its breakdown from `api.novita.ai/openapi/v1/billing/balance/detail`.

## Moonshot

`{km_balance}`, `{km_voucher}`, `{km_cash}`, `{currency}` report the account
balance from `api.moonshot.ai` or `api.moonshot.cn`. The global service uses
USD; the China service uses CNY.

## Grok

`{grok_balance}` is the prepaid USD balance from the xAI Management API.

## SuperGrok

`{sgk_plan}`, `{sgk_pct}`, `{sgk_reset}`, `{sgk_period}`, `{sgk_prepaid}`

- `{sgk_period}` is `Weekly`, `Monthly`, or `Current period`.
- The default bar format is `{sgk_pct}% · {sgk_reset}`.
- `{session_pct}` and `{weekly_pct}` remain aliases for `sgk_pct`.
- `{plan}` is the subscription tier when Grok Build supplies one.

SuperGrok is the subscription path provided by Grok Build's `x.ai/billing` ACP
extension. It is separate from the Grok Management API prepaid balance.
ai-usagebar never parses, copies, caches, refreshes, or sends the SuperGrok
token in ACP messages. It hashes auth and config files only to keep caches
separate between logins.

The default executable is `$GROK_HOME/bin/grok`, or `~/.grok/bin/grok` when
`GROK_HOME` is unset. ai-usagebar does not search `PATH`. Set
`[supergrok] grok_binary` only when the trusted official binary lives elsewhere.

## Anthropic API

`{aapi_headline}`, `{aapi_spent}`, `{aapi_limit}`, `{aapi_pct}` report
month-to-date spend from the Admin API `cost_report`.

- With a positive finite `monthly_limit`, the headline looks like
  `$1.34 / $1000 · 0%`.
- Without a limit, it looks like `$1.34/mo`.
- `{plan}`, `{session_pct}`, and `{weekly_pct}` are generic aliases. Both
  percentage aliases use spend versus limit.

This is spend, not prepaid balance. Anthropic does not expose prepaid balance
through the API. The
[Cost API documentation](https://platform.claude.com/docs/en/manage-claude/usage-cost-api)
also says that Priority Tier costs are omitted.

## Cursor

`{cursor_plan}`, `{cursor_auto_pct}`, `{cursor_api_pct}`,
`{cursor_total_pct}`, `{cursor_reset}`, `{cursor_on_demand}`,
`{cursor_unlimited}`

- `{cursor_auto_pct}` is the Cursor Models pool (Auto and Composer).
- `{cursor_api_pct}` is the Other Models pool (named and API models).
- `{cursor_total_pct}` is the overall included-usage headline.
- `{cursor_on_demand}` and `{cursor_unlimited}` return `on`/`off` and
  `yes`/`no`.
- `{session_pct}`, `{weekly_pct}`, and `{plan}` alias Cursor Models, Other
  Models, and `Cursor <Plan>`.

A pool can exceed 100%. The default format is
`{cursor_auto_pct}·{cursor_api_pct}%` and uses the worse pool's severity color.

Cursor's dashboard also reports overage and per-member team spend; ai-usagebar
does not. Team payloads without `individualUsage.plan` fall back to the
dashboard's display messages and add `(team)` to the inferred plan. This path
has not been verified against a live team account.

## GitHub Copilot

`{copilot_headline}`, `{copilot_login}`, `{copilot_plan}`, `{copilot_reset}`,
`{copilot_primary_pct}`, `{copilot_chat_state}`, `{copilot_chat_pct}`,
`{copilot_chat_remaining}`, `{copilot_chat_entitlement}`,
`{copilot_completions_state}`, `{copilot_completions_pct}`,
`{copilot_completions_remaining}`, `{copilot_completions_entitlement}`,
`{copilot_premium_state}`, `{copilot_premium_pct}`,
`{copilot_premium_remaining}`, `{copilot_premium_entitlement}`

- `{copilot_premium_*}` is the Premium Requests pool when the account has one.
- A pool whose state is `n/a` is not part of that plan (`has_quota = false`);
  it is not the same as `unlimited`.
- The default bar format is `{copilot_headline}`. It shows Premium Requests
  when present, otherwise falls back to chat/completions.
- `{plan}` aliases `Copilot <Plan>`. `{session_pct}` aliases the primary pool
  percentage, and `{weekly_pct}` the next-most-useful secondary pool.

## Kiro CLI

`{kiro_plan}`, `{kiro_pct}`, `{kiro_used}`, `{kiro_limit}`, `{kiro_reset}`

These describe the current credit cycle returned by
`AmazonCodeWhispererService.GetUsageLimits`, the same call used by kiro-cli's
`/usage` command. Raw credit counts keep two decimal places only when needed.
The default format is `{kiro_pct}%`; `{session_pct}` and `{weekly_pct}` alias
the same pool, and `{plan}` aliases the subscription title.

Kiro access tokens expire after roughly an hour. ai-usagebar refreshes them
through the documented AWS SSO OIDC `CreateToken` API and stores refreshed or
rotated credentials in an account-scoped `kiro/oauth.json` file. That file is
mode `0600` on Unix. kiro-cli's database is opened read-only and is never
modified.
