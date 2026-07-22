# Changelog

All notable changes to **ai-usagebar** are recorded here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Each release is also published at
<https://github.com/akitaonrails/ai-usagebar/releases>.

## [Unreleased]

### Fixed

- Bordered tooltips no longer ragged-edge on rows containing an escaped
  character: `visible_width` counted `&amp;` as five glyphs instead of one, so
  every such row stopped short of the right border. Affects any vendor whose
  API-supplied labels contain `&`, `<` or `>`.

## [0.15.0] — 2026-07-22

### Added

- **Google Antigravity vendor.** Reports the four real quota windows — a 5-hour
  and a weekly limit for each of the two independent model pools (Gemini, and
  Claude & GPT OSS) — from `RetrieveUserQuotaSummary` on whichever Antigravity
  product is running locally. Antigravity 2.0, the Antigravity IDE and an
  interactive `agy` session all share one account-wide quota, so any of them
  serves it; the local server's port is assigned dynamically and is discovered
  rather than assumed. No credentials to configure: enable `[antigravity]` in
  `config.toml`. Percentages are *consumed*, matching every other vendor — the
  Antigravity UI shows the inverse (what remains).

  Quota is parsed strictly: a bucket without a finite `remainingFraction`, or a
  cached payload missing a required window, is an error that triggers a refetch
  rather than a confident "0% used". The cache fingerprints the signed-in
  account, so switching Google accounts cannot show the previous account's
  figures. A cached window whose reset has already passed is refused rather than
  served as current — with nothing running the cache would otherwise be offered
  for up to seven days, long past the five hours after which the session window
  is guaranteed to have rolled over. When a fetch fails with nothing usable cached, the original error is
  surfaced instead of a generic "no usable cache".

- The local Claude context monitor docks into the dashboard body instead of
  floating: `v` cycles `full` (its own screen) → `split` (beside the vendor
  panel) → `bottom`. `[context] layout` sets the one it opens with.

### Fixed

- **Credit spend is no longer hidden on plans without a spending cap** (#30).
  The usage endpoint sends `extra_usage.monthly_limit: null` for uncapped
  plans (e.g. Claude Pro); the whole block was discarded, hiding genuine
  `used_credits`. A null limit is semantic — "no cap" — not schema drift, so
  `ExtraUsage.limit` is now optional: the spend renders on every surface, the
  tooltip says `Limit: none reported` (stating the wire fact rather than
  inferring a plan tier), the TUI shows the amount without a denominator, and
  `{extra_limit}` expands to `—` (deliberately non-empty: GNOME and the macOS
  menu bar hide the whole extra row on an empty limit).
  The block is still dropped when `used_credits` itself is missing — without
  the spend there is nothing truthful to show — and no percentage is invented
  when there is no denominator.

- **Extra usage renders in its own currency.** The block's `currency` and
  `decimal_places` fields were ignored and every amount was formatted as `$`
  with a hard-coded cent scale — the #30 reporter's R$ 141.57 would have shown
  as "$141.57", a claim about the wrong currency. Known codes get their symbol
  (`R$`, `€`, `£`, `¥`), unknown ones render as `AMOUNT CODE`, and an explicit
  exponent is honored exactly, including zero- and three-decimal currencies.
  If a currency is present but its exponent is absent, the raw value renders as
  `N minor units CODE` rather than guessing and silently corrupting the amount;
  payloads with neither field keep the historical `$`/cents behaviour. Both
  new fields are gated at the parse boundary: `decimal_places` outside 0..=6 is
  schema drift (integral floats are tolerated, since this endpoint floats its
  numbers), and `currency` must be a three-letter ISO alpha code — the value is
  embedded in Pango markup and the desktop `;;` protocol, so an arbitrary
  string would be an injection vector besides being drift.

## [0.14.0] — 2026-07-20

### Added

- **Opt-in local Claude Code context monitor in the TUI.** Press `c` to list
  the 100 most recently modified top-level sessions from
  `~/.claude/projects`, then `Enter` for a detail gauge. The percentage uses
  Claude Code's input-only formula (fresh input + cache creation + cache
  reads); mixed 200K/1M histories can supply exact per-model window sizes, and
  an unknown denominator stays a raw token count instead of becoming a false
  percentage. The scanner runs off the async runtime, reads only bounded JSONL
  tails, skips subagent transcripts and symlinks, tolerates corrupt/unknown
  records, sanitizes display text, and invalidates a pre-compaction reading
  until the next assistant response. The feature is disabled by default and
  performs no filesystem scan until explicitly enabled.

- **Four account-balance vendors** that read remaining credit via each
  provider's API and render it as money, alongside the existing usage vendors:
  - **Kilo** — `GET api.kilo.ai/api/profile/balance` (USD; optional org id).
  - **Novita** — `GET api.novita.ai/openapi/v1/billing/balance/detail`
    (amounts are in 1/10000 USD).
  - **Moonshot** — `GET api.moonshot.ai|.cn/v1/users/me/balance`
    (USD on `.ai`, CNY on `.cn`).
  - **Grok (xAI)** — `GET management-api.x.ai/v1/billing/teams/{team}/prepaid/balance`
    via a **Management key** (distinct from the inference key); the team is
    auto-resolved from the key, and the inverted-ledger `total.val` (USD cents)
    is converted to dollars.

  All four are opt-in (disabled until a key is configured) and wired into the
  Waybar widget, the TUI panels, and the settings overlay.

  Money is parsed **strictly**: every documented monetary field is required, and
  a malformed or error-carrying 200 response is a schema error rather than a
  fresh "$0.00" snapshot. Moonshot's in-band `code`/`status` failure indicators
  are honored. Each cache records the target it was fetched for (Kilo
  organization, Moonshot region/currency, Grok team or key), so changing the
  target refetches instead of showing the previous account's figure. When a
  fetch fails with nothing usable cached, the original error is surfaced instead
  of a generic "no usable cache".

  For Grok, `scopeId` is only treated as a team id when the management key is
  **team-scoped**. An organization-scoped key reports an actionable error asking
  for `[grok] team_id` rather than querying a URL built from an organization id.

- **Anthropic (API) vendor** — month-to-date **spend** for the API/Console
  account, separate from the Claude Code OAuth account the existing `anthropic`
  vendor covers. Sums the current calendar month's daily buckets from
  `GET api.anthropic.com/v1/organizations/cost_report` (Admin API, paginated via
  `has_more`/`next_page`), converting the `amount` field from cents to dollars.
  Renders `$1.34 / $1000 · 0%` when `monthly_limit` is configured, `$1.34/mo`
  otherwise — the limit is a config value, since the API exposes neither it nor
  the remaining prepaid balance (Console dashboard only). Opt-in; requires a
  Console **Admin key** (`sk-ant-admin01-…`), which is only available to
  **organization** accounts.

  The cost API omits **Priority Tier** costs, so for an affected organization
  this figure is below its real total spend; the tooltip, TUI panel, README, and
  `config.example.toml` all say so rather than implying it is complete.

  Parsing is strict — the documented envelope fields are required, so a 200
  error envelope or a drifted shape is a schema error instead of a fabricated
  "$0.00 this month"; a genuine `data: []` is still a real zero. Incomplete
  pagination (`has_more` with no `next_page`, a repeated cursor, or exceeding
  the page cap) fails rather than caching a partial month. The cache records the
  UTC month it covers, so a rollover — including during an outage — refetches
  instead of showing last month as the current one. When a fetch fails with
  nothing usable cached, the original error is surfaced so the Admin-key
  guidance reaches the user.

  The cache also fingerprints the Admin key, preventing a key switch to another
  organization from reusing the previous organization's spend. Cost records
  must carry the documented `USD` currency before they are summed, configured
  limits must be positive and finite, response bodies are bounded, and fallback
  data older than seven days is refused.

### Changed

- **PRs are now gated on Linux — the platform the widget actually ships on.**
  Only Windows ran on pull requests; Linux was first exercised *after* a tag
  was pushed, by which point the tag is immutable and any failure costs a new
  patch release. The Linux job also runs `cargo fmt --check`, `cargo clippy
  --all-targets -- -D warnings` and `cargo machete`, none of which ran in CI at
  all. A macOS job runs the test suite and compiles the menu bar app, whose
  700+ lines of Swift nothing verified.

- **A release can no longer publish artifacts that disagree with its tag.** A
  new `verify-version` job — which every downstream job depends on — requires
  the tag to be an existing `vX.Y.Z`, and `Cargo.toml`, both PKGBUILDs, both
  `.SRCINFO`s and a `CHANGELOG.md` section to match it. This is not
  hypothetical: at the v0.13.0 tag both `.SRCINFO` files still declared
  `0.8.0`, and the release shipped anyway. `workflow_dispatch` also stops
  accepting an arbitrary commit — it must name a tag that exists — and
  `contents: write` is now scoped to the single job that publishes rather than
  granted to the whole workflow.

### Changed

- **A misspelled config *section* is now an error instead of being ignored.**
  `[openrouer]` used to parse cleanly, leave OpenRouter on its defaults, and
  give no hint that the section had been dropped. `Config` denies unknown
  top-level keys. This is deliberately section-level only: the set of sections
  is small and stable, whereas denying unknown keys inside every section would
  hard-fail configs carrying a field from a future or removed version.

### Fixed

- **Switching vendors no longer leaves the previous vendor's numbers on the
  desktop bars.** GNOME dropped any refresh requested while one was in flight,
  so a vendor change during a fetch never started one for the new vendor: the
  old vendor's result was applied and stayed until the next timer tick. The
  request is now queued and run when the current attempt settles, and a result
  is discarded if it has been superseded or if the selection changed while it
  ran. The macOS menu bar had no such protection at all — the timer, the
  Preferences window and a vendor change could each start a subprocess, and
  whichever finished last won. It now runs at most one at a time, tags each
  attempt with a generation, and ignores stale results. macOS also gains a
  45-second watchdog: the subprocess can block on the cache lock and then
  refresh OAuth, and without a bound a hung run left the panel frozen with no
  explanation.

- **The config file is found at one agreed location on every platform.** The
  binary resolved it through `directories::ProjectDirs` (macOS:
  `~/Library/Application Support/ai-usagebar/`), while the README, the shipped
  example, `--help`, the GNOME preferences and the macOS menu bar all used
  `~/.config/ai-usagebar/`. The two never had to be the same file, so the
  desktop integrations could report "no key configured" for a key the binary
  was using. The platform path stays canonical; the legacy Unix path is honored
  when the canonical file does not exist, and both desktop surfaces now check
  the same pair. Nothing is moved or rewritten — the file can hold API keys,
  and relocating a secret behind the user's back is not this tool's business.
  GNOME additionally honors `$XDG_CONFIG_HOME` instead of hard-coding
  `~/.config`.

- **`~` in configured paths is expanded.** `credentials_path = "~/..."` — the
  form the README documents — was kept literally by `PathBuf` and resolved to a
  directory named `~` relative to the working directory. Applies to
  `[anthropic] credentials_path`, `[openai] codex_auth_path` and every
  `[[anthropic.accounts]]` entry. `~user` is left untouched.

- **macOS: a locked Keychain is no longer reported as "not logged in".**
  `keychain::read_raw` mapped *every* `security(1)` failure to "no item",
  so a locked login Keychain, a denied ACL, or an operational error all
  produced the friendly "run `claude` to authenticate" message while the
  credentials sat there intact. Only `errSecItemNotFound` (44) now means
  absent; anything else surfaces with the exit code, `security`'s own stderr,
  and what to do about it — and it takes precedence over the file's error,
  since it is the more actionable one.

- **macOS: a refresh can no longer create a second, unreadable Keychain item.**
  With `$USER` unset the read selected by service alone while the write passed
  `-a ""`, so the two no longer addressed the same item. Both now use the same
  selection.

- **The TUI no longer freezes while a cache lock is contended.** `acquire_lock`
  parks the thread in a sleep loop for up to 15–45s, and the TUI runs on a
  current-thread runtime — so a lock held by a concurrent widget invocation
  stalled keyboard input, the refresh timer and every other vendor's request at
  once. Adds `Cache::acquire_lock_async`, which waits on the blocking pool, and
  routes every vendor through it.

- **The TUI no longer leaks a blocking task per event-loop iteration.** A fresh
  `spawn_blocking(event::poll)` was created on every `select!`; whenever another
  branch won, the previous one kept running, so several orphaned pollers raced
  on `event::read()` and could swallow keypresses. A single reader thread now
  feeds keys through a channel.

- **The terminal is restored even when the TUI exits through an error or a
  panic.** Raw mode, the alternate screen and the cursor are now owned by an
  RAII guard rather than undone by straight-line code after the event loop,
  which was skipped entirely on any early return.

- **A rotated OAuth refresh token is no longer lost silently.** Both Anthropic
  and OpenAI persisted refreshed credentials with `let _ = write_back(...)`.
  When the server rotates the refresh token and that write fails, the old token
  on disk is already spent: the current run works, and the *next* one cannot
  refresh, so the user appears to be logged out for no visible reason. A failed
  write-back after a rotation is now reported and treated as an auth failure.
  A failed write that only carried a new *access* token is still ignored —
  nothing is lost there, the next run simply refreshes again.

- **OpenAI no longer re-refreshes on every run after an id_token-less refresh.**
  Expiry was read exclusively from the `id_token` exp claim, and the explicit
  `expires_at` field in `auth.json` was ignored. A refresh response without a
  new `id_token` therefore left the old, expired claim in place. `expires_at`
  is now used as the fallback source and is written from the response's
  `expires_in`.

- **An invalid config is no longer silently replaced by the defaults.** Every
  caller used `Config::load().unwrap_or_default()`, so a TOML syntax error, a
  permission problem, or a failed validation produced the default vendor set
  with no diagnostic — the user saw the wrong tabs and credentials and had
  nothing to go on. The widget now reports it through the existing `⚠` fallback
  (still exiting 0, as Waybar requires), the TUI prints the path and the parse
  error *before* entering raw mode, in-session reloads keep the last good
  config instead of reverting to defaults, and `--cycle-next/--cycle-prev` does
  nothing rather than persisting a selection derived from the wrong vendor set.
  A missing file remains the legitimate "use defaults" case.

- **Cached data is no longer served forever after a failure.** `MAX_STALE`
  (7 days) was declared but never referenced, so every vendor's fallback path
  called `maybe_payload()` with no age limit: after weeks without network or
  credentials the bar kept showing historical numbers as if they were current,
  distinguished only by a `⏸` and an old timestamp. Failure paths now use the
  new `Cache::fallback_payload(MAX_STALE)` and surface the real error once the
  last good value ages out.

- **A corrupt or incompatible *fresh* cache no longer renders as a zeroed
  snapshot.** Anthropic, OpenAI, Z.AI, OpenRouter and DeepSeek turned an
  unparseable payload into "$0.00" / "0%" / "Unknown plan" and displayed it as
  current data; they now fall through to a live fetch, matching what Kimi
  already did. Cached monetary fields are required rather than
  `unwrap_or(0.0)`, so a truncated write is refetched instead of shown as an
  empty balance.

- **Z.AI no longer accepts an in-band failure as valid usage.** The API signals
  errors inside HTTP 200 (`success: false`, non-200 `code`, `data: null`).
  That body deserialized cleanly, was written to the cache — clearing the
  previously recorded error — and rendered as an unknown plan with empty
  windows, indistinguishable from an account with no usage. The envelope is now
  validated before anything is cached, so a failure keeps the last good payload
  and reports the error.
## [0.13.0] — 2026-07-17

### Added

- **Kimi vendor** (`--vendor kimi`): fetches weekly subscription quota and a
   5-hour rolling rate-limit window from `api.kimi.com/coding/v1/usages`.
   API key is read from `KIMI_API_KEY` env var or `[kimi] api_key` in config.
   Disabled by default (requires explicit opt-in).
- Kimi panel in the TUI and a Kimi API key field in the Settings overlay.
- Live API smoke test `kimi_live` for the Kimi endpoint.
- `{scoped_model}`, `{scoped_pct}`, `{scoped_reset}`, `{scoped_elapsed}` and
  `{scoped_bar}` placeholders — the primary model-scoped weekly window (the
  common case is one, e.g. **Fable**) exposed as flat fields. The tooltip
  already rendered every `snap.scoped` entry, but the desktop surfaces redraw
  from `--format` and had no way to read them.
- **The meta reference (pace marker) now renders on the macOS menu bar and GNOME
  desktop bars**, matching the Waybar tooltip. Each time-windowed bar draws a
  thin blue `│` at the elapsed-time position; the fill stays in the calm
  absolute-usage color up to the marker, and only the part that overshoots it —
  how far ahead of pace you are, i.e. the risk of spilling into **paid extra
  usage** if you keep the pace — is painted in the warning color. So a bar at
  41% used but only 20% into the week reads calm with a small red tail, not all
  red. macOS adds a *"Mostrar referência da meta"* toggle to switch it off;
  GNOME draws it whenever the window reports a reset.

### Fixed

- **Desktop pace markers no longer appear on windows without a reset.** A
  missing reset still displays the scoped model row with `—`, but suppresses
  the marker even when a legacy formatter supplies neutral elapsed `0`.

- **macOS menu bar and GNOME extension showed a stale "Sonnet only 0%" bar
  instead of the model-scoped weekly window (e.g. Fable).** Both redraw from
  `--format` and read `{sonnet_pct}` (the flat `seven_day_sonnet` field, now
  `null`); they now read the new `{scoped_*}` placeholders and label the row by
  the model's display name, falling back to the flat window + "Sonnet only".
- **macOS Preferences window clipped its top rows with no way to scroll to
  them** on short displays. The pane is now a `ScrollView` of `GroupBox`
  sections in a resizable window whose initial height is clamped to the visible
  screen (hosting-controller sizing disabled), so the content always scrolls
  and the top rows are reachable.
- **Configured `api_key_env` values are never echoed in credential errors.**
  Pasting an API key into `api_key_env` (which expects an env var *name* like
  `KIMI_API_KEY`) no longer prints that value in the widget's error tooltip.
  Invalid names are ignored for lookup so the section's inline `api_key` can
  still be used; when no inline key is present, the error explains the correct
  `api_key_env` usage without repeating its configured value.

## [0.12.0] — 2026-07-08

### Added

- **Model-scoped weekly limits (e.g. the Fable weekly cap)** now render in the
  widget tooltip, the TUI Claude tab, and the bar's severity class. Anthropic's
  usage endpoint reports these only inside the newer `limits[]` array
  (`kind == "weekly_scoped"`, labeled by `scope.model.display_name`) — there is
  no dedicated `seven_day_<model>` field — so they were previously invisible:
  a Fable week at 84%/warning showed nothing while the bar stayed green on a
  55% overall weekly. Labels come from the API, so future scoped models show
  up without a code change. Accounts without scoped limits are unchanged.

## [0.11.0] — 2026-07-06

### Added

- **Per-account tabs in the TUI** (#17, follow-up to #14). `ai-usagebar-tui`
  now shows the default Claude tab plus one tab per `[[anthropic.accounts]]`
  entry, each fetching with its own credentials file and `anthropic/<label>`
  cache (the same resolution the widget's `--account` uses, extracted into a
  shared `AnthropicConfig::account_target`). Anthropic-only; other vendors are
  still one tab each. With no extra accounts configured the tab set and order
  are unchanged.

## [0.10.0] — 2026-07-05

### Added

- **Config-driven multiple Anthropic accounts** (#14). Declare extra
  subscriptions once under `[[anthropic.accounts]]` (`label` +
  `credentials_path`) and select one on the CLI with `--account <label>`,
  instead of repeating `--creds-path`/`--cache-dir` on every widget module.
  Each named account gets an isolated cache at
  `~/.cache/ai-usagebar/anthropic/<label>/`. Anthropic-only; `--account`
  conflicts with `--creds-path`. Fully back-compatible: the singular
  `[anthropic] credentials_path` stays the default account, `--vendor
  anthropic` with no `--account` is byte-identical to before, and configs
  with zero or one account keep the unchanged `~/.cache/ai-usagebar/anthropic/`
  cache path (no migration). Per-account TUI tabs remain a follow-up.
  (thanks @zanlucathiago)

### Fixed

- **macOS: the Keychain fallback now also rescues a stale
  `~/.claude/.credentials.json`** (#15). Previously the Keychain was only
  consulted when the file was *missing*, so a leftover zeroed file (no
  access token, no refresh token, no expiry — e.g. from a pre-Keychain
  Claude Code install) shadowed valid Keychain credentials forever and
  Anthropic refresh failed with a stale cache. The default location now
  falls back to the Keychain when the file is missing **or** clearly
  unusable, and token refreshes are written back to whichever source was
  actually read. The predicate is deliberately narrow: a file with a live
  access token but empty refresh token (the trusted-device shape handled
  in v0.7.2) stays authoritative. Explicit paths (`--creds-path`, config
  `credentials_path`, and named accounts) are now read **strictly** — they
  never consult the Keychain, so a typo'd path fails loudly instead of
  silently showing a different account's usage. (thanks @igorsdm)

## [0.9.0] — 2026-07-04

### Changed

- **`--cache-dir` and `--creds-path` are now documented, supported flags**
  (previously hidden, "for tests / debugging"). Together they are the
  official way to track multiple accounts of the same vendor: one widget
  instance per account, each with its own credentials file and cache
  directory. `--creds-path` applies to the Anthropic vendor only. See the
  new "Multiple accounts (advanced)" section in the README. Behavior is
  unchanged — the flags parse and act exactly as before; they only became
  visible in `--help` and part of the stable CLI surface. First-class
  `[[accounts]]` config remains under discussion in #14.

## [0.8.0] — 2026-07-01

### Added

- **GNOME Shell extension** under `gnome-extension/` for showing the 5-hour,
  weekly, optional Sonnet-only, and optional extra-usage bars in the GNOME top
  panel. It shells out to the existing `ai-usagebar` binary, renders native `St`
  widgets, includes libadwaita preferences, and adds vendor credential helpers.
- **macOS menu bar app** under `macos/` for showing the same usage bars as a
  native `NSStatusItem` menu-bar agent with SwiftUI preferences, LaunchAgent
  install helper, vendor credential status, and login/config helper actions.

## [0.7.2] — 2026-06-24

### Fixed

- **Anthropic widget no longer shows a false `0%` on recent Claude Code (macOS).**
  Newer Claude Code builds rotate the OAuth access token via a host-side
  trusted-device flow and leave `refreshToken` **empty** in the shared
  credential blob (Keychain / `~/.claude/.credentials.json`). Once the access
  token expired, the widget POSTed that empty string as a `refresh_token` grant,
  the token endpoint answered `400 "Invalid request format"`, and the bar cached
  a zeroed snapshot — `0%` on session/weekly/sonnet with an `HTTP 400` tooltip.
  The fetch now skips the refresh when no refresh token is present, clears any
  stale token-endpoint error from older builds, and still attempts the usage
  request with the current access token before deciding whether to fall back to
  cache. The usage request was also trimmed to the four
  headers the live endpoint actually accepts — `Authorization`, `anthropic-beta`,
  a Claude Code `User-Agent` (without which the endpoint hard-rate-limits to
  `429`), and `Content-Type`.
- **`anthropic_live` smoke test no longer hard-fails on macOS Keychain-only
  setups.** It assumed `~/.claude/.credentials.json` always exists, but recent
  Claude Code keeps the blob in the login Keychain (no file). The test now falls
  back to the Keychain reader and skips cleanly when no credentials exist at all,
  matching the module doc's "won't fail on machines without creds" promise.

## [0.7.1] — 2026-06-08

### Changed

- TUI narrow layouts now render the vendor picker as a compact horizontal row
  above the detail panel instead of keeping the wide-layout vertical sidebar.

## [0.7.0] — 2026-06-08

### Changed

- **TUI adopts `ratatui-bubbletea` styling and components.** The native
  terminal app now uses a Bubble Tea-inspired dashboard layout with rounded
  blocks, a selectable vendor sidebar, themed help text, block-style progress
  bars, and loading spinners while preserving ai-usagebar's existing
  usage-severity colors. The MSRV is now Rust 1.88 to match the new dependency
  metadata.

## [0.6.0] — 2026-06-06

### Added

- **Native Windows support for the `ai-usagebar` and `ai-usagebar-tui`
  binaries.** Credential paths now resolve the home directory through
  `directories::BaseDirs` (a new shared `cache::home_dir()` helper) instead
  of reading `$HOME` directly, so Anthropic (`%USERPROFILE%\.claude\.credentials.json`)
  and OpenAI Codex (`%USERPROFILE%\.codex\auth.json`) credentials are found
  natively on Windows. The Waybar refresh (`pkill -RTMIN+13 waybar`) is gated
  to Unix and becomes a no-op elsewhere, since Waybar is Wayland-only. Linux
  and macOS behavior is unchanged. The Waybar widget itself remains
  Wayland-only; on Windows the TUI is the entry point. (thanks @EaeDave)

### Fixed

- **The widget now honors `[anthropic] credentials_path` from config.**
  `anthropic_output()` only consulted the `--creds-path` CLI flag before
  falling back to the default `~/.claude/.credentials.json`, silently
  ignoring a `credentials_path` set in `config.toml` — so the widget
  errored on the default path while the TUI (which already read the
  config value) worked. Resolution order is now `--creds-path` flag →
  config `credentials_path` → default, mirroring the existing OpenAI
  behavior. (thanks @mauricio-ms)
- **AUR source install no longer fails for users with a customized
  `active_vendor`.** The PKGBUILD's `check()` runs `cargo test --release`
  against the building user's real `$HOME`, so a planted
  `~/.cache/ai-usagebar/active_vendor` (e.g. set by widget scroll-cycle to
  any non-default vendor) flipped two unit tests via the documented
  vendor-precedence rule #2 and aborted the build. Tests now exercise
  vendor precedence + TUI theme resolution through hermetic seams
  (`Cli::resolve_vendor_with`, `active::cycle_at`/`read_from`/`write_to`,
  `App::with_theme`) and never read real `$HOME` / `$XDG` paths. Production
  behaviour is unchanged (thanks @sombraSoft).
- **TUI double-processed keystrokes on Windows Terminal.** Terminals that
  report key `Repeat`/`Release` events in addition to `Press` (Windows
  Terminal, and emulators advertising the Kitty keyboard protocol) made one
  Tab/arrow press move several tabs and holding a key fly through them. The
  TUI now acts only on `KeyEventKind::Press`. Harmless and beneficial on all
  platforms. (thanks @EaeDave)

## [0.5.1] — 2026-06-01

### Changed

- Documented optional Waybar CSS padding for `custom/aibar` when themes place
  it next to tray expander modules such as Omarchy's `group/tray-expander`
  / `custom/expand-icon`.

## [0.5.0] — 2026-05-30

### Added

- **DeepSeek vendor** (`--vendor deepseek`): fetches credit balance from
  `GET /user/balance`, preferring USD over CNY when both currencies are
  present. Severity thresholds are scaled per currency (CNY ≈ 7× USD).
  API key is read from `DEEPSEEK_API_KEY` env var or `[deepseek] api_key`
  in config. Disabled by default (requires explicit opt-in).
- DeepSeek API key field added to the TUI Settings overlay (`s` key),
  consistent with Z.AI and OpenRouter.
- **macOS Keychain fallback for Anthropic credentials.** Recent Claude
  Code builds on macOS store their OAuth state in the login Keychain
  (generic-password service `Claude Code-credentials`) instead of
  `~/.claude/.credentials.json`, so the widget failed with an I/O error
  on a missing file. When the file is absent on macOS, ai-usagebar now
  reads the same `{ claudeAiOauth, mcpOAuth }` JSON from the Keychain
  via `security(1)`, and writes refreshed tokens back to that same item
  so it keeps a single source of truth with Claude Code instead of
  forking a stale copy. Linux behavior is unchanged.

## [0.4.5] — 2026-05-28

### Fixed

- **AUR-bin CI publish was pushing empty commits.** The
  `KSXGitHub/github-actions-deploy-aur` action copies the file at
  `pkgbuild:` into the AUR repo verbatim — preserving the source
  filename. The AUR-bin remote only tracks a file literally named
  `PKGBUILD`, so passing `./packaging/aur/PKGBUILD-bin` landed the
  bumped file alongside (untracked) while leaving the stale `PKGBUILD`
  intact. Result: `makepkg --printsrcinfo` ran against the old file,
  generated an identical `.SRCINFO`, and the action committed an
  empty bump (`allow_empty_commits: true` by default). v0.4.4's bin
  push went through (`055b104..6bc8a68`) but never advanced the
  version — AUR-bin stayed at 0.4.3 even though all four other
  channels (GitHub Release, crates.io, source AUR, source tag) shipped
  0.4.4. Fix: stage the `-bin` variant under a literal `PKGBUILD`
  filename before handing it to the action.

## [0.4.4] — 2026-05-28

### Changed

- **CI now publishes both AUR packages automatically** on every
  `v*` tag push. New `publish-aur` job in `.github/workflows/release.yml`
  computes the real sha256s (source tarball + both arch binaries),
  injects them into `packaging/aur/PKGBUILD{,bin}`, and pushes via
  the `KSXGitHub/github-actions-deploy-aur@v2.7.2` action — which
  spins up an Arch container to regenerate `.SRCINFO`s, then commits
  + pushes to the two AUR git repos. Skips gracefully when the
  `AUR_SSH_KEY` secret isn't set, leaving the manual flow (described
  in `CLAUDE.md`) as a fallback.
- **Release loop is now one tag push end-to-end.** A `git push origin
  vX.Y.Z` now builds binaries (x86_64 + aarch64) once, uploads them
  to the GitHub Release, runs `cargo publish`, and updates both AUR
  packages — all without leaving the laptop or touching any AUR
  clone. Whole cycle takes ~5 minutes.

## [0.4.3] — 2026-05-28

### Added

- **Published to crates.io** — `cargo install ai-usagebar` works on
  any Linux/macOS box with rustup, no Arch / AUR required. Both
  binaries (`ai-usagebar`, `ai-usagebar-tui`) land in `~/.cargo/bin`.
- **`cargo binstall ai-usagebar` support** — if you have
  [cargo-binstall](https://github.com/cargo-bins/cargo-binstall), it
  fetches the prebuilt binary from the matching GitHub Release
  (x86_64 or aarch64 Linux) instead of compiling. Same artifact the
  `ai-usagebar-bin` AUR package uses, just without yay. Metadata in
  `[package.metadata.binstall]`.

### Changed

- **Cargo.toml metadata** filled in: `repository`, `homepage`,
  `documentation`, `keywords`, `categories`, `readme` — so the
  crates.io listing has a proper sidebar.
- **`exclude`** added to `[package]` so screenshots (~6 MiB) and
  AUR packaging files aren't shipped in the published crate
  tarball. Crate size went from 6.6 MiB compressed to 118 KiB.
- **CI**: new `publish-crates-io` job in `.github/workflows/release.yml`
  runs `cargo publish` after the binary build + GitHub release
  succeed. Skips gracefully when `CARGO_REGISTRY_TOKEN` isn't set
  or when the version is already on crates.io (idempotent for
  workflow-dispatch re-runs).

## [0.4.2] — 2026-05-28

### Changed

- **TUI panel header** — the "Updated HH:MM:SS" timestamp is now
  right-aligned on the title row (next to the plan label like
  `Claude Max 20x`) instead of taking its own body row at the
  bottom of the panel. Tighter rhythm and one less line of body
  content. Also dropped the duplicate `· updated …` suffix from
  the global footer — was cropped on 875x600 windows anyway.
- **Release notes** — `release.yml` now extracts the matching
  `CHANGELOG.md` section into the GitHub Release body and appends
  a `Full diff` compare link against the previous tag (thanks
  @sombraSoft, PR #3). Replaces the prior install-and-checksums
  body. Merged with two small regex hardenings so version dots
  (`v0.4.1`) aren't treated as wildcards.

### Fixed

- **Drifting "Updated" timestamp** — the previous panel-body
  timestamp recomputed `now - cache_age` on every redraw, so the
  displayed clock ticked upward continuously instead of holding
  at the actual cache-write moment. Snapshot the absolute
  `fetched_at` instant once when the tab is built and format
  from that; redraws no longer affect it.

## [0.4.1] — 2026-05-26

### Changed

- Centralized local-time formatting for cache update timestamps across the
  widget, vendor tooltips, and TUI.

### Fixed

- Fixed user-facing `Updated` timestamps to display in the local timezone
  instead of UTC.
- Kept timestamp snapshot tests deterministic across machines with different
  local timezones.

## [0.4.0] — 2026-05-24

### Added

- Added unit coverage for TUI primary-vendor reselection after settings changes.

### Changed

- Ran a code-quality pass across the Rust codebase, removing stale abstractions
  and tightening formatting while preserving existing behavior.
- Centralized shared Waybar refresh and HTTP client timeout constants so the
  widget and TUI do not carry duplicated hardcoded values.
- Simplified repeated widget setup for cache directories and theme overrides.

### Fixed

- Fixed the TUI Settings save path so saved API keys and primary-vendor changes
  are reloaded immediately before refreshing tabs.

### Security

- Removed the unused `async-trait` dependency from the direct dependency tree.

## [0.3.3] — 2026-05-24

### Added

- **aarch64 (ARM64) Linux binaries** in GitHub Releases. CI now builds
  both `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`
  tarballs via [`cross`](https://github.com/cross-rs/cross). Tests
  still run only on the native x86_64 target (aarch64 binaries can't
  execute on the x86 runner).
- **`ai-usagebar-bin` PKGBUILD now multi-arch** with per-arch
  `source_x86_64=` / `source_aarch64=` declarations. Arch users on
  Asahi / RPi5 / Ampere / etc. can install the prebuilt binary the
  same way as x86_64 users: `yay -S ai-usagebar-bin`.

## [0.3.2] — 2026-05-23

### Added

- **Auto-signal waybar from Settings save** —
  `settings::save_to_config_default()` now fires `SIGRTMIN+13` to any
  running `waybar` process after a successful save, so widget modules
  configured with `signal: 13` refresh their bar text immediately
  instead of waiting up to 300 s for the next interval tick. No-op
  when waybar isn't running.

### Changed

- **CI**: bumped `actions/checkout`, `actions/upload-artifact`, and
  `actions/download-artifact` from v4 → v5 to silence Node 20
  deprecation warnings ahead of GitHub forcing Node 24 in June 2026.
- **README**: dropped the "future release" caveat on the manual
  `pkill -SIGRTMIN+13 waybar` workaround (it's now automatic).
- **README**: clarified that `ai-usagebar-tui` is a fully standalone
  TUI requiring no Waybar / Hyprland / compositor dependencies — use
  it from any terminal, including plain SSH sessions.
- **README**: corrected the Hyprland floating-window snippet to use
  the current `windowrule = …, match:class …` syntax (Hyprland 0.46+),
  not the deprecated `windowrulev2`.

## [0.3.1] — 2026-05-23

### Added

- **Cross-vendor placeholder aliases** —
  every vendor's `build_placeholders()` now exposes `{session_pct}`,
  `{session_reset}`, `{weekly_pct}`, `{weekly_reset}`, `{plan}` as
  aliases to its primary metric. A single format string like
  `'{vendor_short} {session_pct}% · {session_reset}'` now renders
  correctly across all four vendors during scroll-cycle, instead of
  showing literal `{session_pct}` text for OpenAI / Z.AI / OpenRouter
  (which previously only exposed `{oai_session_*}` / `{zai_session_*}`
  / `{or_*}` namespaced names).
- For OpenRouter (no session/reset concept) the alias maps
  `session_pct` → consumed-credit % and `session_reset` → `—`.

### Fixed

- **AUR `-debug` collision** —
  `ai-usagebar-bin` now sets `options=('!strip' '!debug')`, suppressing
  the auto-generated debug-info split. Without it the `-bin` variant's
  auto-debug pkg fought over `/usr/lib/debug/usr/bin/ai-usagebar*.debug`
  with an existing source-variant `ai-usagebar-debug`, preventing
  swapping from source to bin without first manually removing the
  orphan. The source PKGBUILD also adds `'!debug'` for symmetry, and
  both PKGBUILDs now declare the cross-variant `conflicts` so pacman
  auto-removes whichever is being replaced.

## [0.3.0] — 2026-05-23

### Added

- **TUI Settings overlay** — press `s` from any tab to open a modal
  that lets you pick the primary vendor (radio: anthropic / openai
  / zai / openrouter) and set inline `ZAI_API_KEY` /
  `OPENROUTER_API_KEY`. Keys are masked as you type; `Ctrl-V`
  toggles reveal. `Ctrl-S` saves, `Esc` cancels.
- **`toml_edit`-based config writes** preserve existing comments and
  unrelated fields when the Settings overlay saves. The file is
  automatically `chmod 600`ed so inline keys aren't world-readable.

### Changed

- **Panel layout**: panels now harmonize vertical space — added
  spacer rows between OpenRouter / Z.AI sections so they don't clump
  at the top, and the "Updated …" footer is pinned to the bottom of
  every panel regardless of content height.

## [0.2.0] — 2026-05-23

### Added

- **Config-driven primary vendor**: new `[ui] primary` field in
  `config.toml` selects which vendor the widget shows when
  `--vendor` is omitted and which TUI tab opens first.
- **Inline API keys in config**: `zai.api_key` / `openrouter.api_key`
  accept inline values for users who don't source secrets in their
  shell. Resolution order: `api_key_env` → `api_key` → error with a
  clear message naming both fallbacks.
- **Scroll-to-cycle on the bar**: new `--cycle-next` / `--cycle-prev`
  flags persist the active vendor to `~/.cache/ai-usagebar/active_vendor`
  and signal waybar (`SIGRTMIN+13`) to refresh instantly. Wire to
  `on-scroll-up` / `on-scroll-down` for a single bar item that cycles
  through enabled vendors.
- **`{vendor_short}` placeholder**: always expands to `cld` / `gpt`
  / `zai` / `opr` so the bar can label which vendor is currently
  shown when scroll-cycling.
- **Native ratatui panels in the TUI**: replaced the
  Pango-string-to-ratatui shim with native widgets (`Gauge`,
  `Block`, `Paragraph`). Progress bars scale to the terminal width,
  and all four vendor panels share a consistent layout.

### Changed

- **Widget `--vendor` is now optional** — defaults to `[ui] primary`
  in config, falling back to `anthropic` only when nothing is set.
- **Extracted duplicated tooltip helpers** (`Line`, `render_bordered`,
  `pad_*`) from 4 vendor files into a shared `src/tooltip.rs`
  (~70 LOC saved).

### Fixed

- **Live tests against real APIs continue to pass** — Z.AI's
  undocumented `{type:"TIME_LIMIT"}` block parses correctly now
  that we tolerate float `0.0` where integer was expected.

### Security

- New "Authentication" section in README documents the credential
  resolution order and includes a `chmod 600` recommendation for
  config files containing inline keys.

## [0.1.0] — 2026-05-23

Initial release. Drop-in replacement for
[`claudebar`](https://github.com/mryll/claudebar) extended to four
vendors. Highlights:

- Per-vendor Waybar widget producing the same JSON shape as claudebar.
- Tabbed TUI (`ai-usagebar-tui`) with one tab per enabled vendor.
- Vendors supported:
  - **Anthropic**: OAuth via `~/.claude/.credentials.json`,
    `GET api.anthropic.com/api/oauth/usage`.
  - **OpenAI**: OAuth via `~/.codex/auth.json`,
    `GET chatgpt.com/backend-api/wham/usage` (same undocumented
    endpoint the official Codex CLI uses).
  - **Z.AI**: API key via `ZAI_API_KEY`,
    `GET api.z.ai/api/monitor/usage/quota/limit`
    (note: header `Authorization: <key>` with **no** `Bearer` prefix).
  - **OpenRouter**: API key via `OPENROUTER_API_KEY`,
    `GET openrouter.ai/api/v1/{credits,key}`.
- Drop-in claudebar compatibility — same CLI flags
  (`--icon`, `--format`, `--tooltip-format`, `--pace-tolerance`,
  `--format-pace-color`, `--tooltip-pace-pts`, `--color-*`) and the
  same `{placeholders}`.
- Always exits 0 (Waybar hides modules that don't).
- Atomic cache writes + `flock`-protected OAuth refresh — multi-monitor
  Waybar instances coexist without API stampedes.
- Live API smoke test suite (`make smoke`) that exercises the real
  undocumented endpoints to detect schema drift before users do.

[Unreleased]: https://github.com/akitaonrails/ai-usagebar/compare/v0.15.0...HEAD
[0.15.0]: https://github.com/akitaonrails/ai-usagebar/compare/v0.14.0...v0.15.0
[0.14.0]: https://github.com/akitaonrails/ai-usagebar/compare/v0.13.0...v0.14.0
[0.13.0]: https://github.com/akitaonrails/ai-usagebar/compare/v0.12.0...v0.13.0
[0.12.0]: https://github.com/akitaonrails/ai-usagebar/releases/tag/v0.12.0
[0.11.0]: https://github.com/akitaonrails/ai-usagebar/releases/tag/v0.11.0
[0.10.0]: https://github.com/akitaonrails/ai-usagebar/releases/tag/v0.10.0
[0.9.0]: https://github.com/akitaonrails/ai-usagebar/releases/tag/v0.9.0
[0.8.0]: https://github.com/akitaonrails/ai-usagebar/releases/tag/v0.8.0
[0.7.2]: https://github.com/akitaonrails/ai-usagebar/releases/tag/v0.7.2
[0.7.1]: https://github.com/akitaonrails/ai-usagebar/releases/tag/v0.7.1
[0.7.0]: https://github.com/akitaonrails/ai-usagebar/releases/tag/v0.7.0
[0.6.0]: https://github.com/akitaonrails/ai-usagebar/releases/tag/v0.6.0
[0.5.1]: https://github.com/akitaonrails/ai-usagebar/releases/tag/v0.5.1
[0.5.0]: https://github.com/akitaonrails/ai-usagebar/releases/tag/v0.5.0
[0.4.5]: https://github.com/akitaonrails/ai-usagebar/releases/tag/v0.4.5
[0.4.4]: https://github.com/akitaonrails/ai-usagebar/releases/tag/v0.4.4
[0.4.3]: https://github.com/akitaonrails/ai-usagebar/releases/tag/v0.4.3
[0.4.2]: https://github.com/akitaonrails/ai-usagebar/releases/tag/v0.4.2
[0.4.1]: https://github.com/akitaonrails/ai-usagebar/releases/tag/v0.4.1
[0.4.0]: https://github.com/akitaonrails/ai-usagebar/releases/tag/v0.4.0
[0.3.3]: https://github.com/akitaonrails/ai-usagebar/releases/tag/v0.3.3
[0.3.2]: https://github.com/akitaonrails/ai-usagebar/releases/tag/v0.3.2
[0.3.1]: https://github.com/akitaonrails/ai-usagebar/releases/tag/v0.3.1
[0.3.0]: https://github.com/akitaonrails/ai-usagebar/releases/tag/v0.3.0
[0.2.0]: https://github.com/akitaonrails/ai-usagebar/releases/tag/v0.2.0
[0.1.0]: https://github.com/akitaonrails/ai-usagebar/releases/tag/v0.1.0
