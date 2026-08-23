# Plan: Five Official Usage Providers

> Revised 2026-08-23 — audit against current `ai-usagebar` contracts (Rust 1.88, 18 `VendorId` variants, `Cache`/`VendorSnapshot`/`SectionBuilder` primitives, hermetic-test and `usage --json` invariants). Adjustments are marked with **REV**. The companion specification at `spec/spec-tool-five-provider-usage-integrations.md` is authoritative for provider contracts (endpoints, wire fields, placeholders, cache rules); this plan governs sequencing, rollback, and release gating.

## Goal

Extend `ai-usagebar` with native Rust support for **Tavily**, **Firecrawl**, **Requesty**, **ZenMux**, and **Vercel AI Gateway** while preserving current cache, security, rendering, and additive `usage --json` contracts.

## Non-Goals (Explicit)

- Generic adapter framework or refactor of existing providers.
- Multi-account/project per new provider in this release (single scope only).
- Windows 11 notification-area frontend — separately planned.
- Importing external databases, daemons, JS runtimes, or CLI output contracts.

## Fixed Decisions

- Deliver five vertical slices in this order: Tavily, Firecrawl, Requesty, ZenMux, then Vercel AI Gateway.
- Support one account or project per provider in this release.
- Keep provider-specific modules and `VendorSnapshot` variants; do not create a generic adapter framework or refactor existing providers.
- Reuse existing Rust primitives (`Cache`, capped response reads, redirect policy, money validation, `UsageWindow`, `SectionBuilder`, `vendor_secret_env_vars_to_remove`) and port useful MIT-licensed patterns selectively; external CLIs are not runtime dependencies.
- Fetch balance plus detailed usage where official endpoints provide it.
- When a secondary endpoint fails, display valid primary data, leave missing fields absent, and expose a sanitized warning; never coerce missing data to zero.
- Use provider billing cycles when returned. Date-range APIs use calendar month-to-date in UTC.
- Vercel custom reporting is opt-in, with a configurable six-hour default TTL; `/v1/credits` remains always available when the provider is enabled.
- Include parity across the current widget, TUI, JSON report, settings bridge, GNOME, KDE Plasma, Omarchy, and macOS adapters. **REV**: GNOME/macOS parity means marker mapping and `fixedFieldMapping` tests; widget/TUI/JSON remain authoritative — no duplicate provider-name tables in frontends.
- Treat a Windows 11 tray frontend as a separately planned future phase; this release must keep the Rust CLI, TUI, and JSON report portable on Windows (home via `%USERPROFILE%`, no Waybar dependency).
- **REV**: `vendor_short` codes for placeholders are fixed in this plan to avoid collisions: Tavily `tav`, Firecrawl `fcw`, Requesty `rqy`, ZenMux `zmx`, Vercel AI Gateway `vag`.

## External Contracts

| Provider | Authentication | Primary data | Detailed data | Notes |
|---|---|---|---|---|
| Tavily | `Authorization: Bearer TAVILY_API_KEY`; optional `X-Project-ID: <id>` | `GET https://api.tavily.com/usage` | Same response: key/account limits, plan, pay-as-you-go, and endpoint breakdown | Single endpoint; no reset timestamp unless API adds one. **REV**: `X-Project-ID` header only when `project_id` is non-empty; must be part of cache scope fingerprint. |
| Firecrawl | `Authorization: Bearer FIRECRAWL_API_KEY` | `GET https://api.firecrawl.dev/v2/team/credit-usage` | `GET /v2/team/credit-usage/historical?byApiKey=false` | **REV**: historical query `byApiKey=false` is required — `true` would scope to one key and undercount team usage. |
| Requesty | `Authorization: Bearer REQUESTY_API_KEY` (management read) | `GET https://api-v2.requesty.ai/v1/manage/org` | `GET /v1/manage/org/usage?start=<RFC3339>&end=<RFC3339>&resolution=daily&groupBy=none` | **REV**: bounds are first-of-month 00:00:00Z to now, built from injected clock for hermetic tests. |
| ZenMux | `Authorization: Bearer ZENMUX_MANAGEMENT_API_KEY` (standard inference keys are invalid) | `GET https://zenmux.ai/api/v1/management/payg/balance` | `GET /api/v1/management/subscription/detail` | **REV**: either block may succeed alone; standard-key 401/403 must surface as "management key required" without leaking key material. |
| Vercel AI Gateway | `Authorization: Bearer AI_GATEWAY_API_KEY` (env-overrideable to OIDC token) | `GET https://ai-gateway.vercel.sh/v1/credits` | Opt-in `GET /v1/report?from=<RFC3339>&to=<RFC3339>&groupBy=day` | **REV**: report is paid/beta, restricted to eligible plans; `403` or "unsupported plan" must not suppress credits. Configurable env var name defaults to `AI_GATEWAY_API_KEY`. |

Official documentation is authoritative. Before implementing each slice, recheck its response schema and authentication notes against the linked API reference; tests must encode only documented fields used by the application.

## Data and Failure Model

1. Add one snapshot type and one `VendorSnapshot` variant per provider (5 new variants → 23 total).
2. Keep exact vendor fields instead of flattening them into a universal shape.
3. Require documented envelope and monetary/quota fields. Reject malformed, negative where impossible, non-finite, duplicate, unsupported-currency, or inconsistent values as schema errors (`AppError::Schema`).
4. Store a non-secret scope fingerprint in each cache payload, derived from the API key hash plus optional project/target context (Tavily `project_id`, Vercel env-var override identity). A changed key or project must force a refetch and must never reuse another scope's data. **REV**: fingerprint is a full-hex SHA-256 digest of the key plus normalized optional context — the same convention as `src/opencode_go/fetch.rs` — stored as `scope_fingerprint` alongside `fetched_at` (spec REQ-005).
5. For multi-endpoint providers, represent optional detail blocks explicitly (`Option<Detail>`). A snapshot is usable when its primary block succeeds; ZenMux is usable when either PAYG or subscription data succeeds.
6. Cache a partial live snapshot atomically (`tempfile + persist`, `flock`), then persist one sanitized secondary diagnostic through the existing `.last_error` sidecar. Keep `stale = false` when current primary data is live. **REV**: partial snapshots use a fixed 300-second horizon so a failing secondary is retried at most every 5 minutes (instead of every 60-second tick) while the cached primary block keeps displaying; full snapshots keep the 60-second `DEFAULT_TTL` and 7-day `MAX_STALE` fallback (spec REQ-016/AC-018).
7. Keep Vercel report data in an independently cached detail component (`usage_report.json` beside `usage.json` with its own `DEFAULT_TTL` override of 6h) so its six-hour TTL does not delay credit refreshes. A report-plan `403`, unsupported plan response, or transient failure must not suppress `/v1/credits` data.
8. Preserve the seven-day maximum stale fallback, atomic writes, asynchronous locks (`acquire_lock_async` on current-thread runtime), capped bodies (`MAX_BODY_BYTES` 2 MiB), same-origin redirects, authentication-body redaction (`401/403` → `AUTH_FAILURE_MESSAGE`), and widget exit-zero fallback behavior (`widget::run::fallback`).
9. **REV**: All money fields validate via `finite_amount`/`parse_amount`; integer aggregates use checked arithmetic (`checked_add`) and fail to schema error on overflow.

## Provider Presentation Contracts

### Tavily

- Snapshot: current plan; plan used/limit; pay-as-you-go used/limit; key used/optional limit; search, extract, crawl, map, and research usage.
- Headline/severity: plan percentage when the plan limit is positive; otherwise a truthful text headline with no fabricated percentage.
- Optional `project_id` scopes the request and cache fingerprint.
- No reset timestamp is shown unless the API starts returning one.
- **REV**: `vendor_short = tav`; the `{tav_*}` placeholder family is defined in spec §6.4.

### Firecrawl

- Snapshot: remaining credits, base plan credits, billing period start/end, and optional current-period total credits consumed from historical usage matched to the current billing period.
- Headline/severity: current-period used/base-plan percentage only when both values are available and denominator > 0. Preserve percentages above 100 in labels; clamp only the visual gauge.
- Show remaining credits separately because packs/coupons/recharge credits are not included in `planCredits`.
- Use billing period end as the absolute `reset_at` via `SectionBuilder::push_metric`.
- **REV**: `vendor_short = fcw`; historical row matching is exact billing-period overlap; unmatched history is absent, not arbitrarily selected.

### Requesty

- Snapshot: organization name, balance (USD), month-to-date spend (USD), request counts, input/output/total tokens, and queried interval `[start, end)`.
- Aggregate all returned daily entries with checked integer addition and finite monetary addition.
- Headline: balance. Detailed spend and token rows remain non-percentage text because no budget denominator is reported.
- **REV**: `vendor_short = rqy`; single `Tool`/`Section::Text` spend row plus `Block` for token totals; no fake `consumed_pct`.

### ZenMux

- Snapshot contains optional PAYG and subscription blocks.
- PAYG block: total, top-up, and bonus USD credits.
- Subscription block: tier, status, expiry, five-hour and seven-day windows, monthly maximums, flow counts, and USD values.
- Headline/severity: worst live five-hour/seven-day quota when subscription data exists; otherwise PAYG balance with balance-specific severity (thresholds 50/75/90).
- Treat `usage_percentage` as a fraction (0.0–1.0) and validate before conversion to integer percent (round, clamp 0–100 for display, preserve raw for label).
- Preserve account statuses without treating unknown future values as healthy — unknown → `Other(status_string)` with degraded severity.
- **REV**: `vendor_short = zmx`; snapshot `enum Status { Active, PastDue, Canceled, Other(String) }`.

### Vercel AI Gateway

- Snapshot: USD balance, lifetime total used, and optional month-to-date report totals for cost, token categories, and requests.
- Headline: balance; report values are non-percentage details.
- Config: `report_enabled = false` and `report_cache_ttl_seconds = 21600`; reject zero or unreasonable values (valid range 300–86400 inclusive) using existing `Config::validate` conventions.
- Env override: `api_key_env` (default `AI_GATEWAY_API_KEY`) may point to an OIDC token env var; fingerprint must include the resolved env-var name when overridden.
- UI and documentation must state that custom reporting is beta, restricted to eligible plans, and billed per query.
- **REV**: `vendor_short = vag`; report cache file `usage_report.json` with independent TTL; never confuse lifetime `total_used` with month-to-date report cost.

## Ordered Implementation Tasks

### 1. Shared Registration Surface

- Add the five IDs, stable slugs (`tavily`, `firecrawl`, `requesty`, `zenmux`, `vercel-ai-gateway`), display names (`Tavily`, `Firecrawl`, `Requesty`, `ZenMux`, `Vercel AI Gateway`), secret environment variables (`TAVILY_API_KEY`, `FIRECRAWL_API_KEY`, `REQUESTY_API_KEY`, `ZENMUX_MANAGEMENT_API_KEY`, `AI_GATEWAY_API_KEY`), command-line enum mappings (`VendorId` + `Vendor` in `src/vendor.rs` / `src/widget/cli.rs`), and active-vendor slug parsing in `src/active.rs` as each vertical slice lands. **REV**: also extend `VENDOR_SECRET_ENV_VARS` and `vendor_secret_env_vars_to_remove` in `src/vendor.rs`.
- Add opt-in configuration sections and defaults in `src/config.rs`; include inline-key permission detection (`has_inline_api_keys`, `protect_inline_api_keys`), validation (`validate`: duplicate labels, TTL bounds for Vercel, non-empty `api_key_env`), environment-key resolution (`resolve_api_key`), and `config.example.toml` coverage with `enabled = false` defaults.
- Register API-key providers in `src/tui/settings.rs` (`KEY_VENDORS` array) without exposing key values through settings JSON (stdin-only, `toml_edit` atomic write, `chmod 600`).
- Export each module from `src/lib.rs` and add its snapshot variant to `src/usage.rs` (`VendorSnapshot::Tavily(...)` etc.).

### 2. Tavily Vertical Slice

- Create `src/tavily/{mod,types,fetch,vendor}.rs`, following the single-endpoint cache flow (fresh-payload → flock → fetch → capped read → strict parse → atomic write).
- Implement defensive wire parsing (required fields, non-negative quotas, finite money), project-scoped cache attribution (fingerprint includes `X-Project-ID` when set), renderer placeholders (`tav_*`), severity (plan %), widget dispatch, TUI refresh, overview cells, and detailed sections via `SectionBuilder`.
- Add mocked response/cache/failure tests, configuration tests (missing key, invalid project_id), ignored live smoke test behind `#[ignore]` with `TAVILY_API_KEY`, JSON report assertions (ordered sections, no fake percentages), placeholder coverage, endpoint matrix (`docs/vendor-endpoints.md`), README usage, and changelog entry (Keep-a-Changelog `Added`).
- Complete all current desktop adapter mappings and tests before starting Firecrawl (GNOME marker, macOS `fixedFieldMapping`, KDE client-side selection, Omarchy `model.test.mjs`).

### 3. Firecrawl Vertical Slice

- Create `src/firecrawl/{mod,types,fetch,vendor}.rs`.
- Fetch current and historical endpoints concurrently where cache state allows (`tokio::join`); retain current credit data when historical usage fails (partial snapshot + `.last_error`).
- Validate billing timestamps (`start < end`, not in future), match the historical row to the current billing period (exact `period_start` equality), and keep unmatched history absent rather than selecting an arbitrary row.
- Implement cache round trips, partial-warning tests, reset metadata through `SectionBuilder::push_metric` (absolute `reset_at`), all interface mappings, documentation, and ignored live smoke coverage.

### 4. Requesty Vertical Slice

- Create `src/requesty/{mod,types,fetch,vendor}.rs`.
- Query organization and month-to-date usage concurrently; build RFC 3339 UTC bounds (`YYYY-MM-01T00:00:00Z` to now) from an injected clock (`Clock` trait / `now: DateTime<Utc>` param) for hermetic tests.
- Aggregate daily usage with checked integer addition and finite monetary addition; preserve live organization balance when usage fails or permission (`403`) is insufficient — surface sanitized warning.
- Add non-percentage report rows, all interface mappings, mocked permission and partial-failure cases, documentation, and ignored live smoke coverage.

### 5. ZenMux Vertical Slice

- Create `src/zenmux/{mod,types,fetch,vendor}.rs`.
- Fetch PAYG and subscription endpoints independently with the management key (`tokio::join`); require at least one valid block (`PAYG.is_some() || subscription.is_some()`), otherwise schema error.
- Convert documented quota fractions and timestamps into native `UsageWindow`s while retaining flow/value details and account status (`Other` for unknown).
- Test PAYG-only, subscription-only, both-success, both-fail, invalid standard key (401 with "management key required"), rate-limit `422`, unknown status, schema drift, cache attribution, all interfaces, documentation, and ignored live smoke coverage.

### 6. Vercel AI Gateway Vertical Slice

- Create `src/vercel_gateway/{mod,types,fetch,vendor}.rs` with slug `vercel-ai-gateway` and canonical display name `Vercel AI Gateway` (`vendor_short = vag`).
- Always refresh credits under normal cache rules (60s TTL). Query the paid report only when `report_enabled == true` and its independent six-hour cache (`usage_report.json`, TTL `report_cache_ttl_seconds`) is expired; do not block credits on report fetch.
- Aggregate daily report rows with checked arithmetic (cost as `f64` finite, tokens/requests as `u64` checked). Do not confuse lifetime `total_used` with month-to-date report cost.
- Config validation: `report_cache_ttl_seconds` ∈ [300, 86400], reject 0/negative/non-finite.
- Test report disabled (no `/v1/report` call), eligible report success, unsupported-plan partial warning (`403` with plan check), report rate/cost protection (capped TTL prevents tight loop), independent cache TTLs, OIDC env override (`AI_GATEWAY_API_KEY` vs custom), all interfaces, documentation, and ignored live smoke coverage.

### 7. Cross-Provider Contract Verification

- Update exhaustive matches in `src/tui/app.rs`, `src/tui/panels.rs`, and `src/widget/run.rs`; keep `src/report.rs` vendor-agnostic (no per-vendor metric-order table; build via `sections_for` projection).
- Ensure every provider emits generic placeholders (`{vendor_short}`, `{plan}`, `{session_pct}` etc.) plus vendor-specific ones (`{tav_*}`, `{fcw_*}`, `{rqy_*}`, `{zmx_*}`, `{vag_*}`). Balance-only providers must not expose fake zero-percent metrics.
- Update `docs/format-placeholders.md` (add `tav/fcw/rqy/zmx/vag` rows and short-code table), `docs/configuration.md`, `config.example.toml` (annotated with `enabled = false`), and `docs/vendor-endpoints.md` (stability notes). Update GNOME `marker-logic.js` + `marker-logic.test.mjs` classification, macOS `fixedFieldMapping` + tests, and any provider lists in adapter READMEs. KDE Plasma and Omarchy must continue selecting entries client-side from one `usage --json` call and must never add `--vendor` to KDE's fetch command (`kde-plasmoid/package/contents/code/plasmoid-logic.mjs`).
- Verify `VendorId::display_name()` remains the only canonical provider-name table (grep for duplicate tables, fail if found).
- **REV**: Run `make desktop-test` (GNOME + KDE + Omarchy) after each slice; do not batch at end.

### 8. MIT Source-Reuse Gate

- Before each adapter, inspect OpenUsage, ccusage, TokenTracker, and aiusage at a pinned commit (record commit SHA in `docs/third-party-notices.md` draft) for parsing, cache-scope, and test ideas. Current research found no confirmed first-class adapter for these five providers, so official API contracts remain the implementation source of truth.
- Prefer independent Rust implementation. If a substantial code portion is ported (≥15 lines or non-trivial parsing logic), record upstream repository, commit, source file/function, copyright, and MIT license in a new `docs/third-party-notices.md`; retain required license text and add focused provenance comments only where legally necessary.
- Do not import their databases, daemons, JavaScript runtimes, Go binaries, or command-line output contracts into production execution.

### 9. Validation and Release Readiness

- Run focused unit and mocked integration tests after every vertical slice (`cargo test --test live -- --ignored --nocapture` is NOT run without credentials).
- Run the repository baseline: `make test` (Rust + `omarchy/model.test.mjs` + GNOME/KDE marker tests).
- Run Continuous Integration parity:
  - `cargo fmt --all -- --check`
  - `cargo clippy --all-targets --locked -- -D warnings`
  - `cargo test --all-targets --locked`
  - `make desktop-test`
  - `cargo machete`
- Run `make qml-lint` and `make qml-test` when Qt 6 tools are available; keep Qt Modeling Language (QML) V4 compatibility (no optional catch bindings, no `\p{...}`).
- Run `./macos/run-tests.sh` on macOS. Record platform validation as pending if the implementation environment cannot execute it; do not claim success from Windows/Linux substitutes.
- Keep all live tests ignored and credential-gated (`#[ignore]` + env check). Never print keys, payloads containing account identifiers, or real configuration (`grep -v api_key` redaction).
- Verify `usage --json` ordering (canonical vendor order), non-percentage rows (balance-only), severity, absolute reset timestamps, partial warnings (`.last_error` sanitized), stale state, and additive compatibility using fixtures (insta snapshots).
- Update `README.md` (Authentication table, Vercel OIDC note), `docs/configuration.md`, `docs/format-placeholders.md`, `docs/vendor-endpoints.md`, `config.example.toml`, adapter READMEs, and `CHANGELOG.md` (Keep-a-Changelog `Added` + compare links) with generic placeholder data only (e.g., `jane.doe@example.com`, `Acme Corp`, `example.com`).
- Follow `CLAUDE.md` only when cutting a release; provider work alone must not opportunistically change version/package artifacts (`Cargo.toml` version, `manifest.json`, PKGBUILDs, `.SRCINFO`s, `CHANGELOG` compare links are release-only).
- **REV**: Verify Windows portability: `cargo build --release` on Windows produces `ai-usagebar.exe` + `ai-usagebar-tui.exe`; config path resolves via `directories::ProjectDirs` (`%APPDATA%` fallback), not hard-coded `~/.config`.

## Rollback Strategy

- Each provider defaults disabled and is independently removable (single `VendorId` + config section + snapshot variant).
- Keep each vertical slice in a separate logical commit or pull request so a failing provider can be reverted without touching earlier slices.
- If a detailed endpoint becomes unstable, disable only its detail query (`report_enabled = false` for Vercel, historical skip for Firecrawl) and retain the provider's primary balance/quota path; do not remove its stable ID from the additive JSON contract after release (additive only).
- If a wire schema drifts, fail closed to stale cache plus warning; never loosen required fields merely to make tests pass.

## Future Windows 11 Phase

- Preserve `ai-usagebar usage --json` as the sole data contract for a future notification-area application; it must not fetch providers or store credentials independently.
- Define a separate plan comparing Windows Presentation Foundation and Windows App SDK/WinUI 3 only after the five-provider release. Framework selection, installer, auto-start, signing, update channel, and accessibility are explicitly outside this implementation.
- Future acceptance criteria: per-user installation, no administrator rights, safe binary discovery (`where ai-usagebar` bounded), bounded subprocess execution, graceful fallback when the Rust binary is absent, theme/DPI support, keyboard accessibility, and parity with JSON entry ordering and warnings.

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
