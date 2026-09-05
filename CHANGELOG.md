# Changelog

All notable changes to **ai-usagebar** are recorded here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Each release is also published at
<https://github.com/akitaonrails/ai-usagebar/releases>.

## [Unreleased]

### Added

- **Banked reset credits for Codex and SuperGrok.** Both providers let you earn
  quota resets and redeem them by hand, on their own expiry clock — information
  that existed nowhere in ai-usagebar, because it is not the window rollover
  the `*_reset` placeholders already showed. The count and the next expiry now
  appear in the Waybar tooltip, the TUI panel, and `ai-usagebar usage --json`
  (and so in the Omarchy, GNOME and KDE frontends, which read that report) as
  a list, one row per credit, with its own title and expiry — two Codex
  "Full reset (Weekly + 5 hr)" credits that lapse hours apart on the same day
  no longer collapse into a single "next expires" line. New placeholders are
  `{oai_resets_available}` / `{oai_resets}` and `{sgk_resets_available}` /
  `{sgk_resets}` (the compact count). A provider with none reports nothing
  rather than a standing `0`.

  Codex's count rides its usage response; the expiries come from
  `wham/rate-limit-reset-credits`, called only when something is banked.
  SuperGrok's come from `grok.com`'s `ConsumerUiSvc/GetRemainingResets`, a
  gRPC-Web call authenticated with the Grok Build login's own key, parsed by a
  bounded hand-written protobuf reader rather than a new dependency.

  Read-only: **ai-usagebar never redeems a reset.** The redemption identifier
  each provider returns beside the expiry is skipped during parsing rather than
  parsed and dropped, so it reaches neither the cache nor the screen. Both
  extra calls fail quietly — a broken one costs the expiry date and leaves
  every quota figure beside it untouched.

### Changed

- SuperGrok's panel no longer repeats the current period's rollover as a
  standalone "Resets" row. That countdown already sits under the weekly
  credits bar, the same way Codex 5h and Codex weekly do.

- `README.md` documents the macOS Keychain prompt storm as a known issue, with
  the workaround, until #148 is fixed. Every release so far is affected on
  macOS: ai-usagebar's token write-back claims the `Claude Code-credentials`
  item for its own code signature, and Claude Code's own reads then raise a
  permission dialog per process.
- `CONTRIBUTING.md` and a PR template record the pre-PR gate, the checklist
  review kept asking for, and the bar a new provider has to clear.

- `docs/vendor-endpoints.md` records which providers have been evaluated and not
  added, and the credential bar a new one has to clear (#146, #147). Xiaomi MiMo
  is blocked on Xiaomi: its quota routes need a web SSO session, not the plan's
  API key. Alibaba Cloud Model Studio's Token Plan is wanted and fits the
  existing patterns, pending the evidence to build it against.

## [1.10.0] — 2026-09-02

### Added

- **GitHub Copilot** is supported as a vendor, selectable with
  `--vendor copilot` and enabled with `[copilot]` in config. It shows the
  premium-request, Chat and Completions quotas with their reset, from the
  `api.github.com/copilot_internal/user` endpoint the VS Code extension uses.
  There is no key to paste: the OAuth token comes from `gh auth token`, so an
  existing `gh auth login` is the whole setup, and `GITHUB_COPILOT_TOKEN`
  overrides it. `gh` is looked up on `PATH` — it has no canonical install
  location — so `[copilot] gh_binary` pins the executable when that matters.
  The token is only ever read: it is used in one outgoing `Authorization`
  header and never copied, cached, logged, or echoed in an error.

### Fixed

- Kimi leads with its rolling 5h window and carries the weekly quota under it,
  the order every other two-window vendor uses — Claude's `Session (5h)` above
  `Weekly (7d)`, Codex's `Codex 5h` above `Codex weekly`, GLM's `Session (5h)`
  above `Weekly`. It was the one provider drawing the long window first, so a
  glance across vendors compared different rows. The Waybar tooltip and the
  shared panel projection both move, which carries the Omarchy Quattro panel,
  the KDE plasmoid, the TUI detail panel, the TUI Overview row (`5h` before
  `wk`) and `usage --json` with them. Kimi's Waybar bar text, the macOS menu
  bar and the GNOME dropdown already read the 5h window off the `session_*`
  alias and are unchanged.

- Kimi's quota rows drop their `55 / 100 · 45 left` counters and show the
  plain `Resets in 3d 20h` every other window row shows. Kimi was the only
  vendor spelling a ratio out beside the bar that already draws it, and the
  raw figures stay available through the `{kimi_*_used}`, `{kimi_*_limit}`
  and `{kimi_*_remaining}` placeholders for anyone who wants them in a
  custom format — the bar and its `55%` directly above already said it, three
  times over. Kimi's rows now go through the shared `push_window` instead of
  a hand-rolled near-copy of it, so the panel and the Waybar tooltip cannot
  drift apart again. The tooltip is 16 columns narrower for it. The shared
  `WindowRow::with_detail` hook the old rows used goes with them — Kimi was
  its only caller.

- A credential typed into the Omarchy settings panel is now scrubbed from the
  panel's memory even when the save *fails*. It was only cleared on the success
  path, so a failed save left the pasted value in a long-lived QML shell. This
  affects every key-authenticated vendor, not just the new one.

- Antigravity renders whatever windows the running product reports instead of
  failing when a 5-hour bucket is absent (#139). Antigravity CLI 1.1.22 returns
  weekly buckets only, so requiring a Gemini 5h window threw away two perfectly
  good ones and failed the whole vendor with "quota summary has no Gemini 5h
  bucket". All four windows are optional now — as Z.AI's already were — and a
  snapshot needs one recognised bucket rather than two specific ones. A cadence
  with nothing under it no longer draws an empty heading, and the placeholders
  for a window that did not arrive are empty rather than claiming a figure.
  Rejecting a summary with nothing recognisable in it is unchanged, and it
  still names the buckets it did receive. `docs/vendor-endpoints.md` gains the
  Antigravity row it never had — it was the only provider missing from that
  table — and the placeholder reference no longer promises all four windows.

## [1.9.1] — 2026-08-30

### Fixed

- Antigravity's "quota summary has no Gemini 5h bucket" error now names the
  buckets the summary *did* contain, with their windows and groups (#139). The
  old message said only what was wanted, so a plan with no such pool, a renamed
  bucket, and a new cadence were indistinguishable from each other — to the user
  and to a maintainer reading the report. A summary carrying no buckets at all
  says that instead of listing nothing. The parser is unchanged and still
  refuses to invent a window it cannot find.

- Named Codex accounts (`[[openai.accounts]]`, added in 1.8.0) are now actually
  reachable: `--vendor openai --account <label>` was rejected by CLI validation
  before it could dispatch, `~` in an account's `codex_auth_path` was never
  expanded, and the TUI and `usage` report skipped openai named accounts
  entirely. All three paths now mirror the OpenRouter account handling — one
  tab/report entry per named account, each with its own isolated cache — and
  openai account labels are validated and de-duplicated on config load like the
  other multi-account vendors.
- SuperGrok works again with grok CLI 1.0.13, which dropped the `x.ai/billing`
  ACP extension the vendor was built on (`-32601 Method not found`, probed
  directly, after a session handshake, and in leader mode). The vendor now
  calls the CLI's documented `cli-chat-proxy.grok.com/v1/billing` endpoint
  first, using the long-lived `key` already stored in the login's `auth.json`
  — read-only, size-bounded, used only inside one outgoing `Authorization`
  header, never copied, cached, logged, or echoed in an error — and falls back
  to the ACP process for CLI builds where the endpoint is unavailable. When
  both transports fail, the direct error is reported because it reflects the
  actual login state. Response parsing is unchanged: the proxy returns the
  same camelCase `BillingConfig` shape the strict wire types already accept.
  The endpoint is fixed: `GROK_CLI_CHAT_PROXY_BASE_URL` still scopes the
  cache (it changes which login is in play) but does not choose where the
  key is sent. The README frontend table, `docs/configuration.md`,
  `docs/vendor-endpoints.md` and `docs/format-placeholders.md` are updated to
  match: they described SuperGrok as ACP-only and stated that the login files
  were never parsed, which stopped being true with this change.

### Changed

- Internal: `account.rs` grew from 7 tests to 22 by splitting its decisions
  from its prompting and printing (#137) — the deletion-conflict authorization,
  the keep-list parser, and the status and switch-plan renderers are now pure
  functions with coverage. No behaviour changes; the extracted code is the code
  that was there. Regions covered went 22% → 50%, whole-tree 83.9% → 85.1%.
- Internal: the four-field record every vendor fetch returns, and the policy
  around it, is now `outcome::Outcome<T>` instead of eighteen private copies
  (#136). No behaviour changes — the copies had already been reconciled in
  1.9.0 — but the reconciliation is now structural rather than repeated, and a
  guard test forbids a second reader of the stale payload. Net −508 lines.

## [1.9.0] — 2026-08-28

### Added

- **Command Code** (`commandcode.ai`) is supported as a vendor, selectable with
  `--vendor commandcode` and enabled with `[commandcode]` in config. It shows
  the 5-hour and weekly rolling spend windows — priced in dollars, as Command
  Code meters spend rather than tokens — plus the plan and the monthly credit
  remaining. Like Cursor and Kiro CLI it needs no key of its own: it reuses the
  OAuth credential from either the official CLI or pi
  (`~/.commandcode/auth.json`, then `~/.pi/agent/auth.json`), with
  `COMMANDCODE_API_KEY` as an override. The credential is only ever read —
  refreshing it belongs to the CLI that owns the file — so an expired token is
  reported as expired rather than silently rewritten.

### Changed

- EUR, GBP, BRL and JPY amounts render with their symbol everywhere. The two
  money formatters — one for decimal amounts, one for integer minor units —
  each carried their own currency table, and they disagreed: the same euro
  figure read `3.50 EUR` in one panel and `€3.50` in another. Both now share a
  single table. USD and CNY are unchanged, and a currency with no symbol still
  trails its code rather than guessing one.

### Fixed

- A vendor whose cache is cold now reports **what actually failed** instead of
  a generic "no usable cache". Claude, Codex, Z.AI, OpenRouter and DeepSeek
  replaced the original error with that message when there was no cached
  figure to fall back on, so on a first run an expired key, a `500` and a
  genuinely empty cache all rendered identically — the useful diagnostic was
  written to disk and shown only on the *next* refresh. The other thirteen
  vendors already returned the original error; a guard test keeps the two
  groups from diverging again.
- Z.AI's rows in the native panels — Omarchy Quattro, GNOME, KDE and the TUI —
  carry the pace footnote every other percentage vendor's rows carry
  (`60% elapsed · 20pts under`) instead of a bare `Resets in 2h 00m`. GLM's
  session, weekly and monthly MCP windows each report a duration and a reset,
  so all three pace, off the same `pacing::calc` the `{zai_*_pace}`
  placeholders already use. Each surface keeps its own way of showing it, so
  nothing else moves: the arrow stays the widget's, the bar tick the macOS
  menu bar's, the footnote the panels'.

## [1.8.0] — 2026-08-28

### Added

- Multiple OpenAI (Codex) logins, via `[[openai.accounts]]` (#134). Each entry
  is a label plus its own `codex_auth_path`, the same shape
  `[[anthropic.accounts]]` uses and for the same reason: Codex is an OAuth
  vendor, so an account is a credential file and refreshes write back into
  whichever one they came from. Named accounts are selected with
  `--account <label>` and cached separately under
  `~/.cache/ai-usagebar/openai/<label>`, so two subscriptions can never serve
  each other's usage. A config without the array behaves exactly as before.

- The Omarchy Quattro plugin can show which provider the bar entry is about.
  **Show provider name in the top bar** — a new opt-in toggle beside the
  existing usage-value one, or `omarchy bar set akitaonrails.ai-usagebar
  showProvider true --json` — prefixes the label with the provider's
  three-letter code, the same one Waybar's `{vendor_short}` prints, so the
  entry reads `cld 29%` or `gpt 95%` after the icon. It matters most on a bar
  that cycles several providers, where the percentage alone never said whose
  it was. Off by default, so an existing bar entry keeps the label it has
  today; with the usage value turned off the entry keeps the icon and the
  code alone, and a vertical bar still shows the icon alone.
- `ai-usagebar usage --json` reports each entry's `short_name`. It is the field
  the toggle above draws, and it is additive like the rest of the report.

### Changed

- `{vendor_short}` is now `VendorId::short_name` for every provider instead of
  a literal repeated in eighteen renderers, so the report, the placeholder and
  the native panels cannot drift apart. The codes themselves are unchanged, and
  a test now rejects a duplicate or a non-three-letter one.
- The `{vendor_short}` reference table lists Nous Research (`nrs`) and OpenCode
  Go (`ocg`), which both shipped codes without ever being written down.
- Kimi accepts a **Kimi For Coding subscription** as a credential: when no
  `KIMI_API_KEY` (or inline `api_key`) is set, the vendor uses the OAuth
  session the Kimi Code CLI already stored at
  `~/.kimi-code/credentials/kimi-code.json` — so a subscriber who works through
  the CLI has nothing to create, paste, or rotate by hand. Same
  `/coding/v1/usages` endpoint, same weekly + 5h windows; an API key still takes
  precedence when one is set. New optional `[kimi] credentials_path` (for
  a relocated `KIMI_CODE_HOME`) and `[kimi] region` — `auto` follows
  kimi-code's own `~/.kimi-code/region` marker, `cn` pins `api.kimi.com`,
  `global` pins `api.kimi.ai`.

  The CLI's access token lives 15 minutes, so ai-usagebar refreshes it against
  `auth.kimi.com/api/oauth/token` and writes the rotated pair **back into
  kimi-code's own credential file** (atomically, mode 0600) rather than into a
  private sidecar the way the Kiro vendor does. Kimi rotates the refresh token
  on every grant: a private copy would leave the CLI holding a superseded token
  after each widget tick and log the user out of their own CLI. The refresh
  runs under kimi-code's own `proper-lockfile` protocol so the two clients
  never rotate at once, and the file is re-read inside the lock so a refresh
  that landed while waiting is used instead of being redone.

- Kimi's plan label now reads the subscription's own tier name ("Andante",
  "Moderato", "Allegretto", "Allegro") from `/coding/v1/me`, fetched
  concurrently with `/usages` so a widget tick still costs one round-trip.
  `/usages` only carries the `LEVEL_*` wire enum, which was what the bar used
  to show. When `/me` is unreachable or the account has no coding profile, the
  enum is humanized instead (`LEVEL_INTERMEDIATE` → `Intermediate`) — never
  mapped to an invented tier name. A still-fresh cache entry holding a raw
  `LEVEL_*` plan is refreshed once rather than waiting out its TTL. The
  profile response's personal fields (email, phone, nickname) are never
  deserialized, so they cannot reach a snapshot, the cache, or an error
  message.

### Changed

- Kimi's default widget format now shows both independent quotas —
  `5h X% · 7d Y%` — instead of a single percentage that silently discarded
  one of them. A custom `format` in config.toml is untouched. The TUI panel's
  value column shows the same `{pct}%` every other vendor shows instead of
  raw request counts; the counts move to the footnote next to the remaining
  figure and the reset countdown.

## [1.7.0] — 2026-08-25

### Added

- The macOS menu bar shows Z.AI's monthly MCP-tools pool as a fourth row, with
  its own bar, reset and pace marker — the widget, TUI and native panels have
  always listed it, only the menu bar had no field for it. It fills the same
  fourth-window slot Antigravity's second pool uses. An account with no MCP
  quota reports no reset for it and keeps three rows, and an older binary that
  does not know `{zai_mcp_*}` degrades the same way.

### Changed

- Kimi's tooltip is drawn with the same window block every other vendor uses —
  icon + label, progress bar with the percentage, then the reset countdown —
  instead of the bare `26 / 100  (26%)` pairs it printed before. The counters
  and the vendor's own remaining figure ride the reset line
  (`26 / 100 · 74 left`) so nothing reported is lost, and its bar text follows
  the `{pct}% · {reset}` shape the other percentage vendors already use
  (`{kimi_weekly_pct}% · {kimi_weekly_reset}`, was a bare `{kimi_weekly_pct}%`).
- MiniMax's tooltip rows are drawn with the shared window block too, so each
  pool shows a progress bar rather than a bare `Session 20%` pair.

### Fixed

- Z.AI and MiniMax show the pace arrow (`↑` / `→` / `↓`) next to each
  percentage in the widget and `--vendor` tooltips. The pace placeholders added
  in 1.3.0 reached the macOS menu bar, but the default tooltip never consulted
  them: `tooltip::push_window` had no way to render a glyph, and only the
  Anthropic renderer had one hand-rolled. The shared helper now takes a
  `WindowRow`, so the arrow travels with the row. As on the Anthropic tooltip,
  the elapsed marker inside the bar stays behind `--tooltip-pace-pts`; Codex and
  Antigravity rows are unchanged.

## [1.6.0] — 2026-08-25

### Added

- First-party Nix flake packaging supports `nix run`, profile installation,
  direct NixOS and Home Manager consumption, an overlay, and a development
  shell on x86_64 and aarch64 Linux and macOS.

### Fixed

- A `401`/`403` response body no longer reaches the widget tooltip or the TUI on
  the run that hit it. The body was redacted on its way to the `.last_error`
  file but the copy handed to the outcome was built separately from the raw
  body, so signing out with a warm cache showed the body once and the neutral
  message on every run after. `Cache::write_last_error` now returns exactly what
  it persisted, and every vendor that built the pair itself passes that value on
  — Anthropic, Anthropic API, Antigravity, Deepseek, Grok, Kilo, MiniMax,
  Moonshot, Novita, OpenAI, OpenRouter and Z.ai. Cursor, Kimi and Kiro already
  redacted at this point and are unchanged.
- Antigravity no longer reports a TLS listener's `400 Client sent an HTTP
  request to an HTTPS server` as the reason a probe run failed. Each product
  binds an RPC port and an HTTPS port, and the probe order reaches the HTTPS
  one only after the RPC one has already answered, so that reply describes our
  own probe rather than the product. Because it is an `Http` and not a
  `Transport`, letting it stand as the last failure also cost the silent cache
  fallback that a not-yet-serving product is supposed to get. It is now ranked
  below every other failure, and still reported when nothing else answered.

## [1.5.2] — 2026-08-24

### Fixed

- Terminal escape sequences in a subprocess's stderr, or in a filesystem path,
  can no longer repaint or forge a line in output the user is reading (#122).
  `security` and `tar` diagnostics, `AppError::Io`'s path, the Cursor database
  diagnostics, and the notes printed by `account switch` are all sanitized now.

## [1.5.1] — 2026-08-23

### Fixed

- `cargo clippy -- -D warnings` now runs on macOS and Windows as well as Linux.
  Each platform compiles a different slice of the crate — the macOS Keychain
  fallback, the Windows process and TCP-table walk — so a lint on the slice the
  Linux job never sees was a lint nobody saw. Two credential helpers and a test
  seam that were unreachable on Windows are gated accordingly.

- Google Antigravity probes the RPC listener ahead of the TLS one on Linux and
  macOS too, not just Windows, and keeps each running product's listeners in
  their own group when ordering them. With more than one product up, every RPC
  listener is now tried before any TLS listener instead of the two products'
  ports interleaving by number and putting back the `agy` handshake warnings
  v1.5.0 set out to silence (#121). An `ANTIGRAVITY_LS_ADDRESS` that leaves no
  host to connect to is dropped rather than probed.
- A negative balance is now spelled the same way everywhere. DeepSeek, Moonshot
  (whose `cash_balance` is explicitly a debt), Kimi, SuperGrok, and the TUI
  panel rows rendered it as `$-5.71` while OpenRouter, Grok, Kilo, and Novita
  rendered `-$5.71`. Every renderer now goes through one `format::money`, which
  also keeps a negative zero or a sub-cent debt from printing as `-$0.00`.

## [1.5.0] — 2026-08-23

### Added

- The Omarchy panel's reset row now shows the wall-clock time the limit window
  reopens alongside the countdown — `Resets in 4h 5m · 13:54` — and dates it
  whenever the reset lands on another day (#120).

### Fixed

- OpenRouter no longer hides a negative credit balance behind a healthy-looking
  `$0.00` in green (#118). A balance in debt is shown with its sign — `-$5.71` —
  and is treated as critical everywhere, including on an account that never
  bought credits, where the consumed-percentage has no denominator and used to
  report a reassuring 0%.
- Google Antigravity no longer gives up when `ANTIGRAVITY_LS_ADDRESS` points at
  a port that has moved. The override is still tried first, but discovered local
  ports are now probed behind it, and a signed-out server's authentication error
  is reported instead of being masked by connection refusals from products that
  are simply not running (#119). On Windows the RPC listener is probed before
  the TLS one, which also silences the TLS handshake warnings `agy` used to log
  on every poll.

## [1.4.0] — 2026-08-21

### Added

- Omarchy can hide the selected provider's percentage or balance for an
  icon-only top-bar entry while keeping full details in the panel and tooltip
  (#104). The established right-click TUI shortcut is unchanged.

### Fixed

- Z.AI usage parsing accepts both `CREDIT_LIMIT` and the legacy
  `TOKENS_LIMIT` bucket names, including mixed responses during rollout.
- Google Antigravity now discovers its dynamically assigned local server on
  Windows through native process and TCP-table APIs, so CLI, TUI, and JSON
  consumers no longer need to update `ANTIGRAVITY_LS_ADDRESS` after restarts.

## [1.3.1] — 2026-08-19

### Fixed

- Omarchy remembers the exact provider or named account selected in the
  Quattro panel and restores it after shell reloads, including sleep/unlock
  cycles. If that entry is no longer available, the configured primary remains
  the safe fallback.

## [1.3.0] — 2026-08-19

### Added

- OpenRouter supports multiple named keys through `[[openrouter.accounts]]`.
  Named accounts work with `--account`, appear separately in aggregate views,
  and keep isolated caches; existing singular `[openrouter]` configs and cache
  paths remain unchanged.
- Z.AI and MiniMax now expose pace and elapsed-time placeholders
  (`{zai_session_elapsed}`, `{zai_session_pace}`, `{zai_weekly_elapsed}`,
  `{zai_weekly_pace}`, `{zai_mcp_elapsed}`, `{zai_mcp_pace}`,
  `{minimax_session_elapsed}`, `{minimax_session_pace}`,
  `{minimax_weekly_elapsed}`, `{minimax_weekly_pace}`,
  `{minimax_video_elapsed}`, `{minimax_video_pace}`,
  `{minimax_video_weekly_reset}`, `{minimax_video_weekly_elapsed}`,
  `{minimax_video_weekly_pace}`, and their `_pace_indicator` variants), plus
  the cross-vendor `{session_elapsed}` / `{weekly_elapsed}` aliases — the macOS
  menu bar's pace marker now renders for both vendors the same way it already
  does for Claude and Codex.

### Fixed

- Omarchy now reports a missing `ai-usagebar` binary with the required install
  command instead of leaving the Quattro widget stuck in its loading state.
- Omarchy's Quattro panel no longer evaluates hidden row components against
  incompatible report rows, eliminating repeated QML type and string-binding
  errors without changing the rendered layout.

## [1.2.0] — 2026-08-18

### Added

- Added Nous Research subscription usage through its OAuth device flow and
  OpenCode Go rolling, weekly, and monthly usage through its API key.

### Fixed

- Nous Research refreshes now send the refresh token in the form and the
  required Portal header, work with existing safe configuration directories,
  and use portable atomic credential replacement on Linux, macOS, and Windows.
- Nous Research percentages now use subscription credits only. Purchased and
  total usable credits remain separate balances instead of changing the plan
  percentage.
- OpenCode Go now rejects empty or unsupported usage responses and keeps live
  and stale cache entries isolated by endpoint and API-key identity.

### Security

- Updated `h2` to 0.4.16 to bound empty DATA-frame processing
  (`RUSTSEC-2026-0258`).
- Nous browser launches no longer pass Portal URLs through the Windows command
  shell, and OAuth traffic uses bounded requests with same-origin redirects.
- OAuth fields and expiry arithmetic are bounded, and provider-specific error
  classes are preserved without exposing credential-bearing response bodies.

## [1.1.0] — 2026-08-16

### Added

- **KDE Plasma 6 plasmoid** (`kde-plasmoid/`). The native panel widget renders
  every provider returned by `ai-usagebar usage --json`, follows the active
  Plasma colour scheme, and keeps provider selection per applet instance. It
  includes a popup, live reset countdowns, configurable compact bars, and Qt 6
  and Node regression suites.

### Fixed

- **Aggregate views now source a Claude label shared by a CLI account and a
  Desktop profile only from Desktop.** The same account in two stores means two
  of them refreshing one rotating refresh token — each refresh invalidates the
  other's copy — and the CLI copy can even refresh to a stale/wrong identity
  that still authenticates but reports another account's (often zero) usage. The
  symptom: a heavily-used account showing 0% while its Desktop token returns the
  real number. The previous guard only dropped a CLI entry whose credential was
  *empty* (a half-finished `account add`), which cannot catch a token that
  authenticates but is misattributed. On a label collision the app-maintained
  Desktop token now always wins, which both avoids the rotation war and stops
  the silent misattribution. A CLI account with no Desktop profile of the same
  name is unaffected. Direct widget commands remain explicit: add `--desktop`
  when selecting the Desktop profile with `--account`.

## [1.0.3] — 2026-08-15

### Security

- macOS OAuth refreshes now update Claude Code's login-Keychain entry through
  Security.framework instead of placing access and refresh tokens in a
  subprocess argument list.
- Unix configuration files containing inline API keys are automatically
  tightened to mode `0600`; the app fails closed if it cannot protect them.
- Cached and live user-facing authentication failures discard provider response
  bodies, and widget fallback diagnostics are Pango-escaped before display.
- Claude and Grok subprocesses no longer inherit API keys belonging to unrelated
  ai-usagebar providers.

## [1.0.2] — 2026-08-14

### Changed

- Updated the Base64, bundled SQLite, error-derivation, SHA-2, AES, and CBC
  dependency stacks. Chromium safeStorage encryption remains byte-compatible,
  and SuperGrok cache identities remain stable across the hash upgrade; both
  formats now have independent fixed regression vectors.
- Updated the pinned Rust build-cache and cross-compilation installer actions.
  All dependency changes passed the full Linux, macOS, Windows, and Rust 1.88
  compatibility matrix.
- Documented how to disable Quattro's stock `omarchy.agents` widget when AI
  Usage should be the bar's only agent-status item.

## [1.0.1] — 2026-08-14

### Added

- The Omarchy Quattro panel now includes a native QML settings form for the
  primary provider and every supported API-key provider. Stored secret values
  never enter the shell; it receives presence metadata only and sends changed
  values to the Rust config owner over stdin.

### Changed

- Native and terminal settings share the existing `toml_edit` persistence
  path, including comment preservation, explicit clear-versus-unchanged
  behavior, automatic provider opt-in, mode-0600 writes on Unix, Waybar
  refresh, environment-variable precedence, and legacy config-path fallback.
  Existing configs and non-Omarchy frontends require no migration.

## [1.0.0] — 2026-08-14

### Added

- **Native Omarchy 4 / Quattro plugin.** The repository is now directly
  installable with `omarchy plugin add` and renders every configured provider
  in Quattro's shared Quickshell design system: native bar interaction and
  popup placement, theme-aware typography/surfaces/meters, provider switching,
  keyboard navigation, live reset countdowns, refresh state, and stale/error
  handling. It keeps credential access and network collection in the Rust
  binary instead of duplicating vendor logic inside the shell.
- `ai-usagebar usage --json` now exposes the configured `primary` id, canonical
  `display_name`, additive `status`, `stale`, and `fetched_at` entry metadata,
  plus `severity` and absolute `reset_at` values on percentage metrics.
  Existing `metrics` and lossless `sections` consumers are unchanged;
  long-lived native panels no longer have to parse human countdown strings.

### Changed

- Promoted the project to its first stable release with the provider, config,
  CLI, report, cache, and native frontend compatibility guarantees established
  across the 0.x series.
- User-facing subscription labels now consistently use the recognizable
  **Claude** and **Codex** product names. Stable machine ids remain `anthropic`
  and `openai`, and the separate organization-spend integration remains
  **Anthropic API**.
- Canonical provider names and metric reset metadata now originate in the Rust
  core. Native frontends remain platform-specific presentation adapters rather
  than carrying copied vendor tables or metric-order assumptions.

### Security

- UI-bound report fields now remove Unicode bidirectional control characters
  in addition to terminal controls, preventing untrusted labels or diagnostics
  from visually reordering neighboring text.

## [0.22.0] — 2026-08-11

### Added

- **SuperGrok subscription vendor** (`--vendor supergrok`, `[supergrok]`,
  opt-in). Shows the current weekly or monthly included-credit usage, reset,
  tier, and prepaid balance from the official Grok Build CLI's `x.ai/billing`
  ACP extension. ai-usagebar never parses, copies, caches, refreshes, or places
  Grok credentials in ACP messages:
  Grok Build retains account-scope, custom OIDC/external-provider, proxy,
  rotation, and `auth.json.lock` ownership. Cache isolation uses only an opaque
  digest of Grok's auth/config state, never a raw token or account identifier.
  Distinct from the existing `grok` vendor, which reads prepaid Management API
  balance with `XAI_MANAGEMENT_KEY`. `{sgk_*}` placeholders include the actual
  period kind; legacy generic weekly aliases remain available for format
  compatibility.
- `--version` / `-V` on the `ai-usagebar` binary, reporting the crate version
  (#81). Until now the only way to tell which build was installed was parsing
  `cargo install --list`.
- **Kiro CLI vendor** (`--vendor kiro`, `[kiro]`, opt-in). Reads the credit
  pool from `AmazonCodeWhispererService.GetUsageLimits` — the exact call
  kiro-cli's own `/usage` slash command makes — using the AWS SSO OIDC
  session kiro-cli already cached in its local `data.sqlite3` after
  `kiro-cli login`. No separate login step; the OIDC access token (valid
  ~1h) is refreshed via the documented AWS SSO OIDC `CreateToken` API when
  close to expiry, using the refresh token + client credentials kiro-cli
  registered for itself. Refreshed and rotated credentials are kept in an
  atomic, mode-0600, account-scoped ai-usagebar sidecar and are never written
  back to kiro-cli's own database.
- **Cursor: `cursor-agent` fallback credential** (`[cursor] agent_auth_path`).
  Text-only machines that never open the desktop IDE now get usage too: when
  the IDE's `state.vscdb` is absent, the vendor falls back to the session
  token the headless `cursor-agent` CLI wrote to its own
  `~/.config/cursor/auth.json`. The IDE database stays the preferred source
  when both exist; an existing but unreadable or malformed IDE database still
  surfaces its own error instead of silently switching to another login.

### Fixed

- **A routine renamed in one account now converges to one title everywhere.** A
  scheduled task has no `updatedAt`, so a rename leaves `createdAt` untouched and
  previously only reached the account you switched *to*. A switch now carries
  the title selected by the baseline-aware routine merge into *every* account's
  registry, so the name stops disagreeing across accounts. The convergence pass
  changes only `displayName` and preserves the rest of each registry, including
  unknown top-level fields. There is no prompt, and it applies to the terminal
  and menu bar alike since both drive the same switch path. Mirrored in
  claude-acc.

- **Antigravity now works on macOS, in both the CLI and the menu-bar app.**
  Local-server discovery (`discover_ls_ports`) only ever walked `/proc`, so on
  macOS — which has no `/proc` — it silently returned nothing and every
  Antigravity fetch failed with "no local server found" even while Antigravity
  was running. It now shells out to `lsof -iTCP -sTCP:LISTEN -F pcn`, the
  macOS equivalent, and matches listening processes with the same predicate
  the Linux path already used (now case-insensitive, since the packaged macOS
  app's process name is capitalized). Separately, the menu-bar app's own
  vendor list (`VENDOR_AUTH` in `macos/ai-usagebar-menubar.swift`) had never
  been updated when Antigravity shipped, so it stayed invisible there even
  after enabling `[antigravity]` — it's now a `local`-kind entry alongside
  Cursor, "configured" the same way the GNOME extension already detects it
  (any of `~/.gemini/{antigravity,antigravity-cli,antigravity-ide}`).

### Security

- Updated the transitive `lru` dependency from 0.18.0 to 0.18.2, fixing
  RUSTSEC-2026-0253 (a panic-safety use-after-free in `LruCache::pop`).

## [0.21.0] — 2026-08-03

### Added

- **Claude Desktop accounts now report usage with no `claude` CLI login.** A
  saved Desktop account (`account add <label> --desktop`) previously needed a
  *second*, separate `claude` login before its quota could show — because usage
  came only from a CLI credential. It turns out the Desktop app stores its own
  token under the same public OAuth client as Claude Code, and that token is
  accepted by the usage endpoint, so ai-usagebar now reads it directly. Every
  saved Desktop profile appears as a Claude account in `ai-usagebar usage`, the
  TUI, and the macOS menu-bar overview — labelled `· <label> (desktop)` — with
  zero CLI involvement.

  The token lives in the app's encrypted `safeStorage` blob; ai-usagebar
  decrypts it with the login-Keychain key (macOS), picks the
  `user:inference`-scoped entry, and maps it onto the existing OAuth path so
  fetching and rendering stay unchanged. The **active** account is read-only
  from the live `config.json` the app keeps fresh; ai-usagebar never rotates
  that credential,
  even while the app happens to be stopped. Every other account is read from
  its profile snapshot and refreshed under the same lock as account switching,
  with the rotation written back before a switch can install it. Desktop caches
  are isolated by account UUID, so a reused label cannot expose another CLI or
  Desktop account's usage. A half-finished CLI `account add <label>` no longer
  masks a working Desktop profile of the same name: the Desktop source takes
  over when the CLI credential can't authenticate. The menu bar consumes this
  same Rust-resolved list, including a configured `desktop_profiles_dir`.
  macOS-only (the Desktop app and its Keychain key exist nowhere else).

- **Deleted routines and chats are now confirmed instead of silently
  resurrected.** The merge is a union, so deleting a routine or a conversation
  in one account meant it came straight back from whichever account still held a
  copy — and there was no way to tell that apart from something the account had
  simply never received.
  ai-usagebar now records what each account held after the last merge
  (`~/.claude-acc/synced.json`, shared with claude-acc) and uses it to detect a
  genuine deletion, then asks: keep them all, delete them everywhere, or choose
  individually. Confirming sweeps it from *every* account so it stops returning.
  A confirmed chat loses only its **index** — the transcript in the
  account-agnostic `~/.claude/projects/` is never touched, so the conversation
  stops following you between accounts without the text being destroyed. The
  macOS menu bar asks the same question in a dialog with one checkbox per item —
  checked keeps it — and passes the verdict through as the type-scoped
  `--delete-conflict <key>`; `account status --json` lists each pending
  conflict's opaque `key` under `deletion_conflicts` so scripts can do the same
  without confusing a routine id with a chat filename.

  Deleting is only ever reachable from an answered prompt: `-y` does not imply
  it, and a switch with no terminal (the menu bar's subprocess, a pipe, a cron)
  keeps everything and says so. With no record yet — the first run after
  upgrading — nothing is reported as a deletion, so behaviour is unchanged until
  there is real history to compare against.

- **Routine edits now reconcile per task instead of per registry file.** The
  sync record keeps a three-way baseline, so editing one routine in each of two
  accounts preserves both edits. Concurrent edits to the same routine remain
  local and are reported during the switch instead of silently choosing one;
  editing the desired copy resolves it on the next switch. Existing sync files
  remain readable and keep their flat claude-acc-compatible shape.

- **`ai-usagebar usage` — quota and time-to-reset for everything in the config,
  in one command.** The widget answers "how is *this* vendor doing" one process
  at a time, which is what a status bar needs and what a person checking on four
  Claude accounts does not. This walks the same set the TUI builds — every
  enabled vendor plus one entry per named Claude account — and prints each
  window's percentage next to when it resets. `--json` keeps gauge rows in a
  convenient `metrics` list and provides a lossless ordered `sections` list for
  balance text and grouped breakdowns, keyed by a stable id
  (`anthropic@work`), for scripting and logging. A vendor that fails to fetch
  reports inline instead of hiding the rest, and the exit code is non-zero only
  when every entry failed.

  Thin by construction: it reuses the TUI's existing tab enumeration, fetch, and
  snapshot-to-sections projection, so no vendor needs to know it exists.

### Changed

- Refreshed the Rust UI, configuration, SQLite, serialization, and base64
  dependencies and the pinned checkout, artifact, and AUR deployment actions.
  The resulting dependency graph remains compatible with the declared Rust
  1.88 minimum.

### Fixed

- **TUI refresh flicker.** Auto-refresh and manual refresh now keep the last
  successful vendor snapshot visible with a `↻` indicator while revalidating.
  Initial loads still show `fetching…`; failed revalidation preserves the old
  snapshot with an explicit stale warning instead of briefly or permanently
  hiding useful data. Duplicate requests for the same tab are suppressed
  (#64).

## [0.20.1] — 2026-07-30

### Security

- Redact successful-but-malformed OAuth token response bodies from diagnostics,
  strip terminal control characters from vendor text and cached errors, and cap
  untrusted display fields before they reach Pango, ANSI, or ratatui output.
- Restrict vendor HTTP redirects to the original scheme, host, and port so
  non-standard API-key headers cannot be forwarded cross-origin.
- Create Claude Desktop rollback backup directories and archives with private
  Unix permissions (`0700` and `0600`, respectively).
- Pin every GitHub Action to an immutable commit, add automated pin updates,
  and require release tags to be annotated and point to commits on `main`.

## [0.20.0] — 2026-07-29

### Added

- **MiniMax Token Plan vendor** (`--vendor minimax`, `[minimax]`, opt-in). Reads
  the subscription quota from the officially published
  `GET /v1/token_plan/remains` route (response shape verified against the live
  global endpoint). The plan reports one row per model bucket, each with a
  rolling interval window and a weekly window, so it renders as a two-pool
  quota vendor: `general` (text/coding) drives the bar and the generic
  `{session_pct}` / `{weekly_pct}` aliases, and `video` rides along in the
  tooltip and TUI panel. `{vendor_short}` is `mmx`.
  Four properties of this API are encoded deliberately, each with a test:
  it answers **HTTP 200 even when auth fails** (the real status is
  `base_resp.status_code`; the two credential codes map onto HTTP 401 so a bad
  key reports as an auth problem, not schema drift); the percentages are what
  **remains**, not what was consumed, and are inverted on the way in; the
  interval length is **not fixed** (5h for `general`, 24h for `video`), so each
  window's duration comes from its own start/end; and all timestamps are epoch
  **milliseconds**. `[minimax] region` picks the *instance* rather than a unit —
  the global and CN deployments issue separate keys and reject each other's, so
  the endpoint and a non-secret key fingerprint are recorded in the cache
  payload, and a mismatched cache is discarded instead of being shown against
  the wrong account.
- **`ai-usagebar account status` and `account switch <label>` — see and change
  which Claude account you are actually signed in as (macOS).** There are two
  separate identities on a Mac and they drift apart constantly: the **Claude
  Desktop app** (signed in through its own `config.json`) and the **`claude`
  CLI** (one default login in the login Keychain). `account status` reports both
  — with each account's e-mail, session count, and whether its credential and
  browser state have been captured — and `--json` makes that available to
  scripts and the menu bar. `account switch` moves either one: `--desktop`,
  `--cli`, or neither for both, with `--dry-run` to see exactly what would
  happen first.

  Switching the **Desktop app** merges your local history into the target
  account first — session indexes newest-wins, routines/schedules unioned by
  task id — so the account you land on shows the union of everything rather
  than only its own chats; then it quits the app, swaps the credential and the
  cookie/LevelDB state, and reopens it. Before any of that it writes a rollback
  archive of everything a switch can destroy (`--keep-backups`, default 10;
  `--backup-sessions` for a full session-tree archive), and it writes
  `config.json` atomically so a crash mid-switch cannot strand every account's
  tokens. The volatile `bridge-state.json` is cleared each time, since a stale
  cloud-session id makes `/remote-control` fail to disconnect; `--keep-bridge`
  turns that off for diagnosing browser-connection issues.

  Switching the **CLI** moves the account's stored credential into the one
  default slot plain `claude` reads and removes its named copy. The outgoing
  account's credential is saved back into its own slot first, and while a label
  is the live CLI login
  ai-usagebar reads that label from the default slot — so one rotating refresh
  token is never live in two places, which is what would otherwise 401 one of
  the two copies within hours. A CLI login that belongs to no configured
  account is never silently discarded: the switch refuses unless `--force`.

- **`ai-usagebar account add <label> --desktop` captures a Claude Desktop
  account**, so a machine can build its account list from nothing. The CLI half
  of `add` is easy — `CLAUDE_CONFIG_DIR` gives `claude` as many isolated logins
  as you want — but the Desktop app has a single login slot and no way to ask
  for a second, so the only way to obtain another account's credential is to
  sign the app out, wait for you to sign in as that account, and keep what it
  writes. That is what this does: it saves the current account into its own
  profile, copies the live login aside, clears it, reopens the app at its login
  screen, polls until the sign-in completes, then captures the credential,
  browser state and organisation, and seeds the new account with the history
  this machine already has so its first login is not an empty sidebar. Press
  Ctrl-C to cancel — or let the five-minute window lapse — and your previous
  login is put back exactly as it was.

- **Claude Desktop ▸ and Claude Code ▸ submenus in the macOS menu bar.** Each
  lists the accounts that surface knows, checkmarks the active one, and
  switches on click; **Adicionar conta…** captures a new one (in Terminal,
  since it is interactive). A dim line under the header shows both active
  accounts at a glance. The Desktop switch confirms first, because it quits and
  reopens Claude.app. The submenus refresh on launch, on a `config.toml` change,
  and when the menu opens (debounced), so a switch made in a terminal shows up
  without restarting anything.

  Desktop accounts are stored in [claude-acc](https://github.com/ohmaseclaro/claude-acc)'s
  profile format, so existing claude-acc users' profiles work here untouched and
  either tool can capture or switch them; `[anthropic] desktop_profiles_dir`
  overrides the location. That project's reverse-engineering of the Claude
  Desktop internals is what this builds on, and the Desktop halves of `add` and
  `switch` are ports of its commands. Removing an account and chat filtering
  (`only`/`reset`) are not implemented here. Nothing affects the Linux build:
  the modules compile and are tested everywhere, and simply find no Claude
  Desktop installation.

- **Configurable TUI vendor navigation.** Set `[ui] vendor_box` to `sidebar`
  (the responsive existing default), `navbar` (always use the horizontal top
  strip), or `none` (hide the navigation and give the active panel the full
  terminal width). Live config reload applies the layout immediately.

### Security

- Updated `quinn-proto` to 0.11.15 to prevent remote memory exhaustion from
  unbounded out-of-order stream reassembly (RUSTSEC-2026-0185), and `anyhow` to
  1.0.104 to fix unsound mutable error downcasting (RUSTSEC-2026-0190).

## [0.19.0] — 2026-07-27

### Added

- **Per-provider on/off toggle in the Overview (macOS menu bar).** Each row in
  the Overview dropdown is now a checkbox: click it to drop that provider from
  the always-visible top-bar summary (checkmark = shown; unchecked + dimmed =
  hidden). Hidden providers stay listed in the dropdown so you can turn them
  back on, and dropping some also frees up the top bar to draw mini bars again
  instead of compact text. The choice persists (UserDefaults). Jumping to a
  provider's detail view moves to the *Trocar vendor* submenu / **⌥⌘\\** (the
  Overview row click now toggles instead).

- **`ai-usagebar account add <label>`** takes a new custom Claude (Anthropic)
  account from nothing to signed-in in one command: it appends an
  `[[anthropic.accounts]]` block to `config.toml` (creating the file if needed,
  preserving comments and formatting via `toml_edit`), creates the account's
  credentials directory, and then **launches `claude` to sign in** with that
  account's own `CLAUDE_CONFIG_DIR` — so the login writes exactly where
  ai-usagebar reads it back (the config-dir-scoped Keychain item on macOS, a
  `.credentials.json` on Linux/Windows) and **your default Claude login is never
  touched**. When it returns, it re-stamps `config.toml` so the running menu bar
  / TUI re-fetches and the enabled account shows up **with data immediately** —
  no restart, no hand-copying credentials. It's idempotent (re-run it to sign an
  already-registered account back in), never touches the default account, and
  `--no-login` skips the login step to just register the entry (headless boxes,
  or add-now-sign-in-later). If `claude` isn't on `PATH` or the login is
  cancelled, the entry is still registered and it prints the exact login command
  to finish by hand.

- **Live `config.toml` reload — no more restart after editing it.** Both the
  **macOS menu-bar app** and the **TUI** now watch `config.toml` and pick up
  changes on the fly: enable a vendor, add an `[[anthropic.accounts]]` entry,
  tweak an `[ui]` knob, and it takes effect within a second or two — the vendor
  submenu, swap ring, Overview, and TUI tab set all rebuild in place. The menu
  bar watches natively (`DispatchSource`, re-arming across an editor's atomic
  save) so it's instant; the TUI polls the file's mtime every 2s (no new
  dependency). In the TUI, a half-written/broken file mid-edit is ignored and
  retried until it parses, so the running config is not replaced with defaults.

## [0.18.0] — 2026-07-27

### Added

- **Claude multi-account in the macOS menu bar.** Every named Anthropic account
  — explicit `[[anthropic.accounts]]` entries and `[anthropic] accounts_dir`
  discoveries, the same config the binary and TUI already read — now appears as
  its own entry ("Claude · work") in the *Trocar vendor* submenu, the **⌥⌘\\**
  swap ring, the Preferences vendor selector, and the **Overview** (its own
  dropdown row and status-bar segment, labeled by account). Fetches run as
  `--vendor anthropic --account <label>`, so each account keeps its own cache
  and refresh, and the dropdown header shows which account is active
  ("Claude Max 20x · work"). `[anthropic] show_default_account = false` hides
  the default (unnamed) Claude entry, mirroring the TUI.

- **Overview across the TUI and the macOS menu bar.** A single view summarizing
  every vendor at once — one compact row each (key metric, colored by severity)
  — so all your limits are visible without switching tabs. In the **TUI** it is
  a virtual first tab that `Tab`/`h`/`l` wrap through at both ends and the
  default landing view (unless `[ui] primary` opens on a specific vendor);
  `[ui] overview_vendors = [...]` picks and orders which vendors it lists on
  both surfaces. In the **macOS menu-bar app** it is a target in the vendor
  submenu and in the global
  **⌥⌘\\** swap ring (which now cycles all providers *and* the overview); its
  dropdown lists every configured vendor — each row **clickable** to jump to that
  vendor — and the bar shows every vendor at once (a mini bar each when few, or
  compact %-text past `[ui] overview_menubar_bars_max` (default 4), capped at
  `overview_menubar_max`, in stable provider-grouped entry order). A
  **Compactar** item right under the usage rows forces the compact %-text
  mode even under the threshold; while compact it reads **Expandir** and turns
  it back off. It also has a **global ⌥⌘E shortcut** (hinted on the item,
  toggleable in Preferências → Atalho next to the ⌥⌘\\ swap toggle; overview
  mode only). Each vendor's headline is the metric that matters:
  **Cursor** shows its combined *included total usage*; **Anthropic** the biggest
  of 5h / weekly / the scoped-model (Fable) window. The menu-bar title now also
  shows each vendor's **time to reset**, squeezed to its leading unit ("4d",
  "2h", "5m") to fit both bar and %-text modes — same countdown the dropdown
  and the per-vendor detail view already show, just shortened for the bar.

- **Instant "Loading…" feedback on a vendor swap** (menu bar). Switching vendor —
  by ⌥⌘\\, the submenu, or an overview row — immediately replaces the view with a
  placeholder naming the target, instead of leaving the previous vendor's data up
  (which read as a freeze). The **⌥⌘\\ shortcut is also hinted** on the *Trocar
  vendor* menu item.

- **Cursor vendor.** Shows this billing cycle's two included-usage pools —
  **Cursor Models** (Auto + Composer) and **Other Models** (named / API) — as
  percentages, from `GET cursor.com/api/usage-summary`, the same undocumented
  endpoint the Cursor dashboard's own frontend calls. Also surfaces the plan
  (`membershipType`), the billing-cycle reset, whether on-demand spend is on,
  and unlimited plans. No API key: the session token is read **read-only** from
  the local `state.vscdb` SQLite database the Cursor IDE already wrote after you
  signed in there (the JWT's `sub` claim yields the user id; combined with the
  raw token it forms the `WorkosCursorSessionToken` cookie the endpoint
  expects). Opt-in (`[cursor] enabled = true`) and wired into the Waybar widget,
  `--vendor cursor`, the TUI panel (a bar per pool), scroll-cycling, **the macOS
  menu bar app** (its two pools relabel the session/weekly bars as "Cursor
  Models" / "Other Models"), and the config-example/README docs. Adds a
  `rusqlite` (bundled) dependency. Not wired into the GNOME extension yet.
  **Team accounts** (`membershipType` with no `individualUsage.plan`) are now
  parsed too, via the auto/named "You've used N% of your included … usage"
  display-message strings the payload also carries — the only percentage
  source Cursor exposes for those accounts, per an independent
  reverse-engineering of the same endpoint. Unverified against a live team
  account (labeled `"<Plan> (team)"` in the UI so it's visibly a best-effort
  path); falls back to the existing schema error rather than a fabricated
  0% if the display messages don't parse.
- **Auto-discovered Anthropic accounts (`[anthropic] accounts_dir`).** Point it
  at a directory and ai-usagebar discovers each account under it automatically,
  using Claude Code's own `CLAUDE_CONFIG_DIR` layout: every immediate
  subdirectory becomes an account labeled by the subdirectory name — a TUI tab
  and `--account <label>`, refreshed independently — with no per-account config
  entry. This directory-based discovery also sees macOS logins whose credentials
  live only in a config-dir-scoped Keychain item. Populate it by running the
  `claude` CLI with a per-account `CLAUDE_CONFIG_DIR`, the general way to keep
  several Claude Code logins side by side. Discovered accounts merge with
  explicit `[[anthropic.accounts]]` (explicit wins on a label clash); a missing
  or unreadable directory is ignored. Because it keys only on the standard
  Claude Code layout, any tool that manages multiple logins works with it, not
  one specific account switcher.
  - **`[anthropic] show_default_account`** (default `true`): set `false` to hide
    the default (unnamed) Claude tab when every account is managed explicitly,
    so you don't get a redundant tab for the ambient Keychain/`~/.claude` login.
    Ignored when there are no named accounts.
  - **Staggered multi-account refreshes.** The TUI previously refreshed every
    tab at once; with several Anthropic accounts that burst the shared
    `/api/oauth/usage` + token endpoints and tripped their rate limit (`429`).
    Anthropic tabs now refresh spaced out (~0.8s apart) so each account fetches
    politely; other vendors still start immediately.
- **"Iniciar no login" (start at login) toggle** in the macOS menu-bar app's
  Preferences. Flipping it on installs a per-user LaunchAgent
  (`~/Library/LaunchAgents/com.akitaonrails.ai-usagebar-menubar.plist`) pointing
  at the running binary, so the app comes up automatically at each login; off
  removes it. This is the GUI equivalent of `macos/install-agent.sh` — no
  `launchctl` needed. macOS only: the app is a menu-bar agent that doesn't exist
  on Linux (where the GNOME Shell extension autostarts with the session, and the
  Waybar widget starts with the bar) or Windows.

### Fixed

- **Cursor caches are now bound to the signed-in account and billing cycle.**
  A fresh cache from one Cursor login can no longer be shown after the IDE
  switches accounts, and a stale snapshot is not served past its recorded
  billing reset during an outage. Cached integers are range-checked before
  narrowing, and the live payload must include a finite, representable
  `totalPercentUsed` instead of silently turning schema drift into `0%`.

- **macOS Overview, shortcuts, and login startup now reflect their real
  configuration/state.** Overview honors `[ui] overview_vendors` (including all
  named Claude accounts selected by `anthropic`) and grows its dropdown row pool
  as accounts are discovered. Carbon handler/hot-key registration errors are
  checked; an unavailable shortcut turns its preference back off instead of
  appearing enabled while doing nothing. “Iniciar no login” reads the actual
  LaunchAgent file, writes it atomically, and surfaces filesystem errors rather
  than drifting from a stale `UserDefaults` value.

- **Named/`accounts_dir` Anthropic accounts now find macOS Keychain-backed
  logins.** `CLAUDE_CONFIG_DIR=<accounts_dir>/<label> claude` was documented
  to make `<label>` "just work", but on macOS Claude Code stores the login in
  the Keychain (service `Claude Code-credentials-<hash>`, hashed from the
  config dir's absolute path) and never writes `<label>/.credentials.json` —
  so a named account could look logged in via `claude` yet ai-usagebar kept
  reporting "no usable cache" / stale file errors. Named accounts now prefer
  the Keychain item hashed from their own directory, falling back to the file
  (the Linux layout). Keychain-first matters: a `.credentials.json` copied by
  hand shares its refresh-token lineage with the original, and dies with a
  401 as soon as the real holder rotates it — reading the file first kept
  resurrecting those dead snapshots over the live login sitting in the
  Keychain. Token refreshes write back to the same scoped item, so
  ai-usagebar and Claude Code keep sharing one source of truth per account.
  Account discovery itself now keys on each immediate directory, rather than
  requiring the file that Keychain-only logins intentionally do not create.
  A different account's item can never match (the hash is per-directory), so
  this doesn't reopen the cross-account ambiguity the original file-only
  rule (#15) was written to avoid.

- **Menu-bar app no longer freezes in Overview mode.** The appearance observer
  fired on every layout pass (not just real light↔dark flips), and in Overview
  each fire rebuilt the vendor submenu — which relaid out the button, re-firing
  the observer: a main-thread loop that also spawned a keychain subprocess each
  iteration, so the menu stopped responding to clicks. The observer now reacts
  only to actual theme changes, appearance repaints skip the submenu rebuild, and
  the keychain check is cached.

- **A failed terminal resize no longer exits the TUI.** A transient
  `terminal.resize` error (e.g. an ioctl failure) now just skips that resize
  instead of tearing down the whole UI; the next resize or redraw recovers.

## [0.17.2] — 2026-07-25

### Fixed

- **TUI redraws on terminal resize.** The crossterm reader thread discarded
  `Event::Resize`, so maximizing or restoring the terminal left the UI painted
  in a corner of the alternate screen until a manual `R`. Resize events are now
  forwarded to the main loop, which resizes the viewport and redraws.

## [0.17.1] — 2026-07-24

### Added

- **Eleven-vendor parity in the macOS menu bar app.** The selector previously
  exposed only five vendors (Anthropic, OpenAI, Z.AI, OpenRouter, DeepSeek);
  it now covers all binary vendors except Antigravity (see below). Added Kimi,
  Kilo, Novita, Moonshot, Grok (xAI), and Anthropic (API).
  - **Real balances for every balance-only vendor.** OpenRouter, DeepSeek,
    Kilo, Novita, Moonshot, Grok, and Anthropic (API) now render their actual
    balance/credits via per-vendor format fields (`{or_balance}`,
    `{ds_balance}`, `{kilo_balance}`, `{nv_balance}`, `{km_balance}`,
    `{grok_balance}`, `{aapi_headline}`) instead of fake 0% session/weekly
    rows. Anthropic (API) shows a spend-vs-limit bar when a monthly limit is
    configured. The balance is dispatched by the selected vendor (not by
    `vendor_short`, which collides between Kimi and Moonshot).
  - **TOML `enabled` handling matches the Rust config.** Bare booleans
    (`enabled = false`) and inline comments are parsed, and an omitted
    `[vendor].enabled` reproduces the `src/config.rs` defaults. The Preferences
    picker and the "Trocar vendor" submenu only offer enabled vendors.
  - **Generalized `config.toml` reader.** Reads any key under any `[section]`,
    so `api_key_env` is resolved per vendor instead of being hardcoded.

- **Quick vendor switch submenu in the macOS dropdown.** A "Trocar vendor"
  submenu between "Abrir TUI" and "Preferências…" lists only configured
  vendors, with a checkmark on the active one, so switching no longer requires
  opening Preferences.

- **Optional ring indicator layout in the macOS app.** A new "Estilo do
  indicador" preference selects between the default block bars (`░█`) and a
  ring drawn with `NSBezierPath` (AppKit). The ring paints the usage fraction
  as a severity-colored arc over a faint track, and honors the pace marker the
  same way the block bar does (calm fill up to the lesser of the current
  percentage and the blue tick at the elapsed position, warning color on any
  fill past the tick). Both the menu bar and the dropdown rows honor the
  choice. The track adapts to the effective appearance — faint white on dark
  menu bars /
  wallpapers (where the block bar's dark `COLOR_EMPTY` would be invisible),
  `COLOR_EMPTY` on light ones.

- **Dark and light appearance awareness.** Status text now resolves against
  the effective status-bar appearance using the new `menuBarTextColor()`
  helper, and the menu bar re-renders immediately when the system appearance
  changes (e.g., switching wallpapers or dark/light mode) via KVO on
  `effectiveAppearance`, without waiting for the usage refresh timer.

- **Pure-logic test harness for the menu bar app.** The single-file app has no
  Xcode project, so it is compiled with `-D SWIFT_TEST_HARNESS` alongside a
  test file that calls its helpers directly (`macos/run-tests.sh`). Covers arc
  geometry, TOML `enabled` parsing, Rust defaults, and per-vendor balance
  dispatch. The CI macOS job runs it.

### Fixed

- **Moonshot's `{vendor_short}` no longer collides with Kimi's.** Both reported
  `kmi`; Moonshot now reports `msh`. Anything dispatching on `vendor_short`
  (custom Waybar formats, desktop integrations) could attribute one vendor's
  data to the other.
- **Ring pace arc.** The overshoot arc previously restarted at 12 o'clock and
  overpainted the start of the calm fill; it now spans from the elapsed marker
  to the current percentage.
- **Preferences window crash.** The SwiftUI preferences view is now hosted
  through `contentViewController` instead of being installed directly as
  `contentView`, avoiding an AppKit exclusivity crash during window
  measurement on certain macOS versions.
- **OpenAI's temporary weekly-only Codex limit is labeled and rendered
  correctly.** During the July 2026 rollout, OpenAI moved the 7-day window into
  `primary_window` and omitted `secondary_window`
  ([openai/codex#32707](https://github.com/openai/codex/issues/32707)).
  ai-usagebar treated wire position as meaning, so the real weekly percentage
  appeared under "Codex 5h" while a fabricated 0% weekly gauge was shown.
  Windows are now classified from `limit_window_seconds`; absent windows stay
  absent in the widget, tooltip, TUI, GNOME extension, and macOS menu bar.
  Accounts that still receive both 5-hour and 7-day windows keep the existing
  layout and placeholders.
- **macOS menu bar now shows OpenAI pace markers.** The `{session_elapsed}` and
  `{weekly_elapsed}` cross-vendor aliases were never registered for OpenAI, so
  the fields always rendered empty and the pace markers never appeared.

### Not supported

- **Google Antigravity on macOS.** The binary only discovers its local language
  server on Linux (via `/proc`); on macOS there is no reachable quota source,
  so Antigravity is not offered in the macOS app. Safe macOS server discovery
  is dedicated future work.

## [0.16.0] — 2026-07-22

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

  Quota and cached values are parsed strictly: malformed, out-of-range,
  duplicate or missing required buckets trigger a refetch rather than a
  confident bar. Response bodies are bounded on success and error paths. The
  cache fingerprints the signed-in account, so switching Google accounts
  cannot show the previous account's figures, and a window whose reset has
  passed is refused rather than served as current. When a fetch fails with
  nothing usable cached, the original actionable error is preserved.

- **Two-pool support in the GNOME extension.** The dropdown groups Antigravity's
  four windows under `Session` and `Weekly` headings, one bar per pool. The new
  `Panel pools` preference draws both pools (default), either alone, or `auto`,
  which falls back to an available other pool once the shown one reaches
  `Auto threshold`.
  Pace markers are rendered for all four windows. The grouped layout is opted
  into by the data — a vendor naming its primary rows — so single-pool vendors
  are unaffected, and a binary predating the new placeholders keeps the flat
  four-row layout.

### Changed

- The GNOME extension supports GNOME Shell 45–50 (was 45–48).

### Fixed

- Bordered tooltips no longer ragged-edge on rows containing an escaped
  character: `visible_width` counted `&amp;` as five glyphs instead of one, so
  every such row stopped short of the right border. Affects any vendor whose
  API-supplied labels contain `&`, `<` or `>`.

## [0.15.0] — 2026-07-22

### Added

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

[Unreleased]: https://github.com/akitaonrails/ai-usagebar/compare/v1.10.0...HEAD
[1.10.0]: https://github.com/akitaonrails/ai-usagebar/compare/v1.9.1...v1.10.0
[1.9.1]: https://github.com/akitaonrails/ai-usagebar/compare/v1.9.0...v1.9.1
[1.9.0]: https://github.com/akitaonrails/ai-usagebar/compare/v1.8.0...v1.9.0
[1.8.0]: https://github.com/akitaonrails/ai-usagebar/compare/v1.7.0...v1.8.0
[1.7.0]: https://github.com/akitaonrails/ai-usagebar/compare/v1.6.0...v1.7.0
[1.6.0]: https://github.com/akitaonrails/ai-usagebar/compare/v1.5.2...v1.6.0
[1.5.2]: https://github.com/akitaonrails/ai-usagebar/compare/v1.5.1...v1.5.2
[1.5.1]: https://github.com/akitaonrails/ai-usagebar/compare/v1.5.0...v1.5.1
[1.5.0]: https://github.com/akitaonrails/ai-usagebar/compare/v1.4.0...v1.5.0
[1.4.0]: https://github.com/akitaonrails/ai-usagebar/compare/v1.3.1...v1.4.0
[1.3.1]: https://github.com/akitaonrails/ai-usagebar/compare/v1.3.0...v1.3.1
[1.3.0]: https://github.com/akitaonrails/ai-usagebar/compare/v1.2.0...v1.3.0
[1.2.0]: https://github.com/akitaonrails/ai-usagebar/compare/v1.1.0...v1.2.0
[1.1.0]: https://github.com/akitaonrails/ai-usagebar/compare/v1.0.3...v1.1.0
[1.0.3]: https://github.com/akitaonrails/ai-usagebar/compare/v1.0.2...v1.0.3
[1.0.2]: https://github.com/akitaonrails/ai-usagebar/compare/v1.0.1...v1.0.2
[1.0.1]: https://github.com/akitaonrails/ai-usagebar/compare/v1.0.0...v1.0.1
[1.0.0]: https://github.com/akitaonrails/ai-usagebar/compare/v0.22.0...v1.0.0
[0.22.0]: https://github.com/akitaonrails/ai-usagebar/compare/v0.21.0...v0.22.0
[0.21.0]: https://github.com/akitaonrails/ai-usagebar/compare/v0.20.1...v0.21.0
[0.20.1]: https://github.com/akitaonrails/ai-usagebar/compare/v0.20.0...v0.20.1
[0.20.0]: https://github.com/akitaonrails/ai-usagebar/compare/v0.19.0...v0.20.0
[0.19.0]: https://github.com/akitaonrails/ai-usagebar/compare/v0.18.0...v0.19.0
[0.18.0]: https://github.com/akitaonrails/ai-usagebar/compare/v0.17.2...v0.18.0
[0.17.2]: https://github.com/akitaonrails/ai-usagebar/compare/v0.17.1...v0.17.2
[0.17.1]: https://github.com/akitaonrails/ai-usagebar/compare/v0.16.0...v0.17.1
[0.16.0]: https://github.com/akitaonrails/ai-usagebar/compare/v0.15.0...v0.16.0
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
