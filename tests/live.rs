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
//! - **Kimi (subscription)**: reads the Kimi Code CLI's own OAuth session,
//!   refreshing it (and writing the rotation back to the CLI's credential
//!   file) when it is close to expiry, then asserts the same public snapshot
//!   fields. `kimi_subscription_live` skips when the CLI is not logged in and
//!   `KIMI_CODE_HOME` is unset.
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
//! - **Antigravity (app closed)**: reads the Google session Antigravity saved
//!   in the OS keyring and asks the Cloud Code API for the quota summary,
//!   bypassing local discovery so the remote path is what runs. Asserts the
//!   snapshot is attributed to the remote source and every reported window
//!   is a bounded percentage. `antigravity_remote_live` skips when the
//!   keyring holds no session, or an expired one it cannot renew; set
//!   `ANTIGRAVITY_OAUTH_CLIENT_ID` / `ANTIGRAVITY_OAUTH_CLIENT_SECRET` to
//!   exercise the refresh as well.

use std::time::Duration;

use ai_usagebar::anthropic;
use ai_usagebar::antigravity;
use ai_usagebar::cache::Cache;
use ai_usagebar::cursor;
use ai_usagebar::error::AppError;
use ai_usagebar::kimi;
use ai_usagebar::kiro;
use ai_usagebar::minimax;
use ai_usagebar::ollama;
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
async fn kimi_subscription_live() {
    // The subscription path has no API key — the credential is the OAuth
    // session the Kimi Code CLI stored after its device-code login. So this
    // test needs `kimi` installed and logged in (or `KIMI_CODE_HOME` pointing
    // at a copy of that home) and skips otherwise, like `kiro_live`.
    //
    // It exercises the refresh too whenever the stored access token is inside
    // its 2-minute buffer, which for a 15-minute token is most runs — and that
    // rotation is written back to the CLI's own credential file, exactly as
    // production does. Point `KIMI_CODE_HOME` at a copy if that matters to you.
    let home = match std::env::var("KIMI_CODE_HOME") {
        Ok(p) if !p.trim().is_empty() => std::path::PathBuf::from(p),
        _ => kimi::oauth::default_home().expect("resolve home dir"),
    };
    let credentials = kimi::oauth::credentials_path_in(&home);
    if !kimi::oauth::is_logged_in(&credentials) {
        eprintln!(
            "kimi_subscription_live: no Kimi Code CLI login at {} — skipping (run `kimi` and log in, or set KIMI_CODE_HOME)",
            credentials.display()
        );
        return;
    }

    let region = kimi::oauth::read_region_marker(&home).unwrap_or(kimi::oauth::Region::MainlandCn);
    let cache = xdg_cache_for("kimi-subscription");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let out = kimi::fetch::fetch_snapshot_with_auth(
        &client,
        &kimi::fetch::Auth::KimiCode(kimi::fetch::KimiCodeAuth::in_home(&home)),
        &cache,
        &kimi::fetch::Endpoints::for_region(region),
        Duration::from_secs(0),
    )
    .await
    .expect("kimi subscription fetch should succeed against the real API");

    assert_pct("kimi.weekly", out.snapshot.weekly_pct());
    if out.snapshot.window_limit > 0 {
        assert_pct("kimi.window", out.snapshot.window_pct());
    }
    // The refresh must never leave the CLI without a usable login.
    assert!(
        kimi::oauth::is_logged_in(&credentials),
        "the Kimi Code CLI credential file must still hold a login afterwards"
    );
    println!(
        "✅ kimi (subscription) — plan={:?}, weekly={} / {}, window={} / {}",
        out.snapshot.plan,
        out.snapshot.weekly_used,
        out.snapshot.weekly_limit,
        out.snapshot.window_used,
        out.snapshot.window_limit,
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

/// Command Code reuses whichever local agent harness is signed in, so this
/// test needs no key of its own — it skips when nothing on the machine holds
/// a credential.
#[tokio::test]
#[ignore]
async fn commandcode_live() {
    use ai_usagebar::commandcode;

    let credential = match commandcode::creds::resolve(None) {
        Ok(credential) => credential,
        Err(error) => {
            eprintln!("commandcode_live: {error} — skipping Command Code smoke test");
            return;
        }
    };
    let cache = xdg_cache_for("commandcode");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let endpoints = commandcode::fetch::Endpoints::default();
    let out = commandcode::fetch::fetch_snapshot(
        &client,
        &credential.token,
        &cache,
        &endpoints,
        Duration::from_secs(0),
    )
    .await
    .expect("command code fetch should succeed against the real API");

    // The ledger is load-bearing; the windows are what the bar leads with.
    assert!(
        out.snapshot.five_hour.is_some()
            || out.snapshot.weekly.is_some()
            || out.snapshot.credits.is_some(),
        "commandcode returned neither a window nor a ledger"
    );
    for (label, window) in [
        ("commandcode.five_hour", out.snapshot.five_hour.as_ref()),
        ("commandcode.weekly", out.snapshot.weekly.as_ref()),
    ] {
        if let Some(window) = window {
            assert_pct(label, window.pct());
            assert!(window.cap > 0.0, "{label} reported a non-positive cap");
            assert!(
                window.resets_at.is_some(),
                "{label} lost its reset timestamp — the API sends a ms epoch"
            );
        }
    }
    println!(
        "✅ command code — plan={:?}, 5h={:?}, weekly={:?}, credits={:?} of pool {:?}",
        out.snapshot.plan,
        out.snapshot
            .five_hour
            .as_ref()
            .map(|w| (w.used, w.cap, w.pct())),
        out.snapshot
            .weekly
            .as_ref()
            .map(|w| (w.used, w.cap, w.pct())),
        out.snapshot.credits.as_ref().map(|c| c.remaining()),
        out.snapshot.credit_pool,
    );
}

/// Antigravity's remote fallback: the keyring session against the real Cloud
/// Code API, with local discovery bypassed so this is the path that runs even
/// while a product happens to be open. Skips when nothing is saved.
#[tokio::test]
#[ignore = "live API; run with --ignored"]
async fn antigravity_remote_live() {
    use ai_usagebar::usage::AntigravitySource;
    use antigravity::cloud::OauthClient;
    use antigravity::fetch::{RemoteOverride, SavedCredential, fetch_snapshot_at};

    match antigravity::credential::read() {
        Ok(Some(_)) => {}
        Ok(None) => {
            eprintln!(
                "antigravity_remote_live: no saved Google session in the OS keyring — skipping \
                 (sign in to Antigravity once)"
            );
            return;
        }
        Err(e) => panic!("antigravity keyring read failed: {e}"),
    }

    let env = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
    let oauth = OauthClient::from_config(
        env("ANTIGRAVITY_OAUTH_CLIENT_ID").as_deref(),
        env("ANTIGRAVITY_OAUTH_CLIENT_SECRET").as_deref(),
    );
    let cache = xdg_cache_for("antigravity-remote");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let out = fetch_snapshot_at(
        &client,
        &cache,
        Duration::from_secs(0),
        oauth.as_ref(),
        RemoteOverride {
            credential: SavedCredential::Keyring,
            endpoints: None,
            local_bases: Some(vec![]),
        },
        chrono::Utc::now(),
    )
    .await;
    let out = match out {
        Ok(out) => out,
        // An expired session with nothing to renew it is the user's state, not
        // drift in the vendor's shape: the message already says what to do.
        Err(AppError::Credentials(msg)) if msg.contains("sign in again") => {
            eprintln!("antigravity_remote_live: {msg} — skipping");
            return;
        }
        Err(e) => panic!("antigravity remote fetch should succeed against the real API: {e}"),
    };

    let snap = out.snapshot;
    assert_eq!(
        snap.source,
        AntigravitySource::Remote,
        "antigravity: with discovery bypassed the snapshot must come from the API"
    );
    assert!(!snap.plan.is_empty(), "antigravity plan label empty");
    assert!(
        snap.account.starts_with("acct:"),
        "antigravity: remote account attribution changed: {}",
        snap.account
    );
    for (label, window) in [
        ("antigravity.session", snap.session.as_ref()),
        ("antigravity.weekly", snap.weekly.as_ref()),
        (
            "antigravity.third_party_session",
            snap.third_party_session.as_ref(),
        ),
        (
            "antigravity.third_party_weekly",
            snap.third_party_weekly.as_ref(),
        ),
    ] {
        if let Some(window) = window {
            assert_pct(label, window.utilization_pct);
        }
    }
    println!(
        "✅ antigravity (app closed) — plan={}, session={:?}, weekly={:?}, third-party session={:?}, third-party weekly={:?}",
        snap.plan,
        snap.session.as_ref().map(|w| w.utilization_pct),
        snap.weekly.as_ref().map(|w| w.utilization_pct),
        snap.third_party_session.as_ref().map(|w| w.utilization_pct),
        snap.third_party_weekly.as_ref().map(|w| w.utilization_pct),
    );
}

#[tokio::test]
#[ignore = "live API"]
async fn ollama_live() {
    let Ok(api_key) = std::env::var("OLLAMA_API_KEY") else {
        eprintln!("OLLAMA_API_KEY not set — skipping ollama_live");
        return;
    };

    let cache = xdg_cache_for("ollama");
    let client = reqwest::Client::new();
    let endpoints = ollama::fetch::Endpoints::default();
    let out = ollama::fetch_snapshot(
        &client,
        &api_key,
        "pro",
        &cache,
        &endpoints,
        Duration::from_secs(0),
    )
    .await
    .expect("ollama fetch should succeed against the real API");

    let snap = &out.snapshot;
    if let Some(w) = snap.session.as_ref() {
        assert_pct("ollama.session", w.utilization_pct);
    }
    if let Some(w) = snap.weekly.as_ref() {
        assert_pct("ollama.weekly", w.utilization_pct);
    }
    assert!(!snap.plan.is_empty(), "ollama plan label empty");
    println!(
        "\u{2705} ollama — plan={}, session={:?}, weekly={:?}, session models={}, weekly models={}",
        snap.plan,
        snap.session.as_ref().map(|w| w.utilization_pct),
        snap.weekly.as_ref().map(|w| w.utilization_pct),
        snap.session_models.len(),
        snap.weekly_models.len(),
    );
}
