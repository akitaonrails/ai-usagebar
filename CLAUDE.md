# CLAUDE.md

Notes for Claude Code (and humans) about working in this repo. Keep tight:
these are invariants we keep almost-forgetting, not a project tour.

## Release checklist — must do all of these

When cutting a new version (patch, minor, or major):

1. **Bump both versions** — `Cargo.toml` `version` and the root Omarchy
   `manifest.json` `version` must match the release tag.
2. **Do not add a changelog file.** Release notes are generated from the
   commits and PR titles between tags.
3. **Run gate before tagging**:
   ```
   make test                                   # cargo test + Omarchy plugin contract
   cargo clippy --all-targets -- -D warnings
   cargo machete
   omarchy plugin validate .
   ```
4. **Commit, tag, push**:
   ```
   git commit -m "vX.Y.Z — …"
   git tag -a vX.Y.Z -m "vX.Y.Z — …"
   git push origin main && git push origin vX.Y.Z
   ```
5. **Wait for CI**: `.github/workflows/release.yml` builds Linux tarballs and
   publishes a GitHub Release. This fork does not publish AUR or crates.io.

This is an Omarchy-only fork. After merging `upstream`, run
`./scripts/strip-platforms.sh` so GNOME/KDE/macOS/Nix/AUR trees stay gone.
See [FORK.md](FORK.md).

Tags are immutable; do **not** force-move a tag once it's pushed. Cut a new
patch version instead.

## Hard invariants — never break these

- **Widget always exits 0.** Consumers hide modules that don't. Wrap
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
  `*.credentials.json`, and `.claude/`.
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
  that disagreed for five vendors. The one sanctioned synthesized error is
  `handle_auth_failure`'s — "run `claude`/`codex login` to re-auth" is
  actionable where the underlying OAuth error is not.
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
4. Tag a patch release if the parser change ships. This fork does not
   publish AUR packages.

## What lives where

- `src/active.rs` — scroll-cycle active vendor state file
- `src/anthropic/`, `src/openai/`, `src/openrouter/`, `src/zai/`,
  `src/deepseek/` — per-vendor types + fetch + render
- `src/antigravity/` — Google Antigravity. Unlike every other vendor it has
  no credential and no remote endpoint: quota comes from whichever local
  Antigravity product is running (2.0, the IDE, or an interactive `agy`
  session), over a loopback RPC on a **dynamically assigned** port that is
  discovered from `/proc` on Linux, `lsof` on macOS, and the process/TCP-table
  APIs on Windows. `ANTIGRAVITY_LS_ADDRESS` is a *first* candidate, not an
  exclusive one — discovered ports are still probed behind it, so a stale
  override degrades to a slower success instead of a hard failure.
  Discovered ports are grouped per pid and emitted rank by rank (`probe_order`),
  so with two products up every RPC listener is probed before any TLS one.
  Tests must never probe `/proc`, `lsof` or the wall clock — use
  `candidate_bases_with`, `probe_order`, `matching_windows_ports`,
  `parse_lsof_pcn` and `parse_cache_at`/`fetch_snapshot_at`, not their
  production wrappers.
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
- `manifest.json`, `omarchy/` — Omarchy 4 / Quattro plugin (`bradflaugher.ai-usagebar`)
- `src/tooltip.rs` — shared Pango bordered-box renderer (used by
  every vendor's tooltip)
- `.github/workflows/release.yml` — tag-driven Linux release (x86_64 + aarch64)
- `tests/anthropic_e2e.rs` — mockito + insta snapshot tests
- `tests/live.rs` — `#[ignore]`d smoke tests against real APIs
