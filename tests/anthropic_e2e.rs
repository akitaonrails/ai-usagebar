//! End-to-end integration test for the Anthropic vendor.
//!
//! Stands up a mockito server that pretends to be the Anthropic OAuth +
//! usage endpoints, drives the full `fetch_snapshot` + `render_anthropic`
//! pipeline against canned fixtures, and asserts the resulting Waybar JSON
//! via `insta` snapshots. Catches schema drift in either the wire types or
//! the rendered output.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use ai_usagebar::anthropic::{self, fetch::Endpoints};
use ai_usagebar::cache::Cache;
use ai_usagebar::format::updated_at_hm;
use ai_usagebar::theme::Theme;
use ai_usagebar::widget::render::{DEFAULT_FORMAT, RenderInput, render_anthropic};
use chrono::{TimeZone, Utc};
use tempfile::{NamedTempFile, TempDir};

fn write_creds() -> NamedTempFile {
    // Token expires far in the future → no refresh needed during the test.
    let expires_ms = chrono::Utc::now().timestamp_millis() + 3_600_000;
    let body = format!(
        r#"{{"claudeAiOauth":{{
            "accessToken":"AT","refreshToken":"RT",
            "expiresAt": {expires_ms},
            "subscriptionType":"max","rateLimitTier":"default_claude_max_5x"
        }}}}"#
    );
    let mut f = NamedTempFile::new().unwrap();
    f.write_all(body.as_bytes()).unwrap();
    f.flush().unwrap();
    f
}

fn cache_in(td: &TempDir) -> Cache {
    let c = Cache::at(td.path().join("anthropic"));
    c.ensure_dir().unwrap();
    c
}

fn read_fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("missing fixture {}: {e}", path.display());
    })
}

fn normalize_updated_time(
    tooltip: &str,
    now: chrono::DateTime<Utc>,
    cache_age: Option<Duration>,
) -> String {
    let updated = updated_at_hm(now, cache_age);
    let expected = format!("Updated {updated}");
    assert!(
        tooltip.contains(&expected),
        "tooltip did not contain {expected:?}"
    );
    tooltip.replace(&expected, "Updated HH:MM")
}

#[tokio::test]
async fn full_response_renders_expected_waybar_json() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/api/oauth/usage")
        .with_status(200)
        .with_body(read_fixture("anthropic_usage_full.json"))
        .create_async()
        .await;

    let td = TempDir::new().unwrap();
    let cache = cache_in(&td);
    let creds = write_creds();
    let client = reqwest::Client::new();
    let endpoints = Endpoints {
        usage: format!("{}/api/oauth/usage", server.url()),
        token: format!("{}/v1/oauth/token", server.url()),
    };
    let outcome = anthropic::fetch_snapshot(
        &client,
        creds.path(),
        &cache,
        &endpoints,
        Duration::from_secs(0),
    )
    .await
    .unwrap();

    // Pin "now" to a known value so countdown / pacing are deterministic.
    let now = Utc.with_ymd_and_hms(2026, 5, 23, 12, 0, 0).unwrap();
    let theme = Theme::default(); // One Dark — stable across machines
    let format = DEFAULT_FORMAT.to_string();
    let input = RenderInput {
        outcome: &outcome,
        theme: &theme,
        format: &format,
        tooltip_format: None,
        icon: None,
        pace_tolerance: 5,
        format_pace_color: false,
        tooltip_pace_pts: false,
        now,
    };
    let out = render_anthropic(&input);
    insta::assert_snapshot!("full_response_bar_text", &out.text);
    insta::assert_snapshot!(
        "full_response_tooltip",
        normalize_updated_time(&out.tooltip, now, outcome.cache_age)
    );
    assert_eq!(format!("{:?}", out.class), "Mid");
}

#[tokio::test]
async fn no_sonnet_no_extra_renders_minimal_tooltip() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/api/oauth/usage")
        .with_status(200)
        .with_body(read_fixture("anthropic_usage_minimal.json"))
        .create_async()
        .await;

    let td = TempDir::new().unwrap();
    let cache = cache_in(&td);
    let creds = write_creds();
    let client = reqwest::Client::new();
    let endpoints = Endpoints {
        usage: format!("{}/api/oauth/usage", server.url()),
        token: format!("{}/v1/oauth/token", server.url()),
    };
    let outcome = anthropic::fetch_snapshot(
        &client,
        creds.path(),
        &cache,
        &endpoints,
        Duration::from_secs(0),
    )
    .await
    .unwrap();
    let now = Utc.with_ymd_and_hms(2026, 5, 23, 12, 0, 0).unwrap();
    let theme = Theme::default();
    let format = DEFAULT_FORMAT.to_string();
    let input = RenderInput {
        outcome: &outcome,
        theme: &theme,
        format: &format,
        tooltip_format: None,
        icon: None,
        pace_tolerance: 5,
        format_pace_color: false,
        tooltip_pace_pts: false,
        now,
    };
    let out = render_anthropic(&input);
    assert!(!out.tooltip.contains("Sonnet only"));
    assert!(!out.tooltip.contains("Extra usage"));
    insta::assert_snapshot!(
        "minimal_response_tooltip",
        normalize_updated_time(&out.tooltip, now, outcome.cache_age)
    );
}

#[tokio::test]
async fn http_429_falls_back_to_stale_cache_with_pause_indicator() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/api/oauth/usage")
        .with_status(429)
        .with_body(r#"{"error":{"type":"rate_limit_error","message":"slow down"}}"#)
        .create_async()
        .await;

    let td = TempDir::new().unwrap();
    let cache = cache_in(&td);
    // Seed cache so fallback has something to serve.
    cache
        .write_payload(read_fixture("anthropic_usage_full.json").as_bytes())
        .unwrap();
    let creds = write_creds();
    let client = reqwest::Client::new();
    let endpoints = Endpoints {
        usage: format!("{}/api/oauth/usage", server.url()),
        token: format!("{}/v1/oauth/token", server.url()),
    };
    let outcome = anthropic::fetch_snapshot(
        &client,
        creds.path(),
        &cache,
        &endpoints,
        Duration::from_secs(0),
    )
    .await
    .unwrap();
    assert!(outcome.stale);
    assert_eq!(outcome.last_error.as_ref().map(|(c, _)| *c), Some(429));

    let now = Utc.with_ymd_and_hms(2026, 5, 23, 12, 0, 0).unwrap();
    let theme = Theme::default();
    let format = DEFAULT_FORMAT.to_string();
    let input = RenderInput {
        outcome: &outcome,
        theme: &theme,
        format: &format,
        tooltip_format: None,
        icon: None,
        pace_tolerance: 5,
        format_pace_color: false,
        tooltip_pace_pts: false,
        now,
    };
    let out = render_anthropic(&input);
    assert!(out.text.contains("⏸"));
    assert!(out.tooltip.contains("HTTP 429"));
    assert!(out.tooltip.contains("slow down"));
}
