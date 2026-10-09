## [Unreleased]

### Added

- **Ollama Cloud provider mark in the Omarchy bar and panel.** Ship the
  monochrome Lobe Icons SVG under MIT, with source provenance and the
  existing LobeHub licence notice. The same mark is used for custom
  providers that declare `brand = "ollama"`.

### Changed

- **Ollama Cloud quota now comes from `/api/balance`.** `/api/usage` returned
  request and token history rather than quota, so the provider reads the
  documented balance route instead. Both account shapes are served there —
  verified against a real capture from a session/weekly account — so existing
  quota users see no change beyond the source.

### Fixed

- **Ollama Cloud credit-based Pro plans no longer show an empty quota.**
  Fetch the documented `/api/balance` endpoint instead of `/api/usage`, which
  now returns request/token history. Automatically detect monthly included
  credits (such as Pro's $60 allowance) or legacy session/weekly limits.
  Show remaining/allowance dollars and purchased credits, derive monthly
  utilization from included credits, and preserve billing resets and actual
  period lengths. Historical quota caches remain readable; usage-history
  payloads are rejected rather than silently accepted as empty limits.

## [1.34.0] — 2026-10-08

### Added

- **Several GitHub Copilot accounts.** `[[copilot.accounts]]` names the logins
  `gh` already holds for a host, so a personal and a work plan can be watched
  side by side:

  ```toml
  [[copilot.accounts]]
  label = "work"
  user = "my-work-login"
  ```

  Each account resolves through `gh auth token --user <user>` — ai-usagebar
  still stores no GitHub token of its own — and gets its own tab, its own
  `--account <label>`, and its own cache. With no array the behavior is
  unchanged: whichever account `gh` has active. `show_default_account = false`
  hides that unnamed entry once every account is named.

  Two refusals rather than guesses: `GITHUB_COPILOT_TOKEN` names no account, so
  pairing it with `--account` is an error instead of reporting one account's
  quota under another's label; and a login outside GitHub's own grammar is
  rejected before it reaches the command line, so a config value can never
  arrive as a flag (#378).

- **Spanish (Español) in the Omarchy panel and the tray popover.** `uiLocale`
  gains `es`, and Auto picks it up from any `es_*` system locale. Settings →
  Appearance → Language gains Español on Windows and macOS, with a full
  `messages/es.json` catalog; metric labels and usage strings from the report
  are translated as they are for Português.

### Changed

- **The Claude tooltip's session line follows the account the module shows.**
  It reads the `sessions/` directory beside the credentials that module fetches
  its quota from, so `--account work` shows work's sessions and a Claude Desktop
  profile (`--desktop`) shows none, where 1.33.0 read `~/.claude` or the bar's
  `$CLAUDE_CONFIG_DIR` for every module. Two consequences for setups that
  differ from the default: a module with `--creds-path` or `[anthropic]
  credentials_path` reads `sessions/` next to that file, and an exported
  `$CLAUDE_CONFIG_DIR` no longer moves the default module's line, just as it
  never moved its quota. Point `credentials_path` at that directory's
  `.credentials.json`, or add it as an account, to see its sessions (#356).

- **The macOS menu bar's Name look is now called Quattro.** It draws one chip
  the way the Quattro bar does, the selected provider's logo, an optional short
  name and its highest percentage, so it takes that bar's name in Settings →
  Menu Bar → Menu Bar Shows. `[tray] menu_bar_style` is written as `"quattro"`;
  `"name"`, what 1.32.0 saved, is still read as the same look.

### Fixed

- **The macOS menu bar's compact bars follow "Show Usage As: Left".** The
  chart glyph previously mapped its bar fractions directly from used quota,
  ignoring `state.usage_reading`. With Left reading selected, the popover
  drew remaining quota while the menu bar stayed in Used mode (empty bars for
  fresh quota, solid bars for exhausted quota). The chart fractions now
  invert to remaining quota when Left reading is selected, matching the popover
  meters — including a starred balance row's bar, whose chip text keeps its
  figure the way the popover's headline does (#388).

- **The Korean Omarchy panel no longer falls back to English for two labels.**
  The `LABELS` table was checked by asserting that a single key existed, so
  Korean shipped without `Gemini` and `Claude & GPT OSS` and quietly rendered
  those two in English. Both are translated, and the contract test now requires
  full key parity across every locale — the same rule `MESSAGES` already had,
  which is why `MESSAGES` never drifted (#381).

- **The tray popover's Portuguese money footnote now actually translates.**
  `$X of $Y used` was rewritten through a pattern beginning with `\b`, which
  can never match before a `$`, so the rewrite was dead and pt-BR users saw the
  English footnote. Matching is now anchored on a line start or whitespace
  rather than a word boundary — deliberately not a lookbehind, which only
  reached Safari 16.4 while the macOS tray renders this in WebKit (#380).

- **A blank Kimi access token reports a logged-out CLI instead of an opaque
  401.** A credentials file carrying an empty `access_token` beside a future
  `expires_at` is internally inconsistent — the CLI never writes one — but the
  expiry was trusted, so `Authorization: Bearer ` went to the API. A blank
  token now counts as expired, so the refresh token gets its chance and a
  genuinely signed-out CLI says so. Completes the blank-token guard across
  providers after Claude (#370) and Codex (#373) (#382).


- **`agy` 1.3.1's prefixed missing-CSRF message still falls back to the saved
  Google session.** The CLI now answers `unauthenticated: missing CSRF token`
  instead of the historical `missing CSRF token`. The matcher treated that as
  a signed-out 401 and hid the working Cloud Code figures. Both wordings now
  take the remote fallback (#376).

- **The Plasma settings page no longer logs `ReferenceError: index is not
  defined`.** The "Current vendor" drop-down's delegate declares
  `required property`, and in Qt 6 that stops `index` being injected, so
  the `highlighted` binding threw once per row every time the page opened. The
  delegate now declares `required property int index`. A QML test opens the
  drop-down and fails on that warning.

- **A signed-out Anthropic credentials file reports 'run `claude`', not a rate
  limit.** A blank `accessToken` was sent as `Authorization: Bearer `, whose
  401/429 answer was shown as a transient error — and the 429 variant armed the
  five-minute backoff, so the card stayed wrong even after logging back in. A
  blank access token is now a credentials error with the login hint, no request
  is fired, and no backoff is armed (#370).

- **A signed-out OpenAI credentials file reports 'run `codex login`', not a rate
  limit.** A blank `access_token` was sent as `Authorization: Bearer `, whose
  401/429 answer was shown as a transient error - and the 429 variant armed the
  five-minute backoff, so the card stayed wrong even after logging back in. In
  addition, an empty `refresh_token` triggered an invalid refresh request. A
  blank access token is now a credentials error with the login hint, empty
  refresh tokens are skipped, no request is fired, and no backoff is armed.


- **macOS menu bar: wire Devin daily and weekly quotas.** The Devin CLI reports
  daily and weekly quota windows but deliberately avoids faking a 5-hour
  session on the session alias, so selecting Devin left the daily quota
  unparsed and the primary bar blank. Append `{devin_daily_pct}`,
  `{devin_daily_reset}`, and `{devin_daily_elapsed}` to the menu bar `FORMAT`
  string at slots 54-56, route the daily quota to the primary window with the
  `1d` tag and `Daily` label, and keep the weekly window on the `7d` tag and
  `Weekly` label.

- **The Claude tooltip's session line counts the right sessions.** It now
  shares `usage --json`'s reader (#356), so the two can no longer disagree, and
  with it the reader's rules. Agent SDK runs, which also write
  `kind: "interactive"` (`entrypoint: "sdk-cli"`, as `claude -p` and background
  agents such as claude-mem's observers do), no longer read as "working". A file
  named for another pid, a symlink or a file written on another operating
  system no longer counts, nor, on Linux, a zombie process. Crash leftovers no
  longer crowd a live session out of a 64-file cap taken before filtering; a
  scan now reads at most 256 files of 8 KiB. macOS checks the pid with
  `kill(pid, 0)` instead of spawning `ps` for every file on every tick, and
  Windows asks `OpenProcess` whether the process is still running instead of
  trusting any status updated in the last half hour.
  Two readings loosen with the shared reader: on Linux a session
  file without `procStart` counts while its pid is alive, and a file
  with no `kind` counts as interactive.

- **HTTPS now trusts the OS certificate store.** The HTTP client compiled in
  Mozilla's roots only, so a TLS-inspecting corporate proxy (its root installed
  in the macOS keychain or the system store) failed every vendor request with an
  opaque `error sending request`. The OS store is now consulted alongside the
  bundled roots, and transport errors carry their source chain, so the
  certificate cause (`UnknownIssuer` and friends) is visible instead of looking
  like an outage (#377).

- **A guard keeps the macOS menu bar's FORMAT mirror complete.** The bar parses
  a flat format string by field index, so a vendor whose figures are missing
  from it cannot be shown at all — three vendors in a row shipped that way
  (#372 was the third). Every vendor is now classified in an exhaustive match:
  either it has a slot of its own in the Swift mirror, or it names the generic
  placeholders it renders through. Adding a vendor therefore does not compile
  until someone decides which, and the claimed tokens are checked against both
  the Swift format string and the placeholders Rust actually emits — so a
  rename on either side fails rather than leaving the two agreeing about a
  placeholder nothing produces.

### Security

- **The tray's self-update stops reading a response once it passes its size
  limit.** The release check and the binary and checksum downloads now read
  through the same capped reader the providers use. A response without a
  `Content-Length` used to be buffered whole before the download limit was
  compared, and the release check had no limit at all.

## [1.33.0] — 2026-10-07

### Added

- **`usage --json` says which Claude Code sessions are working or waiting on
  you.** With `[context] enabled`, each Claude account whose sessions are busy
  or stopped on a permission prompt or a question gets an `Activity` row such
  as `2 working · 1 waiting`, so every panel built on the report shows it with
  no frontend change, plus an additive `activity` field
  (`{"working": 2, "waiting": 1}`) for frontends that want an icon. It is read
  from the `sessions/<pid>.json` files Claude Code keeps in the config directory
  the account's credentials are fetched from, so an account `account switch`
  moved into the default slot is read from `~/.claude` like its quota. Those
  files outlive a crashed Claude Code, so a session counts only while a process
  with its pid is running — on Linux, the same process by the start time Claude
  Code recorded, so a recycled pid does not count either — and a file written on
  another operating system never does. Agent SDK runs (`entrypoint: "sdk-cli"`,
  which `claude -p` and background agents use) are not counted. The files are
  only read; an idle account gets neither the row nor the field (#356).

- **The Claude tooltip shows live Claude Code session status.** Claude Code
  rewrites `<CLAUDE_CONFIG_DIR>/sessions/<pid>.json` whenever an interactive
  session's status changes; the widget now reads those files (read-only,
  bounded to 64 files of 8 KiB each) and appends one dim line to Claude's
  tooltip — "2 working · 1 waiting" — counting only interactive sessions
  whose process identity still checks out: `procStart` against procfs field
  22 on Linux (recycled pids included), `ps` on macOS, and a thirty-minute
  recency check on Windows where neither is available. A missing sessions
  directory renders nothing; an all-idle summary stays silent (#356).
- **`[nous] headline = "amount"` puts the Nous Research credits balance on the
  bar**, the way the prepaid-balance vendors already do, instead of the consumed
  percentage of the monthly allocation. There is no tank to state: the plan's
  monthly credits are the percentage's own denominator. The default stays
  `"percent"`, and the percentage keeps the meter, the severity colour and the
  detail line. The Omarchy panel, the KDE plasmoid and the tray popover read the
  metric's `headline` out of `usage --json` and are unaffected by the default;
  Waybar and GNOME are fed by the per-vendor formats and do not see it at all.
- **Devin CLI quota.** Reuses the official CLI's existing credential file
  read-only to show daily and weekly quota usage and the optional overage
  balance. The token is held in memory for the status request only; no login,
  refresh, or credential writeback is performed. The provider is opt-in.
- **Omarchy panel and settings in Korean (한국어).** `uiLocale` gains `ko`, and
  Auto picks it up from a `ko_KR` system locale.
- **Omarchy Quattro: switch the provider logos off.** **Show provider logos**
  in the panel Settings (`brandIcons`, on by default) draws each provider's own
  mark in the top bar and the panel hero. Off restores the generic robot icon
  the bar used before the marks: one robot for a single provider, and the
  provider's short code leading each chip's label when Show all providers is
  on. The codes are wider than the marks, so a long row can reach the clock.

- **Omarchy Quattro: choose which metrics the bar shows, and read them as used
  or left.** A new **Metrics** section in the panel Settings expands per
  provider, with one switch per metric, the way the tray's Customize does. A
  switched-off metric disappears from the panel, the top bar and the tooltip,
  per entry (each account of a provider is its own entry): Z.AI's monthly MCP
  window can be switched off while Session and Weekly stay. It is also left out
  of the highest percentage the bar and its icon alert state take, so a spent
  window you chose not to watch no longer reddens the icon or sets the number.
  The last metric still on cannot be switched off (its switch is dimmed and disabled), so
  the bar never goes blank. Cursor and Antigravity list their time windows
  there (Session, Weekly, Monthly) instead of their model pools, which keep
  their buttons on the panel: switching a window off hides every pool of it,
  and a provider with a single window, as Cursor today, has that switch locked.
  **Show usage as** in the panel Settings (`showAs` in the widget settings,
  `used` by default, so an existing bar reads as before) switches the bar, the
  tooltip, the hero and the panel meters between what is used and what is left
  of the same window. Which window the bar picks never changes with the
  reading: Left shows the remainder of the most-used window, and the alert
  state follows the used share. Both are the Quattro counterparts of the tray's
  Customize switches and Show Usage As (#340, #353).
### Changed

- macOS **Menu Bar Shows → Logos** displays one readable allowance value per
  provider instead of two tightly stacked percentages. Like Quattro's default
  auto window and the Name chip, it selects the highest used percentage across
  visible quota windows (the lowest remaining percentage in Left). A fresh short
  window no longer obscures a mostly spent monthly allowance. Hidden metrics
  stay excluded; Used/Left still follows the existing preference. Chart and
  the detailed popover keep their multiple metrics.

- **Omarchy Quattro: Antigravity gets model-pool buttons, like Cursor.** Gemini
  and Claude & GPT OSS are independent pools, so the panel gains a button for
  each (`showAntigravityGemini`, `showAntigravityClaudeGpt`, both on). The top
  bar shows one figure per pool, each the pool's most-used Session or Weekly
  window (for example `6% · 81%`), where it showed a single value before. A pool
  switched off leaves the bar, the tooltip and the panel list with both of its
  windows, and at least one pool stays on.

- **Omarchy Quattro settings fold into an accordion.** Display, Language, Top
  bar window, Show usage as, Metrics, Primary provider, Providers and Credentials (with
  the login buttons) are collapsible sections, one open at a time with Display
  open by default, so the page no longer needs a long scroll. A folded header
  shows the current value (the provider switches show `on/total`), and what you
  typed in a folded section is still saved. The Display switches are listed as
  usage value, logos, provider name, color-coding, then all providers.

### Fixed

- macOS release archives include **AI Usage.app**, with a stable bundle ID
  and a Finder icon based on the Windows tray artwork. The install guide uses
  `/Applications` so menu-bar managers can identify the tray, and documents
  Hidden Bar's macOS 27 issues with bare executables and user-local bundles.
  Standalone binaries remain available for command-line use and the self-updater.

- **A metric hidden in Customize no longer sets the provider's percentage.**
  The Native popover's provider tab and the macOS menu bar's Name chip showed
  the highest percentage among all of a provider's windows, including one
  switched off in Customize, so Z.AI with Session and Weekly at `0%` and the
  monthly MCP window hidden at `18%` still read `18%`. Hidden metrics are now
  left out, so it reads `0%` (`100%` left). With every metric of the provider
  hidden, the tab falls back to its balance or `—`, and the Name chip moves on
  to the next provider with a value, as it does for a provider with no value.
  Stars are unaffected: the Chart and Logos looks still show the starred
  metrics.
- **KDE plasmoid: grouped session rows no longer take a panel cell.** The
  compact representation's cells came from the first metric sections in report
  order, grouped or not, so a full Claude Code context session (or a SuperGrok
  product slice) could occupy the second cell beside the quota windows. The
  cells now apply the same partition the headline does: ungrouped quota rows
  first, grouped rows standing in only when an entry has nothing else
  (fixes #348).
- **macOS menu bar: the Name chip never lets a balance outrank a quota
  window.** A value-headline metric that also carries a percent (a prepaid
  balance meter, a Cursor credit grant) entered the chip's highest-window
  race on the host side only, so the chip could show its money figure while
  the Native tab that selects it showed the percent window, in either
  Used/Left reading. The chip now ranks only percent headlines, the rule the
  tab's `previewMetric` already applies, and a value headline stands in only
  when the provider has no window (fixes #349).
- **macOS menu bar shows DeepInfra's USD balance.** The Swift balance mirror
  did not include `{dif_balance}` in its FORMAT slot or balance field dispatch,
  so a DeepInfra entry rendered with no balance value. It is appended at
  index 53 (keeping every existing index stable) with a parser test.
- **Nous Research: an allocation that is spent to the last credit reports the
  rounding residue instead of a clean zero, and 1.31.0/1.32.0 rejected the whole
  account snapshot for it.** The Portal answers a drained plan with
  `subscription.credits_remaining = -1.64e-20`, `optional_credit` refused any
  value below zero ("account credit must be finite and non-negative"), and the
  widget, panel and TUI showed `Nous Research account response schema mismatch`
  for an account whose only problem was being at zero. A value within `1e-6` of
  zero now reads as `0.0`; a genuinely negative balance is still an error, so the
  guard against nonsense payloads is unchanged.
- **The tray's Global Shortcut recorder works on macOS.** The popover is a
  WKWebView, and WebKit does not focus a `<button>` when it is clicked, so the
  recorder's button-local key handler never ran and the field stayed on
  "Press keys…" for every chord. The chord is now captured on `document` while
  recording. The field also shows the canonical `Win`/`Alt` modifiers as
  `Cmd`/`Option` on macOS (the stored value is unchanged) instead of the
  Windows spelling (fixes #357).
- **Omarchy Quattro now draws Z.AI's Z mark.** The panel and top bar were
  using the older Zhipu molecule mark; they now use the same Z logo as the
  tray.
- **Omarchy Quattro: a provider with no mark no longer runs its short code into
  the value.** Command Code, which has no logo, drew its code centred in the
  one-glyph icon box, where three letters overflowed into the gap and read
  `cmd5%`. The code now leads the label with a space (`cmd 5%`) and is not
  repeated when the provider name is also on.
## [1.32.0] — 2026-10-04

### Added

- **Cursor billing-cycle pacing in the widget, tooltip, TUI and macOS menu bar.**
  Each pool (Cursor Models, Other Models) is paced against the billing cycle
  when the API states both `billingCycleStart` and `billingCycleEnd`. The widget
  gains `{cursor_elapsed}` (aliased as `{session_elapsed}` / `{weekly_elapsed}`,
  which is what places the macOS pace marker) and a per-pool pace family,
  `{cursor_auto_pace*}` and `{cursor_api_pace*}`, honouring `--pace-tolerance`,
  `--format-pace-color` and `--tooltip-pace-pts`. The tooltip marks each pool
  with its pace glyph, and the TUI and `usage --json` footnotes add the elapsed
  share and point delta for Quattro, GNOME and KDE. A cycle whose length the API
  did not state is never paced against a guessed month.

- **Cursor spending-page credits.** A visible credit grant from
  `GetClientVisibleCreditGrants` is a meter in the same form as On-Demand:
  remaining dollars beside a bar of how much of the grant is spent, with the
  expiry in the caption. The row is titled Credits, the spending card's own
  title, unless Cursor named that grant as a product credit. A Credits switch
  on the Cursor page shows or hides it on the top bar and tooltip, the same
  way as Cursor Models, Other Models, and On-Demand. The Omarchy chip adds
  that used percent after the other Cursor pools, and the hover line is that
  percent alone, the same shape as the other pools. The TUI, the
  `usage --json` report, and `{cursor_credits}` carry it too. A cache written
  before that field existed is refetched instead of being served until its TTL
  elapses. The usage bars stay up when that call fails, and an account with no
  grant shows nothing extra. A grant with nothing left stays visible at 100%
  used.

- **The macOS menu bar can show one provider like the Quattro bar.** Settings →
  Menu Bar → Menu Bar Shows gains **Name** next to Chart and Logos
  (`[tray] menu_bar_style = "name"`): a single chip with the provider's logo,
  its short name (`cld`, `cdx`, …) and its highest quota window, for the
  provider selected in the popover. Like Quattro's default window, the value
  is the highest percentage among all of the provider's windows, starred or
  not, so it matches the provider's tab: a spent weekly limit reads `100%`
  even while the 5h session reads `0%`, and Z.AI's monthly MCP window counts
  as much as its 5h and weekly ones. Until one is picked it
  follows `[ui] primary`, then the first provider with a value. The chart
  stays the default. **Show Short Name** (`[tray] menu_bar_short_name =
  false`) drops the name when the logo already says which provider it is; a
  provider with no logo keeps its name.

- **Lyceum Technology balance provider.** Lyceum's credit amounts are shown as
  USD balances; no quota percentage or reset is inferred.

### Changed

- **macOS: Claude Desktop and Claude Code account switching moved into
  Preferences.** The dropdown no longer carries two permanent **Claude Desktop ▸**
  and **Claude Code ▸** rows — noise for anyone who uses one of them or neither.
  Preferences gains a **Claude accounts** section with the same switch and
  add-account actions, and the dim `Desktop: … · Code: …` line under the header
  stays.

- **The Cursor tooltip draws each pool as a progress bar.** Cursor Models and
  Other Models were the only tooltip rows printing a bare "49% used"; they now
  use the same gauge block as every other provider (label, bar with percentage,
  reset countdown), followed by the dim line saying what the pool covers. The
  trailing "Resets …" line is gone, since each pool now carries its own.

- **The Native popover's provider tabs show the highest percentage.** A tab
  read its first window, so Z.AI with the weekly limit spent and the 5h session
  idle showed `0%`. It now shows the highest quota window, like the Quattro
  bar; grouped rows (SuperGrok's product slices, Claude's CLI sessions) count
  only when the provider has nothing else.
- **Tab and menu-bar percentages follow Show Usage As.** With the popover
  reading what is left (the default), the Native tabs and the macOS menu
  bar's Logos values and tooltip still showed what was used, so a tab read
  `18%` above meters reading `82% left`. They now show the same reading as
  the meters, the remaining share of that most-used window, and so does the
  new Name chip. The Chart look still draws usage.

### Fixed

- **Release verify-version guards AUR checksum-array parity.** When sources and
  checksums drift (e.g. adding a detached signature or tarball without matching
  checksum entries), makepkg rejects the package. The release workflow's
  `verify-version` job now fails the tag if `PKGBUILD` / `PKGBUILD-bin` source
  and `sha256sums` arrays or `.SRCINFO` entries differ in length (fixes #335).

- **Custom format placeholders escape API-controlled plan and model names.**
  Cursor, Z.AI, OpenAI, Kilo and Ollama substituted their plan or model
  placeholders into the bar text and custom `--tooltip-format` unescaped,
  which reached Waybar's Pango markup directly. Any plan or label containing
  an `&` or `<` broke the markup. The text placeholders are now escaped at
  the projection boundary, matching the established pattern in other vendors
  (fixes #333).

- **KDE plasmoid: a full Claude Code session no longer stands in for quota.**
  With `[context] enabled = true`, each recent session reaches the Claude entry
  as a grouped "Sessions" row whose percent is how full its context window is.
  The headline picked the highest percent over every row, so a session at 90%
  became the plasmoid's headline value, coloured it critical and triggered the
  alarm while the 5h and weekly quotas were low. The plasmoid now reads the
  ungrouped quota rows; grouped rows only stand in when an entry has nothing
  else (fixes #332).

- **Quota notifications no longer repeat on every refresh for Antigravity and
  MiniMax.** Both list a `Session` and a `Weekly` heading over the same pool
  labels, so two windows shared one dedupe key: a window past the threshold
  (Antigravity's weekly Gemini at 98%) notified again each time its namesake
  under the re-arm band (the session Gemini at 3%) cleared the record. Metrics
  under a heading now always include it (`Weekly · Gemini`) in the key and
  notification title, keeping the identity stable even when another window
  is absent. Metrics without a heading keep their names.

- **macOS dropdown rows line their bars up when a label is longer than 12
  characters.** Labels were padded only up to 12, so Cursor's "Cursor Models"
  pushed its bar one column right of "Other Models". Every row now pads to the
  longest label in the dropdown plus one space, and the percentages are
  right-aligned so the reset column stays straight.

- **The usage goal reads like the bar above it.** With the popover showing
  what is left, the goal still showed the share of the window that had passed,
  so a session with `7% left` and 10 minutes to go sat above `Goal now 97%`.
  The goal now follows the Used/Left reading, like the pace tick: `3%` left
  there, and the same `97%` once the bar shows what is used.

- **macOS menu-bar logos are no longer drawn upside down.** The Logos look
  painted every provider mark flipped vertically; symmetric marks hid it, but
  Z.AI's Z read as a mirrored S. Its single values also sat about a point
  below the mark; they are now centred on it.

- **macOS menu bar shows Lyceum's USD balance.** The Swift balance mirror was
  not extended when the provider registered: `{lyceum_balance}` had no FORMAT
  slot, so a Lyceum entry rendered with no value at all. It is appended at
  index 52 (keeping every existing index stable) with a parser test.

### Security

- **Subprocess environment scrubbing extended to `codex login`.** Running
  `ai-usagebar account add <label> --codex` now drops the static
  `VENDOR_SECRET_ENV_VARS` list and per-account custom `api_key_env` variables
  before spawning the interactive `codex` process, matching the scrub already
  applied to `claude` and Grok ACP subprocesses (fixes #334).

## [1.31.0] — 2026-10-03

### Added

- **Grok Bot weekly pacing in the widget and macOS menu bar.** Added elapsed
  aliases and ratio/point pace placeholders using the account's reported period,
  with configurable tolerance, placeholder colors and tooltip pace markers.
- **Grok Bot pacing details in the TUI and usage report.** Added elapsed-time
  and point-delta notes to the shared weekly metric, making them available to
  Quattro, GNOME, KDE and Linux Mint alongside the existing Windows projections.

- **Omarchy panel and settings speak English, Russian, and Brazilian Portuguese.**
  A Language dropdown (`uiLocale`: Auto / English / Português (Brasil) / Русский)
  remaps chrome, formatters, known report footnotes, and credential hints through
  a local catalog; Auto follows the system locale. Long settings copy wraps under
  the hero instead of overflowing the detail pill, and API-key notes wrap on their
  own line under the env var name.

- **Korean (한국어) in the tray popover.** Settings → Appearance → Language
  gains 한국어 on Windows and macOS, with a full `messages/ko.json` catalog;
  metric labels and usage strings from the report are translated as they are
  for Português.

- **OpenRouter: recent models activity and real-dollar credit balance.**
  `GET /api/v1/activity` queries the 2 most recently used models, showing their
  per-model cost and request counts alongside the existing spend breakdown in both
  the TUI and the popover. When no quota reset date is published, the meter row
  displays the account's available credit balance in USD ($) directly below the
  gauge. Full Portuguese (pt-BR) localization support in the popover. The
  activity endpoint requires an OpenRouter *management* key
  (`management_api_key_env`, default `OPENROUTER_MANAGEMENT_API_KEY`, also per
  `[[openrouter.accounts]]`); without one the activity request is skipped and
  the block simply stays hidden.

### Fixed

- **Ollama Cloud monthly-only accounts no longer paint a fake 0% 5h/7d pair.**
  Some Pro accounts report `limits.monthly` instead of `session`/`weekly`.
  The TUI, tooltip and `usage --json` already showed that month; the widget
  default format and the `{session_pct}`/`{weekly_pct}` aliases still emitted
  `0` for the omitted windows, so Waybar and the macOS menu bar read as two
  exhausted rate-limit windows. Absent windows are now empty placeholders (a
  present month at 0% used still renders `0`), the default bar shows
  `{oll_monthly_pct}%`, and the macOS selector draws that pool on the primary
  bar as Monthly.

- **Omarchy bar chips keep their icon next to their own value.** 1.30.0 put a
  6 px spacer between a chip's brand mark and its value inside a row that
  already spaces its children 4 px apart, so the gap became 14 px — wider than
  the gap to the previous chip, and each icon read as part of the chip before
  it. The spacer is gone; the row's spacing is the gap again, and an icon-only
  chip still collapses to the mark alone.

- **macOS menu bar parses DeepInfra named accounts.**
  PR #291 added named account support to DeepInfra in Rust, but
  `API_KEY_ACCOUNT_VENDORS` in the macOS menu bar was not updated. It now
  includes `deepinfra` so `[[deepinfra.accounts]]` entries appear as menu
  choices and in Preferences.

- **Provider catalog and detection recognize named accounts and path overrides.**
  `ai-usagebar vendors --json` and the TUI/macOS provider views reported
  providers as unconfigured ("needs credential") when authentication was
  configured via named accounts (`[[<vendor>.accounts]]`) without setting the
  ambient key, or when `show_default_account = false` was set. The catalog now
  checks named API-key accounts as well as Anthropic and OpenAI named accounts
  and path overrides (`credentials_path`, `codex_auth_path`), matching the
  credential resolution of the fetch and `detect` (see #307).

- **Saving settings no longer switches a disabled primary provider back on.**
  A key provider that was still `[ui] primary` after being switched off (from
  the overlay's provider switches or by hand) stayed the selected primary
  whenever it kept a key, inline or exported, so the next save from the TUI
  overlay or the Omarchy settings panel wrote its `enabled = true` back,
  whatever the edit was. A disabled primary now shows the first enabled
  provider instead, as Copilot and keyless providers already did; picking a
  provider explicitly still switches it on.

- **Grok Bot reuses the token pair it refreshed.** After a 401 the widget
  refreshes the desktop app's session and saves the new pair in its own
  `oauth.json`, but it saved it under the fingerprint of the rotated refresh
  token, while the next poll looks it up by the app's sign-in. Once the token
  rotated the pair was never found again: every poll retried the expired
  access token and refreshed with the original refresh token, which a server
  that enforces rotation rejects. The pair is now keyed by the sign-in it was
  refreshed from, as Kiro and Antigravity already do.

- **Kiro no longer asks for a new login when the network drops during a
  token refresh.** A refresh that could not reach the token endpoint was
  reported as a credentials error ("Run `kiro-cli login` again") and recorded
  as the last refresh error, so a laptop waking up offline with an expired
  access token showed a sign-in warning for a login that was fine. Network
  failures now take the same silent cache fallback as the usage call; a
  refresh the endpoint rejects still asks for a new login.

- **Antigravity's custom formats no longer break Waybar's markup.** The
  documented `{scoped_model}` and `{extra_model}` placeholders carry the
  third-party pool's name, "Claude & GPT OSS", and the bar text and a custom
  `--tooltip-format` substituted it unescaped, so any format naming the pool
  handed Waybar a bare `&` inside its markup. The pool names and the plan
  label are now escaped in both, as Kiro already does for its plan.

- **SuperGrok no longer shows an authentication failure's response body.**
  When a refresh failed with a 401 or 403 and a cached figure was shown
  instead, the cache recorded the neutral authentication message but the
  outcome kept the raw response body, so the Waybar tooltip, the TUI and
  `usage --json` displayed it on every poll that refetched. The outcome now
  carries exactly what the cache recorded, as the other vendors already do.

- **Grok reports a rejected management key under its HTTP status.** Without
  a `team_id`, every refetch first validates the key, and an HTTP error from
  that step was recorded as a generic error with code 0. The response body of
  a 401 or 403 therefore reached `.last_error` and the TUI and report
  warnings, the Waybar tooltip showed the stale balance without any error,
  and a 429 never armed the five-minute backoff. That step is now recorded
  like the balance call: under its status, with an auth failure's body
  replaced by the neutral message.

- **`~` now works in `[commandcode] auth_paths` and `[copilot] gh_binary`.**
  Every other path setting expands a leading `~` when the config loads, but
  these two kept it literally. Uncommenting the documented
  `auth_paths = ["~/.commandcode/auth.json"]` made Command Code report "not
  signed in" for a signed-in user, and `gh_binary = "~/bin/gh"` made Copilot
  report that the GitHub CLI is not installed.

- **The in-tree `ai-usagebar-bin` PKGBUILD builds again.** Since #282 each
  architecture downloads a tarball and its detached `.sig`, so it needs two
  checksums, but the v1.29.0 version bump reset `sha256sums_x86_64` and
  `sha256sums_aarch64` to a single `'SKIP'` (and `.SRCINFO-bin` to one line
  each), which makepkg rejects as an array that differs in size from its
  sources. Packages published by the release workflow were unaffected,
  because it rewrites both arrays with two entries; local builds and the
  manual AUR fallback were not.

- **A config whose only inline key is Ollama Cloud's is tightened to `0600`.**
  On Unix, a config holding an inline credential is made private when it is
  loaded, but `[ollama] api_key` was missing from the list of fields that
  triggers it, so such a file kept whatever mode it was created with,
  typically readable by every local user. It now gets the same protection as
  every other provider's inline key.

- **A key pasted into `api_key_env` is no longer repeated in errors.** A value
  that is not an environment variable name is most likely the key itself in
  the wrong field, and the shared resolver already refuses to echo it, but
  two messages still did: Kimi's "no credentials" error, when no Kimi Code
  CLI login exists either, and the `[[custom]]` validation error, which fails
  the whole config load. Both reached the widget's tooltip, `usage --json`
  and the TUI. They now name `api_key_env` without its value.

- **Named accounts' key variables no longer reach other tools' processes.**
  The `gh`, `grok`, `agy` and `claude` processes ai-usagebar starts get every
  provider key variable removed from their environment, but that list held
  only the default names and `[[custom]]` variables. A key read from a named
  account's variable (`[[deepseek.accounts]] api_key_env`), from a renamed
  `api_key_env`, or from an OpenRouter `management_api_key_env` other than the
  default was passed to all of them; these are now removed as well.

- **Linux Mint: the tray menu's summary reads the quota, not a Claude Code
  session.** With `[context] enabled = true`, the Claude entry's metrics also
  list each recent session with how full its context window is, and the
  menu's one-line summary showed the highest of them all, so a session at 90%
  read as "Claude: 90%" while the 5h and weekly quotas were low. The summary
  now picks among the quota rows; grouped rows only stand in when an entry
  has nothing else.

- **Omarchy: a full Claude Code session no longer stands in for the Claude
  quota.** With `[context] enabled = true`, each recent session reaches the
  Claude entry as a grouped "Sessions" row whose percent is how full its
  context window is. The bar picked the highest percent over every row, so a
  session at 90%, even one from yesterday, became the Claude chip's value,
  coloured it critical and turned on the bar's alarm while the 5h and weekly
  quotas were low. The bar now reads the quota rows; grouped rows only stand
  in when an entry has nothing else.

- **The documented Windows config file is the one the binary reads.** On
  Windows the default config is `%APPDATA%\ai-usagebar\config\config.toml`,
  but `--help`, the README and three docs pages named
  `%APPDATA%\ai-usagebar\config.toml`, and the PowerShell snippet in
  `docs/windows-build.md` created that file. Nothing reads it, so a config set
  up from the docs was silently ignored. They now name the real location, as
  `windows/README.md` already did.

- **Provider catalog recognizes Anthropic named accounts backed by Keychain.**
  On macOS, an Anthropic named account configured via `[[anthropic.accounts]]`
  or discovered via `accounts_dir` whose credentials exist only in its
  `CLAUDE_CONFIG_DIR`-scoped Keychain item reported `configured: false` in
  `ai-usagebar vendors --json`, even though the fetch and `detect` resolved it.
  The catalog now probes the account's Keychain item when no on-disk credentials
  file exists (fixes #329).

### Security

- **A broken `config.toml` no longer has its offending line quoted back.**
  TOML parse errors quote the line the parser stopped on, and the commonest
  mistake, a missing quote, is often on an inline `api_key` line, so the key
  itself reached the widget's tooltip, `usage --json` (and with it every
  desktop frontend), the TUI, the Settings overlay and stderr. Config parse
  errors now give the line and column with the parser's message, never the
  line's content.

## [1.30.0] — 2026-10-01

### Added

- **Meta Muse (Muse Spark) evaluation and `[[custom]]` local-spend recipe.**
  `docs/vendor-endpoints.md` records why Muse is not implementable as a native
  vendor (Meta publishes no quota or billing endpoint; the dashboard's private
  GraphQL route needs a browser session), and `config.example.toml` gains a
  commented recipe that tallies Muse Code's local session logs through a
  loopback `[[custom]]` provider.

- **The Omarchy bar dims the chips the open panel is not showing.** While the
  panel is open, the chip for the entry it displays keeps its colour and the
  others step back to 45% over 140 ms, so the bar says which entry the panel
  belongs to, next to the shell's own underline under the widget. An alarming
  chip that is not the selected one dims as well, so its red reads softer for as
  long as the panel is open; the lone, vertical and loading placeholders never
  dim, and the chip under the pointer, its click target and the tooltip are
  untouched.

- **Omarchy can color-code usage by level.** The Quattro bar chips, panel meters,
  and hover tooltip can paint green → yellow → orange → red as usage climbs,
  reading those colours from the active Omarchy theme (`colors.toml`) with
  Quattro's urgent colour for the critical rung. A new **Color-code usage by
  level** display toggle (`colorCodeUsage`, off by default) turns the palette
  on or off immediately; when it is off, everything stays on the normal
  foreground colour and the classic alarm chrome (#278) still fires for a
  critical quota. Theme key precedence matches Waybar/`theme.rs` after #289
  (named `red`/`green`/`yellow` win over `color1`–`color3`).

### Fixed

- **GNOME: the top bar no longer goes blank when every window is hidden.** With
  both the 5h and weekly bars switched off, the indicator drew an empty label
  and left an invisible click target in the panel. It now shows the top-bar
  vendor's symbolic icon instead, and the click menu works as before.

- **`vendors --json` no longer reports Command Code as configured just because
  pi's shared keystore exists.** The catalog treated any file on Command Code's
  auth search list as a login, but the second path is pi's keystore for every
  provider the user signed pi into — so a machine with pi and no Command Code
  login still showed the provider as having the credential it needs (and the
  macOS preferences as "credential available"). Command Code is configured only
  when one of those files holds its live credential, the same answer detection
  and the fetch already give, and the check now honors a configured
  `auth_paths` override.

- **Cached quotas stay marked stale during HTTP 429 backoff.** An expired
  payload served while requests are paused no longer appears fresh in the
  report and desktop frontends.
- **Linux Mint tray polls quota endpoints every five minutes.** The previous
  one-minute interval could trigger Claude and Codex rate limits.

- **AUR `ai-usagebar-bin` installs again: the release signing key is now on a keyserver.**
  The v1.29.0 key (`AE42EF5D73DD92E248815C95B65CCCAF64A99438`) was only shipped
  as a release asset, so `makepkg`/`yay` could not fetch it for `validpgpkeys`
  and the install aborted. The public key is now published to keys.openpgp.org
  (served by fingerprint; a confirmation email makes it searchable by address).
  The v1.29.0 release notes' `AA`→`CA` fingerprint typo is fixed in the release
  body; the changelog's released [1.29.0] section keeps the original text
  because released sections are immutable (#301).
- **Omarchy's open-panel underline spans the full chip width.** Quattro's bar
  paints the active-plugin mark at ~55% of the slot unless the widget hints
  otherwise; the AI Usage chip now reports its full width so the underline
  tracks Cursor's multi-percentage label as indicators come and go.

## [1.29.0] — 2026-09-30

### Added

- **AUR `ai-usagebar-bin` package verifies detached PGP signatures against `validpgpkeys`.**
  Following upstream release signing introduced in v1.28.0 (#257), `packaging/aur/PKGBUILD-bin`
  and `.SRCINFO-bin` now declare maintainer key `AE42EF5D73DD92E248815C95B65CCAAF64A99438`
  in `validpgpkeys` and fetch detached `.sig` signatures alongside each architecture's
  binary archive (`source_x86_64` and `source_aarch64`), allowing `makepkg` to automatically
  verify release integrity and authenticity (#282).
- **Native DeepInfra billing support.** The widget, TUI, aggregate report, and
  named API-key accounts now read `DEEPINFRA_API_KEY`, combine the documented
  billing checklist and current-month usage endpoints, convert usage cents to
  dollars, and show prepaid balance, monthly spend, optional limit, and period.
- **The Omarchy bar's per-provider chips open that provider.** With **Show all providers** on, a left-click on a chip selects the entry that chip stands for and opens the panel there, the way the panel's own provider buttons do, instead of toggling the panel on whatever was selected last. Clicking the chip the panel already shows closes it; right-click and middle-click keep their panel-wide meaning, and hovering a chip still shows the button tooltip.
- **Multiple Antigravity CLI accounts on macOS.** The optional `agy` status-line
  integration adds one live usage entry per distinct active Google account,
  deduplicates repeated sessions, and displays only a masked email with an
  opaque stable account ID. Active sessions are marked stale after 15 minutes
  without a new status-line payload. It does not read or store OAuth tokens and
  falls back to the existing Antigravity collector when no valid status-line
  session is active.

### Changed
- **The meter colour and the flame follow the pace line, with a tolerance.**
  Any row the least bit over the pace tick was red with a "Limit in …" flame,
  so a weekly Claude row at 4% used seven hours into its week warned of a
  run-out 7 hours before the reset, and a row at 98% left read like one that
  needed attention. The verdict now allows for noise: up to 110% of the pace
  line is blue with no flame ("~N% spare" or "~N% left at reset"); 110–130%
  is yellow with "~N% over pace" and still no flame; over 130%, or over the
  line with under 10% left, is red with the flame and "Limit in …". Each band
  also needs the bar to sit past the tick by 3 points (yellow) or 5 points
  (red), because early in a long window one whole percent of use swings the
  projection by twenty points or more. Before a window has a projection the
  colour reads what is left (blue, yellow under 50%, red under 20%). The same
  in Left and Used mode, in the tray popover and in the Linux Mint tray.

### Fixed
- **The macOS Z.AI row says “MCP tools” without the monthly suffix.**
  The suffix made the row wider than the other usage rows, while the reset
  countdown already shows the length of the quota window.
- **Account CLI commands sanitize filesystem paths and account labels in terminal output.**
  Terminal output from `account add`, `account switch`, and `account merge-history`
  previously interpolated raw `.display()` paths and unsanitized labels directly
  into `println!` and `eprintln!`, violating the project invariant that untrusted
  text is sanitized at the sink and risking ANSI escape sequence injection into
  the terminal. Paths now route through `sanitize_untrusted_path`; errors,
  capture notes and config parse messages through one `printable` helper. A
  guard test fails on any `account` print that interpolates `.display()`,
  `{error}` or `{note}` bare.
- **`account merge-history` synchronizes under `account_switch_lock`.**
  Running history merges now acquires the profile switch lock before staging
  and merging, preventing race conditions and potential profile corruption
  when concurrent switches or refreshes target the same profile.
- **Antigravity keeps reporting with only the `agy` CLI installed.** The saved
  Google session lasts about an hour and only a running Antigravity renews it;
  with the desktop app closed nothing did, so the widget fell to "session
  expired and ai-usagebar has no OAuth client" until the user ran `agy`
  themselves. When the session is expired and no OAuth client is configured,
  the fetch now runs `agy models` (no TTY, no prompt, read-only; it rewrites
  the saved credential as a side effect) and reads the credential again. `agy`
  is found on `PATH`, then in `~/.local/bin`. The run is bounded to 25 seconds
  and attempted at most once every ten minutes, failures included, so a dead
  refresh token never turns into a spawn per poll, and the spawn never
  inherits this process's provider key env vars. `agy`'s own background
  updater is switched off for that run (`AGY_CLI_DISABLE_AUTO_UPDATE=true`):
  on Windows it opens a console window of its own that no flag on our spawn
  can hide. Configuring
  `oauth_client_id` and `oauth_client_secret` still refreshes directly and
  never spawns anything.
- **Cursor is detected from `cursor-agent` alone on macOS.** The CLI keeps its
  login in the login Keychain (`cursor-access-token`, account `cursor-user`)
  rather than in an `auth.json`, so a Mac with the CLI and no desktop IDE had
  neither credential source and Cursor read as signed out. The Keychain is now
  the third source, after the IDE's `state.vscdb` and the agent's `auth.json`,
  and is only read when the IDE database does not exist and the agent path is
  the default one.
  The agent file's default location on macOS was also wrong: `cursor-agent`
  writes `~/.cursor/auth.json`, not `~/Library/Application Support/cursor/`, so
  the file fallback could never be found there. The default now follows the
  CLI's own per-OS path (Linux and Windows are unchanged).
- **Popover error messages keep their path.** The card removed absolute paths
  from a diagnostic, but only up to the next space, so on macOS "Cursor
  database not found at ~/Library/Application Support/…" became "not found at
  Support/Cursor/…", and on Windows the path vanished and the sentence read
  "not found at Open the Cursor IDE". The path now stays whole, with only the
  home prefix (`/Users/<name>`, `/home/<name>`, `/root`, `C:\Users\<name>`)
  folded to `~`, so the account name still stays off the card. A diagnosis that
  is nothing but a path still falls back to "Open TUI for details".
- **The update banner says "Updating…" once.** Clicking Install put the same
  "Updating…" on the button and in the sentence above it, and the download
  and install that followed repeated each step in both places too. Progress
  now shows on the button only; the sentence keeps naming the release
  ("AI Usage vX.Y.Z is ready to install.") until it is done, and a failure
  still explains itself there.
- **The Omarchy palette is read from where Omarchy applies it.** `omarchy-theme-set` writes the active theme to `~/.local/state/omarchy/current/theme`, while the TUI and the widget looked in `~/.config/omarchy/current/theme`, a layout Omarchy no longer populates. The lookup came up empty, the One Dark fallback was silent, and every themed surface stayed One Dark — the older path is now the fallback rather than the only candidate. Theme files that name their colors (`red`, `green`, `yellow`) are also read: only the pre-Omarchy-4 `color1`-`color3` aliases were parsed, so a current theme file would have overridden the foreground and background but left every severity color at One Dark.
- **The Omarchy bar's chips answer a click anywhere in their column.** The bar
  presses a slot's widget by geometry, so a chip only won a press inside its own
  rect: the glyph sat 12 px tall in a 26 px slot, and a press on the padding
  above or below it fell through to the button, which toggled whichever entry was
  already selected instead. Each chip now registers its whole column of the slot
  — the full height, half of every gap beside it, split at the midpoint with its
  neighbour, and the button's padding at either end of the widget — so the row
  is a partition of the slot rather than glyphs
  floating in a button. The row's width and each chip's place in it are
  unchanged.
- **A quota notification no longer repeats while nothing changes.** The dedupe
  recorded each window's reset instant and re-armed the key whenever a later
  fetch reported a later one. Vendors report that instant with sub-second
  precision that drifts between fetches — Anthropic's five-hour window came back
  0.70s apart on two fetches four minutes apart — so a window sitting above the
  threshold re-notified on a good share of refreshes. A move now has to clear an
  hour and a half: longer than any refresh interval this ships with, and far
  shorter than the shortest window, so a window that really rolled over still
  notifies.

## [1.28.0] — 2026-09-29

### Changed

- **Cursor on-demand in `usage --json` is numeric.** The On-Demand text row
  carries `used_cents`, `limit_cents`, and `percent` (USD cents and the
  consumed percent) when Cursor reports a prepaid cap. The Omarchy chip and
  panel meter read those fields. The formatted `$spent / $cap` value is
  unchanged for every other surface. A report from an older binary, which has
  only that formatted value, still works.
- **The Omarchy bar no longer turns red for a cached or failed refresh.** That alert state now follows the highest-percent window alone, like the Waybar `class` and every other frontend; stale and error text stays in the panel. A refresh that yields no report at all still marks the bar. Thresholds are unchanged.

### Fixed

- **Omarchy panel scrolls long settings forms faster.** Touchpad gestures, mouse wheels, and keyboard steps now cover more of the popup per movement, so the Save button remains reachable without dozens of gestures.
- **Grok Bot reads its session on Linux when the app used Chromium's
  `"peanuts"` key.** The Grok Bot desktop app picks its OSCrypt key at runtime
  from whichever Secret Service backend Electron selected, and encrypts with
  `"peanuts"` whenever that backend is `basic_text`. A machine can therefore
  hold an `application="Grok Bot"` keyring item while the blobs in
  `sand-secrets.json` were keyed with `"peanuts"` — and the reader preferred
  the item's key, so every token failed to decrypt and the bar reported
  "a stored token could not be decrypted; sign in to the Grok Bot desktop app
  again" for a session that was perfectly valid. Both keys are now tried, most
  specific first; a wrong AES key almost always fails PKCS#7 unpadding, so the
  right candidate is effectively ruled in. No configuration, re-login or
  keyring change is needed, and a genuinely unreadable file still reports the
  same error it always did.

## [1.27.0] — 2026-09-28

### Added

- **`account merge-history` for relocated Claude Desktop profiles (macOS).**
  Merges every account's sessions and schedules into whichever account a given
  profile is signed into, without swapping a credential or touching the app:
  `ai-usagebar account merge-history --data-dir <DIR> [--from <DIR>]...`.
  Intended for side-by-side Desktop copies launched with `--user-data-dir`,
  where `account switch` cannot be used because it installs a stored token
  over the profile's live login and quits the app by application name. The
  merge is additive — no deletion sweep runs, so an unattended run cannot lose
  history — sources are opened read-only, and a second run is a no-op. Note
  that it deliberately crosses accounts: afterwards one account's window lists
  conversations started under the others.
- **GNOME menu supports all enabled providers.** Native submenus display
  the shared usage report, including Cursor, named accounts and custom
  providers, with metric labels, balances, errors and reset details supplied
  by the binary. Collapsed rows preview the first two metrics in report order,
  retaining their labels and groups, with optional mini bars and symbolic
  provider icons. Menu preferences offer values only, hidden icons and compact
  spacing. The top bar continues to follow its vendor preference.
- **Omarchy bar shows both Cursor pools and prepaid on-demand.** The Quattro
  chip lists Cursor Models, Other Models, and on-demand used percent in that
  order (`35% · 7% · 0%`), the same consumed-percent reading OpenRouter uses
  for a credit balance. The tooltip is one short line per pool. Three switches
  on the Cursor page turn those figures on and off in the top bar and tooltip
  only; the open panel still lists every pool, and the last remaining figure
  cannot be turned off. A pool the report does not contain, such as on-demand
  with no prepaid row, does not count as that last figure. The bar's urgent
  color follows the pools still on the chip.
- The TUI vendor menu is now navigated with the Up/Down arrow keys (wrapping),
  with `Tab`/`Shift+Tab`/`←`/`→`/`h`/`l` kept as secondary shortcuts. Mouse
  clicks work in the TUI: click a vendor menu entry to select it, click a
  footer action to refresh, refresh all, open Settings, or quit, click a
  Settings field to focus it, or click **Save** to save. In Settings the
  on/off cells toggle their provider (or the quota-alerts switch) and the
  focused Primary vendor's ◀/▶ arrows step the radio; the hint line is
  clickable too: save, close, toggle, reveal and change-vendor segments send
  their key through the same handler.

### Changed

- **Right-clicking the tray icon opens the Options menu.** On macOS a
  right-click on the menu bar item no longer opens the popover like a
  left-click: it shows the footer's Options menu as a native menu —
  Customize (Classic only), Settings, Refresh, Detect Providers, Open TUI,
  Start at Login, Check for Updates…, About, Quit — in the popover's
  language, and the items that name a screen open the popover on that
  screen. The Windows right-click menu, which had only Refresh, Detect
  Providers, Open TUI, Start with Windows and Quit, is now the same menu.
- **Refresh lives in the Options menu only.** The ↺ button in each provider
  card's header was Reset, not Refresh: one click threw away that
  provider's row order and visibility. It is gone from the dashboard (Reset
  stays in the provider's Customize screen, behind a second click), and the
  row menu's per-provider Refresh went with it; Options → Refresh updates
  every provider.
- **Quit in the Options menu is no longer red.** It uses the same color as
  the other items.

### Fixed

- **GNOME Shell 45–46 compatibility.** Vertical menu rows now use the
  layout property available in the running Shell, avoiding the unsupported
  `orientation` property on older versions. The extension had failed to
  enable on Shell 45 and 46 since it first shipped (#272).
- **GNOME preferences display literal labels correctly.** The pool description
  and colour labels no longer treat `&` and `<` as markup.

## [1.26.0] — 2026-09-27

### Added

- **Named accounts for every API-key provider.** The `[[openrouter.accounts]]`
  array (#221) now works for `[zai]`, `[deepseek]`, `[kilo]`, `[novita]`,
  `[moonshot]`, `[grok]`, `[minimax]`, and `[orcarouter]`: one entry per extra
  key, each with its own TUI tab, `usage` report entry (`deepseek@work`),
  macOS menu choice, and `<vendor>/<label>` cache, selected in the widget with
  `--vendor <vendor> --account <label>`. The section's existing key stays the
  default account and its cache path does not move; `show_default_account`
  hides it once every key is named. Labels follow OpenRouter's rules — no path
  separators, no duplicates, a key source per entry — and an unknown label
  fails instead of falling back to the default key. Region, team,
  organization, and display settings stay per provider, which the new
  [API-key account guide](docs/api-key-accounts.md) spells out. Kimi is left
  out: its fallback is the Kimi Code CLI's single OAuth login. Existing
  configs need no change.
- **Scoop installs update through Scoop.** Since 1.25.0 a Scoop-installed
  Windows tray only offers the release page; now Install Update (and
  Automatic) hands off to `scoop update <app>`, quits while Scoop replaces
  the tray (usually 10–60 seconds) and comes back by itself through Scoop's
  `current` folder. Success is Scoop's `current\manifest.json` reaching the
  new version, not an exit code. The Scoop transcript is at
  `%LOCALAPPDATA%\ai-usagebar\updates\scoop.log`; if Scoop does not deliver
  the requested version, the tray reports that log path, and Automatic does
  not retry it in the background. A global Scoop install without the
  `scoop.ps1` shim keeps the release page.
- **Linux Mint Cinnamon tray frontend.** An experimental GTK dashboard reads
  the existing `usage --json` report, shows provider quota groups with the
  icon marks already shipped for Omarchy, color-coded pacing bars, a flame
  warning when projected usage exceeds the limit, and concise disconnected
  cards. It opens from the native status icon, hides on focus loss, stays out
  of the taskbar, and offers settings and refresh actions beside each provider.
  The frontend supports English and Portuguese and retains cached quota rows
  when authentication temporarily fails. The installer recognizes the Cargo
  installation in `~/.cargo/bin` and retains explicit binary paths for
  autostart after it exits; the desktop launcher's TUI action uses the
  detected TUI path.
- **Release artifacts are PGP-signed when a signing key is configured.** The
  release workflow now signs every tarball, zip, and bare binary with a
  detached ASCII signature (`*.sig`), attaches the public key as
  `ai-usagebar-signing-key.asc`, and adds a verification section with the
  key's fingerprint to the release notes. This is opt-in at the repo level:
  nothing changes until the `GPG_PRIVATE_KEY` (armored secret key) and
  `GPG_PASSPHRASE` secrets are set; releases cut without them are
  byte-for-byte what they were before (#257).

### Fixed

- **Grok Bot on Linux when the sign-in password is in the Secret Service.**
  Chromium tags those `sand-secrets.json` tokens `v11`. The ciphertext is the
  same AES-128-CBC envelope as the `v10` peanuts fallback; decryption rejected
  the tag, so the card stayed on "a stored token could not be decrypted".
- **OrcaRouter from the macOS menu bar.** The menu bar selects a provider with
  `--vendor <slug>`, but the widget only accepted OrcaRouter as `orca-router`,
  so `--vendor orcarouter` was rejected and the entry never fetched. The
  widget now takes the slug; `orca-router` stays accepted as an alias.
- **`detect` counts a named API-key account as a credential.** A provider whose
  keys all live in `[[<vendor>.accounts]]` — OpenRouter included — was treated
  as unconfigured and never switched on, although each named key has a tab of
  its own.
- **The tray popover no longer logs a 404 for `/favicon.ico`.** The page had
  no icon, so the WebView asked the tray's custom protocol for
  `/favicon.ico` on every open, and the console showed a failed request. The
  page now declares an empty icon (`data:,`), so nothing is requested.

## [1.25.0] — 2026-09-25

### Added

- **Claude CLI sessions in the report and popover (#255).** When the opt-in
  `[context]` monitor is enabled, the usage report's Claude entry gains a
  `"Sessions"` group — up to eight recent Claude Code sessions, one row each,
  with context health on the same severity colours as quota meters (a 90%
  context reads as saturated), plus the model, token count and last-active
  time; compacted or unreadable contexts keep an honest `compacted` /
  `unknown` value instead of a fabricated percentage. The Windows and macOS
  popover renders them as grouped rows on the Claude card, the Omarchy panel
  under a "Sessions" heading, and `usage` prints them in the text report.
  Sessions are machine-local, so they attach to the first ready Claude entry
  exactly once, never per account; with `[context]` disabled nothing changes.
  The reporter's suggested icon beside the options/refresh buttons is a
  follow-up — the card section is the first slice. The TUI keeps its dedicated
  `c` overlay.
- **OpenRouter across multiple workspaces (#221).** The existing
  `[[openrouter.accounts]]` entries (v1.3.0) are now documented for the
  reporter's setup — one entry per workspace key, so each workspace gets its
  own tab, report entry, and cache. The docs state the split's limit honestly:
  keys created inside one OpenRouter workspace share that workspace's billing
  account, so entries separate login sessions (workspaces), not keys within a
  single bill.
- **Grok Bot on Windows.** `[grokbot]` read the desktop app's session only on
  Linux and macOS and failed closed elsewhere. On Windows it now reads
  `%APPDATA%\Grok Bot\sand-secrets.json`, whose tokens are Chromium's Windows
  `v10` values (AES-256-GCM), with the key from the `Local State` beside it,
  unprotected by DPAPI for the signed-in user. Both files stay read-only;
  refreshed tokens still go only to ai-usagebar's own cache. Chromium's newer
  app-bound `v20` encryption is refused with an error that says so.
- **The macOS tray updates itself.** Releases now also publish the tray, CLI
  and TUI for Apple Silicon and Intel Macs, as a tarball and as bare binaries
  with `.sha256` sidecars. The tray checks once an hour (Settings → Updates:
  Automatic, Notify me or Off), downloads the binary for its architecture,
  verifies it, swaps it in place and relaunches — nothing is compiled on the
  user's machine. The CLI and TUI are replaced only when they already sit
  beside the tray, so an update never drops a new executable into a `PATH`
  directory that could shadow a `cargo install` copy.
- **Options → Check for Updates opens a dialog over the current screen**:
  Checking, then You're Up to Date, Update Available with Install, or the
  reason it failed with Try Again. Settings → Check Now opens the same dialog.

- **Popover Style: Classic or Native, on macOS and Windows** (Settings →
  Appearance → Popover Style). Classic is the app's own card layout, the same
  everywhere; Native follows the system: v1.23.0's glass dashboard over AppKit
  glass on macOS, and Windows 11 Fluent over Acrylic on Windows (a solid panel
  on Windows 10). Both draw the same provider card, so collapsing, the reset
  popover, the row menu, pace notes, errors and account switching work in
  either; Native shows one provider at a time behind tabs of logos and
  percentages. Classic keeps each host's width (320 pt on macOS, 300 on
  Windows); Native is 390.
- **The macOS menu bar can show logos** (Settings → Menu Bar → Menu Bar
  Shows: Chart or Logos): each provider's logo followed by the values of the
  metrics starred in it, two starred metrics stacked. Both looks show exactly
  the starred metrics.
- **The usage goal works in both styles**; in v1.23.0 only the glass
  dashboard drew it.
- **Text cut short shows the full value on hover**, and picker values are
  capped so a long one no longer pushes its label out.
- **Per-provider on/off switches in the settings surfaces (#244).** The
  terminal Settings overlay grew a Providers section — one on/off row per
  known vendor — and the Omarchy settings form a Providers section of
  toggles, both writing `enabled = true/false` under the vendor's own
  config.toml section through the same comment-preserving save path. Only
  toggled providers are written, so an untouched save adds no section; an
  explicit off in the same save wins over the enable-a-pasted-key rule; and
  the switch names built-in vendors only (the slug is validated against the
  vendor list before anything is written, on both the TUI and the native
  stdin patch). Defaults are unchanged — this is only the switch. The
  overlay's body now scrolls to follow focus, keeping Save reachable with
  every provider listed.

### Changed

- **Grouped rows on the Windows/macOS popover and menu bar now carry their
  group in the row key.** A metric that names its group in the report
  (SuperGrok's "Breakdown" slices, the new "Sessions" rows) used to be keyed
  without it, so the popover and the menu-bar strip could disagree with the
  Omarchy panel's rendering. Both now label and key such rows exactly as a
  positional heading would; a SuperGrok slice starred in an older build needs
  re-starring once.
- **The Grok Bot card names the subscription that bills it** — "Cursor
  Ultra" — instead of the app's own "Grok Bot Plan", which reads the same on
  every account (the popover trimmed it to a bare "Plan"). It comes from the
  usage response's `billingBrand` and the plan it reports; a brand not seen
  yet shows the old label rather than a guess. `{gbt_plan}` is unchanged.
- **The About screen says what AI Usage is** and links the source code,
  release notes, issue tracker and license. It no longer repeats the version
  from the footer or hosts the update check.
- **An update the tray cannot install offers its release page.** A release
  without a build for this OS and architecture, an install directory the tray
  cannot write (a root-owned `/usr/local/bin`), or a copy another tool owns —
  Homebrew, Nix, a link into place, cargo's `target/` directory — shows View
  Release instead of an Install button that could only fail or would fight the
  tool that installed it.
- **A meter too early in its window for a pace estimate says "Estimating…"**
  (with Always Show Pacing on), and explains on hover when the pace appears.
  The estimate now waits 1% of the window but never more than an hour, so a
  weekly or monthly meter no longer sits blank for 1h 41m or 7h 12m.

- **The macOS popover opens in Classic again**, the layout it had before
  v1.23.0; the glass dashboard is one choice away as the Native style. A new
  look now ships as a style instead of replacing the one people use
  (`CONTRIBUTING.md` → Changing the tray popover UI).
- **The macOS menu bar shows the chart by default again**, and the text
  summary's options are gone: provider names, Show All Providers, Hide Usage
  Value, Usage Window, Focused Provider and the middle-click provider
  cycling. The stars already choose which providers and which quota windows
  appear, so these either repeated that choice or overrode it. Their old
  `config.toml` keys are ignored, not rejected.
- **Every popover screen is translated**, the update dialog, error hints and
  update messages included, and the language list reads English and
  Português. Strings are Paraglide JS messages now, so a missing translation
  fails the build instead of falling back to English.
- **Native lets the system material show**: thin surfaces with one margin on
  every edge. On macOS it follows the macOS 26 UI kit: 24 pt controls with a
  6 pt radius, tabs as a segmented control, the small switch with its capsule
  knob, menus with an accent-filled highlight, group boxes for cards and
  square tooltips. On Windows it follows Fluent: 4 px controls with their
  hairline border, 8 px cards, WinUI toggles and Fluent 2 tabs. Settings uses
  tabs only in Native.

### Fixed

- **Command Code no longer appears without a login on fresh configurations.**
  It was enabled by default, so the bar showed a red credentials error even for
  people who never used it. It now starts off and can be enabled explicitly or
  by local credential detection. Existing explicit `[commandcode] enabled = true`
  settings remain respected; switch that setting off to hide it.
- **The Windows tray popover keeps its layout when the exe moves.** WebView2
  kept the popover's profile next to the exe (`<exe dir>\ai-usagebar-tray.exe.WebView2`),
  so running the tray from another folder, or a Scoop update into a new
  version folder, started from an empty profile and lost the Customize
  layout, theme, style and dismissed hints; under Program Files the folder is
  not writable at all. The profile now lives in
  `%LOCALAPPDATA%\ai-usagebar\popover`, beside `detect.json`. The first run
  copies the `Local Storage` of the profile beside the exe (the layout, a few
  KB) into it and leaves the old folder alone. If the folder cannot be created the popover
  falls back to the old location and still opens.
- **Antigravity says what a free plan means.** Accounts whose plan does not
  include Antigravity get 403 `SUBSCRIPTION_REQUIRED` from the cloud quota
  fallback; the widget called that a rejected session, sending the user to
  re-sign-in for nothing. The message now says the plan has no quota to
  report and names the `[antigravity]` toggle, and only a 403 without that
  reason keeps the session wording. (#256)
- **A Scoop install of the Windows tray no longer updates itself behind
  Scoop's back.** The built-in updater only knew Homebrew, Nix and cargo
  builds, so under Scoop "Install Update" (or Automatic, silently) wrote the
  new exes into Scoop's version folder: `scoop list` kept the old version, the
  next `scoop update` fetched the running version again and `scoop reset`
  handed back the new one. A tray running from
  `<scoop>\apps\<app>\<version or current>\`, with Scoop's `install.json`
  beside it, now offers the release page like a Homebrew install does, and
  Scoop owns the update (`scoop update ai-usagebar`), as the README already
  said.
- **The Windows tray popover no longer runs under the taskbar.** A tall popover
  was sized and kept on screen against the whole monitor, so on a 1440 px
  display with a 48 px taskbar its bottom went behind it. It now uses the
  monitor's work area and counts the window frame, and it opens just clear of
  the taskbar, leaving room for its shadow. A click on the tray icon opens it
  centered on the icon on the side away from the taskbar, so a taskbar docked
  at the top, left or right works the same; it used to hang a margin above the
  icon, twice as far from a bottom taskbar as the global shortcut put it.
- **A Codex credit balance sent as a numeric string reads as dollars.** The
  usage endpoint sometimes sends the balance as a bare string (`"0"` on a Pro
  account with no extra-usage credits) instead of a number, and only numbers
  were formatted, so the Credits block, the Waybar tooltip,
  `{oai_credit_balance}`, `usage --json` and the tray popover showed
  "balance: 0". A string that is only a finite number is now formatted like a
  number (`$0.00`, a negative as `-$1.00`); anything else, such as an already
  formatted `$2.50`, passes through unchanged.
- **A click outside the Windows tray popover closes it right after opening.**
  The popover took focus 400 ms after the tray click and ignored blurs for 400
  ms more, so a click elsewhere in that time left it open until it was clicked
  into and out of again. It is focused at once now, and a press outside it
  closes it whether or not Windows handed it focus. A click on the tray icon
  while the popover is open closes it; the press used to close it and the
  release reopened it.
- **The Windows tray icon is white on a dark taskbar.** It was always drawn in
  black, which almost disappears on the Windows 11 default. It follows the
  "default Windows mode" setting the taskbar uses, and recolors when it changes.
- **A manual update check could fail once with "error sending request".** The
  tray kept an idle connection that GitHub had already closed; checks now open
  a fresh one each time.
- **Enter in the update dialog presses its default button** (Install, Try Again
  or OK), and closing a menu or popover with the mouse no longer leaves a focus
  ring on the button that opened it.
- **A starred SuperGrok meter was missing from the menu-bar bars.** The meter
  was renamed "Weekly usage" and the popover learned to drop the suffix, but
  the tray host still only dropped "Build credits", so the star it looked up
  never matched and that bar was skipped. Both sides now derive star keys from
  one shared fixture that their tests read, so a rule changed on one side alone
  fails a test.
- **The armed Reset button in the tray popover turns red.** The first click of
  Reset asks for a second; the red it was meant to show never applied, because
  the button's own style outranked it, so only its tooltip changed.
- **Install Update did nothing on macOS.** The banner sent a command the macOS
  host never handled, so the button read "Updating…" and nothing happened.
- **A failed update check no longer poses as an available update.** The footer
  dot and the dashboard banner need a release in hand; the dialog reports a
  failed check.
- **Tray popover reset details open the way each one is used.** Hovering a
  meter's "Resets in …" shows the other format (the exact time, or the
  countdown in exact mode) in the same hint style as Settings; the banked
  "Rate Limit Resets" list now opens on click, on the Options menu's surface.
  The two had it the other way round and neither followed the popover's own
  radius and padding. Banked resets are colored by how soon each expires
  instead of by position.
- **Menus and pickers in the tray popover highlight the hovered row in dark
  mode.** The highlight was the card gray, a shade away from the dark menu
  background, so it was nearly invisible. Menus, pickers and popover lists now
  share one hover color per theme.
- **The update banner matches the other dashboard notices** (padding, icon,
  button), instead of a near-copy with its own spacing.
- **Everything clickable in the tray popover shows a hover highlight** — metric
  readings, row chevrons, dismiss buttons, list rows — the same one the menus
  use; things that only show a hint on hover get none. Every hover hint is the
  popover's own tooltip now, never the system's `title` bubble: it appears after
  half a second, dark gray on the light theme and a step above the cards on the
  dark one, with no arrow. The footer's "Next update in" countdown is plain
  text now; Refresh lives in Options and on each provider. Dashboard notices put the
  icon beside the title, with the message and button on the card's left edge.
- **Buttons, chips and pickers share one height and label size**, the Options
  button included; the banked-resets count is a chip like Status and
  Dashboard.
- **No pace tick on a spent meter.** A tray popover row at 100% reads "Limit
  reached", yet it still got a behind verdict, so the even-pace tick sat on
  the full bar as if there were room left, and Always Show Pacing counted it
  as visible. A spent row now has no pace at all, as in OpenUsage; a row one
  percent short of the limit keeps its tick.
- **The macOS tray offers Quit when the popover's webview cannot be built
  (#249).** A WKWebView that fails to build left the accessory app (no Dock
  icon, no app menu) with no menu and no way out but `killall`: clicking the
  status item flashed an empty window. The status item now attaches a
  minimal fallback menu — Refresh and Quit AI Usage — only on that failure
  path, with Quit exiting through the same clean loop shutdown the popover's
  own Quit control uses. Normal operation is unchanged: the status item stays
  menu-free so both mouse buttons open the popover.


## [1.24.0] — 2026-09-24

### Added

- **Claude's banked limit resets.** Claude now grants redeemable usage-limit
  resets during a campaign — the "Resets" offer with its own expiry date —
  and they land on the same row Codex and SuperGrok resets already use: the
  tooltip, the TUI panel, `usage --json`'s `reset_credits`, and from there the
  Omarchy, GNOME and KDE surfaces, plus the 48-hour expiry notification. Two
  new placeholders, `{resets_available}` and `{resets}`. The figures come from
  the `cedar_ember` block on the existing usage endpoint, so there is no
  second request; an account without a grant is the normal case and shows no
  row at all. The redemption handle the API returns alongside each grant is
  never deserialized — ai-usagebar reports that a reset exists and when it
  lapses, and redeeming it stays with Claude Code (`/limit-reset`).

### Changed

- **macOS usage panel layout.** Current usage and goal percentages sit to the
  right of their bars, with the pace projection following the reset note below
  both bars. Provider buttons switch the main view without a nested card.

### Fixed

- **A named Codex account no longer caches another account's usage.** A
  fetch chose its `auth.json` before taking the credentials lock, so an
  `account switch --codex` landing in between made it read the other login
  and store that usage under its own label. The route is now resolved again
  once the lock is held, and followed if the switch moved the login.
- **The unnamed Codex tab shows the new login right after a switch.** A
  successful `account switch --codex` now drops the default account's usage
  cache, which otherwise kept showing the previous account's quota until it
  expired. Named accounts keep their own caches.
- **The unnamed Claude tab shows the new login right after a switch.** The
  Claude CLI switch had the same stale default cache as the Codex one, and now
  drops it the same way after a successful `account switch`.
- **Account labels, paths and errors printed by `account` are sanitized.**
  The Codex status line, the switch output and the `add` / `--adopt-current`
  messages for both vendors now pass through the untrusted-text sanitizers,
  so a label carrying bidi controls cannot reorder terminal output.
- **The macOS tray switches accounts without a separate `ai-usagebar`
  binary.** The documented build produces only `ai-usagebar-tray`, and the
  switch looked for `ai-usagebar` beside it or in `~/.cargo/bin`, so it
  failed there or could run a different version. The tray now runs the
  switch itself.
- **Every rendered account card keeps its switch control.** The popover
  offered switch controls to only the first 32 accounts while rendering up to
  64 cards, and a label longer than a card id's cut matched no card.

## [1.23.0] — 2026-09-24

### Fixed

- **macOS Claude Code Keychain prompts, the oversized case (#148).** Releases
  1.16.0 through 1.21.1 still wrote a refreshed credential through the native
  Security.framework API whenever the composed `security -i` line exceeded
  the 4000-byte operational cap. That case is now the normal one: Claude Code
  keeps `mcpOAuth` discovery state for every MCP plugin in the same item, so a
  real blob (3640 bytes, 302 quotes, ~4020 bytes composed, measured
  2026-09-23) took the native path at every token refresh, re-stamped the
  item with ai-usagebar's `cdhash:` partition, and brought the dialog back
  daily — "Always Allow" with the Keychain password does restore `apple-tool:`,
  but only until the next refresh. Oversized blobs are now handed to
  `security add-generic-password` as an argument instead, the same fallback
  Claude Code uses (the JSON is visible to `ps` for the milliseconds `security`
  runs); the native write is gone and `security-framework` is a dev-dependency
  used only by the opt-in Keychain tests, which now also cover an oversized
  blob through the production dispatch.
- **The macOS tray opens its popover on left click again (#236).** On macOS
  27 a left click on the status item opened the Refresh / Quit context menu
  instead of the dashboard. tray-icon 0.24 keeps the menu attached to the
  `NSStatusItem`, and on macOS 27 an attached menu keeps left clicks from
  reaching tray-icon's click handler, so `with_menu_on_left_click(false)` had
  no effect (tauri-apps/tray-icon#355). tray-icon 0.25.1 attaches the menu
  only while it is being shown. The MSRV is now Rust 1.90, which tray-icon
  0.25 and muda 0.20 require.
- **The Omarchy panel keeps the provider you chose.** A refresh gap (fetch
  error, sleep/wake stale list) briefly dropped entries, and the panel's
  fallback re-resolved to the configured primary; when the chosen entry
  returned, that transient selection stuck and the panel showed the primary
  until a shell restart. The persisted choice is now the source of truth:
  once the chosen entry is back in the list, it wins over any selection that
  only exists because of the gap.

### Added

- **Optional macOS usage goal.** Preferences can show an extra bar below each
  usage metric with the percentage expected now for an even path to 100% at
  the reset. The calculation uses the reported window length and reset time;
  monthly windows without an exact start are clearly marked as estimates.

- **Omarchy-style macOS menu-bar summary.** The tray shows every ready
  provider's name and quota headline beside its chart glyph by default.
  Middle-click or the right-click menu cycles providers; the menu can show one,
  hide values, or pin the 5-hour, weekly, or monthly window. Selection and
  display options persist in `[tray]`; Chart Icon Only restores the previous
  glyph-only presentation.

- **macOS usage panel and preferences.** An AppKit glass popover shows only
  enabled providers, their usage and balance, reset times, and refresh controls.
  Settings has General, Providers, Menu, Preferences, and Alerts tabs. The
  provider list can be reordered and customized there; English and Brazilian
  Portuguese are selectable in Preferences. Alerts deliver quota and expiring
  credit notifications through macOS Notification Center, with an enable switch
  and threshold in the panel. Exact reset times now follow the selected display
  mode in the macOS panel.
- **Switch the active Claude or Codex account from the macOS tray.** Each
  named account's card gets a control beside Customize and Reset: a filled star
  on the login in use, an outline star on the others that switches to it. A
  Claude switch moves the `claude` CLI login (which the VS Code extension
  shares) and, when the account has a Desktop profile, Claude Desktop; a Codex
  switch moves `~/.codex/auth.json`, which the Codex CLI, desktop app and IDE
  extension all read. The star spins while the switch runs and turns red with
  the reason when it fails.
- **`ai-usagebar account switch <label> --codex`.** The Codex counterpart of the
  Claude CLI switch: the outgoing login is saved back to its own account
  before the target's `auth.json` is moved into `~/.codex/auth.json`, so the
  switch itself never leaves one refresh token in two files. It refuses
  ambiguous layouts (shared or symlinked credential files, one ChatGPT account
  under two labels, an active account with its own copy), locks every
  directory it touches, and on failure restores what it can and names what it
  could not. A per-file marker (account id only, no token) keeps a moved-away
  account identifiable, and reads for the active account follow it into the
  default file. ai-usagebar's Codex token refresh now takes the same lock.
- **`ai-usagebar account add <label> --codex`** registers an
  `[[openai.accounts]]` entry at `~/.codex-<label>/auth.json` and runs
  `codex login` under that `CODEX_HOME`.
- **`--adopt-current`** on `account add` registers the login already in use
  (plain `claude`, or `~/.codex` with `--codex`) under a label without signing
  in again, so the first switch away can save it.
- **`[openai] show_default_account`**, like the Anthropic and OpenRouter
  settings: `false` hides the unnamed Codex tab once every login is named.
- `account status` lists the Codex accounts and which one `~/.codex` holds.

- **Quota-threshold desktop notifications.** After a fresh fetch, any vendor
  window that crosses `[notifications] threshold` (default 97%) raises a
  `notify-send` notification on Linux (`-a ai-usagebar -c quota`); an
  exhausted window (100%) is marked critical. Banked reset credits (Codex,
  SuperGrok) notify 48 hours before they expire. One crossing is one
  notification: a key re-arms only when usage drops 7 points below the
  threshold or the window's reset moves to a later instant, and the dedupe
  state lives in `~/.cache/ai-usagebar/notifications.json` behind the same
  flock discipline as the vendor caches. Delivery is best-effort by design —
  a missing or failing notifier, an unwritable state file, or lock contention
  is a silent skip that never touches the bar, the report, or an exit code.
  macOS delivers through Notification Center; Windows delivery follows in a
  later release. Config: `[notifications]` with `enabled` (default `true`) and
  `threshold` (1..=100, default `97`), also editable in the TUI Settings
  overlay.

## [1.22.0] — 2026-09-23

### Added

- **A tank for a prepaid balance.** DeepSeek, Kilo, Novita, Moonshot and
  prepaid Grok report money remaining and no denominator, so their row was a
  plain balance. `[vendor] display_limit` states the tank size, in the currency
  that vendor already reports, and turns it into a consumed meter —
  `(display_limit - balance) / display_limit`, clamped 0–100, so a balance over
  the cap reads 0% used. It must be finite and greater than zero, there is no
  default, and it is a fallback rather than an override: a vendor that states
  its own limit keeps it, which is why `[openrouter]` has none.
- **`[vendor] headline`.** Picks which number goes on the bar, `"amount"` or
  `"percent"`; whichever is not the headline stays in the detail line. Balance
  vendors default to `"amount"`, OpenRouter to `"percent"`. Setting
  `display_limit` does not switch it, and `"percent"` with no limit from either
  source leaves the amount on the bar. Report metrics carry the resolved choice
  as a new `headline` field (`"percent"` or `"value"`).
- **Alibaba Cloud Model Studio Token Plan** as an opt-in local-login vendor
  (`[modelstudio]`). Reads the console session the official `bl` CLI stores
  at `~/.bailian/config.json` after `bl auth login --console` (read-only;
  `config_dir`/`BAILIAN_CONFIG_DIR` override the location), and reports the
  plan's 5-hour and weekly windows through the same region×site console
  gateway the CLI uses. Wire percentages are ratios in [0,1] and resets are
  epoch milliseconds; an absent window is no-data (possibly unlimited), never
  0%, and an out-of-range value is schema drift rather than a figure. The
  vendor cache is scoped by a fingerprint of the access token — the token
  itself never persists. (#147)

- **OrcaRouter** as an opt-in API-key vendor (`[orcarouter]`,
  `ORCAROUTER_API_KEY`). Reports the credit card from the one-api compatible
  dashboard billing endpoints — cumulative spend (US cents on the wire,
  rendered as exact dollars), total credit limit, remaining, and the key's
  expiry when it has one. Unlimited-quota keys report the `100000000` sentinel
  in the limit fields and render spend-only, never as a $100M wallet. Errors
  that arrive as HTTP 200 with an OpenAI error envelope surface as failures,
  not zeros. (#193)

### Changed

- **The Omarchy panel and KDE plasmoid read a metric's `headline` instead of
  testing its label for "balance".** The label check put OpenRouter's dollar
  figure on the bar and hid its real consumed percent; OpenRouter now shows the
  percent by default.
- **The tray popover honours `headline` too** (Windows and macOS). A balance
  metered against `display_limit` with `headline = "amount"` shows the money
  figure under its meter, with the percentage and the detail line in the hover
  text; `"percent"` keeps the popover's used/left toggle.

### Fixed

- **Omarchy Quattro panel: the first provider tab keeps its left border at
  fractional display scales.** The panel's scroll content sat flush against
  the `Flickable`'s clip edge, so at a 125% monitor scale Qt snapped the
  first tab's 1px border to a device pixel outside the clip and only that
  strip was dropped — the tab rendered with three borders while every other
  tab kept all four. The content now keeps a hairline of slack on both sides,
  so no bordered control sits exactly on the clip boundary. (#231)

## [1.21.1] — 2026-09-22

### Fixed

- **The Windows tray again embeds the real dashboard** instead of the
  placeholder page. v1.21.0 shipped a stub popover: `build.rs`'s npm
  availability probe called `npm` directly, which cannot spawn the Windows
  `.cmd` shim, so the Vite build was silently skipped and the no-Node
  placeholder was baked into the release binary. The probe now goes through
  the same `cmd /C` wrapper the build itself uses, and CI plus the release
  workflow fail loudly if any tray artifact ever embeds the placeholder
  text again. (#229)

## [1.21.0] — 2026-09-22

### Added

- **macOS WebView tray** (`ai-usagebar-tray`). Same OpenUsage-style popover as
  Windows (WKWebView instead of WebView2), plus a compact usage-chart glyph in
  the menu bar from starred metrics (at most two per provider).
  `cargo build --release --bin ai-usagebar-tray`.
- **macOS Grok Bot.** `[grokbot]` reads
  `~/Library/Application Support/Grok Bot/sand-secrets.json` with the
  Chromium OSCrypt key from the login Keychain item `Grok Bot Safe Storage`
  / `Grok Bot Key` (1003 PBKDF2 rounds, the same scheme as Claude Desktop).
  The Mac app stores `cursor-accounts` as a JSON string wrapping the object
  Linux writes directly; both shapes parse. Windows still fails closed.
  Omarchy and the Windows tray draw Grok Bot's own head-and-eyes logomark
  (`grokbot.svg`) instead of sharing Grok's mark.
- **About and Check for Updates** in the tray Options menu. macOS checks
  GitHub and opens the release page; Windows still installs in place.
  Settings rows that are not obvious (pacing, reset times, shortcut, and
  the rest) show a short hint.

### Fixed

- **`usage --json`'s `primary` is now an entry id, not a bare vendor slug.**
  With named accounts the entry ids carry account labels
  (`anthropic@claude-me`), so a `primary` serialized straight from
  `config.ui.primary` named an id no entry carried and every consumer
  resolved the mismatch differently or not at all. The report resolves the
  configured primary to the first entry of that vendor (the bare slug, or
  the first `{slug}@…` account) before serializing; a primary naming a
  vendor with no entries keeps the slug, and an unset primary stays absent.
  Consumers can now treat `primary` as an entry id present in `entries`.

- **A named Anthropic account keeps reading its own credential file** while
  that file is there. `resolve_active_label` matches `~/.claude.json`'s
  account marker, and two `CLAUDE_CONFIG_DIR` directories can hold the *same*
  account — each with its own live login. Every fetch for such a label was
  routed to `~/.claude/.credentials.json` on the assumption that
  `account switch` had moved the credential into that default slot, so an
  account whose own file was live and unread next to it reported "token
  refresh failed; run `claude` to re-auth" from a slot the user never logs
  into. The default slot is now used only when the account's own file really
  is gone, which is what a switch leaves behind.

- On macOS, a leftover `~/.claude/.credentials.json` no longer shadows Claude
  Code's live Keychain item. That file-first read 400'd "Refresh token expired"
  and the tray showed **Sign-in expired** while `claude` itself was still
  logged in.

- **Grok Bot live `usagePercent` and on-demand `enabled`.**
  `GetSandUsageStatus` has been observed sending a fractional JSON number
  (`19.150778`) and `onDemandSettings.enabled: null`. The parser rounds the
  percent and treats null as off, so a real macOS session no longer dies as
  schema drift.
- **Stop probing sibling ports of a `missing CSRF` `agy`.** When the local
  language server status RPC responds with missing CSRF, the remaining
  listeners of that same process (such as the companion TLS port) are skipped
  instead of probed. This eliminates the spurious `http: TLS handshake error:
  remote error: tls: unrecognized name` diagnostics while still trying other
  Antigravity products that are running.

## [1.20.2] — 2026-09-19

### Fixed

- **`usage` exits 0 after printing a complete document.** Per-entry fetch or
  auth failures stay inside each entry's `error` field instead of making the
  command itself fail, so a script that captures `usage --json` still gets the
  diagnosis when every account is broken. Non-zero remains only when the
  document cannot be produced (missing or unreadable `--config`, unparseable
  TOML, no vendors enabled, or a runtime/bootstrap failure). (#217)

## [1.20.1] — 2026-09-18

### Added

- **Official Scoop manifest for the Windows release.**
  `packaging/scoop/ai-usagebar.json` installs the release ZIP with all three
  binaries and an "AI Usage" Start-menu shortcut for the tray, and a
  `publish-scoop` job in the release workflow pushes the freshly pinned
  manifest (version from the tag, hash recomputed from the published
  `.sha256` sidecar) to the `akitaonrails/scoop-bucket` repo when
  `SCOOP_BUCKET_TOKEN` is set. Proposal: #216.
- **Reset-credit expiry tooltip on Windows.** The Codex and SuperGrok reset
  credit row keeps its compact available-count badge and now reveals each
  credit's expiry date and remaining time on hover or keyboard focus.

### Changed

- The Windows tray popover is 300 logical pixels wide for a more compact
  footprint, including on scaled displays.

### Fixed

- The Windows tray popover remeasures its intrinsic content height whenever it
  opens or receives updated data, instead of retaining a stale work-area-sized
  window with empty space above the footer.
- The Scoop manifest's `version` is bumped in lockstep with the release (the
  new `verify-version` guard caught the never-published v1.20.0's stale
  manifest before anything shipped; that tag remains unused).

## [1.19.0] — 2026-09-17

### Added

- **Grok Bot as its own opt-in vendor** (`[grokbot]`, `--vendor grokbot`,
  Linux-only for now): the Grok Bot desktop app's weekly included-usage
  pool, from its Connect-RPC dashboard call
  (`api2.cursor.sh/aiserver.v1.DashboardService/GetSandUsageStatus`).
  Distinct from `[grok]` (Management API prepaid dollars) and
  `[supergrok]` (Grok Build subscription). The credential is the app's own
  OAuth session in `~/.config/Grok Bot/sand-secrets.json` — Chromium
  OSCrypt `v10` blobs decrypted read-only with the Linux OSCrypt key (one
  PBKDF2 round; `secret-tool lookup application "Grok Bot"`, falling back
  to Chromium's documented default). Refresh goes through Cursor's public
  installed-app OAuth client, and rotated tokens persist only in
  ai-usagebar's vendor cache (`oauth.json`, mode 0600), never back to the
  app's file. The window length is derived from the reported period bounds
  rather than assumed to be 7 days; an account with
  `hasNonZeroIncludedLimit: false` shows a "no included allowance" state
  rather than a 0% meter, and at 100% with the account still serving, an
  on-demand footnote appears when on-demand is enabled. New placeholders:
  `{gbt_plan}`, `{gbt_weekly_pct}`, `{gbt_weekly_reset}`,
  `{gbt_on_demand}`. macOS and Windows fail closed with a credentials
  error explaining the Linux-only support. (#206)
- **SuperGrok included usage, not a single Build-credits bar.** SuperGrok
  already fetched Grok Build's billing document; the parser only kept the
  overall `creditUsagePercent` and labelled it "Build credits". It now
  shows that figure as **weekly/monthly usage** and lists the
  `productUsage` slices beside it (Grok Build, Grok Chat, Grok Imagine,
  and any other named product) — in the tooltip and `--pretty` box each
  slice is one dim line with an aligned percentage, no gauge and no
  severity colour, so only the overall meter reads as the binding
  constraint. `[grok]` is unchanged: that vendor is
  still the Management API prepaid dollar balance.
- **Grouped sub-rows in the report and the Omarchy panel.** Product slices
  now carry a `group` field (`"Breakdown"`) in `usage --json`'s `sections`
  and `metrics`, and the Quattro panel renders grouped rows compactly —
  dim label, thin muted gauge — under a `BREAKDOWN` heading, instead of as
  full meters beside the overall usage row. The field is additive: older
  frontends keep rendering the rows as plain metrics.

### Fixed

- **Kimi accounts on the newer `/coding/v1/usages` response shape no longer
  hard-error as schema drift.** Such accounts return no top-level `usage`
  block — only a `usages` map of ratios — which the parser rejected
  outright. The snapshot now reads the combined monthly pool from
  `limit_month_total` (`used_ratio` × 100, a spelling validated against the
  vendor's own website), exposed as the new `{kimi_monthly_pct}` /
  `{kimi_monthly_reset}` placeholders and a "Monthly" row in the tooltip and
  the detail panel. These accounts have **no weekly window**: the weekly row
  is dropped and the `kimi_weekly_*` placeholders resolve to empty rather
  than a fabricated 0. `limit_month_code` — the Code slice *inside* that
  same pool — is never added to the total or rendered as its own allowance,
  and the 5h rolling window still comes from `limits[]`, which the website
  matches.
- **SuperGrok product names are escaped before reaching the tooltip's Pango
  markup.** A hostile billing document could otherwise inject markup through
  a crafted `productUsage` name. Width-based label alignment happens before
  the escape, so padded columns still line up.
- **SuperGrok no longer shows a "$0.00 Prepaid API" line.** The billing
  document reports `prepaidBalance: 0` unless credit was purchased on top
  of the subscription, and a zero row read as "no money" — especially for
  unified billing accounts, whose real dollars sit in the Management API
  wallet that `[grok]` reports. The row (TUI, tooltip and report) now
  appears only when there is credit to show; `{sgk_prepaid}` still
  publishes the raw figure.

### Security

- **Updated rustls to 0.23.45** (from 0.23.40), fixing RUSTSEC-2026-0285 —
  TLS 1.3 handshake messages incorrectly accepted across encryption-level
  boundaries — in the stack that carries every vendor credential request.
  Added `.cargo/audit.toml` documenting the two remaining transitive-only
  gtk/glib advisories the tray stack pins and ai-usagebar never reaches.

## [1.18.1] — 2026-09-16

### Fixed

- **Column alignment with double-width text.** Labels were measured and padded
  by character count, so any text containing CJK glyphs — a Japanese account
  label, a plan name — left the value column short by one space per ideograph
  in the text report, the tooltip box, and the TUI. Width is now measured in
  terminal columns, and padding is computed from that rather than from
  `format!`'s character-based fill. Combining marks now correctly measure zero.

## [1.18.0] — 2026-09-16

### Added

- **`ai-usagebar settings enable <vendor>`.** A new CLI subcommand that turns one
  provider on, preserving every other setting, comment and credential in
  `config.toml`. Unlike discovery, it is an explicit opt-in and therefore does
  overrule a previous `enabled = false`. It is what the macOS Preferences
  Enable button calls.
- **Ollama Cloud monthly window placeholders.** `{oll_monthly_pct}`,
  `{oll_monthly_reset}`, `{oll_monthly_elapsed}`, `{oll_monthly_pace}` and
  `{oll_monthly_pace_indicator}`, plus a `Monthly` metric in the tooltip, TUI
  panel and report, for accounts whose plan reports a calendar-month quota.

### Fixed

- **macOS named Codex accounts.** Accounts in `[[openai.accounts]]` now appear
  in the provider selector, Overview, Preferences and vendor-cycle shortcut,
  including when no default Codex login exists.
- **macOS disabled providers.** Preferences now distinguish disabled providers
  from missing credentials and offer an explicit Enable action, backed by
  `ai-usagebar settings enable <vendor>`. Other settings and credentials are
  preserved, and write failures remain visible in Preferences.
- **English-only UI strings.** The GNOME preferences and the macOS provider
  list showed `verificando…`, `Configurar (TUI)` and `Re-logar` — Portuguese
  left over from an early draft. They now read `checking…`,
  `Configure (TUI)` and `Sign in again`, matching every other frontend.

- **Ollama Cloud monthly-quota accounts no longer show a bare "Ready".**
  Some Ollama Cloud accounts report `limits.monthly` instead of
  `limits.session` + `limits.weekly` for the same `"pro"` plan label. The
  parser only understood the session/weekly shape, so a monthly account had
  no window to render and every frontend fell back to a plain "Ready"/status
  pill with no percentage. `limits.monthly` is now parsed into a `Monthly`
  metric (bar, tooltip, TUI panel, and top models), alongside the existing
  session/weekly windows for accounts that report those instead.

## [1.17.1] — 2026-09-16

### Fixed

- **Antigravity CLI session fallback.** When `agy` requires an undiscoverable
  CSRF token, ai-usagebar now reads the CLI's saved Google session from
  `~/.gemini/antigravity-cli/antigravity-oauth-token` when the OS keyring is
  unavailable.

- **Antigravity's fallback source no longer claims the app is closed.** Since
  v1.17.0 the Cloud Code fallback also answers when `agy` is running but will
  not publish its CSRF token, yet the TUI labelled those figures
  `Google API (app closed)` — false while the app is open. The source row now
  reads `Google API`, which is true for both reasons the fallback fires.

## [1.17.0] — 2026-09-12

### Added

- **Custom provider brand marks.** A `[[custom]]` provider can set
  `brand = "<vendor slug>"` to use a built-in vendor's mark in the Omarchy
  widget. The brand is chosen explicitly and need not match the provider URL;
  leaving it unset keeps the custom provider's three-letter tag.

- **Versioned usage JSON.** Both aggregate and single-provider `usage --json`
  reports now include top-level `"schema_version": 1`. The documented contract
  remains tolerant: consumers ignore unknown fields and treat absent fields as
  not applicable; the version changes only for incompatible shapes.

### Fixed

- **Antigravity works while the `agy` CLI is running.** When `agy` exposes a
  local RPC server but rejects quota probes because its CSRF token is not
  discoverable, ai-usagebar now uses the saved Google session fallback. Other
  local `401`/`403` responses still surface as signed-out errors.

- **Release-integrity checks now run on pull requests.** CI fetches the tag
  history and runs the existing immutable-changelog and version check before
  changes can reach `main`.

## [1.16.0] — 2026-09-11

### Added

- **Omarchy top bar usage-window picker.** The Quattro settings page gains a
  **Top bar usage window** dropdown (`auto` / `session` / `weekly` /
  `monthly`), also settable with
  `omarchy bar set akitaonrails.ai-usagebar barWindow session` that pins the
  bar label to one quota window instead of always showing the highest percent;
  the tooltip and panel hero echo the pinned value. `session` pins the 5-hour
  window, `weekly` the 7-day window, and `monthly` the monthly pool where one
  exists; `auto` keeps the historical highest-percent behavior and is the
  default, so existing installs are unchanged. A pinned window a vendor does
  not offer falls back to the highest percent rather than blanking the bar.
  Panel rows and alert state still follow the highest percent regardless.

### Changed

- **macOS menu bar keeps no second copy of Rust's enabled defaults.**
  The `defaultEnabled` slug list is deleted, together with the helpers the
  catalog migration had orphaned (`vendorEnabled`, `configEnabledTOML`,
  `configHasApiKeyTOML`, `vendorConfigured`). Every enabled decision in the
  menu bar had already read the catalog's `enabled` field from
  `vendors --json` since the #170 migration — which is why the slug list's
  disagreement about Ollama Cloud (#185) was latent, never user-visible —
  and now nothing else exists to drift: the Rust wire test pins the
  `enabled` field name, and the Swift contract test pins that the field
  decides. A provider added in Rust reaches the menu bar with its own
  default, no Swift change needed.

- **Omarchy install is one paste, and the marketplace card says what it needs.**
  The plugin is the display frontend; it reads the `ai-usagebar` binary, which
  installs through a different manager (the binary is a system package, the
  plugin is per-user config under `~/.config/omarchy/plugins/`), so the two
  steps cannot become one command. They are now one copy-paste, and the
  manifest description — which plugins.omarchy.org shows verbatim on the card —
  names the binary requirement, because the marketplace's Install button copies
  only the `omarchy plugin add` half.

### Fixed

- **macOS menu bar knew the wrong default for Ollama Cloud.**
  `defaultEnabled("ollama")` fell through to `true` while Rust ships
  `[ollama] enabled = false` (opt-in) — a latent disagreement only, since
  the catalog migration had already moved the menu bar's live decisions to
  `vendors --json`. The slug list is now deleted outright (see the Changed
  entry above); the catalog's `enabled` field decides, and the Rust wire
  test pins the field name. Ollama's Session/Weekly bars already rendered
  through the generic `parse()` path — no format change.

- **Cursor on-demand usage:** Cursor Enterprise reports now show the amount
  spent and configured limit when `onDemand.used` and `onDemand.limit` are
  available, instead of showing only whether on-demand billing is enabled.

- **macOS Claude Code Keychain prompts.** Write normal-sized refreshed OAuth
  credentials through `/usr/bin/security -i` so the item keeps the
  `apple-tool:` partition that Claude Code can read, while retaining the native
  Security.framework write only as the oversized fallback. Existing affected
  users can clear the bad partition by running a fresh `claude` + `/login`.

## [1.15.0] — 2026-09-10

### Added

- **macOS menu bar support for remaining CLI vendors.** Copilot, SuperGrok,
  MiniMax, Kiro, Nous Research, OpenCode Go, and Command Code are now available
  in the macOS menu bar, resolving metadata dynamically via `ai-usagebar vendors --json`.

- **macOS specific quota pools.** Support monthly usage windows, MiniMax video
  quotas, Copilot completions, and explicit unlimited quota display without
  misleading 0% progress bars.

- **macOS environment PATH injection.** Injects `/opt/homebrew/bin`,
  `/usr/local/bin`, and `~/.cargo/bin` into subprocess environments for vendor
  tools.

- **Ollama Cloud vendor.** `ollama.com/api/usage`, the quota route the
  official settings page itself uses, behind a Bearer key minted at
  <https://ollama.com/settings/keys>. The local daemon at
  `127.0.0.1:11434` has no quota route, and the Ed25519 key the `ollama`
  CLI keeps in `~/.ollama/id_ed25519` is the registry's signing key, not
  a quota credential — the widget never reads it. The native Rust
  provider adds an `Ollama Cloud` tab, a `[ollama]` config block
  (disabled by default, opt in with `enabled = true` or via the TUI
  Settings overlay), `{oll_session_pct}` / `{oll_weekly_pct}` placeholders
  with the usual pace and reset aliases, a per-model breakdown of the
  five heaviest models in each window in the tooltip, and a
  `usage --json` entry keyed `ollama`. Plan label comes from config; the
  API itself does not report one. `tests/fixtures/ollama/good_full.json`
  pins the real shape, and `tests::live::ollama_live` is the live smoke
  against the real API. The cache stores the projected snapshot only —
  the raw body, and the Bearer key with it, is never written to disk.
- **OpenCode Go pacing.** The rolling (5h) and weekly (7d) windows now expose
  `{ocg_rolling|weekly_elapsed}`, `{ocg_rolling|weekly_pace}`, and
  `{ocg_rolling|weekly_pace_indicator}` placeholders plus `{session_elapsed}`
  / `{weekly_elapsed}` aliases, pace arrows in the Waybar tooltip, and paced
  rows in the TUI panel and `usage --json` report footnotes. The monthly
  window keeps its reset countdown but is not paced: its cycle follows the
  subscription date, so no fixed length is exact and no `window_secs` is
  published for it. Window lengths are constants: the usage endpoint reports
  only `percent` and `resetsAt`.

### Fixed

- **`detect` sees Antigravity with the app closed.** Since v1.14.0 Antigravity
  reports from the Google session it saved, with every product shut — but
  detection still looked only for a *running* local server, so `detect` skipped
  a provider that works, and because a vendor is considered once the miss stuck
  until `--all`. It now also counts the token in our own vendor cache. The
  keyring is deliberately not read: that can raise a Keychain prompt on macOS,
  and a background probe must not pop a dialog. The trade is the very first
  run, before any fetch has persisted a token.

- **Windows tray: "Open TUI" works.** The menu item launched Windows Terminal
  with `wt -e <command>`, but `-e` is wezterm's flag, not Windows Terminal's:
  `wt` rejected it, printed its usage page and exited, so the TUI never
  started and the user saw a flash of help text. It now uses
  `wt new-tab -- <command>`. `spawn()` reports only that the process started,
  which is why the wrong flag looked like a success and fell through to no
  fallback.

- **Omarchy Quattro panel:** the provider tab strip is a wrapping `Flow` again
  instead of a fixed-width horizontal `ListView`. With five or more providers
  enabled the list overflowed the panel's edge and the extra entries were
  simply unreachable — no scrollbar, no way to click them. They now wrap onto
  additional rows.

- **Settings overlay can pick env-only key vendors.** `KEY_VENDORS` in
  `tui::settings` were excluded from the primary list whenever neither an
  inline `api_key` nor a non-empty env var resolved at startup, so a fresh
  install that only ever exports `OLLAMA_API_KEY` (or any other key
  vendor's env var) could not select the matching tab in the TUI to
  flip `enabled = true` without first editing the TOML by hand. The
  overlay now treats a present env var as a sufficient signal that the
  vendor is reachable, surfaces it in the primary list and writes
  `enabled = true` on save like the inline-key path did.

## [1.14.0] — 2026-09-08

### Added

- **Local provider detection.** `detect::has_local_credentials` is a cheap,
  local-only probe per vendor (credential files, sqlite stores, saved API keys,
  env vars, Antigravity's local ports; never the network) that *parses* the
  credential the way the fetch would, so an empty or unreadable file does not
  count. `detect::run_once` writes `enabled = true` into `config.toml` for the
  vendors that have one and are still off — so Cursor, Kiro, Grok, Copilot and
  friends show up without editing the config by hand. Detection only ever
  enables, and never overrules an explicit `enabled = false` — that is the
  user's answer, it lives in the config file, and not even `--all` rewrites it.
  `detect.json` in the cache dir remembers which vendors were already checked so
  repeat runs probe nothing; because it is a cache file and may be deleted, it
  is a shortcut, not the thing protecting a decision.
  `config::enable_vendors_in` and `VendorId::config_section` are the shared
  toml_edit write path the TUI Settings overlay now reuses, with a guard test
  that every section name parses to its own vendor's `enabled` switch.

- `ai-usagebar detect [--all] [--json]` runs that local provider detection
  from the command line, for the user or for any frontend that reads
  `usage --json` and wants a first run to show the tools that are actually
  installed. `--all` re-checks vendors already seen; `--json` prints
  `{"enabled": [...], "known": [...], "probed": n}` with vendor slugs and no
  paths or secrets.

- `usage --json` metrics carry `window_secs`, the exact length of the reset
  window, for the vendors that state it (Anthropic, Codex, Z.AI, Antigravity,
  MiniMax, Kimi, SuperGrok weekly, and Cursor when the API sends both
  `billingCycleStart` and `billingCycleEnd`); absent otherwise — an unstated
  window omits the field rather than guessing — so a frontend that reads the
  report can pace a metric without a per-vendor window table of its own.

- **Custom providers.** `[[custom]]` tables in `config.toml` declare a
  provider from a JSON endpoint and a static token: `url`, `api_key_env` /
  `api_key`, optional auth header/scheme and extra headers, a literal or
  pointed `plan`, and `[[custom.metrics]]` / `[[custom.texts]]` mapped with
  RFC 6901 JSON Pointers (`used` + `limit` or `percent`, `resets_at` as RFC
  3339 or epoch seconds/milliseconds, `window_secs`). Each gets a TUI tab and
  a `usage --json` entry (`custom:<id>`) with the same cache, severity and
  reset metadata as a built-in vendor, so every frontend that reads
  `usage --json` shows it. The cache stores the projected snapshot, not the
  response body. Validation rejects duplicate ids and short names, non-https
  URLs (unless `allow_http`), bad pointers and header names; the token's env
  var is scrubbed from child processes. Not covered: OAuth, the Waybar
  `--vendor` list, the TUI Settings overlay, `[ui] primary` and the `vendors`
  catalog.

- **Antigravity with the app closed.** When no Antigravity product answers
  locally, the vendor reads the Google OAuth session Antigravity saved in the
  OS keyring (Windows Credential Manager `gemini:antigravity`, macOS
  Keychain, `secret-tool` on Linux), refreshes it through Google when it
  expired (cached in `antigravity/oauth.json`, never written back to the
  keyring) and asks the Cloud Code API for the same quota summary the local
  RPC serves, plus the plan from `loadCodeAssist`. The TUI panel and every
  frontend that reads `usage --json` carry a "Source · Google API (app
  closed)" row on that path; the "no local server found" error only remains
  when there is no saved session either, and a server that is up but signed
  out is still reported as such. Renewing the session needs `[antigravity]
  oauth_client_id` / `oauth_client_secret` (Antigravity's public
  installed-app client, not shipped in source); without them the fallback
  lasts while the saved access token does.

- **Windows system-tray popover.** `ai-usagebar-tray` shows a NotifyIcon whose
  left-click opens an OpenUsage-style dashboard fed in-process by
  `usage --json`: a 320 px panel that sizes itself to its content, provider
  sections with the provider mark, name and plan over a grouped card, capsule
  meters in blue / yellow / red with a `52% left ⟷ Resets in 4d 17h` line
  under each bar (click either side to flip Used/Left or countdown/exact time
  everywhere), spend rows, and a pace note ("~12% spare", "Limit in 1h 53m")
  with a tick on the meter for every metric whose `window_secs` is known.
  Right-click the icon for Refresh, Detect Providers, Open TUI, Start with
  Windows, and Quit; right-click a row to hide it, move it between Always
  Visible and On Demand, refresh just that provider or open its Customize
  screen. Customize orders and hides providers and rows; Settings has Launch
  at Login, **Refresh Every** (1, 5 or 10 minutes), a **Global Shortcut**
  recorded in place that toggles the popover from any window, Theme, Density,
  Time Format, Show Usage As, Reset Times and Always Show Pacing. Before its
  first report the tray runs local provider detection and turns on the
  vendors that already have a credential on this PC; the first report seeds
  the layout the way OpenUsage does (providers whose only error is a missing
  key start hidden behind a welcome card). Errors show a short title and a
  next step instead of an HTTP status line, and a rate-limited vendor says
  when it retries. `config.toml` gains a `[tray]` section (`shortcut`,
  `refresh_minutes`), written by the popover's Settings. The UI is a Vite +
  React + shadcn app in `windows/popover/` with a Node contract test on its
  view-model; the host is Windows-only, so Linux and macOS builds never pull
  WebView2 or GTK. The release workflow gains a Windows job that publishes
  `ai-usagebar-windows-x86_64.zip` (tray, CLI and TUI) with a `.sha256`.
  The NotifyIcon is a bar-chart-in-circle mark shipped as anti-aliased
  rasters at 16/20/24/32/40/48 px and picked by `SM_CXSMICON`, so the shell
  never resamples it; `node windows/icon/rasterize.js` regenerates them from
  `windows/tray-icon.svg`. The SuperGrok (`grok agent stdio`) and Copilot
  (`gh auth token`) children run with `CREATE_NO_WINDOW`, so a refresh from a
  GUI process no longer flashes a console that takes the foreground.

- **Windows tray: in-app updates.** **Settings → Updates** (Automatic /
  Notify me / Off; `[tray] updates`, default `notify`) checks GitHub Releases
  of the repository in `Cargo.toml` (`CARGO_PKG_REPOSITORY`) once an hour.
  Notify shows a dashboard banner with an Install button (✕ snoozes that
  version; a blue dot by the footer version remembers it) and **Check Now**
  in Settings says when the last check ran; Automatic installs unattended.
  Installing downloads the bare `*-windows-x86_64.exe` assets, verifies each
  against its `.sha256` sidecar (integrity, not authenticity), swaps the
  binaries beside the running exe leaving `ai-usagebar-tray.exe.old` for the
  next start to remove, and relaunches — the relaunched process waits for
  the old one to release the single-instance mutex. Debug builds check but
  refuse to install. The Windows release job now also publishes the bare
  exes and their sidecars next to the zip; the first release cut after this
  change is the first one the tray can install.

### Fixed

- **Rate-limit backoff.** A vendor that answers HTTP 429 arms a five-minute
  backoff in its cache dir (`.retry_after`). While it is armed
  `Cache::fresh_payload` — the one pre-network step every vendor takes —
  serves the last good snapshot if there is one and otherwise reports
  `rate limited; next attempt in 4m` without touching the network, so the
  60-second poll no longer prolongs the block. A successful fetch clears it.
  Nous Research has its own fetch path without the shared cache and is not
  covered. Frontends that read `usage --json` see the same message in the
  vendor's error field.

- Claude error cards in `usage --json` and the TUI keep the OAuth plan label
  when the usage endpoint fails, so a 401/429 still shows Max/Pro instead of a
  plan-less error. Quotas are not invented; only the label from
  `~/.claude/.credentials.json` is kept.

## [1.13.0] — 2026-09-08

### Added

- The macOS menu bar can show *when* a window resets — a wall-clock time, or a
  date once the reset is past today — instead of the countdown, under
  **Preferences → Display**. Off by default; the countdown is unchanged unless
  you turn it on. It follows the system's 12h/24h convention.



- **`ai-usagebar vendors --json`** — the provider catalog: one row per
  provider with how it authenticates (`oauth` / `apikey` / `local`), whether
  config has it `enabled`, whether this machine holds the credential it needs
  (`configured`), the environment variable it reads (honoring an `api_key_env`
  override), and the `login` command that fixes it. It contacts nothing.
  `usage --json` reports only *enabled* providers, so the switched-off and the
  never-credentialed were exactly the rows a "is anything broken?" list could
  not describe; this is the answer for them. `needs_credential` is `false` only
  for Antigravity, which has no credential to be missing, so a frontend never
  offers to fix one that cannot be.


- **Omarchy bar: show every provider at once.** A new **Show all providers in
  the top bar** toggle (and `showAll` widget setting) draws each configured
  provider as its own chip with a brand mark and usage. Claude, Codex,
  Copilot, Grok/SuperGrok, DeepSeek, Kimi, Cursor, OpenRouter, MiniMax,
  Moonshot, Z.AI, Kilo, Novita, Antigravity, Kiro, Nous, and OpenCode Go
  ship an SVG; Command Code (no public mark) falls back to its three-letter
  code rather than a shared robot. The panel hero uses the same mark,
  colored only when that provider is critical. Off by default.

- `--config <PATH>` on both binaries to read and write an alternate config
  file instead of the default location. Accepted in any position (including
  beside a subcommand); the file must already exist, and the override applies
  to loads, Settings saves, and path hints for the whole process.

### Changed

- The macOS menu bar and the GNOME extension are in English. Both shipped with
  a Brazilian Portuguese UI while the Rust core, the Omarchy panel and the KDE
  plasmoid were already English, so the project read as two different products
  depending on which surface you opened. Display strings only — no setting key,
  comparison, or stored value changed — and the macOS test that asserted a
  Portuguese label moves with it.



- `KEY_VENDORS` no longer stores each provider's environment variable name: it
  comes from `VendorId::api_key_env`, and `Config::api_key_env_for` /
  `Config::inline_api_key` replaced two private helpers that matched on a
  section *string* with a `_ =>` fallback arm — where a new key vendor nobody
  added would silently read the wrong default and report as unconfigured for
  ever. Both match on `VendorId`, so that case now fails to compile.

### Fixed

- The macOS menu bar icon no longer disappears mid-session. `AppMain` held its
  `AppDelegate` in a `main()` local, and `NSApplication.delegate` is a *weak*
  reference — so in optimised builds ARC was free to release it after the
  assignment, since nothing later in the function mentions it, taking the
  status item with it. The delegate is now held for the program's lifetime.


## [1.12.0] — 2026-09-06

### Added

- Command Code renders the monthly credit allowance as a full third window:
  the Quattro panel, TUI, and Overview show a `Monthly` progress row with the
  derived spend (`$20.72 of $70.00`), a `Resets` countdown from the
  subscription's billing period end, and the Overview gains a monthly mini
  bar. New placeholders `{cc_monthly_pct}`, `{cc_monthly_reset}`,
  `{cc_monthly_used}`, `{cc_monthly_cap}`, and `{cc_credits_reset}`; the bar
  headline and severity now consider the monthly window when it is the
  closest to its cap. An unrecognised plan or a missing ledger leaves the
  row out rather than guessing a denominator.

- The KDE plasmoid offers a **one card per vendor** popup layout beside the
  existing provider tabs, selectable per applet instance. The cards are drawn
  entirely from the aggregate `usage --json` report — labels, windows,
  severities, staleness and error text all come from Rust — so a newly added
  provider gets a card with no widget change. Provider tabs remain the
  default and are unchanged.

### Changed

- `make test` fails if a changelog entry appears under two versions, or if one
  release section repeats a category heading. Both are what a merge produces
  when a branch predates the last tag, and both had happened here before — the
  documented remedy was a manual `git diff` that the AUR build cannot run and
  that a person has to remember.

### Fixed

- **Codex works again on accounts with no extra limits.** 1.11.0 added
  `additional_rate_limits` and `model_usage` as plain collections, and OpenAI
  sends `null` — not `[]`/`{}` — when an account has none. `#[serde(default)]`
  covers a *missing* field but not a present-but-null one, so the whole usage
  response failed and Waybar showed `⚠ API schema drift … expected a sequence`.
  Explicit `null` is now read as empty for those two and for
  `rate_limit_reset_credits.credits`. A wrong *type* is still drift: a string or
  a number where a collection belongs is refused rather than read as empty.
  Reported within a day by three people independently — thank you.
## [1.11.0] — 2026-09-05

### Security

- Codex's cache no longer holds the account's `user_id`, `account_id` and
  `email`. It stored the raw `wham/usage` body, so all three sat in
  `~/.cache/ai-usagebar/openai/usage.json` for the life of the TTL, though no
  renderer reads any of them. It now stores the parsed response, which is an
  allowlist by construction — a field OpenAI adds later cannot start living on
  disk without someone adding it to the type first. The file was, and remains,
  mode 0600. This is the rule `CLAUDE.md` already stated for Command Code.

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

[Unreleased]: https://github.com/akitaonrails/ai-usagebar/compare/v1.34.0...HEAD
[1.34.0]: https://github.com/akitaonrails/ai-usagebar/compare/v1.33.0...v1.34.0
[1.33.0]: https://github.com/akitaonrails/ai-usagebar/compare/v1.32.0...v1.33.0
[1.32.0]: https://github.com/akitaonrails/ai-usagebar/compare/v1.31.0...v1.32.0
[1.31.0]: https://github.com/akitaonrails/ai-usagebar/compare/v1.30.0...v1.31.0
[1.30.0]: https://github.com/akitaonrails/ai-usagebar/compare/v1.29.0...v1.30.0
[1.29.0]: https://github.com/akitaonrails/ai-usagebar/compare/v1.28.0...v1.29.0
[1.28.0]: https://github.com/akitaonrails/ai-usagebar/compare/v1.27.0...v1.28.0
[1.27.0]: https://github.com/akitaonrails/ai-usagebar/compare/v1.26.0...v1.27.0
[1.26.0]: https://github.com/akitaonrails/ai-usagebar/compare/v1.25.0...v1.26.0
[1.25.0]: https://github.com/akitaonrails/ai-usagebar/compare/v1.24.0...v1.25.0
[1.24.0]: https://github.com/akitaonrails/ai-usagebar/compare/v1.23.0...v1.24.0
[1.23.0]: https://github.com/akitaonrails/ai-usagebar/compare/v1.22.0...v1.23.0
[1.22.0]: https://github.com/akitaonrails/ai-usagebar/compare/v1.21.1...v1.22.0
[1.21.1]: https://github.com/akitaonrails/ai-usagebar/compare/v1.21.0...v1.21.1
[1.21.0]: https://github.com/akitaonrails/ai-usagebar/compare/v1.20.2...v1.21.0
[1.20.2]: https://github.com/akitaonrails/ai-usagebar/compare/v1.20.1...v1.20.2
[1.20.1]: https://github.com/akitaonrails/ai-usagebar/compare/v1.19.0...v1.20.1
[1.19.0]: https://github.com/akitaonrails/ai-usagebar/compare/v1.18.1...v1.19.0
[1.18.1]: https://github.com/akitaonrails/ai-usagebar/compare/v1.18.0...v1.18.1
[1.18.0]: https://github.com/akitaonrails/ai-usagebar/compare/v1.17.1...v1.18.0
[1.17.1]: https://github.com/akitaonrails/ai-usagebar/compare/v1.17.0...v1.17.1
[1.17.0]: https://github.com/akitaonrails/ai-usagebar/compare/v1.16.0...v1.17.0
[1.16.0]: https://github.com/akitaonrails/ai-usagebar/compare/v1.15.0...v1.16.0
[1.15.0]: https://github.com/akitaonrails/ai-usagebar/compare/v1.14.0...v1.15.0
[1.14.0]: https://github.com/akitaonrails/ai-usagebar/compare/v1.13.0...v1.14.0
[1.13.0]: https://github.com/akitaonrails/ai-usagebar/compare/v1.12.0...v1.13.0
[1.12.0]: https://github.com/akitaonrails/ai-usagebar/compare/v1.11.0...v1.12.0
[1.11.0]: https://github.com/akitaonrails/ai-usagebar/compare/v1.10.0...v1.11.0
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
