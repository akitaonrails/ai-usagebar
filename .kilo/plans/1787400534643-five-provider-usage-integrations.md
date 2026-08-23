# Plan: Five Official Usage Providers

## Goal

Extend `ai-usagebar` with native Rust support for Tavily, Firecrawl, Requesty,
ZenMux, and Vercel AI Gateway while preserving current cache, security,
rendering, and additive `usage --json` contracts.

## Fixed Decisions

- Deliver five vertical slices in this order: Tavily, Firecrawl, Requesty,
  ZenMux, then Vercel AI Gateway.
- Support one account or project per provider in this release.
- Keep provider-specific modules and `VendorSnapshot` variants; do not create a
  generic adapter framework or refactor existing providers.
- Reuse existing Rust primitives (`Cache`, capped response reads, redirect
  policy, money validation, `UsageWindow`, `SectionBuilder`) and port useful
  MIT-licensed patterns selectively; external command-line interfaces are not
  runtime dependencies.
- Fetch balance plus detailed usage where official endpoints provide it.
- When a secondary endpoint fails, display valid primary data, leave missing
  fields absent, and expose a sanitized warning; never coerce missing data to
  zero.
- Use provider billing cycles when returned. Date-range APIs use calendar
  month-to-date in Coordinated Universal Time (UTC).
- Vercel custom reporting is opt-in, with a configurable six-hour default Time
  to Live (TTL); `/v1/credits` remains always available when the provider is
  enabled.
- Include parity across the current widget, Terminal User Interface (TUI),
  JavaScript Object Notation (JSON) report, settings bridge, GNOME, KDE Plasma,
  Omarchy, and macOS adapters.
- Treat a Windows 11 tray frontend as a separately planned future phase; this
  release must keep the Rust command-line interface, TUI, and JSON report
  portable on Windows.

## External Contracts

| Provider | Authentication | Primary data | Detailed data |
|---|---|---|---|
| Tavily | Bearer `TAVILY_API_KEY`; optional `X-Project-ID` | `GET https://api.tavily.com/usage` | Same response: key/account limits, plan, pay-as-you-go, and endpoint breakdown |
| Firecrawl | Bearer `FIRECRAWL_API_KEY` | `GET https://api.firecrawl.dev/v2/team/credit-usage` | `GET /v2/team/credit-usage/historical?byApiKey=false` |
| Requesty | Bearer `REQUESTY_API_KEY` with management read permission | `GET https://api-v2.requesty.ai/v1/manage/org` | `GET /v1/manage/org/usage` for month-to-date UTC, daily resolution, no grouping |
| ZenMux | Bearer `ZENMUX_MANAGEMENT_API_KEY`; standard inference keys are invalid | `GET https://zenmux.ai/api/v1/management/payg/balance` | `GET /api/v1/management/subscription/detail` |
| Vercel AI Gateway | Bearer `AI_GATEWAY_API_KEY`; configurable environment variable permits an OpenID Connect token | `GET https://ai-gateway.vercel.sh/v1/credits` | Opt-in `GET /v1/report`, month-to-date UTC, grouped by day |

Official documentation is authoritative. Before implementing each slice,
recheck its response schema and authentication notes against the linked API
reference; tests must encode only documented fields used by the application.

## Data and Failure Model

1. Add one snapshot type and one `VendorSnapshot` variant per provider.
2. Keep exact vendor fields instead of flattening them into a universal shape.
3. Require documented envelope and monetary/quota fields. Reject malformed,
   negative where impossible, non-finite, duplicate, unsupported-currency, or
   inconsistent values as schema errors.
4. Store a non-secret scope fingerprint in each cache payload, derived from the
   API key plus optional project/target context. A changed key or project must
   force a refetch and must never reuse another scope's data.
5. For multi-endpoint providers, represent optional detail blocks explicitly.
   A snapshot is usable when its primary block succeeds; ZenMux is usable when
   either PAYG or subscription data succeeds.
6. Cache a partial live snapshot atomically, then persist one sanitized
   secondary diagnostic through the existing `.last_error` sidecar. Keep
   `stale = false` when current primary data is live. Use a short retry horizon
   for partial snapshots rather than treating them as fully fresh for the
   normal TTL.
7. Keep Vercel report data in an independently cached detail component so its
   six-hour TTL does not delay credit refreshes. A report-plan `403`, unsupported
   plan response, or transient failure must not suppress `/v1/credits` data.
8. Preserve the seven-day maximum stale fallback, atomic writes, asynchronous
   locks, capped bodies, same-origin redirects, authentication-body redaction,
   and widget exit-zero fallback behavior.

## Provider Presentation Contracts

### Tavily

- Snapshot: current plan; plan used/limit; pay-as-you-go used/limit; key
  used/optional limit; search, extract, crawl, map, and research usage.
- Headline/severity: plan percentage when the plan limit is positive; otherwise
  a truthful text headline with no fabricated percentage.
- Optional project ID scopes the request and cache.
- No reset timestamp is shown unless the API starts returning one.

### Firecrawl

- Snapshot: remaining credits, base plan credits, billing period start/end, and
  optional current-period total credits consumed from historical usage.
- Headline/severity: current-period used/base-plan percentage only when both
  values are available and the denominator is positive. Preserve percentages
  above 100 in labels; clamp only the visual gauge.
- Show remaining credits separately because packs, coupons, or recharge credits
  are not included in `planCredits`.
- Use billing period end as the absolute reset timestamp.

### Requesty

- Snapshot: organization name, balance, month-to-date spend, request counts,
  input/output/total tokens, and queried interval.
- Aggregate all returned daily entries with checked integer addition and finite
  monetary addition.
- Headline: balance. Detailed spend and token rows remain non-percentage text
  because no budget denominator is reported.

### ZenMux

- Snapshot contains optional PAYG and subscription blocks.
- PAYG block: total, top-up, and bonus USD credits.
- Subscription block: tier, status, expiry, five-hour and seven-day windows,
  monthly maximums, flow counts, and USD values.
- Headline/severity: worst live five-hour/seven-day quota when subscription data
  exists; otherwise PAYG balance with balance-specific severity.
- Treat `usage_percentage` as a fraction and validate it before conversion to
  integer percent. Preserve account statuses without treating unknown future
  values as healthy.

### Vercel AI Gateway

- Snapshot: USD balance, lifetime total used, and optional month-to-date report
  totals for cost, token categories, and requests.
- Headline: balance; report values are non-percentage details.
- Config includes `report_enabled = false` and
  `report_cache_ttl_seconds = 21600`; reject zero or unreasonable values using
  existing configuration-validation conventions.
- UI and documentation must state that custom reporting is beta, restricted to
  eligible plans, and billed per query.

## Ordered Implementation Tasks

### 1. Shared Registration Surface

- Add the five IDs, stable slugs, display names, secret environment variables,
  command-line enum mappings, and active-vendor slug parsing in
  `src/vendor.rs`, `src/widget/cli.rs`, and `src/active.rs` as each vertical
  slice lands.
- Add opt-in configuration sections and defaults in `src/config.rs`; include
  inline-key permission detection, validation, environment-key resolution, and
  `config.example.toml` coverage.
- Register API-key providers in `src/tui/settings.rs` without exposing key
  values through settings JSON.
- Export each module from `src/lib.rs` and add its snapshot variant to
  `src/usage.rs`.

### 2. Tavily Vertical Slice

- Create `src/tavily/{mod,types,fetch,vendor}.rs`, using DeepSeek for the
  single-endpoint cache flow and Kiro/Kimi for exact quota arithmetic.
- Implement defensive wire parsing, project-scoped cache attribution, renderer
  placeholders, severity, widget dispatch, TUI refresh, overview cells, and
  detailed sections.
- Add mocked response/cache/failure tests, configuration tests, ignored live
  smoke test, JSON report assertions, format placeholders, endpoint matrix,
  README usage, and changelog entry.
- Complete all current desktop adapter mappings and tests before starting
  Firecrawl.

### 3. Firecrawl Vertical Slice

- Create `src/firecrawl/{mod,types,fetch,vendor}.rs`.
- Fetch current and historical endpoints concurrently where cache state allows;
  retain current credit data when historical usage fails.
- Validate billing timestamps, match the historical row to the current billing
  period, and keep unmatched history absent rather than selecting an arbitrary
  row.
- Implement cache round trips, partial-warning tests, reset metadata through
  `SectionBuilder::push_metric`, all interface mappings, documentation, and
  ignored live smoke coverage.

### 4. Requesty Vertical Slice

- Create `src/requesty/{mod,types,fetch,vendor}.rs`.
- Query organization and month-to-date usage concurrently; build RFC 3339 UTC
  bounds from an injected clock for hermetic tests.
- Aggregate daily usage with overflow and finiteness checks; preserve live
  organization balance when usage fails or permission is insufficient.
- Add non-percentage report rows, all interface mappings, mocked permission and
  partial-failure cases, documentation, and ignored live smoke coverage.

### 5. ZenMux Vertical Slice

- Create `src/zenmux/{mod,types,fetch,vendor}.rs`.
- Fetch PAYG and subscription endpoints independently with the management key;
  require at least one valid block.
- Convert documented quota fractions and timestamps into native windows while
  retaining flow/value details and account status.
- Test PAYG-only, subscription-only, both-success, both-fail, invalid standard
  key, rate-limit `422`, unknown status, schema drift, cache attribution, all
  interfaces, documentation, and ignored live smoke coverage.

### 6. Vercel AI Gateway Vertical Slice

- Create `src/vercel_gateway/{mod,types,fetch,vendor}.rs` with slug
  `vercel-ai-gateway` and canonical display name `Vercel AI Gateway`.
- Always refresh credits under normal cache rules. Query the paid report only
  when enabled and its independent six-hour cache is expired.
- Aggregate daily report rows with checked arithmetic. Do not confuse lifetime
  `total_used` with month-to-date report cost.
- Test report disabled, eligible report success, unsupported-plan partial
  warning, report rate/cost protection, independent cache TTLs, OpenID Connect
  environment override, all interfaces, documentation, and ignored live smoke
  coverage.

### 7. Cross-Provider Contract Verification

- Update exhaustive matches in `src/tui/app.rs`, `src/tui/panels.rs`, and
  `src/widget/run.rs`; keep `src/report.rs` vendor-agnostic.
- Ensure every provider emits generic placeholders plus vendor-specific ones.
  Balance-only providers must not expose fake zero-percent metrics.
- Update GNOME marker classification, macOS fixed field mapping/tests, and any
  provider lists in adapter documentation. KDE Plasma and Omarchy must continue
  selecting entries client-side from one `usage --json` call and must never add
  `--vendor` to KDE's fetch command.
- Verify `VendorId::display_name()` remains the only canonical provider-name
  table.

### 8. MIT Source-Reuse Gate

- Before each adapter, inspect OpenUsage, ccusage, TokenTracker, and aiusage at a
  pinned commit for parsing, cache-scope, and test ideas. Current research found
  no confirmed first-class adapter for these five providers, so official API
  contracts remain the implementation source of truth.
- Prefer independent Rust implementation. If a substantial code portion is
  ported, record upstream repository, commit, source file/function, copyright,
  and MIT license in a new third-party notices document; retain required license
  text and add focused provenance comments only where legally necessary.
- Do not import their databases, daemons, JavaScript runtimes, Go binaries, or
  command-line output contracts into production execution.

### 9. Validation and Release Readiness

- Run focused unit and mocked integration tests after every vertical slice.
- Run the repository baseline: `make test`.
- Run Continuous Integration parity:
  - `cargo fmt --all -- --check`
  - `cargo clippy --all-targets --locked -- -D warnings`
  - `cargo test --all-targets --locked`
  - `make desktop-test`
  - `cargo machete`
- Run `make qml-lint` and `make qml-test` when Qt 6 tools are available; keep
  Qt Modeling Language (QML) V4 compatibility.
- Run `./macos/run-tests.sh` on macOS. Record platform validation as pending if
  the implementation environment cannot execute it; do not claim success from
  Windows/Linux substitutes.
- Keep all live tests ignored and credential-gated. Never print keys, payloads
  containing account identifiers, or real configuration.
- Verify `usage --json` ordering, non-percentage rows, severity, absolute reset
  timestamps, partial warnings, stale state, and additive compatibility using
  fixtures.
- Update `README.md`, `docs/configuration.md`,
  `docs/format-placeholders.md`, `docs/vendor-endpoints.md`, adapter READMEs,
  and `CHANGELOG.md` with generic placeholder data only.
- Follow `CLAUDE.md` only when cutting a release; provider work alone must not
  opportunistically change version/package artifacts.

## Rollback Strategy

- Each provider defaults disabled and is independently removable.
- Keep each vertical slice in a separate logical commit or pull request so a
  failing provider can be reverted without touching earlier slices.
- If a detailed endpoint becomes unstable, disable only its detail query and
  retain the provider's primary balance/quota path; do not remove its stable ID
  from the additive JSON contract after release.
- If a wire schema drifts, fail closed to stale cache plus warning; never loosen
  required fields merely to make tests pass.

## Future Windows 11 Phase

- Preserve `ai-usagebar usage --json` as the sole data contract for a future
  notification-area application; it must not fetch providers or store
  credentials independently.
- Define a separate plan comparing Windows Presentation Foundation and Windows
  App SDK/WinUI 3 only after the five-provider release. Framework selection,
  installer, auto-start, signing, update channel, and accessibility are
  explicitly outside this implementation.
- Future acceptance criteria: per-user installation, no administrator rights,
  safe binary discovery, bounded subprocess execution, graceful fallback when
  the Rust binary is absent, theme/DPI support, keyboard accessibility, and
  parity with JSON entry ordering and warnings.

## References

- Tavily usage: https://docs.tavily.com/documentation/api-reference/endpoint/usage
- Firecrawl current usage: https://docs.firecrawl.dev/api-reference/endpoint/credit-usage
- Firecrawl historical usage: https://docs.firecrawl.dev/api-reference/endpoint/credit-usage-historical
- Requesty organization: https://docs.requesty.ai/api-reference/endpoint/manage-org-get
- Requesty organization usage: https://docs.requesty.ai/api-reference/endpoint/manage-org-get-usage
- ZenMux PAYG balance: https://docs.zenmux.ai/api/platform/payg-balance.html
- ZenMux subscription: https://docs.zenmux.ai/api/platform/subscription-detail.html
- Vercel credits: https://vercel.com/docs/ai-gateway/sdks-and-apis/rest-api#check-credit-balance
- Vercel reporting: https://vercel.com/docs/ai-gateway/observability-and-spend/custom-reporting
- OpenUsage: https://github.com/janekbaraniewski/openusage
- ccusage: https://github.com/ccusage/ccusage
- TokenTracker: https://github.com/mm7894215/TokenTracker
- aiusage: https://github.com/juliantanx/aiusage
