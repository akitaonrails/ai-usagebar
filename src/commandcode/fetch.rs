//! Fetch Command Code usage, with the same cache/lock discipline as every
//! other vendor.
//!
//! The official CLI's `/usage` view is assembled from three calls: `whoami`
//! names the org that scopes the rest, the credit ledger carries the balance
//! and the rolling spend windows, and the subscription names the plan. Only
//! the ledger is load-bearing — a failure anywhere else costs its own detail
//! and nothing more.

use std::fmt::Write as _;
use std::time::Duration;

use chrono::DateTime;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::cache::{Cache, acquire_lock_async};
use crate::error::{AppError, Result};
use crate::vendor::{MAX_BODY_BYTES, read_body_capped};

use super::types::{Snapshot, apply_subscription, parse_credits};

pub const BASE_URL: &str = "https://api.commandcode.ai";

pub const WHOAMI_PATH: &str = "/alpha/whoami";
pub const CREDITS_PATH: &str = "/alpha/billing/credits";
pub const SUBSCRIPTIONS_PATH: &str = "/alpha/billing/subscriptions";

const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const LOCK_TIMEOUT: Duration = Duration::from_secs(15);
const SCHEMA_ERROR: &str = "Command Code usage response schema mismatch";

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: BASE_URL.to_string(),
        }
    }
}

impl Endpoints {
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base.trim_end_matches('/'))
    }
}

/// This vendor's [`Outcome`](crate::outcome::Outcome) — the shared shape,
/// specialised to its snapshot.
pub type FetchOutcome = crate::outcome::Outcome<Snapshot>;

pub async fn fetch_snapshot(
    client: &reqwest::Client,
    token: &str,
    cache: &Cache,
    endpoints: &Endpoints,
    ttl: Duration,
) -> Result<FetchOutcome> {
    cache.ensure_dir()?;
    let _lock = acquire_lock_async(&cache.lock_path(), LOCK_TIMEOUT).await?;
    let target = target_key(endpoints, token);

    if let Some(bytes) = cache.fresh_payload(ttl)?
        && let Ok(snapshot) = parse_cache(&bytes, &target)
    {
        return Ok(crate::outcome::Outcome::cached(snapshot, cache, false));
    }

    match fetch_live(client, token, endpoints).await {
        Ok(snapshot) => {
            let body = serde_json::to_vec(&serde_json::json!({
                "target": target,
                "response": snapshot_repr(&snapshot),
            }))?;
            cache.write_payload(&body)?;
            Ok(crate::outcome::Outcome::fresh(snapshot))
        }
        Err(error @ AppError::Transport(_)) => fallback_or_error(cache, None, &target, error),
        Err(AppError::Http { status, .. }) => {
            let message = status_message(status).to_string();
            cache.mark_stale();
            cache.write_last_error(status, &message);
            fallback_or_error(
                cache,
                Some((status, message.clone())),
                &target,
                AppError::Http {
                    status,
                    body: message,
                },
            )
        }
        Err(AppError::Schema(_)) => {
            let message = SCHEMA_ERROR.to_string();
            cache.mark_stale();
            cache.write_last_error(0, &message);
            fallback_or_error(
                cache,
                Some((0, message.clone())),
                &target,
                AppError::Schema(message),
            )
        }
        Err(error) => fallback_or_error(cache, None, &target, error),
    }
}

async fn fetch_live(
    client: &reqwest::Client,
    token: &str,
    endpoints: &Endpoints,
) -> Result<Snapshot> {
    // whoami scopes the ledger to an org. A personal account has none, and a
    // failure here only costs that scoping, so the error is not propagated.
    let org_id = get_json(client, token, &endpoints.url(WHOAMI_PATH), &[])
        .await
        .ok()
        .and_then(|value| value.get("org")?.get("id")?.as_str().map(str::to_string));
    let scope: Vec<(&str, String)> = org_id.iter().map(|id| ("orgId", id.clone())).collect();

    let credits = get_json(client, token, &endpoints.url(CREDITS_PATH), &scope).await?;
    let mut snapshot =
        parse_credits(&credits).map_err(|error| AppError::Schema(error.to_string()))?;

    // The plan is presentation detail; without it the windows still render.
    if let Ok(subscription) =
        get_json(client, token, &endpoints.url(SUBSCRIPTIONS_PATH), &scope).await
    {
        apply_subscription(&mut snapshot, &subscription);
    }

    Ok(snapshot)
}

async fn get_json(
    client: &reqwest::Client,
    token: &str,
    url: &str,
    query: &[(&str, String)],
) -> Result<Value> {
    let response = tokio::time::timeout(
        HTTP_TIMEOUT,
        client
            .get(url)
            .bearer_auth(token)
            .query(query)
            .header(reqwest::header::ACCEPT, "application/json")
            .send(),
    )
    .await
    .map_err(|_| AppError::Transport("Command Code request timed out".to_string()))??;

    let status = response.status();
    let body = read_body_capped(response, MAX_BODY_BYTES).await?;
    if !status.is_success() {
        return Err(AppError::Http {
            status: status.as_u16(),
            body: status_message(status.as_u16()).to_string(),
        });
    }
    serde_json::from_slice(&body).map_err(|error| AppError::Schema(error.to_string()))
}

/// Stable, non-secret identity for the endpoint and the account the token
/// resolves to. Cache reuse must fail closed when either input changes.
fn target_key(endpoints: &Endpoints, token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let mut fingerprint = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(fingerprint, "{byte:02x}");
    }
    format!("{}|key:{fingerprint}", endpoints.base)
}

fn status_message(status: u16) -> &'static str {
    match status {
        401 | 403 => crate::error::AUTH_FAILURE_MESSAGE,
        429 => "Command Code rate limited the usage request",
        500..=599 => "Command Code usage endpoint is unavailable",
        _ => "Command Code usage request failed",
    }
}

/// Cache the parsed snapshot rather than the raw bodies: the raw ledger is
/// account data with no reason to sit on disk longer than it must.
fn snapshot_repr(snapshot: &Snapshot) -> Value {
    let window = |window: &Option<super::types::SpendWindow>| {
        window.as_ref().map(|w| {
            serde_json::json!({
                "used": w.used,
                "cap": w.cap,
                "resetAt": w.resets_at.map(|at| at.timestamp_millis()),
            })
        })
    };
    serde_json::json!({
        "plan": snapshot.plan,
        "creditPool": snapshot.credit_pool,
        "periodEnd": snapshot.period_end.map(|at| at.timestamp_millis()),
        "credits": snapshot.credits.as_ref().map(|c| serde_json::json!({
            "monthlyCredits": c.monthly,
            "purchasedCredits": c.purchased,
            "freeCredits": c.free,
        })),
        "windowLimits": {
            "fiveHour": window(&snapshot.five_hour),
            "weekly": window(&snapshot.weekly),
        },
    })
}

fn parse_cache(bytes: &[u8], target: &str) -> Result<Snapshot> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| AppError::Schema("Command Code cache is invalid".into()))?;
    if value.get("target").and_then(Value::as_str) != Some(target) {
        return Err(AppError::Schema(
            "Command Code cache belongs to a different account".into(),
        ));
    }
    let response = value
        .get("response")
        .ok_or_else(|| AppError::Schema("Command Code cache is missing its response".into()))?;
    let mut snapshot =
        parse_credits(response).map_err(|error| AppError::Schema(error.to_string()))?;
    snapshot.plan = response
        .get("plan")
        .and_then(Value::as_str)
        .map(str::to_string);
    snapshot.credit_pool = response.get("creditPool").and_then(Value::as_f64);
    // Older caches predate the field; a missing entry simply clears it and
    // the next live refresh restores it.
    snapshot.period_end = response
        .get("periodEnd")
        .and_then(Value::as_i64)
        .and_then(DateTime::from_timestamp_millis);
    Ok(snapshot)
}

fn fallback_or_error(
    cache: &Cache,
    last_error: Option<(u16, String)>,
    target: &str,
    error: AppError,
) -> Result<FetchOutcome> {
    crate::outcome::fallback(cache, last_error, error, |body| parse_cache(body, target))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::Cache;

    const CREDITS_BODY: &str = include_str!("../../tests/fixtures/commandcode/credits.json");
    const SUBSCRIPTION_BODY: &str =
        include_str!("../../tests/fixtures/commandcode/subscriptions.json");

    fn cache_in(dir: &std::path::Path) -> Cache {
        Cache::at(dir.join("commandcode"))
    }

    fn endpoints(server: &mockito::Server) -> Endpoints {
        Endpoints { base: server.url() }
    }

    #[tokio::test]
    async fn walks_the_chain_and_scopes_calls_to_the_org() {
        let mut server = mockito::Server::new_async().await;
        let whoami = server
            .mock("GET", WHOAMI_PATH)
            .with_body(r#"{"org":{"id":"org-42"}}"#)
            .create_async()
            .await;
        let credits = server
            .mock("GET", CREDITS_PATH)
            .match_query(mockito::Matcher::UrlEncoded(
                "orgId".into(),
                "org-42".into(),
            ))
            .with_body(CREDITS_BODY)
            .create_async()
            .await;
        let subscription = server
            .mock("GET", SUBSCRIPTIONS_PATH)
            .match_query(mockito::Matcher::UrlEncoded(
                "orgId".into(),
                "org-42".into(),
            ))
            .with_body(SUBSCRIPTION_BODY)
            .create_async()
            .await;
        let dir = tempfile::tempdir().unwrap();

        let outcome = fetch_snapshot(
            &reqwest::Client::new(),
            "tok",
            &cache_in(dir.path()),
            &endpoints(&server),
            Duration::from_secs(60),
        )
        .await
        .expect("fetch must succeed");

        whoami.assert_async().await;
        credits.assert_async().await;
        subscription.assert_async().await;
        assert_eq!(outcome.snapshot.plan.as_deref(), Some("GOAT"));
        assert_eq!(outcome.snapshot.five_hour.unwrap().pct(), 25);
        assert!(!outcome.stale);
    }

    #[tokio::test]
    async fn a_personal_account_without_an_org_still_gets_its_ledger() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", WHOAMI_PATH)
            .with_body(r#"{"org":null}"#)
            .create_async()
            .await;
        let credits = server
            .mock("GET", CREDITS_PATH)
            .match_query(mockito::Matcher::Missing)
            .with_body(CREDITS_BODY)
            .create_async()
            .await;
        server
            .mock("GET", SUBSCRIPTIONS_PATH)
            .with_body(SUBSCRIPTION_BODY)
            .create_async()
            .await;
        let dir = tempfile::tempdir().unwrap();

        let outcome = fetch_snapshot(
            &reqwest::Client::new(),
            "tok",
            &cache_in(dir.path()),
            &endpoints(&server),
            Duration::from_secs(60),
        )
        .await
        .expect("fetch must succeed without an org");

        credits.assert_async().await;
        assert!(outcome.snapshot.weekly.is_some());
    }

    #[tokio::test]
    async fn a_failing_subscription_costs_the_plan_not_the_windows() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", WHOAMI_PATH)
            .with_body("{}")
            .create_async()
            .await;
        server
            .mock("GET", CREDITS_PATH)
            .with_body(CREDITS_BODY)
            .create_async()
            .await;
        server
            .mock("GET", SUBSCRIPTIONS_PATH)
            .with_status(500)
            .create_async()
            .await;
        let dir = tempfile::tempdir().unwrap();

        let outcome = fetch_snapshot(
            &reqwest::Client::new(),
            "tok",
            &cache_in(dir.path()),
            &endpoints(&server),
            Duration::from_secs(60),
        )
        .await
        .expect("windows must survive a subscription failure");

        assert!(outcome.snapshot.plan.is_none());
        assert_eq!(outcome.snapshot.weekly.unwrap().pct(), 30);
    }

    #[tokio::test]
    async fn a_failing_ledger_is_fatal_and_maps_401_to_the_auth_message() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", WHOAMI_PATH)
            .with_body("{}")
            .create_async()
            .await;
        server
            .mock("GET", CREDITS_PATH)
            .with_status(401)
            .create_async()
            .await;
        let dir = tempfile::tempdir().unwrap();

        let error = fetch_snapshot(
            &reqwest::Client::new(),
            "tok",
            &cache_in(dir.path()),
            &endpoints(&server),
            Duration::from_secs(60),
        )
        .await
        .expect_err("the ledger is load-bearing");

        assert!(
            error
                .to_string()
                .contains(crate::error::AUTH_FAILURE_MESSAGE),
            "{error}"
        );
    }

    #[tokio::test]
    async fn a_fresh_cache_is_served_without_touching_the_network() {
        let mut server = mockito::Server::new_async().await;
        let whoami = server
            .mock("GET", WHOAMI_PATH)
            .expect(1)
            .with_body("{}")
            .create_async()
            .await;
        server
            .mock("GET", CREDITS_PATH)
            .expect(1)
            .with_body(CREDITS_BODY)
            .create_async()
            .await;
        server
            .mock("GET", SUBSCRIPTIONS_PATH)
            .expect(1)
            .with_body(SUBSCRIPTION_BODY)
            .create_async()
            .await;
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_in(dir.path());
        let endpoints = endpoints(&server);
        let client = reqwest::Client::new();

        let first = fetch_snapshot(&client, "tok", &cache, &endpoints, Duration::from_secs(600))
            .await
            .unwrap();
        let second = fetch_snapshot(&client, "tok", &cache, &endpoints, Duration::from_secs(600))
            .await
            .unwrap();

        // Each endpoint was called exactly once across both fetches.
        whoami.assert_async().await;
        assert_eq!(first.snapshot, second.snapshot);
        assert_eq!(second.snapshot.plan.as_deref(), Some("GOAT"));
    }

    #[tokio::test]
    async fn a_cache_written_for_another_token_is_not_reused() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", WHOAMI_PATH)
            .with_body("{}")
            .expect_at_least(2)
            .create_async()
            .await;
        server
            .mock("GET", CREDITS_PATH)
            .with_body(CREDITS_BODY)
            .expect_at_least(2)
            .create_async()
            .await;
        server
            .mock("GET", SUBSCRIPTIONS_PATH)
            .with_body(SUBSCRIPTION_BODY)
            .expect_at_least(2)
            .create_async()
            .await;
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_in(dir.path());
        let endpoints = endpoints(&server);
        let client = reqwest::Client::new();

        fetch_snapshot(
            &client,
            "token-a",
            &cache,
            &endpoints,
            Duration::from_secs(600),
        )
        .await
        .unwrap();
        // A different account must re-fetch rather than read the first's cache.
        fetch_snapshot(
            &client,
            "token-b",
            &cache,
            &endpoints,
            Duration::from_secs(600),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_stale_cache_carries_the_widget_through_an_outage() {
        let mut server = mockito::Server::new_async().await;
        let whoami = server
            .mock("GET", WHOAMI_PATH)
            .with_body("{}")
            .create_async()
            .await;
        let credits = server
            .mock("GET", CREDITS_PATH)
            .with_body(CREDITS_BODY)
            .create_async()
            .await;
        let subscription = server
            .mock("GET", SUBSCRIPTIONS_PATH)
            .with_body(SUBSCRIPTION_BODY)
            .create_async()
            .await;
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_in(dir.path());
        let endpoints = endpoints(&server);
        let client = reqwest::Client::new();

        fetch_snapshot(&client, "tok", &cache, &endpoints, Duration::ZERO)
            .await
            .unwrap();

        // The endpoint starts failing; the cached snapshot still renders.
        whoami.remove_async().await;
        credits.remove_async().await;
        subscription.remove_async().await;
        server
            .mock("GET", CREDITS_PATH)
            .with_status(503)
            .create_async()
            .await;
        server
            .mock("GET", WHOAMI_PATH)
            .with_status(503)
            .create_async()
            .await;

        let outcome = fetch_snapshot(&client, "tok", &cache, &endpoints, Duration::ZERO)
            .await
            .expect("a stale snapshot beats no snapshot");

        assert!(outcome.stale);
        assert_eq!(outcome.snapshot.plan.as_deref(), Some("GOAT"));
        assert_eq!(outcome.last_error.unwrap().0, 503);
    }

    #[tokio::test]
    async fn a_schema_mismatch_is_reported_as_drift_not_as_success() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", WHOAMI_PATH)
            .with_body("{}")
            .create_async()
            .await;
        server
            .mock("GET", CREDITS_PATH)
            .with_body(r#"{"unexpected":true}"#)
            .create_async()
            .await;
        let dir = tempfile::tempdir().unwrap();

        let error = fetch_snapshot(
            &reqwest::Client::new(),
            "tok",
            &cache_in(dir.path()),
            &endpoints(&server),
            Duration::from_secs(60),
        )
        .await
        .expect_err("an unrecognised ledger must not pass as usage");

        assert!(error.to_string().contains("schema"), "{error}");
    }
}
