//! Live API smoke test suite — DETECTS UNDOCUMENTED-ENDPOINT DRIFT.
//!
//! Hits the real Anthropic, OpenAI Codex, Z.AI, and OpenRouter endpoints
//! using credentials from your shell. Asserts only the *fields we depend on*
//! so when a vendor renames or removes one, the failure points at the exact
//! field rather than dumping the whole response.
//!
//! These tests are `#[ignore]` so plain `cargo test` doesn't hit external
//! APIs (and won't fail on machines without creds). Run explicitly:
//!
//! ```bash
//! source ~/.config/zsh/secrets
//! cargo test --test live -- --ignored --nocapture
//! # or:
//! make smoke
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

use std::time::Duration;

use ai_usagebar::anthropic;
use ai_usagebar::cache::Cache;
use ai_usagebar::error::AppError;
use ai_usagebar::openai;
use ai_usagebar::openrouter;
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
    // file). Skip cleanly when neither source resolves — a no-op on machines
    // without creds, as the module doc promises, not a hard failure.
    if !creds_path.exists() {
        match anthropic::creds::read_from(&creds_path) {
            Ok(_) => {}
            Err(err) if is_missing_credentials(&err) => {
                eprintln!("anthropic_live: no Claude credentials (file or Keychain) — skipping");
                return;
            }
            Err(err) => panic!("anthropic_live: failed to read Claude credentials: {err}"),
        }
    }
    let cache = xdg_cache_for("anthropic");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let endpoints = anthropic::fetch::Endpoints::default();
    let out = anthropic::fetch_snapshot(
        &client,
        &creds_path,
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
    if let Some(e) = out.snapshot.extra {
        assert!(e.limit.0 >= 0, "anthropic extra.limit < 0");
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
            .map(|e| (e.spent.fmt_dollars(), e.limit.fmt_dollars())),
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
    assert_pct("openai.session", out.snapshot.session.utilization_pct);
    assert_pct("openai.weekly", out.snapshot.weekly.utilization_pct);
    println!(
        "✅ openai — plan={}, session={}%, weekly={}%, credits={:?}",
        out.snapshot.plan,
        out.snapshot.session.utilization_pct,
        out.snapshot.weekly.utilization_pct,
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
