# Changelog

All notable changes to **ai-usagebar** are recorded here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

Each release is also published at
<https://github.com/akitaonrails/ai-usagebar/releases>.

## [Unreleased]

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

[Unreleased]: https://github.com/akitaonrails/ai-usagebar/compare/v0.7.1...HEAD
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
