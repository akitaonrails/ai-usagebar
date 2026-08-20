//! Live API smoke test suite — DETECTS UNDOCUMENTED-ENDPOINT DRIFT.
//!
//! Hits the real vendor endpoints using credentials from your shell (API keys,
//! or the local CLI/IDE session files for Cursor and Kiro CLI).
//! Asserts only the *fields we depend on* so when a vendor renames or removes
//! one, the failure points at the exact field rather than dumping the whole
//! response.
//!
//! These tests are `#[ignore]` so plain `cargo test` doesn't hit external
//! APIs (and won't fail on machines without creds). Run explicitly:
//!
//! ```bash
//! source ~/.config/zsh/secrets
//! cargo test --test live -- --ignored --nocapture
//! # or:
//! make smoke                         # runs every configured live smoke test
//! cargo test --test live kimi_live -- --ignored --nocapture
//! ```
//!
//! ## When a smoke test fails
//!
//! 1. Re-run with `--nocapture` to see the actual response shape.
//! 2. Paste the response + error into Claude Code and ask it to update the
//!    affected vendor's `types.rs` to match. The error messages here are
//!    deliberately verbose so the update is mechanical.
//! 3. After updating, re-run `cargo test --test live -- --ignored` to confirm.
//!
//! ## What gets tested (only the contract we rely on)
//!
//! - **Anthropic**: `five_hour.utilization` is 0..=100, `resets_at` parses
//!   as RFC3339, `extra_usage.{is_enabled,monthly_limit,used_credits}` round-trip.
//! - **OpenAI**: `rate_limit.primary_window.used_percent` is 0..=100, the
//!   id_token's exp claim is parseable.
//! - **Z.AI**: response is a `{code, data: {limits:[...], level}, success}`
//!   envelope and at least one `TOKENS_LIMIT` entry exists.
//! - **OpenRouter**: `/credits` returns `{data:{total_credits,total_usage}}`
//!   and `/key` returns `{data:{usage,is_free_tier}}`.
//! - **Kimi**: the public snapshot exposes parsed weekly limit/used/remaining
//!   counters and a bounded percentage. Its reset and selected 5-hour rolling
//!   window are optional, so the smoke test validates their public fields only
//!   when present; the snapshot does not expose raw wire duration/unit.
//!   `kimi_live` skips when optional `KIMI_API_KEY` is unset.
//! - **Cursor**: reads the session token from the local `state.vscdb`, then
//!   asserts `premium_pct` is 0..=100 and a future `premium_reset_at` was
//!   derived from `startOfMonth`. `cursor_live` skips when there is no Cursor
//!   credential source (no state DB, no cursor-agent `auth.json`, and neither
//!   `CURSOR_DB_PATH` nor `CURSOR_AGENT_AUTH_PATH` set).
//! - **Kiro CLI**: reads the AWS SSO OIDC session from kiro-cli's local
//!   `data.sqlite3`, then asserts the credit counters are non-negative and the
//!   plan label is non-empty. `kiro_live` skips when there is no kiro-cli
//!   install (no db and no `KIRO_DB_PATH`).
//! - **SuperGrok**: asks the official Grok Build CLI's `x.ai/billing` ACP
//!   extension, then asserts usage percent and plan. Set
//!   `SUPERGROK_GROK_BINARY` to the trusted official executable.
//!
//! ## Calibration and shape probes (encrypted sync, plans 1-08, 2-06 and 3-06)
//!
//! Six probes at the bottom of this file are not vendor smoke tests. They
//! answer sizing and API-shape questions the encrypted-sync format would
//! otherwise have to guess at, and they live here because this is where the
//! project keeps every test allowed to cost real seconds, real gibibytes, or a
//! real network call — all of it behind `#[ignore]`, so `cargo test` and the
//! AUR `check()` run neither. Their measured answers are written down in
//! `docs/sync-calibration.md` and `docs/sync-format.md` §7; re-run one when its
//! answer goes stale.
//!
//! - **CAL-3**, `cal3_argon2id_timing_at_production_parameters`: what Argon2id
//!   actually costs at the shipped m = 1 GiB / t = 3 / p = 1, plus the two
//!   steps down a user on constrained hardware would take. Needs nothing but a
//!   release build:
//!   `cargo test --release --test live -- --ignored --nocapture cal3_`
//! - **CAL-1**, `cal1_range_on_private_release_asset`: whether a private-repo
//!   release asset honours `Range:` after the redirect to signed storage.
//!   Credential-gated and skips cleanly when unset — see its own doc comment
//!   for the three variables and `docs/sync-format.md` for what the answer
//!   changes.
//! - **CAL-2**, `cal2_desktop_state_chunk_stability`: how much of a profile's
//!   `desktop-state/` is new bytes the next time claude-acc captures it, which
//!   decides whether `credentials` dominates the daily sync cost. Records a
//!   digest snapshot per invocation and diffs against the previous one, so the
//!   capture is the user's to make and the probe never forces one. Note it is
//!   an *account switch*, not an app restart, that rewrites those bytes:
//!   `AI_USAGEBAR_CAL2_PROFILE=… AI_USAGEBAR_CAL2_SNAPSHOT=… cargo test --test
//!   live -- --ignored --nocapture cal2_`
//! - **CAL-4**, `cal4_default_bundle_compressed_size`: the real zstd-compressed
//!   and sealed size of this machine's default bundle, per category. Needs no
//!   credential and no network:
//!   `cargo test --release --test live -- --ignored --nocapture cal4_`
//! - **CAL-5**, `cal5_release_asset_state_and_digest`: what `state` a release
//!   asset reports when its upload is cut off mid-body, and whether GitHub
//!   populates `digest` on a complete one — the two MEDIUM-confidence answers
//!   the resume scan and D3's verifying download are built around. It **writes
//!   and deletes release assets**, so it wants a throwaway private repo and a
//!   `Contents: write` PAT; credential-gated and skips cleanly when unset.
//! - `permissions_shape_for_a_fine_grained_contents_token`: whether
//!   `permissions.admin` on `GET /repos/{owner}/{repo}` is a property of the
//!   token or of the user, which is what decides whether D-03's
//!   over-permissioned-token warning can ever be correct. Credential-gated and
//!   skips cleanly when unset — see its own doc comment for the exact token
//!   shape it must be run with.

use std::time::Duration;

use ai_usagebar::anthropic;
use ai_usagebar::cache::Cache;
use ai_usagebar::cursor;
use ai_usagebar::error::AppError;
use ai_usagebar::kimi;
use ai_usagebar::kiro;
use ai_usagebar::minimax;
use ai_usagebar::openai;
use ai_usagebar::openrouter;
use ai_usagebar::supergrok;
use ai_usagebar::zai;

fn xdg_cache_for(test: &str) -> Cache {
    // Use a per-test scratch dir so smoke tests don't clobber the real cache.
    let base = std::env::temp_dir().join(format!("ai-usagebar-smoke-{test}"));
    let _ = std::fs::remove_dir_all(&base);
    Cache::at(base)
}

fn assert_pct(label: &str, p: i32) {
    assert!(
        (0..=100).contains(&p),
        "{label}: utilization {p} outside [0,100] — vendor shape changed?"
    );
}

fn is_missing_credentials(err: &AppError) -> bool {
    matches!(err, AppError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound)
}

#[tokio::test]
#[ignore = "live API; run with --ignored"]
async fn anthropic_live() {
    let creds_path = anthropic::creds::default_path().expect("resolve home directory");
    // Creds live in this file (Linux) or the login Keychain (recent macOS, no
    // file) — CredsTarget::Default covers both. Skip cleanly when neither
    // source resolves — a no-op on machines without creds, as the module doc
    // promises, not a hard failure.
    let creds_target = anthropic::creds::CredsTarget::Default(creds_path);
    match anthropic::creds::resolve(&creds_target) {
        Ok(_) => {}
        Err(err) if is_missing_credentials(&err) => {
            eprintln!("anthropic_live: no Claude credentials (file or Keychain) — skipping");
            return;
        }
        Err(err) => panic!("anthropic_live: failed to read Claude credentials: {err}"),
    }
    let cache = xdg_cache_for("anthropic");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let endpoints = anthropic::fetch::Endpoints::default();
    let out = anthropic::fetch_snapshot(
        &client,
        &creds_target,
        &cache,
        &endpoints,
        Duration::from_secs(0),
    )
    .await
    .expect("anthropic fetch should succeed against the real API");

    assert!(!out.snapshot.plan.is_empty(), "anthropic plan label empty");
    assert_pct("anthropic.session", out.snapshot.session.utilization_pct);
    assert_pct("anthropic.weekly", out.snapshot.weekly.utilization_pct);
    if let Some(s) = out.snapshot.sonnet.as_ref() {
        assert_pct("anthropic.sonnet", s.utilization_pct);
    }
    if let Some(e) = out.snapshot.extra.as_ref() {
        if let Some(l) = e.limit {
            assert!(l.0 >= 0, "anthropic extra.limit < 0");
        }
        // spent can equal or exceed limit briefly during reconciliation; just sanity-check.
        assert!(e.spent.0 >= 0, "anthropic extra.spent < 0");
    }
    println!(
        "✅ anthropic — plan={}, session={}%, weekly={}%, sonnet={:?}, extra={:?}",
        out.snapshot.plan,
        out.snapshot.session.utilization_pct,
        out.snapshot.weekly.utilization_pct,
        out.snapshot.sonnet.as_ref().map(|s| s.utilization_pct),
        out.snapshot
            .extra
            .as_ref()
            .map(|e| (e.fmt_spent(), e.fmt_limit())),
    );
}

#[tokio::test]
#[ignore = "live API; run with --ignored"]
async fn openai_live() {
    let creds_path = openai::creds::default_path().expect("resolve home directory");
    assert!(
        creds_path.exists(),
        "no Codex credentials at {} — log in with `codex login` first",
        creds_path.display()
    );
    let cache = xdg_cache_for("openai");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let endpoints = openai::fetch::Endpoints::default();
    let out = openai::fetch_snapshot(
        &client,
        &creds_path,
        &cache,
        &endpoints,
        Duration::from_secs(0),
    )
    .await
    .expect("openai fetch should succeed against the real API");

    assert!(!out.snapshot.plan.is_empty(), "openai plan label empty");
    assert!(
        out.snapshot.session.is_some() || out.snapshot.weekly.is_some(),
        "openai returned no 5h or 7d usage window"
    );
    if let Some(session) = out.snapshot.session.as_ref() {
        assert_pct("openai.session", session.utilization_pct);
    }
    if let Some(weekly) = out.snapshot.weekly.as_ref() {
        assert_pct("openai.weekly", weekly.utilization_pct);
    }
    println!(
        "✅ openai — plan={}, session={:?}%, weekly={:?}%, credits={:?}",
        out.snapshot.plan,
        out.snapshot
            .session
            .as_ref()
            .map(|window| window.utilization_pct),
        out.snapshot
            .weekly
            .as_ref()
            .map(|window| window.utilization_pct),
        out.snapshot.credits.map(|c| c.balance),
    );
}

#[tokio::test]
#[ignore = "live API; run with --ignored"]
async fn zai_live() {
    let api_key = std::env::var("ZAI_API_KEY")
        .expect("ZAI_API_KEY must be set (source ~/.config/zsh/secrets)");
    let cache = xdg_cache_for("zai");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let endpoints = zai::fetch::Endpoints::default();
    let out = zai::fetch_snapshot(
        &client,
        &api_key,
        &cache,
        &endpoints,
        Duration::from_secs(0),
        None,
    )
    .await
    .expect("zai fetch should succeed against the real API");

    assert!(!out.snapshot.plan.is_empty(), "zai plan label empty");
    // Z.AI may legitimately return 0% on a fresh account, but at least one
    // bucket should exist — if all three are None, the schema changed.
    let has_any = out.snapshot.session.is_some()
        || out.snapshot.weekly.is_some()
        || out.snapshot.mcp.is_some();
    assert!(has_any, "zai snapshot has no buckets — shape changed?");
    for (label, w) in [
        ("session", &out.snapshot.session),
        ("weekly", &out.snapshot.weekly),
        ("mcp", &out.snapshot.mcp),
    ] {
        if let Some(w) = w.as_ref() {
            assert_pct(&format!("zai.{label}"), w.utilization_pct);
        }
    }
    println!(
        "✅ zai — plan={}, session={:?}%, weekly={:?}%, mcp={:?}%",
        out.snapshot.plan,
        out.snapshot.session.as_ref().map(|w| w.utilization_pct),
        out.snapshot.weekly.as_ref().map(|w| w.utilization_pct),
        out.snapshot.mcp.as_ref().map(|w| w.utilization_pct),
    );
}

#[tokio::test]
#[ignore = "live API; run with --ignored"]
async fn openrouter_live() {
    let api_key = std::env::var("OPENROUTER_API_KEY")
        .expect("OPENROUTER_API_KEY must be set (source ~/.config/zsh/secrets)");
    let cache = xdg_cache_for("openrouter");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let endpoints = openrouter::fetch::Endpoints::default();
    let out = openrouter::fetch_snapshot(
        &client,
        &api_key,
        &cache,
        &endpoints,
        Duration::from_secs(0),
    )
    .await
    .expect("openrouter fetch should succeed against the real API");

    assert!(out.snapshot.total_credits >= 0.0, "or.total_credits < 0");
    assert!(out.snapshot.total_usage >= 0.0, "or.total_usage < 0");
    // total_usage being slightly larger than total_credits is possible during
    // reconciliation (debt allowed); don't assert otherwise.
    println!(
        "✅ openrouter — label={}, balance=${:.2}, used=${:.2}, monthly=${:.2}, free={}",
        out.snapshot.label,
        out.snapshot.balance(),
        out.snapshot.total_usage,
        out.snapshot.usage_monthly,
        out.snapshot.is_free_tier,
    );
}

#[tokio::test]
#[ignore = "live API; run with --ignored"]
async fn kimi_live() {
    let Ok(api_key) = std::env::var("KIMI_API_KEY") else {
        eprintln!("kimi_live: KIMI_API_KEY is unset — skipping optional Kimi smoke test");
        return;
    };
    if api_key.trim().is_empty() {
        eprintln!("kimi_live: KIMI_API_KEY is empty — skipping optional Kimi smoke test");
        return;
    }
    let cache = xdg_cache_for("kimi");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let endpoints = kimi::fetch::Endpoints::default();
    let out = kimi::fetch_snapshot(
        &client,
        &api_key,
        &cache,
        &endpoints,
        Duration::from_secs(0),
    )
    .await
    .expect("kimi fetch should succeed against the real API");

    // Kimi permits missing or inconsistent counters, and the production
    // snapshot deliberately preserves them. Exercise all weekly fields while
    // checking the production-facing normalized percentage only.
    assert_pct("kimi.weekly", out.snapshot.weekly_pct());
    // A nonzero public limit means the parser selected its optional rolling
    // window. The public snapshot does not retain the wire duration/unit, so
    // it can only validate that window's normalized percentage and counters.
    if out.snapshot.window_limit > 0 {
        assert_pct("kimi.window", out.snapshot.window_pct());
    }
    println!(
        "✅ kimi — plan={:?}, weekly={} / {} ({} remaining; reset {:?}), window={} / {} ({} remaining; reset {:?})",
        out.snapshot.plan,
        out.snapshot.weekly_used,
        out.snapshot.weekly_limit,
        out.snapshot.weekly_remaining,
        out.snapshot.weekly_reset_at,
        out.snapshot.window_used,
        out.snapshot.window_limit,
        out.snapshot.window_remaining,
        out.snapshot.window_reset_at,
    );
}

#[tokio::test]
#[ignore = "live API; run with --ignored"]
async fn cursor_live() {
    // Cursor has no API key — the credential is a session token, either the
    // one the Cursor IDE wrote to its local state DB, or (headless machines
    // with no IDE) the one the `cursor-agent` CLI wrote to its own auth.json.
    // So this test needs one of the two installed (or `CURSOR_DB_PATH` /
    // `CURSOR_AGENT_AUTH_PATH` pointing at a copy) and skips otherwise, the
    // same way `kimi_live` skips without a key. Nothing to fetch on a CI box
    // with neither.
    let db_path = match std::env::var("CURSOR_DB_PATH") {
        Ok(p) if !p.trim().is_empty() => std::path::PathBuf::from(p),
        _ => cursor::db::default_db_path().expect("resolve platform config dir"),
    };
    let agent_auth_path = match std::env::var("CURSOR_AGENT_AUTH_PATH") {
        Ok(p) if !p.trim().is_empty() => std::path::PathBuf::from(p),
        _ => cursor::db::default_agent_auth_path().expect("resolve platform config dir"),
    };
    if !db_path.exists() && !agent_auth_path.exists() {
        eprintln!(
            "cursor_live: no Cursor state DB at {} and no cursor-agent auth at {} — skipping \
             (sign in to the Cursor IDE or run `cursor-agent`, or set CURSOR_DB_PATH / \
             CURSOR_AGENT_AUTH_PATH)",
            db_path.display(),
            agent_auth_path.display()
        );
        return;
    }

    let cache = xdg_cache_for("cursor");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let endpoints = cursor::fetch::Endpoints::default();
    let out = cursor::fetch_snapshot(
        &client,
        &db_path,
        &agent_auth_path,
        &cache,
        &endpoints,
        Duration::from_secs(0),
    )
    .await
    .expect("cursor fetch should succeed against the real API");

    // The fields the widget depends on: two pool percentages (>= 0; a pool can
    // exceed 100 when over its included allowance, so only the low bound is
    // asserted) and a future billing-cycle reset.
    assert!(
        out.snapshot.auto_pct >= 0 && out.snapshot.api_pct >= 0,
        "cursor: negative pool percentage — shape changed? auto={} api={}",
        out.snapshot.auto_pct,
        out.snapshot.api_pct
    );
    assert!(!out.snapshot.plan.is_empty(), "cursor plan label empty");
    assert!(
        out.snapshot
            .reset_at
            .is_some_and(|r| r > chrono::Utc::now()),
        "cursor: reset_at should be a future instant, got {:?}",
        out.snapshot.reset_at
    );
    println!(
        "✅ cursor — plan={}, Cursor Models {}%, Other Models {}%, total {}%, on-demand={}, reset {:?}",
        out.snapshot.plan,
        out.snapshot.auto_pct,
        out.snapshot.api_pct,
        out.snapshot.total_pct,
        out.snapshot.on_demand_enabled,
        out.snapshot.reset_at,
    );
}

#[tokio::test]
#[ignore = "live API; run with --ignored"]
async fn kiro_live() {
    // Kiro has no API key — the credential is the AWS SSO OIDC session
    // kiro-cli wrote to its own local database after `kiro-cli login`. So this
    // test needs kiro-cli installed and signed in (or `KIRO_DB_PATH` pointing
    // at a copied `data.sqlite3`) and skips otherwise, like `cursor_live`.
    let db_path = match std::env::var("KIRO_DB_PATH") {
        Ok(p) if !p.trim().is_empty() => std::path::PathBuf::from(p),
        _ => kiro::db::default_db_path().expect("resolve platform data dir"),
    };
    if !db_path.exists() {
        eprintln!(
            "kiro_live: no kiro-cli database at {} — skipping (run `kiro-cli login`, or set KIRO_DB_PATH)",
            db_path.display()
        );
        return;
    }

    let cache = xdg_cache_for("kiro");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let out = kiro::fetch_snapshot(&client, &db_path, &cache, Duration::from_secs(0))
        .await
        .expect("kiro fetch should succeed against the real API");

    // The fields the widget depends on: non-negative credit counters, a
    // non-empty plan label, and (when reported) a future reset.
    assert!(
        out.snapshot.used >= 0.0 && out.snapshot.limit >= 0.0,
        "kiro: negative credit counter — shape changed? used={} limit={}",
        out.snapshot.used,
        out.snapshot.limit
    );
    assert!(!out.snapshot.plan.is_empty(), "kiro plan label empty");
    if let Some(reset) = out.snapshot.reset_at {
        assert!(
            reset > chrono::Utc::now(),
            "kiro: reset_at should be a future instant, got {reset:?}"
        );
    }
    println!(
        "✅ kiro — plan={}, credits {} / {} ({}%), reset {:?}",
        out.snapshot.plan,
        out.snapshot.used,
        out.snapshot.limit,
        out.snapshot.pct(),
        out.snapshot.reset_at,
    );
}

#[tokio::test]
#[ignore = "live API; run with --ignored"]
async fn supergrok_live() {
    // Grok Build owns every auth mode and returns only billing data over ACP.
    let binary = std::env::var_os("SUPERGROK_GROK_BINARY")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| ai_usagebar::config::Config::default().supergrok.grok_binary);
    let auth_override = std::env::var_os("SUPERGROK_AUTH_PATH").map(std::path::PathBuf::from);
    let config_override = std::env::var_os("SUPERGROK_CONFIG_PATH").map(std::path::PathBuf::from);
    let scope_paths = supergrok::scope::ScopePaths::with_overrides(
        auth_override.as_deref(),
        config_override.as_deref(),
    )
    .expect("resolve Grok scope paths");
    let cache = xdg_cache_for("supergrok");
    let out = supergrok::fetch_snapshot(&binary, &scope_paths, &cache, Duration::ZERO)
        .await
        .expect("SuperGrok billing should succeed through official Grok Build ACP");

    assert!(
        out.snapshot.weekly_pct >= 0,
        "supergrok: negative weekly_pct — shape changed? {}",
        out.snapshot.weekly_pct
    );
    assert!(!out.snapshot.plan.is_empty(), "supergrok plan label empty");
    if let Some(reset) = out.snapshot.reset_at {
        assert!(
            reset > chrono::Utc::now() - chrono::Duration::days(1),
            "supergrok: reset_at looks implausibly old: {reset:?}"
        );
    }
    println!(
        "✅ supergrok — plan={}, {} {}%, prepaid {:?}, reset {:?}",
        out.snapshot.plan,
        out.snapshot.period.label(),
        out.snapshot.weekly_pct,
        out.snapshot.prepaid_balance,
        out.snapshot.reset_at,
    );
}

/// MiniMax Token Plan — optional: skipped unless a subscription key is present.
///
/// The endpoint answers HTTP 200 even for auth failures, so a green run here is
/// what proves the in-band `base_resp.status_code` check is still doing its job:
/// a wrong key surfaces as an error rather than an all-zero plan.
#[tokio::test]
#[ignore = "live API; run with --ignored"]
async fn minimax_live() {
    let Ok(api_key) = std::env::var("MINIMAX_API_KEY") else {
        eprintln!("minimax_live: MINIMAX_API_KEY is unset — skipping optional MiniMax smoke test");
        return;
    };
    if api_key.trim().is_empty() {
        eprintln!("minimax_live: MINIMAX_API_KEY is empty — skipping optional MiniMax smoke test");
        return;
    }
    let cache = xdg_cache_for("minimax");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let endpoints = minimax::fetch::Endpoints::default();
    let out = minimax::fetch_snapshot(
        &client,
        &api_key,
        &cache,
        &endpoints,
        Duration::from_secs(0),
    )
    .await
    .expect("minimax fetch should succeed against the real API");

    assert_pct("minimax.session", out.snapshot.session.utilization_pct);
    assert_pct("minimax.weekly", out.snapshot.weekly.utilization_pct);
    // The interval length is read from the payload rather than assumed: it has
    // been observed at both 4h and 5h on the same account. A non-positive one
    // would divide the pace math by nothing.
    assert!(
        out.snapshot.session.window_duration > chrono::Duration::zero(),
        "minimax: interval window has no length — payload shape changed?"
    );
    if let Some(v) = out.snapshot.video_session.as_ref() {
        assert_pct("minimax.video", v.utilization_pct);
    }
    println!(
        "✅ minimax: {} · session {}% · weekly {}% · video {:?}",
        out.snapshot.plan,
        out.snapshot.session.utilization_pct,
        out.snapshot.weekly.utilization_pct,
        out.snapshot
            .video_session
            .as_ref()
            .map(|w| w.utilization_pct),
    );
}

// ---------------------------------------------------------------------------
// Calibration probes — encrypted sync bundle format, plan 1-08.
// ---------------------------------------------------------------------------

/// **CAL-3** — what Argon2id really costs at the parameters this build ships.
///
/// The 1582 ms in the research is one Apple M3 Max number, and the shipped
/// default plus the memory floor should not rest on it alone. This times the
/// production parameters and then two steps down, so a user who must lower
/// `--kdf-memory` on constrained hardware has a curve to choose from rather
/// than a single point.
///
/// `#[ignore]`d because it allocates a gibibyte and takes seconds, and the AUR
/// `check()` runs `cargo test` on other people's machines. Run it in release:
/// a debug-build Argon2 timing measures the optimiser, not the KDF.
///
/// ```bash
/// cargo test --release --test live -- --ignored --nocapture \
///     cal3_argon2id_timing_at_production_parameters
/// ```
#[test]
#[ignore = "calibration; allocates 1 GiB and takes seconds — run with --ignored --release"]
fn cal3_argon2id_timing_at_production_parameters() {
    use ai_usagebar::sync::crypto::{KdfParams, available_memory_kib, derive_kek};

    /// The shipped default, spelled out. If it ever drifts from
    /// [`KdfParams::default`], this probe calibrates something nobody runs.
    const PRODUCTION: KdfParams = KdfParams {
        m_kib: 1_048_576,
        t: 3,
        p: 1,
    };
    assert_eq!(
        PRODUCTION,
        KdfParams::default(),
        "the first row must be the parameters this build actually ships"
    );

    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let available = available_memory_kib()
        .map(|kib| format!("{} MiB", kib / 1024))
        .unwrap_or_else(|| "unreported on this platform".into());

    println!(
        "CAL-3 — {}/{}, {profile} profile, available memory {available}",
        std::env::consts::ARCH,
        std::env::consts::OS,
    );
    if cfg!(debug_assertions) {
        println!("  WARNING: a debug-build number is meaningless — re-run with --release");
    }

    // A fixed salt and a fixed password: this measures work, not secrecy, and
    // random inputs would only make two runs incomparable.
    let salt = [0x5au8; 16];
    let password = b"calibration only, never a real passphrase";

    for m_kib in [PRODUCTION.m_kib, PRODUCTION.m_kib / 2, PRODUCTION.m_kib / 4] {
        let params = KdfParams {
            m_kib,
            ..PRODUCTION
        };
        let started = std::time::Instant::now();
        let kek = derive_kek(password, &salt, params).expect("derivation must succeed");
        let elapsed = started.elapsed();
        // Read the result so the derivation cannot be optimised away.
        assert_eq!(kek.len(), 32);
        println!(
            "  m={:>5} MiB  t={}  p={}  ->  {:>6} ms",
            m_kib / 1024,
            params.t,
            params.p,
            elapsed.as_millis(),
        );
    }
}

/// **CAL-1** — does a private-repo release asset honour a `Range:` request
/// after the redirect to signed storage?
///
/// It is an **optimisation** question, not a blocker: a restore fetches whole
/// packs either way, and `PACK_MAX`'s 48 MiB sits under `download_asset`'s
/// 64 MiB body cap, so nothing shipped depends on the answer. What it buys is a
/// partial restore — if ranged reads work, one chunk can be fetched out of a
/// large pack and `sync::pack::PACK_TARGET` could then grow past its 32 MiB
/// fallback without making the waste of a whole-pack fetch worse.
///
/// **Still unrun, through Phases 1, 3, 4 and 5.** Phase 5 shipped the restore
/// path on the pessimistic assumption, which is correct whichever way this
/// comes back. If it ever comes back **positive**, exactly one thing changes:
/// `sync::restore::PackSource` gains a byte-range fetch keyed on the
/// `PackEntry`'s `offset` and `clen`, which `fetch::resolve` already reads out
/// of each pack's own sealed header. Nothing else moves — not the four
/// ceilings, not the content-address check, not the three download rounds, and
/// not the pointer's unauthenticated `offset`/`clen`/`true_len`, which stay
/// unread. That is written down here so the measurement has somewhere to land
/// rather than becoming a redesign.
///
/// **Setup** — a throwaway private repository with one release carrying an
/// asset a little over 1 MiB (large enough that a whole-body `200` is
/// unmistakable, small enough to download inside the timeout), and a
/// fine-grained read-only PAT scoped to it. Delete the repository and revoke
/// the token afterwards.
///
/// ```bash
/// GSD_CAL1_TOKEN=github_pat_… \
/// GSD_CAL1_REPO=owner/throwaway-repo \
/// GSD_CAL1_ASSET=payload.bin \
///   cargo test --test live -- --ignored --nocapture \
///     cal1_range_on_private_release_asset
/// ```
///
/// Skips with a printed message when the token is absent, so it is never a hard
/// failure on a machine that was not set up for it. Reading an environment
/// variable inside an `#[ignore]`d live test is the same carve-out every other
/// probe in this file already uses; nothing in the default `cargo test` set
/// touches the network.
#[tokio::test]
#[ignore = "live API; needs a throwaway private repo and token — run with --ignored"]
async fn cal1_range_on_private_release_asset() {
    let Some(token) = non_empty_var("GSD_CAL1_TOKEN") else {
        eprintln!(
            "cal1_range_on_private_release_asset: GSD_CAL1_TOKEN is unset — skipping; \
             the 32 MiB pack fallback recorded in docs/sync-format.md stands"
        );
        return;
    };
    let (Some(repo), Some(asset_name)) = (
        non_empty_var("GSD_CAL1_REPO"),
        non_empty_var("GSD_CAL1_ASSET"),
    ) else {
        eprintln!(
            "cal1_range_on_private_release_asset: GSD_CAL1_REPO (owner/name) and \
             GSD_CAL1_ASSET (the asset's file name) must both be set — skipping"
        );
        return;
    };

    // Redirects are *not* followed automatically: the hop to signed storage is
    // the thing being measured, and following it silently would also replay the
    // GitHub token to a storage host that neither needs nor should see it.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    // The asset's *API* url, not `browser_download_url`: the latter is an HTML
    // endpoint a token cannot authenticate.
    let release: serde_json::Value = client
        .get(format!(
            "https://api.github.com/repos/{repo}/releases/latest"
        ))
        .header("authorization", format!("Bearer {token}"))
        .header("accept", "application/vnd.github+json")
        .header("user-agent", GITHUB_PROBE_UA)
        .send()
        .await
        .expect("the release lookup must reach api.github.com")
        // A 401/404 here is a broken setup, and must never be recorded as
        // "Range is unsupported".
        .error_for_status()
        .expect("the release lookup must succeed — check the repo name and the token's scope")
        .json()
        .await
        .expect("the release lookup must return JSON");

    let asset = release["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|a| a["name"].as_str() == Some(asset_name.as_str()))
        .unwrap_or_else(|| {
            panic!("the latest release of {repo} carries no asset named {asset_name:?}")
        });
    let asset_url = asset["url"].as_str().expect("asset api url").to_string();
    let asset_size = asset["size"].as_u64().unwrap_or_default();

    println!("CAL-1 — {repo} asset {asset_name:?}, {asset_size} bytes");
    if asset_size <= 1024 * 1024 {
        println!("  WARNING: the asset is under 1 MiB — a whole-body 200 will be hard to tell");
    }

    let mut response = client
        .get(&asset_url)
        .header("authorization", format!("Bearer {token}"))
        .header("accept", "application/octet-stream")
        .header("user-agent", GITHUB_PROBE_UA)
        .header("range", "bytes=0-1023")
        .send()
        .await
        .expect("the ranged asset request must reach api.github.com");

    if response.status().is_redirection() {
        let location = header_value(&response, "location")
            .expect("a redirect response must carry a Location header");
        // Only the host is printed: a signed storage URL carries its
        // credential in the query string, and this output goes to a terminal.
        println!(
            "  {} -> signed storage at {}",
            response.status(),
            reqwest::Url::parse(&location)
                .ok()
                .and_then(|u| u.host_str().map(str::to_string))
                .unwrap_or_else(|| "an unparseable location".into()),
        );
        response = client
            .get(&location)
            .header("user-agent", GITHUB_PROBE_UA)
            .header("range", "bytes=0-1023")
            .send()
            .await
            .expect("the signed-storage request must succeed");
    }

    let status = response.status();
    let content_range = header_value(&response, "content-range");
    let content_length = header_value(&response, "content-length");
    let received = response.bytes().await.expect("a readable body").len();

    println!(
        "  status {status}, content-range {content_range:?}, \
         content-length {content_length:?}, {received} bytes received"
    );
    assert!(
        status.is_success(),
        "the ranged fetch failed with {status} — that is a broken probe, not an answer about Range"
    );
    if status.as_u16() == 206 && content_range.is_some() {
        println!(
            "  CAL-1 = Range IS honoured. A partial restore is possible, and \
             sync::pack::PACK_TARGET may be raised above 32 MiB. Record it in \
             docs/sync-format.md §7, which currently says this was never measured."
        );
    } else {
        println!(
            "  CAL-1 = Range is NOT honoured ({received} of {asset_size} bytes). \
             The 32 MiB PACK_TARGET fallback stands, now measured. Record it in \
             docs/sync-format.md §7."
        );
    }
}

/// **The `permissions` shape for a correctly-scoped fine-grained PAT** — is
/// `permissions.admin` a property of the *token* or of the *user*?
///
/// D-03 wants `sync setup` to warn when the paired token carries more than it
/// needs. Plan 3-04 wired `RepoFacts::admin_permission` up to
/// `permissions.admin` on `GET /repos/{owner}/{repo}` and then deliberately
/// warned on nothing, because for a classic token that field reports the
/// **authenticated user's role on the repository**, not the token's grant — and
/// D-01 has the user create the repository themselves, which makes them its
/// admin. A warning built on that would fire on essentially every legitimate
/// install and teach its reader to ignore warnings, which is a worse security
/// outcome than no warning at all. Whether a *fine-grained* PAT narrows the
/// field is undocumented, so this probe measures it instead of guessing.
///
/// **Run it with exactly the token `docs/sync-github.md` tells every user to
/// create**: fine-grained, `Contents: Read and write`, `Metadata: Read`, no
/// Administration, on a repository the user owns. Any other token answers a
/// different question — a classic PAT, an org-owned repository, or a token with
/// Administration granted each move the field for their own reasons, and a
/// reading from one of those must not be recorded as the answer to this one.
///
/// ```bash
/// GSD_PERM_TOKEN=github_pat_… \
/// GSD_PERM_REPO=owner/name \
///   cargo test --test live -- --ignored --nocapture \
///     permissions_shape_for_a_fine_grained_contents_token
/// ```
///
/// The output is the deliverable, not the assertions: it prints the whole
/// `permissions` object plus the four fields `gate::assert_pushable` decides on,
/// and **nothing else from the response**, which also carries owner and
/// repository metadata that has no business in a terminal transcript. Read
/// `admin`. If it is `true` for a token granted only Contents and Metadata, the
/// field cannot detect an over-permissioned token and D-03's runtime warning is
/// not implementable from this endpoint. If it is `false`, the warning becomes a
/// one-line addition to `assert_pushable`'s warning list.
///
/// One read-only `GET`; it creates nothing and needs no throwaway anything.
/// Skips with a printed message when either variable is absent.
#[tokio::test]
#[ignore = "live API; needs the sync PAT and its paired repo — run with --ignored"]
async fn permissions_shape_for_a_fine_grained_contents_token() {
    let (Some(token), Some(repo)) = (
        non_empty_var("GSD_PERM_TOKEN"),
        non_empty_var("GSD_PERM_REPO"),
    ) else {
        eprintln!(
            "permissions_shape_for_a_fine_grained_contents_token: GSD_PERM_TOKEN and \
             GSD_PERM_REPO (owner/name) must both be set — skipping; D-03's runtime \
             warning stays unshipped and docs/sync-github.md's token recipe stays its \
             sole enforcement"
        );
        return;
    };

    // Redirects are not followed, for the same reason CAL-1 does not follow
    // them: a renamed repository answers `301` with a `Location`, and following
    // it automatically would replay the bearer token to whatever that header
    // names. A `301` here is a wrong `GSD_PERM_REPO`, and the assertion below
    // says so.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let response = client
        .get(format!("https://api.github.com/repos/{repo}"))
        .header("authorization", format!("Bearer {token}"))
        .header("accept", "application/vnd.github+json")
        .header("user-agent", GITHUB_PROBE_UA)
        .send()
        .await
        .expect("the repository lookup must reach api.github.com");

    let status = response.status();
    assert!(
        status.is_success(),
        "the repository lookup returned {status} — that is a broken probe, not an \
         answer about `permissions`. Check GSD_PERM_REPO and the token's repository scope."
    );
    let body: serde_json::Value = response
        .json()
        .await
        .expect("the repository lookup must return JSON");

    println!("permissions probe — {repo}");
    println!("  permissions = {}", body["permissions"]);
    for field in ["visibility", "private", "archived", "fork"] {
        println!("  {field} = {}", body[field]);
    }
    println!(
        "  D-03: `admin` above is the whole answer. true = the field reflects your role \
         on the repository, not the token's grant, and the warning is not implementable \
         here. false = it narrows to the grant, and the warning is one line in \
         sync::github::gate::assert_pushable."
    );
}

/// User agent for the two hand-rolled GitHub probes above. GitHub's API
/// requires one.
const GITHUB_PROBE_UA: &str = "ai-usagebar-probe";

/// `Some` only for a variable that is both set and not blank — an exported but
/// empty variable is how a half-finished setup usually looks, and it should
/// skip rather than send an empty bearer token.
fn non_empty_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn header_value(response: &reqwest::Response, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

// ---------------------------------------------------------------------------
// Calibration probes — bundle scope and sizing, plan 2-06.
// ---------------------------------------------------------------------------

/// **CAL-2** — how much of a profile's `desktop-state/` is new bytes the next
/// time it is written?
///
/// It decides whether `credentials` dominates the *daily* cost of a sync. The
/// category is ~24 MB across this machine's four profiles, and if each rewrite
/// is wholesale then each rewrite re-uploads all of it, which is a sentence
/// `sync status` owes the user.
///
/// **Read the trigger carefully, because the obvious one is wrong.** What the
/// bundle carries is not Claude Desktop's live data dir: it is
/// `~/.claude-acc/profiles/<label>/desktop-state/`, a *snapshot copy* that
/// [`crate::claude_desktop`]'s `snapshot_profile` stages into a tempdir and
/// renames into place when an account is switched away from — with the app
/// already quit. Quitting and relaunching Claude Desktop therefore does not
/// touch these bytes at all; only a capture does. Measuring across a plain
/// restart would report 0% churn and mean nothing by it, so the second run has
/// to sit on the far side of an account switch.
///
/// The probe hashes each file under an injected `desktop-state/` root at fixed
/// 256 KiB offsets and records the digests. Run it again later and it compares
/// against what it recorded. Deliberately `sha2` rather than Phase 1's keyed
/// BLAKE3: the question is whether the *same offsets still hold the same bytes*,
/// which is a property of the boundaries, not of the function that names them —
/// so this stays independent of the sync key material entirely.
///
/// **It never restarts, quits or switches anything.** Two invocations separated
/// by whatever the user does on their own schedule is the whole protocol:
///
/// ```bash
/// # 1. before — writes the baseline
/// AI_USAGEBAR_CAL2_PROFILE=~/.claude-acc/profiles/<label>/desktop-state \
/// AI_USAGEBAR_CAL2_SNAPSHOT=~/.cache/ai-usagebar-cal2/<label>.json \
///   cargo test --release --test live -- --ignored --nocapture cal2_
/// # 2. use Claude Desktop on that account, then switch away from it — that
/// #    switch is what re-captures the profile
/// # 3. after — same two variables, same snapshot path: prints the churn
/// ```
///
/// Output is counts and digests only: no file body, no path outside the
/// injected root, no account label (T-2-25). Skips with a printed message when
/// either variable is unset, so it is never a hard failure.
#[test]
#[ignore = "calibration; reads a real Claude Desktop profile — run with --ignored --nocapture"]
fn cal2_desktop_state_chunk_stability() {
    let (Some(root), Some(snapshot)) = (
        non_empty_var("AI_USAGEBAR_CAL2_PROFILE"),
        non_empty_var("AI_USAGEBAR_CAL2_SNAPSHOT"),
    ) else {
        eprintln!(
            "cal2_desktop_state_chunk_stability: needs AI_USAGEBAR_CAL2_PROFILE (a \
             profile's desktop-state/ directory) and AI_USAGEBAR_CAL2_SNAPSHOT (a \
             scratch .json path) — skipping. Run it once, let an account switch \
             re-capture that profile, run it again; see docs/sync-calibration.md \
             for the fallback that applies until then."
        );
        return;
    };
    let root = std::path::PathBuf::from(root);
    let snapshot = std::path::PathBuf::from(snapshot);
    if !root.is_dir() {
        eprintln!(
            "cal2_desktop_state_chunk_stability: AI_USAGEBAR_CAL2_PROFILE is not a \
             directory — skipping"
        );
        return;
    }

    let current = cal2_scan(&root);
    let bytes: u64 = current.values().map(|(_, len)| *len).sum();
    let files = cal2_paths(&current).len();
    println!(
        "CAL-2 — {files} files, {} windows, {bytes} bytes under the injected root",
        current.len()
    );

    let Some(previous) = cal2_load(&snapshot) else {
        cal2_store(&snapshot, &current);
        println!("  no prior snapshot — baseline written to the given path.");
        println!("  Use Claude Desktop on this account and then switch away from it —");
        println!("  the switch is what re-captures the profile — and re-run this");
        println!("  exact command for the churn figure. A plain app restart will not");
        println!("  move these bytes and would only produce a meaningless 0%.");
        return;
    };

    let unchanged = current
        .iter()
        .filter(|(key, value)| previous.get(*key) == Some(value))
        .count();
    let changed = current.len() - unchanged;
    let changed_bytes: u64 = current
        .iter()
        .filter(|(key, value)| previous.get(*key) != Some(value))
        .map(|(_, (_, len))| *len)
        .sum();

    let before = cal2_paths(&previous);
    let after = cal2_paths(&current);
    let appeared: Vec<_> = after.difference(&before).cloned().collect();
    let disappeared: Vec<_> = before.difference(&after).cloned().collect();

    let pct = |n: usize| {
        if current.is_empty() {
            0.0
        } else {
            n as f64 * 100.0 / current.len() as f64
        }
    };
    println!(
        "  windows {} total, {unchanged} unchanged ({:.1}%), {changed} changed ({:.1}%), \
         {changed_bytes} bytes changed",
        current.len(),
        pct(unchanged),
        pct(changed),
    );
    println!(
        "  files {} appeared, {} disappeared",
        appeared.len(),
        disappeared.len()
    );
    // Relative names only — the injected root is never reprinted.
    for name in appeared.iter().take(20) {
        println!("    + {name}");
    }
    for name in disappeared.iter().take(20) {
        println!("    - {name}");
    }
    if changed == 0 && appeared.is_empty() && disappeared.is_empty() {
        // Not an answer. `snapshot_profile` renames a freshly staged copy into
        // place, so a re-captured profile always has *some* new bytes — an
        // identical tree means no capture happened between the two runs.
        println!(
            "  CAL-2 = INCONCLUSIVE: byte-for-byte identical, so this profile was \
             not re-captured between the two runs. Switch away from this account \
             (or run a capture) and re-run; do not record 0% as the answer."
        );
    } else if changed_bytes * 2 > bytes {
        println!(
            "  CAL-2 = the category churns wholesale. `sync status` must say that \
             credentials re-uploads most of itself whenever the profile is captured."
        );
    } else {
        println!(
            "  CAL-2 = the rewrite is partial. Dedup carries most of the category \
             across a capture."
        );
    }
    cal2_store(&snapshot, &current);
}

/// `(relative path, window index) -> (hex sha-256, window length)`.
type Cal2Windows = std::collections::BTreeMap<(String, u32), (String, u64)>;

/// Hash every file under `root` at fixed 256 KiB offsets. Reads in one window
/// at a time so a large LevelDB table never lands in memory whole.
fn cal2_scan(root: &std::path::Path) -> Cal2Windows {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    use std::io::Read as _;

    let mut out = Cal2Windows::new();
    let mut files = Vec::new();
    cal2_walk(root, &mut files);
    for path in files {
        let name = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        let Ok(mut file) = std::fs::File::open(&path) else {
            continue;
        };
        // Zeroized: this window may be a slice of a saved OAuth token.
        let mut buf = zeroize::Zeroizing::new(vec![0u8; ai_usagebar::sync::CHUNK_SIZE]);
        let mut index = 0u32;
        loop {
            let mut filled = 0usize;
            // `read` may return short of the buffer without being at EOF.
            while filled < buf.len() {
                match file.read(&mut buf[filled..]) {
                    Ok(0) => break,
                    Ok(n) => filled += n,
                    Err(_) => break,
                }
            }
            if filled == 0 && index > 0 {
                break;
            }
            let mut hex = String::with_capacity(64);
            for byte in Sha256::digest(&buf[..filled]) {
                let _ = write!(hex, "{byte:02x}");
            }
            out.insert((name.clone(), index), (hex, filled as u64));
            if filled < buf.len() {
                // A short read that reached EOF: an empty file still records
                // one zero-length window so it stays visible in the file count.
                break;
            }
            index += 1;
        }
    }
    out
}

/// Regular files under `dir`, depth-first. `DirEntry::file_type` does not
/// follow symlinks, so a link planted in the store cannot pull in a host tree.
fn cal2_walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            cal2_walk(&entry.path(), out);
        } else if kind.is_file() {
            out.push(entry.path());
        }
    }
}

fn cal2_paths(windows: &Cal2Windows) -> std::collections::BTreeSet<String> {
    windows.keys().map(|(name, _)| name.clone()).collect()
}

fn cal2_load(path: &std::path::Path) -> Option<Cal2Windows> {
    let text = std::fs::read_to_string(path).ok()?;
    let rows: Vec<serde_json::Value> = serde_json::from_str(&text).ok()?;
    let mut out = Cal2Windows::new();
    for row in rows {
        let (Some(name), Some(index), Some(digest), Some(len)) = (
            row["path"].as_str(),
            row["window_index"].as_u64(),
            row["hex_digest"].as_str(),
            row["len"].as_u64(),
        ) else {
            continue;
        };
        out.insert((name.to_string(), index as u32), (digest.to_string(), len));
    }
    Some(out)
}

fn cal2_store(path: &std::path::Path, windows: &Cal2Windows) {
    let rows: Vec<serde_json::Value> = windows
        .iter()
        .map(|((name, index), (digest, len))| {
            serde_json::json!({
                "path": name,
                "window_index": index,
                "hex_digest": digest,
                "len": len,
            })
        })
        .collect();
    if let Err(e) = std::fs::write(
        path,
        serde_json::to_vec(&rows).expect("a digest list always serialises"),
    ) {
        eprintln!("  could not write the snapshot: {e}");
    }
}

/// **CAL-4** — what the default bundle actually costs once compressed.
///
/// The research's ~33 MB comes from applying an assumed 4-5x ratio to a raw
/// figure. This measures this machine's real bytes through the real collectors
/// and Phase 1's real framing, because SCOPE-03 shows the number *before* the
/// first push and an estimate that flatters the payload is worse than none.
///
/// Per window it runs [`ai_usagebar::sync::chunk::frame`] — zstd level 3, then
/// the power-of-two pad that hides tail lengths — and adds the 40 bytes every
/// seal appends (24-byte nonce, 16-byte Poly1305 tag). So the "stored" column
/// is the ciphertext that would really be uploaded, padding included; the
/// "zstd" column beside it is the compressor's own output, which is the honest
/// place to read a ratio off.
///
/// ```bash
/// cargo test --release --test live -- --ignored --nocapture cal4_
/// AI_USAGEBAR_CAL4_ALL=1 cargo test --release --test live -- --ignored --nocapture cal4_
/// ```
///
/// The variable forces every category on, including opt-in transcripts, so the
/// one category big enough to matter can be calibrated without editing the
/// user's `config.toml`. Without it the probe measures exactly what
/// `config.toml` selects — the real default bundle.
#[test]
#[ignore = "calibration; reads this machine's real bundle — run with --ignored --nocapture"]
fn cal4_default_bundle_compressed_size() {
    use ai_usagebar::config::{Config, SyncCategory};
    use ai_usagebar::sync::{CHUNK_SIZE, SyncRoots, chunk, scope};

    /// 24-byte XChaCha20 nonce stored inline + 16-byte Poly1305 tag.
    const SEAL_OVERHEAD: u64 = 40;

    let config = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("cal4_default_bundle_compressed_size: config unreadable ({e}) — skipping");
            return;
        }
    };
    let roots = match SyncRoots::resolve(&config) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("cal4_default_bundle_compressed_size: roots unresolvable ({e}) — skipping");
            return;
        }
    };
    let mut cfg = config.sync.clone();
    let forced = non_empty_var("AI_USAGEBAR_CAL4_ALL").is_some();
    if forced {
        cfg.categories = SyncCategory::ALL.to_vec();
    }

    println!(
        "CAL-4 — {}/{}, {} profile, {}{}",
        std::env::consts::ARCH,
        std::env::consts::OS,
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        chrono::Utc::now().format("%Y-%m-%d"),
        if forced {
            ", AI_USAGEBAR_CAL4_ALL=1 (every category, not the default bundle)"
        } else {
            ", the configured default bundle"
        },
    );
    println!(
        "  {:<12} {:>7} {:>14} {:>14} {:>14} {:>7}",
        "category", "files", "raw", "zstd", "stored", "ratio"
    );

    let now = chrono::Utc::now();
    let (mut all_files, mut all_raw, mut all_zstd, mut all_stored) = (0usize, 0u64, 0u64, 0u64);
    for cat in SyncCategory::ALL {
        if !cfg.includes(cat) {
            continue;
        }
        let scan = scope::collect(cat, &roots, &cfg, now);
        let (mut raw, mut zstd_bytes, mut stored, mut unreadable) = (0u64, 0u64, 0u64, 0usize);
        for entry in &scan.files {
            // Zeroized: `credentials` is literally a pile of OAuth tokens.
            let Ok(bytes) = std::fs::read(&entry.path).map(zeroize::Zeroizing::new) else {
                unreadable += 1;
                continue;
            };
            raw += bytes.len() as u64;
            for window in bytes.chunks(CHUNK_SIZE) {
                let framed = chunk::frame(window).expect("a <=CHUNK_SIZE window always frames");
                // Bytes 4..8 of the frame are zstd's own output length.
                zstd_bytes += u32::from_le_bytes(framed[4..8].try_into().expect("4 bytes")) as u64;
                stored += framed.len() as u64 + SEAL_OVERHEAD;
            }
        }
        println!(
            "  {:<12} {:>7} {:>14} {:>14} {:>14} {:>6.2}x",
            cat.label(),
            scan.files.len(),
            human(raw),
            human(zstd_bytes),
            human(stored),
            ratio(raw, stored),
        );
        if unreadable > 0 || scan.skipped > 0 || scan.walk_capped {
            println!(
                "    ({unreadable} unreadable, {} skipped by the walker, walk_capped={})",
                scan.skipped, scan.walk_capped
            );
        }
        if scan.excluded_files > 0 {
            println!(
                "    (bounds dropped {} files / {} — D3's byte budget, not the day window)",
                scan.excluded_files,
                human(scan.excluded_bytes),
            );
        }
        all_files += scan.files.len();
        all_raw += raw;
        all_zstd += zstd_bytes;
        all_stored += stored;
    }
    println!(
        "  {:<12} {:>7} {:>14} {:>14} {:>14} {:>6.2}x",
        "TOTAL",
        all_files,
        human(all_raw),
        human(all_zstd),
        human(all_stored),
        ratio(all_raw, all_stored),
    );
    println!(
        "  zstd alone would be {} ({:.2}x); padding and per-chunk seal overhead add {}.",
        human(all_zstd),
        ratio(all_raw, all_zstd),
        human(all_stored.saturating_sub(all_zstd)),
    );
    assert!(
        all_stored > 0 || all_files == 0,
        "a non-empty bundle must produce sealed bytes"
    );
}

fn ratio(raw: u64, packed: u64) -> f64 {
    if packed == 0 {
        0.0
    } else {
        raw as f64 / packed as f64
    }
}

fn human(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

/// **Writes to your real login Keychain** — the sync token's round trip
/// (plan 3-02), under a service name the product never reads.
///
/// It lives here, `#[ignore]`d, rather than in `src/sync/github/keychain.rs`,
/// because a `#[test]` in the library would be reached by `cargo test --
/// --ignored` in any CI leg and by the AUR `check()` on an installer's machine,
/// and would leave an item behind on a developer's Keychain. macOS only:
/// `sync::github::keychain` is not compiled anywhere else.
///
/// What it proves is the read/write *split*: the value goes in through
/// Security.framework (never `argv`) and comes back out through `security(1)`,
/// selected by the same account, which is the pairing that once drifted apart
/// and created a second, empty-account item the read could never find.
#[cfg(target_os = "macos")]
#[test]
#[ignore]
fn sync_token_keychain_live_round_trip() {
    use ai_usagebar::sync::github::keychain;

    const PROBE_SERVICE: &str = "ai-usagebar-sync-token-livetest";
    const PROBE_VALUE: &str = "github_pat_livetest_not_a_real_token";
    assert_ne!(
        PROBE_SERVICE,
        keychain::SERVICE,
        "the probe must never touch the item the product reads"
    );

    // Whatever an interrupted earlier run left behind; also the idempotence
    // `token::clear` depends on.
    keychain::delete_raw_service(PROBE_SERVICE).expect("deleting what is absent is not a failure");
    assert_eq!(keychain::read_raw_service(PROBE_SERVICE).unwrap(), None);

    keychain::write_raw_service(PROBE_SERVICE, PROBE_VALUE).expect("write via Security.framework");
    assert_eq!(
        keychain::read_raw_service(PROBE_SERVICE)
            .unwrap()
            .as_deref(),
        Some(PROBE_VALUE),
        "written natively, read back through security(1) — same (service, account) pair"
    );

    keychain::delete_raw_service(PROBE_SERVICE).expect("cleanup");
    assert_eq!(
        keychain::read_raw_service(PROBE_SERVICE).unwrap(),
        None,
        "the probe left nothing on the login Keychain"
    );
}

/// **CAL-5** — what `state` does a release asset report when its upload is cut
/// off, and does GitHub populate `digest` on a complete one?
///
/// Both questions the research left at MEDIUM confidence, and both change later
/// code if they resolve:
///
/// - **`state`.** `src/sync/push/upload.rs` *does* branch on it: the resume scan
///   skips only on the `"uploaded"` literal and deletes on everything else. What
///   this probe establishes is that the branch fails in the **safe** direction
///   whatever GitHub actually reports, because every unrecognised state
///   re-uploads. Nothing in `src/` may be *relaxed* on the strength of it. In
///   particular **the size check stays**: `state` is not authoritative, and a
///   future reader must not drop a check believing that it is.
/// - **`digest`.** If GitHub populates it over the uploaded bytes, a later phase
///   can compare a locally computed hash instead of re-downloading every asset,
///   and D3's verification pass loses its extra 115 MB on a first push. If it is
///   absent or covers something else, that download stays.
///
/// **Still unrun at the end of Phase 5**, for the same reason as CAL-1: it needs
/// a real private repository and a real token, and this project's suites have
/// neither. Both halves of the shipped behaviour are the conservative branch —
/// an unrecognised `state` re-uploads, and a missing `digest` means the
/// verifying download happens — so the code is correct unmeasured; what is
/// unmeasured is only how much it could be *relaxed*.
///
/// **Setup** — a throwaway **private** repository with one published release,
/// and a fine-grained PAT with `Contents: write` scoped to it. This probe writes
/// and deletes release assets; point it at nothing you care about. Delete the
/// repository and revoke the token afterwards.
///
/// ```bash
/// GSD_CAL5_TOKEN=github_pat_… \
/// GSD_CAL5_REPO=owner/throwaway-repo \
///   cargo test --test live -- --ignored --nocapture \
///     cal5_release_asset_state_and_digest
/// ```
///
/// Skips with a printed message when the variables are absent, so it is never a
/// hard failure on a machine that was not set up for it. Nothing in the default
/// `cargo test` set — which is what the AUR `check()` runs on an installer's
/// machine — touches this.
#[tokio::test]
#[ignore = "live API; writes and deletes release assets — run with --ignored"]
async fn cal5_release_asset_state_and_digest() {
    let (Some(token), Some(repo)) = (
        non_empty_var("GSD_CAL5_TOKEN"),
        non_empty_var("GSD_CAL5_REPO"),
    ) else {
        eprintln!(
            "cal5_release_asset_state_and_digest: GSD_CAL5_TOKEN and GSD_CAL5_REPO \
             (owner/name) must both be set — skipping; the resume scan's conservative \
             `delete anything that is not \"uploaded\"` rule stands either way"
        );
        return;
    };

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .unwrap();
    let api = |suffix: String| format!("https://api.github.com/repos/{repo}{suffix}");

    let release: serde_json::Value = client
        .get(api("/releases/latest".into()))
        .header("authorization", format!("Bearer {token}"))
        .header("accept", "application/vnd.github+json")
        .header("user-agent", GITHUB_PROBE_UA)
        .send()
        .await
        .expect("the release lookup must reach api.github.com")
        // A 401/404 here is a broken setup, and must never be recorded as an
        // answer about asset states.
        .error_for_status()
        .expect("the release lookup must succeed — check the repo name and the token's scope")
        .json()
        .await
        .expect("the release lookup must return JSON");
    let release_id = release["id"].as_u64().expect("the release's numeric id");

    // Every asset this probe writes, so a re-run starts from a clean release
    // and leaves nothing behind.
    const TORN: &str = "cal5-torn.bin";
    const WHOLE: &str = "cal5-whole.bin";
    let list = |c: reqwest::Client, token: String, url: String| async move {
        c.get(url)
            .header("authorization", format!("Bearer {token}"))
            .header("accept", "application/vnd.github+json")
            .header("user-agent", GITHUB_PROBE_UA)
            .send()
            .await
            .expect("the asset listing must reach api.github.com")
            .json::<Vec<serde_json::Value>>()
            .await
            .expect("the asset listing must return JSON")
    };
    let assets_url = api(format!("/releases/{release_id}/assets?per_page=100"));

    for asset in list(client.clone(), token.clone(), assets_url.clone()).await {
        let name = asset["name"].as_str().unwrap_or_default();
        if name == TORN || name == WHOLE {
            let id = asset["id"].as_u64().expect("an asset id");
            client
                .delete(api(format!("/releases/assets/{id}")))
                .header("authorization", format!("Bearer {token}"))
                .header("user-agent", GITHUB_PROBE_UA)
                .send()
                .await
                .expect("the cleanup delete must reach api.github.com");
            println!("CAL-5 — removed a leftover {name} from a previous run");
        }
    }

    let upload = |name: &str, body: Vec<u8>| {
        client
            .post(format!(
                "https://uploads.github.com/repos/{repo}/releases/{release_id}/assets?name={name}"
            ))
            .header("authorization", format!("Bearer {token}"))
            .header("accept", "application/vnd.github+json")
            .header("content-type", "application/octet-stream")
            .header("user-agent", GITHUB_PROBE_UA)
            .body(body)
            .send()
    };

    // ---- question 1: the state a torn upload leaves behind -----------------
    //
    // The tear is a dropped future, not a truncated body: without reqwest's
    // `stream` feature there is no way to hand it a body that ends early, and
    // enabling one is the dependency decision plan 4-01 recorded as closed.
    // Cutting the request at 250 ms of a 32 MiB body is the same thing from
    // GitHub's side — a connection that stops mid-transfer.
    println!("CAL-5 — {repo}, release {release_id}");
    let torn = tokio::time::timeout(
        Duration::from_millis(250),
        upload(TORN, vec![0x5a; 32 * 1024 * 1024]),
    )
    .await;
    match torn {
        Err(_) => println!("  the 32 MiB upload was cut at 250 ms, as intended"),
        Ok(done) => println!(
            "  WARNING: the 32 MiB upload COMPLETED inside 250 ms ({:?}) — this run says \
             nothing about a torn upload; re-run on a slower link or with a larger body",
            done.map(|r| r.status())
        ),
    }

    // Polled rather than slept on: each listing is a real round trip, which is
    // the only delay this probe is willing to spend.
    for attempt in 1..=5 {
        let found = list(client.clone(), token.clone(), assets_url.clone())
            .await
            .into_iter()
            .find(|a| a["name"].as_str() == Some(TORN));
        match found {
            Some(a) => println!(
                "  poll {attempt}: {TORN} state={:?} size={:?} digest={:?}",
                a["state"].as_str(),
                a["size"].as_u64(),
                a["digest"].as_str()
            ),
            None => println!("  poll {attempt}: {TORN} is not in the listing at all"),
        }
    }
    println!(
        "  CAL-5a = whatever state is printed above, the resume scan deletes and re-uploads \
         anything that is not exactly \"uploaded\", so an unrecognised value fails safe. \
         Record the observed value in docs/sync-format.md §10 — and do NOT relax the size \
         check on the strength of it."
    );

    // ---- question 2: digest on a complete upload ---------------------------
    let body = b"cal5 complete upload".to_vec();
    let whole: serde_json::Value = upload(WHOLE, body.clone())
        .await
        .expect("the complete upload must reach uploads.github.com")
        .error_for_status()
        .expect("the complete upload must succeed")
        .json()
        .await
        .expect("the upload response must be JSON");
    println!(
        "  {WHOLE} on upload: state={:?} size={:?} digest={:?}",
        whole["state"].as_str(),
        whole["size"].as_u64(),
        whole["digest"].as_str()
    );
    let relisted = list(client.clone(), token.clone(), assets_url.clone())
        .await
        .into_iter()
        .find(|a| a["name"].as_str() == Some(WHOLE));
    let digest = relisted
        .as_ref()
        .and_then(|a| a["digest"].as_str())
        .map(str::to_string);
    println!(
        "  {WHOLE} relisted:  state={:?} digest={digest:?}",
        relisted.as_ref().and_then(|a| a["state"].as_str())
    );

    match &digest {
        Some(d) => {
            let sha = {
                use sha2::{Digest, Sha256};
                let hex: String = Sha256::digest(&body)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect();
                format!("sha256:{hex}")
            };
            println!(
                "  CAL-5b = digest IS populated as {d:?}; a plain SHA-256 of the body is \
                 {sha:?} — {}. If they agree, a later phase can verify uploads without \
                 re-downloading them and D3's extra 115 MB on a first push disappears. \
                 Record it in docs/sync-format.md §10.",
                if *d == sha {
                    "they AGREE"
                } else {
                    "they DIFFER"
                }
            );
        }
        None => println!(
            "  CAL-5b = digest is NOT populated. D3's verifying download stays exactly as \
             `upload::run` implements it. Record it in docs/sync-format.md §10."
        ),
    }

    // Leave the release as it was found.
    for asset in list(client.clone(), token.clone(), assets_url).await {
        let name = asset["name"].as_str().unwrap_or_default();
        if name == TORN || name == WHOLE {
            let id = asset["id"].as_u64().expect("an asset id");
            client
                .delete(api(format!("/releases/assets/{id}")))
                .header("authorization", format!("Bearer {token}"))
                .header("user-agent", GITHUB_PROBE_UA)
                .send()
                .await
                .expect("the cleanup delete must reach api.github.com");
            println!("  cleaned up {name}");
        }
    }
}
