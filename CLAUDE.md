# CLAUDE.md

Notes for Claude Code (and humans) about working in this repo. Keep tight:
these are invariants we keep almost-forgetting, not a project tour.

## Release checklist — must do all of these

When cutting a new version (patch, minor, or major):

1. **Bump both versions** — `Cargo.toml` `version` and the root Omarchy
   `manifest.json` `version` must match the release tag.
2. **Update `CHANGELOG.md`**:
   - Add a new `## [X.Y.Z] — YYYY-MM-DD` section above the previous one.
   - Categorize entries by **Added / Changed / Fixed / Security** (Keep-A-Changelog).
   - Update the `[Unreleased]` compare link and add a new release link at the bottom.
   - **Prove no published section moved**, before tagging. Compare the newest
     released section against its own tag, byte for byte:
     ```
     prev=$(git describe --tags --abbrev=0)
     diff <(git show "$prev:CHANGELOG.md" | sed -n "/^## \[${prev#v}\]/,/^## \[/p") \
          <(sed -n "/^## \[${prev#v}\]/,/^## \[/p" CHANGELOG.md)
     ```
     Any output means the released section changed. **It must compare, not
     grep for removals.** The first version of this check greped `^-` and
     therefore only caught a *rewritten* entry; #129 branched before v1.8.0 and
     its `[Unreleased]` bullet merged in as a pure *insertion*, adding a feature
     to a shipped release with no removed line for the grep to find. It passed
     clean while the section was wrong.

     **`make changelog-check` now does this comparison for you** — it diffs
     every released section against its own tag and refuses a version file
     that moved backwards, which is the other half of the same hazard (four
     branches once came back reading 1.12.0 after v1.13.0 shipped). Run it
     instead of composing the diff by hand; the manual form below is kept
     because it documents what the script checks.

     `make test` also enforces part of this without git: a guard test fails if
     any changelog entry appears under two versions, or if one section repeats
     a category heading. The manual comparison above stays because it is
     stronger for the newest section — the guard cannot tell a *reworded*
     entry from a new one — but the automated check is what catches a
     conflict resolution that quietly copies an entry into a published
     section, which is how it happened the third time.

     The cause is the same every time: a branch that predates the last tag
     carries its entries under `[Unreleased]`, and git merges them *cleanly*
     into whatever now sits at that position — which is the section you just
     published. It happened to v1.6.0 (#127), to v1.8.0 (#129), and to
     v1.11.0 when a maintainer resolved #152's conflict with a script. A clean
     merge is not evidence here; the comparison is, and now the guard is too.
3. **Bump `packaging/aur/PKGBUILD`** — `pkgver=X.Y.Z`, `pkgrel=1`, reset `sha256sums` to `'SKIP'`.
4. **Bump `packaging/aur/PKGBUILD-bin`** — same `pkgver`, `pkgrel=1`, reset both
   `sha256sums_x86_64` and `sha256sums_aarch64` to `'SKIP'`.
5. **Regenerate both `.SRCINFO`s NOW, before tagging** — the release
   workflow's `verify-version` job rejects the tag if `packaging/aur/.SRCINFO`
   or `.SRCINFO-bin` still carry the old `pkgver` (learned at v0.17.0, which
   never shipped for exactly this reason):
   ```
   cd packaging/aur && makepkg --printsrcinfo > .SRCINFO
   # PKGBUILD-bin must be named PKGBUILD for makepkg — use a scratch dir:
   t=$(mktemp -d) && cp PKGBUILD-bin "$t/PKGBUILD" &&
     (cd "$t" && makepkg --printsrcinfo > .SRCINFO-bin) &&
     cp "$t/.SRCINFO-bin" .SRCINFO-bin && rm -rf "$t"
   ```
   The committed files keep `sha256sums = SKIP`; CI pins the real hashes later.
6. **Run gate before tagging**:
   ```
   make changelog-check                        # released sections + version files intact
   make test                                   # cargo test + the desktop JS gate
   cargo clippy --all-targets -- -D warnings   # clean
   cargo machete                               # no unused deps
   omarchy plugin validate .                   # plugin manifest + entry points
   ```
   `make test` rather than `cargo test`: it also runs the GNOME, KDE, and
   Omarchy frontend contract suites. If `kde-plasmoid/` changed, also bump
   `KPlugin.Version` in `kde-plasmoid/package/metadata.json`; it is versioned
   independently of `Cargo.toml`, like the GNOME `metadata.json`.
7. **Commit, tag, push**:
   ```
   git commit -m "vX.Y.Z — …"
   git tag -a vX.Y.Z -m "vX.Y.Z — …"
   git push origin main && git push origin vX.Y.Z
   ```
8. **Wait for CI** (3–5 min): the tag push auto-triggers
   `.github/workflows/release.yml` which builds both x86_64 and
   aarch64 tarballs and publishes a GitHub Release.
9. **AUR push is automated via CI** when `AUR_SSH_KEY` is set (since
   v0.4.4). The `publish-aur` job in `.github/workflows/release.yml`
   runs after `build` + `release` succeed, pins the real sha256s into
   both PKGBUILDs (steps 3-4's `'SKIP'`s), regenerates the `.SRCINFO`s,
   and pushes via `KSXGitHub/github-actions-deploy-aur`. The manual
   fallback below is for when the secret isn't configured or CI is
   unavailable.

   **Manual fallback** — pin the real sha256s in both PKGBUILDs first:
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
   Then regenerate the `.SRCINFO`s exactly as in step 5 (now with the
   real hashes), and push to the separate AUR git repos:
   - `~/Projects/aur-ai-usagebar` → `ssh://aur@aur.archlinux.org/ai-usagebar.git`
   - `~/Projects/aur-ai-usagebar-bin` → `ssh://aur@aur.archlinux.org/ai-usagebar-bin.git`

   **Always `git fetch origin && git reset --hard origin/master` in each
   AUR clone first.** A previous session may have pushed an intermediate
   release that your local clone never saw — in which case naively
   committing on top diverges and produces a non-trivial rebase
   conflict. The clones are throwaway: reset, then overlay the canonical
   `packaging/aur/PKGBUILD*` + regen'd `.SRCINFO*` from the main repo,
   commit, push.

**Anything skipping any of 1–9 is an incomplete release.** Tags are
immutable; do **not** force-move a tag once it's pushed. Cut a new
patch version instead.

## Hard invariants — never break these

- **Widget always exits 0.** Waybar hides modules that don't. Wrap
  every error in a fallback `⚠` JSON. See `widget::run::fallback`.
- **Cache writes are atomic** (tempfile + persist). Multi-monitor
  Waybar instances coexist via per-vendor `flock`.
- **Tag immutability.** Never `git push --force origin vX.Y.Z` once a
  release is public. The one-time exception in v0.3.0 was a mistake.
- **Untrusted text is sanitized at the sink, not at the call site.** A
  subprocess's stderr, a vendor response, and a path carrying an account label
  are all data, not terminal programs. `pango::escape` and the TUI already
  sanitize what they render; `AppError::Io`'s `Display` sanitizes its path so
  every one of ~94 sites is covered. The gap is plain `println!`/`eprintln!`:
  anything reaching one — `claude_desktop` notes especially — goes through
  `display::sanitize_untrusted_{line,path}` first. A guard test forbids a bare
  `.display()` inside `notes.push`. Note the exception it documents: a path
  used as an *argument* (tar members) must stay raw, or the filename breaks.
- **No secrets in tracked files.** Inline API keys in config.toml are
  the user's choice (and `chmod 600`ed by the Settings overlay), but
  **never commit** a real key. The `.gitignore` covers `.env`,
  `*.credentials.json`, and `.claude/`. That includes secret-*shaped*
  public values: Antigravity's installed-app OAuth client secret is public by
  Google's definition, yet it lives in the user's config, never in source,
  because a literal with Google's client-secret prefix trips every secret
  scanner for ever. Test fixtures use plain words (`test-client`).
- **One fetch outcome, one fallback policy.** `outcome::Outcome<T>` is the
  four-field record every vendor returns (`VendorOutcome` is
  `Outcome<VendorSnapshot>`; each vendor's `FetchOutcome` is an alias, so
  `outcome.map(VendorSnapshot::Whichever)` is the whole conversion). Build one
  with `Outcome::fresh` off the wire or `Outcome::cached` out of the cache.
  `outcome::fallback` owns what a *failed* refresh means: serve the last good
  payload, or return the error that caused the failure — never a synthesized
  message about the cache, because with no figure on screen the error is the
  whole output. A cached payload that will not parse counts as nothing to
  show and also reports the original. Each vendor supplies only a closure that
  parses its own cache format. A guard test forbids a second caller of
  `Cache::fallback_payload`: that function is the entry point to this decision,
  and eighteen private copies of it had already drifted into two generations
  that disagreed for five vendors. Two synthesized errors are sanctioned, both
  on the same ground — they tell the user something actionable that the
  underlying failure does not. `handle_auth_failure`'s "run `claude`/`codex
  login` to re-auth", and `fresh_payload`'s "rate limited; next attempt in 4m"
  after a 429 armed the backoff. The second is a *pre-network refusal*, not a
  failed refresh: it is reached only when there is also no payload inside
  `MAX_STALE` to show instead, so it never replaces a figure with a note about
  the cache. Anything beyond these two returns the error that caused it.
- **Frontend adapters stay thin.** Provider fetching, credentials, canonical
  product names, metric projection, and reset metadata belong in Rust.
  `VendorId::display_name` is the shared label source; do not add a complete
  provider-name table to a frontend. `format::{money, usd}` is the shared money
  source — a balance can be negative (OpenRouter overrun, Moonshot
  `cash_balance`) and the sign belongs outside the symbol, so never reach for
  `format!("${v:.2}")`; it had regrown into four disagreeing copies once
  already. Build report metrics through
  `SectionBuilder::push_metric` so the absolute reset travels with its row;
  never recreate a per-vendor metric-order table in `report.rs`.
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
- `src/antigravity/` — Google Antigravity. Two sources, local first: quota
  comes from whichever local Antigravity product is running (2.0, the IDE, or
  an interactive `agy` session), over a loopback RPC on a **dynamically
  assigned** port that is discovered from `/proc` on Linux, `lsof` on macOS,
  and the process/TCP-table APIs on Windows. `ANTIGRAVITY_LS_ADDRESS` is a
  *first* candidate, not an exclusive one — discovered ports are still probed
  behind it, so a stale override degrades to a slower success instead of a
  hard failure.
  Discovered ports are grouped per pid and emitted rank by rank (`probe_order`),
  so with two products up every RPC listener is probed before any TLS one.
  With no local server at all (and only then — a server that is up but
  signed out keeps its own diagnosis), `fetch.rs` falls back to the Google
  OAuth session Antigravity saved in the OS keyring (`credential.rs`:
  Credential Manager on Windows, `security` on macOS, `secret-tool` on
  Linux; read-only) and asks the Cloud Code API (`cloud.rs`) for the same
  quota summary. A refreshed token goes to the vendor cache's
  `oauth.json`, keyed by a fingerprint of the refresh token, never back to
  the keyring. The OAuth client that refresh needs is config-only.
  Tests must never probe `/proc`, `lsof`, the keyring, Google or the wall
  clock — use `candidate_bases_with`, `probe_order`,
  `matching_windows_ports`, `parse_lsof_pcn`, `parse_cache_at`, and
  `fetch_snapshot_at` with a `RemoteOverride` (`SavedCredential::Blob` /
  `Absent`, mockito `Endpoints`, `local_bases: Some(vec![])`), not their
  production wrappers. The two tests that touch the real keyring
  (`reading_the_real_windows_credential_never_errors`, `antigravity_remote_live`)
  are `#[ignore]`d.
- `src/kiro/` — Kiro CLI. Reads kiro-cli's own `data.sqlite3` (read-only) for
  the AWS SSO OIDC session, refreshes the ~1h access token via the documented
  CreateToken API, and calls the undocumented `GetUsageLimits` — same operation
  kiro-cli's `/usage` makes. Rotated credentials go to the vendor cache's
  account-scoped mode-0600 `oauth.json`, never back to kiro-cli's db. Test
  seams: `db::read_credentials(&path)` with a seeded temp db and
  `fetch::fetch_snapshot_at` with an `Endpoints` override pointed at mockito.
- `src/cursor/` — Cursor. Reads the IDE's own `state.vscdb` (read-only), with
  a fallback to the headless `cursor-agent` CLI's `auth.json` when the IDE db
  is absent — `db::resolve_access_token` tries both. Tests seed a temp db /
  auth file and pass the paths in; never touch the real ones.
- `src/anthropic/keychain.rs` — macOS-only Keychain fallback when
  `~/.claude/.credentials.json` is absent (Claude Code on macOS stores
  the OAuth blob in the login Keychain). Reads use `security(1)`; writes use
  Security.framework so OAuth JSON never enters process arguments. Module-gated with
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
- `manifest.json`, `omarchy/` — Omarchy 4 / Quattro plugin manifest, native
  Quickshell panel, pure report model, and Node contract tests
- `src/tooltip.rs` — shared Pango bordered-box renderer (used by
  every vendor's tooltip)
- `gnome-extension/marker-logic.js` — pure GNOME formatting helpers and their
  own Node contract tests.
- `kde-plasmoid/` — KDE Plasma 6 plasmoid (KPackage). Vendor selection is
  per applet instance via KConfigXT. Its single `usage --json` request omits
  `--vendor`; selection happens client-side, so it never reads
  `~/.cache/ai-usagebar/active_vendor`. The pure report adapter lives in
  `package/contents/code/plasmoid-logic.mjs` and is not a copy of the GNOME
  helpers. Test popup work with `plasmawindowed`, not `plasmoidviewer`; the
  latter never instantiates the full representation.
- `packaging/aur/PKGBUILD` — source-build AUR pkg
- `packaging/aur/PKGBUILD-bin` — prebuilt-binary AUR pkg (multi-arch)
- `.github/workflows/release.yml` — tag-driven release (x86_64 + aarch64)
- `tests/anthropic_e2e.rs` — mockito + insta snapshot tests
- `tests/live.rs` — `#[ignore]`d smoke tests against real APIs
