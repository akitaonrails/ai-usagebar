# CLAUDE.md

Notes for Claude Code (and humans) about working in this repo. Keep tight:
these are invariants we keep almost-forgetting, not a project tour.

## Release checklist — must do all of these

When cutting a new version (patch, minor, or major):

1. **Bump `Cargo.toml` `version`** — e.g. `0.3.2` → `0.3.3`.
2. **Update `CHANGELOG.md`**:
   - Add a new `## [X.Y.Z] — YYYY-MM-DD` section above the previous one.
   - Categorize entries by **Added / Changed / Fixed / Security** (Keep-A-Changelog).
   - Update the `[Unreleased]` compare link and add a new release link at the bottom.
3. **Bump `packaging/aur/PKGBUILD`** — `pkgver=X.Y.Z`, `pkgrel=1`, reset `sha256sums` to `'SKIP'`.
4. **Bump `packaging/aur/PKGBUILD-bin`** — same `pkgver`, `pkgrel=1`, reset both
   `sha256sums_x86_64` and `sha256sums_aarch64` to `'SKIP'`.
5. **Run gate before tagging**:
   ```
   cargo test                                  # 200+ tests must pass
   cargo clippy --all-targets -- -D warnings   # clean
   cargo machete                               # no unused deps
   ```
6. **Commit, tag, push**:
   ```
   git commit -m "vX.Y.Z — …"
   git tag -a vX.Y.Z -m "vX.Y.Z — …"
   git push origin main && git push origin vX.Y.Z
   ```
7. **Wait for CI** (3–5 min): the tag push auto-triggers
   `.github/workflows/release.yml` which builds both x86_64 and
   aarch64 tarballs and publishes a GitHub Release.
8. **Pin the real sha256s** in both PKGBUILDs:
   ```
   cd packaging/aur
   # Source:
   curl -sLO https://github.com/akitaonrails/ai-usagebar/archive/refs/tags/vX.Y.Z.tar.gz
   sha256sum vX.Y.Z.tar.gz   # paste into PKGBUILD
   # Bin x86_64:
   curl -sL https://github.com/akitaonrails/ai-usagebar/releases/download/vX.Y.Z/ai-usagebar-linux-x86_64.tar.gz.sha256
   # Bin aarch64:
   curl -sL https://github.com/akitaonrails/ai-usagebar/releases/download/vX.Y.Z/ai-usagebar-linux-aarch64.tar.gz.sha256
   ```
9. **Regenerate `.SRCINFO`s**:
   ```
   cd packaging/aur && makepkg --printsrcinfo > .SRCINFO
   # And from a scratch dir with the bin PKGBUILD: makepkg --printsrcinfo > .SRCINFO-bin
   ```
10. **AUR push is automated via CI** when `AUR_SSH_KEY` is set (since
    v0.4.4). The `publish-aur` job in `.github/workflows/release.yml`
    runs after `build` + `release` succeed, pins the real sha256s into
    both PKGBUILDs, and pushes via `KSXGitHub/github-actions-deploy-aur`.
    Step 10 below is the manual fallback when the secret isn't
    configured or when CI is unavailable.

    **Manual fallback** — separate AUR git repos:
    - `~/Projects/aur-ai-usagebar` → `ssh://aur@aur.archlinux.org/ai-usagebar.git`
    - `~/Projects/aur-ai-usagebar-bin` → `ssh://aur@aur.archlinux.org/ai-usagebar-bin.git`

    **Always `git fetch origin && git reset --hard origin/master` in each
    AUR clone first.** A previous session may have pushed an intermediate
    release that your local clone never saw — in which case naively
    committing on top diverges and produces a non-trivial rebase
    conflict. The clones are throwaway: reset, then overlay the canonical
    `packaging/aur/PKGBUILD*` + regen'd `.SRCINFO*` from the main repo,
    commit, push.

**Anything skipping any of 1–10 is an incomplete release.** Tags are
immutable; do **not** force-move a tag once it's pushed. Cut a new
patch version instead.

## Hard invariants — never break these

- **Widget always exits 0.** Waybar hides modules that don't. Wrap
  every error in a fallback `⚠` JSON. See `widget::run::fallback`.
- **Cache writes are atomic** (tempfile + persist). Multi-monitor
  Waybar instances coexist via per-vendor `flock`.
- **Tag immutability.** Never `git push --force origin vX.Y.Z` once a
  release is public. The one-time exception in v0.3.0 was a mistake.
- **No secrets in tracked files.** Inline API keys in config.toml are
  the user's choice (and `chmod 600`ed by the Settings overlay), but
  **never commit** a real key. The `.gitignore` covers `.env`,
  `*.credentials.json`, and `.claude/`.
- **Tests are hermetic.** A `#[test]`/`#[tokio::test]` must never read or
  write a real `$HOME`/`$XDG` path (config, cache, creds, Omarchy theme)
  or branch on an ambient env var — the AUR `check()` runs `cargo test`
  during `makepkg`, so any test that reads a user's *customized* files
  fails the install on their machine. Always inject the path/dependency
  via the test seam, never the real-path resolver:
  `Cache::at` not `for_vendor`, `active::{read_from,write_to,cycle_at}`
  not `{read,write,cycle}`, `creds::read_from` not `default_path`,
  `Theme::merged_with_omarchy_file` not `merged_with_omarchy`,
  `Cli::resolve_vendor_with` not `resolved_vendor`, `App::with_theme`
  not `new`. Live API tests stay behind `#[ignore]` (see `tests/live.rs`).
  *Carve-out:* a test that asserts the path *resolver itself* honors the
  OS convention may read the env var it's testing (e.g. the Windows-gated
  `default_path_uses_userprofile_on_windows` reads `%USERPROFILE%`) —
  USERPROFILE is the production input being verified, not ambient state
  the test is incidentally coupled to.

## Secret-discipline rules (learned the hard way)

Two separate leaks in early sessions where real keys appeared in the
Claude conversation transcript (never on disk/GitHub/AUR, but still
worth rotating):

- **Never `cat` a config file** that could contain `api_key` / `token`
  / OAuth credentials. Use `jq 'keys'` for structure, or
  `grep -v 'api_key\|token\|secret\|password'` to show non-secret lines.
- **Never `env | grep …`** without a tight filter. Even
  `env | grep -E "^(RUST|CARGO|LD)"` matched `AWS_*` and
  `WHATSAPP_*` once because of shared substring patterns. Prefer
  `printenv VAR | sed 's|.*|<value-set>|'` per variable.
- **For OAuth credential files** (`~/.claude/.credentials.json`,
  `~/.codex/auth.json`): `jq 'keys'` only.

## Live API smoke discipline

`make smoke` exercises real undocumented endpoints (Anthropic OAuth,
OpenAI Codex OAuth, Z.AI monitor). If the smoke test fails after a
vendor's response shape drifts:

1. Capture the actual response (`curl -sH "Authorization: …" …`).
2. Update the matching `types.rs` in `src/{anthropic,openai,zai,openrouter,deepseek}/`.
3. Re-run `make smoke` until green.
4. **Bump pkgrel (not pkgver) in both PKGBUILDs** — the user-visible
   functionality is unchanged; it's a packaging update tracking a
   silent upstream change.

## What lives where

- `src/active.rs` — scroll-cycle active vendor state file
- `src/anthropic/`, `src/openai/`, `src/openrouter/`, `src/zai/`,
  `src/deepseek/` — per-vendor types + fetch + render
- `src/antigravity/` — Google Antigravity. Unlike every other vendor it has
  no credential and no remote endpoint: quota comes from whichever local
  Antigravity product is running (2.0, the IDE, or an interactive `agy`
  session), over a loopback RPC on a **dynamically assigned** port that is
  discovered from `/proc` (Linux only; elsewhere set `ANTIGRAVITY_LS_ADDRESS`).
  Tests must never probe `/proc` or the wall clock — use `candidate_bases_with`
  and `parse_cache_at`/`fetch_snapshot_at`, not their production wrappers.
- `src/anthropic/keychain.rs` — macOS-only `security(1)` fallback when
  `~/.claude/.credentials.json` is absent (Claude Code on macOS stores
  the OAuth blob in the login Keychain). Module-gated with
  `#[cfg(target_os = "macos")]`; Linux build never compiles it.
- `src/cache.rs` — atomic per-vendor cache writes + flock, plus the shared
  cross-platform path resolvers (`xdg_cache_dir`, `home_dir`). `home_dir`
  resolves `$HOME` / `%USERPROFILE%` via `directories::BaseDirs` and is reused
  by both OAuth-credential vendors (`anthropic`, `openai`) so the OS convention
  lives in one place.
- `src/context/` — opt-in, bounded reader for local Claude Code JSONL
  transcripts. This format is best-effort and schema-tolerant; tests must use
  `scan_dir(&Path)` with a temp directory and never inspect a real user history.
- `src/tui/settings.rs` — Settings overlay (toml_edit-backed,
  auto-signals waybar after save)
- `src/tui/panels.rs` — native ratatui per-vendor panels
- `src/widget/` — Waybar widget shell (CLI, render, pretty, run)
- `src/tooltip.rs` — shared Pango bordered-box renderer (used by
  every vendor's tooltip)
- `packaging/aur/PKGBUILD` — source-build AUR pkg
- `packaging/aur/PKGBUILD-bin` — prebuilt-binary AUR pkg (multi-arch)
- `.github/workflows/release.yml` — tag-driven release (x86_64 + aarch64)
- `tests/anthropic_e2e.rs` — mockito + insta snapshot tests
- `tests/live.rs` — `#[ignore]`d smoke tests against real APIs
