//! Fetch Mistral's monthly budgets from the billing routes the official
//! `mistral-vibe` CLI uses.
//!
//! `/v1/billing/budget` is the primary read and must answer. The subscription
//! read is best-effort: a failure costs the plan label, never the budgets.

use std::fmt::Write as _;
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::cache::{Cache, acquire_lock_async};
use crate::error::{AppError, Result};
use crate::usage::MistralSnapshot;
use crate::vendor::{MAX_BODY_BYTES, read_body_capped};

use super::types::{Budget, CachedPayload, Subscription};

/// The 403 body is never shown: `/billing/budget` requires
/// `see_org_inference_usage`, and the official CLI reads 403 as a member
/// without that permission rather than a bad key.
const PERMISSION_MESSAGE: &str = "Mistral accepted the key but the account may not read billing usage. \
     The organization needs the `see_org_inference_usage` permission; ask an admin to grant it.";

pub const BASE_URL: &str = "https://api.mistral.ai/v1";
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const LOCK_TIMEOUT: Duration = Duration::from_secs(15);

pub type FetchOutcome = crate::outcome::Outcome<MistralSnapshot>;

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub budget: String,
    pub subscription: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            budget: format!("{BASE_URL}/billing/budget"),
            subscription: format!("{BASE_URL}/billing/subscription"),
        }
    }
}

pub async fn fetch_snapshot(
    client: &reqwest::Client,
    api_key: &str,
    cache: &Cache,
    endpoints: &Endpoints,
    cache_ttl: Duration,
) -> Result<FetchOutcome> {
    cache.ensure_dir()?;
    let _lock = acquire_lock_async(&cache.lock_path(), LOCK_TIMEOUT).await?;
    let target = target_key(endpoints, api_key);
    let parse_cache = |bytes: &[u8]| -> Result<MistralSnapshot> {
        let payload: CachedPayload = serde_json::from_slice(bytes)
            .map_err(|_| AppError::Schema("Mistral billing cache is invalid".into()))?;
        if payload.target != target {
            return Err(AppError::Schema(
                "Mistral billing cache belongs to a different account".into(),
            ));
        }
        payload.snapshot()
    };

    if let Some(bytes) = cache.fresh_payload(cache_ttl)?
        && let Ok(snapshot) = parse_cache(&bytes)
    {
        return Ok(crate::outcome::Outcome::cached(snapshot, cache, false));
    }
    // An unparseable cache still must not bypass an active 429 backoff.
    if let Some(remaining) = cache.backoff_remaining() {
        return Err(AppError::Http {
            status: 429,
            body: format!(
                "rate limited; next attempt in {}",
                crate::cache::human_backoff(remaining)
            ),
        });
    }

    match request(client, api_key, endpoints, &target).await {
        Ok((payload, snapshot)) => {
            cache.write_payload(&serde_json::to_vec(&payload)?)?;
            Ok(crate::outcome::Outcome::fresh(snapshot))
        }
        Err(error) if error.is_transient() => {
            crate::outcome::fallback(cache, None, error, parse_cache)
        }
        // A revoked key must never keep showing the last good budgets. 403 is
        // not a bad key: `/billing/budget` needs `see_org_inference_usage`, so
        // it gets an actionable permission message instead of the generic
        // authentication text `Cache::write_last_error` applies to 401/403.
        Err(error @ AppError::Http { status: 401, .. }) => {
            cache.mark_stale();
            let _ = cache.write_last_error(401, &error.to_string());
            Err(error)
        }
        Err(AppError::Http { status: 403, .. }) => {
            cache.mark_stale();
            let _ = cache.write_last_error(0, PERMISSION_MESSAGE);
            Err(AppError::Permission(PERMISSION_MESSAGE.to_string()))
        }
        Err(error) => {
            let code = match &error {
                AppError::Http { status, .. } => *status,
                _ => 0,
            };
            cache.mark_stale();
            let diagnostic = cache.write_last_error(code, &error.to_string());
            crate::outcome::fallback(cache, Some(diagnostic), error, parse_cache)
        }
    }
}

/// Stable, non-secret identity for the endpoints and account selected by the
/// API key. The billing routes resolve the key to an account, so cache reuse
/// must fail closed when either input changes.
fn target_key(endpoints: &Endpoints, api_key: &str) -> String {
    let digest = Sha256::digest(api_key.as_bytes());
    let mut fingerprint = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(fingerprint, "{byte:02x}");
    }
    format!("{}|key:{fingerprint}", endpoints.budget)
}

async fn request(
    client: &reqwest::Client,
    api_key: &str,
    endpoints: &Endpoints,
    target: &str,
) -> Result<(CachedPayload, MistralSnapshot)> {
    // The budget is the reading; the subscription only names the plan. Run
    // both together and keep the budget's failure, not the plan's.
    let (budget, subscription) = tokio::join!(
        get_json::<Budget>(client, api_key, &endpoints.budget, "budget"),
        get_json::<Subscription>(client, api_key, &endpoints.subscription, "subscription"),
    );
    let payload = CachedPayload {
        target: target.to_string(),
        subscription: subscription.ok(),
        budget: budget?,
    };
    let snapshot = payload.snapshot()?;
    Ok((payload, snapshot))
}

async fn get_json<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    api_key: &str,
    url: &str,
    what: &str,
) -> Result<T> {
    let response = tokio::time::timeout(
        HTTP_TIMEOUT,
        client
            .get(url)
            .bearer_auth(api_key)
            .header(reqwest::header::ACCEPT, "application/json")
            .send(),
    )
    .await
    .map_err(|_| AppError::Transport(format!("Mistral {what} request timed out")))??;
    let status = response.status();
    let bytes = read_body_capped(response, MAX_BODY_BYTES).await?;
    if !status.is_success() {
        return Err(http_error(status.as_u16(), &bytes));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| AppError::Schema(format!("Mistral {what} response schema mismatch")))
}

/// Mistral's error body is `{"message": "...", "request_id": "..."}`. The
/// message is prose and must be sanitized before it can reach a tooltip.
#[derive(Deserialize)]
struct Problem {
    #[serde(default)]
    message: Option<String>,
}

fn http_error(status: u16, bytes: &[u8]) -> AppError {
    let detail = serde_json::from_slice::<Problem>(bytes)
        .ok()
        .and_then(|problem| problem.message)
        .unwrap_or_else(|| String::from_utf8_lossy(bytes).chars().take(200).collect());
    AppError::Http {
        status,
        body: crate::display::sanitize_untrusted_line(&detail),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn cache() -> (TempDir, Cache) {
        let dir = TempDir::new().unwrap();
        let cache = Cache::at(dir.path().join("mistral"));
        cache.ensure_dir().unwrap();
        (dir, cache)
    }

    const BUDGET: &str = r#"{
        "vibe_budget": {
            "usage_percentage": 0.0,
            "initial_budget": 150.0,
            "currency": "USD",
            "reset_at": "2026-12-01T00:00:00Z",
            "payg_enabled": false
        },
        "api_budget": {
            "usage_percentage": 15.6,
            "initial_budget": 15.0,
            "currency": "USD",
            "reset_at": "2026-12-01T00:00:00Z",
            "payg_enabled": false
        }
    }"#;

    const OTHER_BUDGET: &str = r#"{
        "vibe_budget": {
            "usage_percentage": 40.0,
            "initial_budget": 100.0,
            "currency": "EUR",
            "reset_at": "2026-12-01T00:00:00Z",
            "payg_enabled": false
        },
        "api_budget": {
            "usage_percentage": 42.0,
            "initial_budget": 20.0,
            "currency": "EUR",
            "reset_at": "2026-12-01T00:00:00Z",
            "payg_enabled": false
        }
    }"#;

    const SUBSCRIPTION: &str =
        r#"{"plan_name":"EDU","payg_enabled":false,"vibe_payg_enabled":false,"enterprise":false}"#;

    fn endpoints(server: &mockito::Server) -> Endpoints {
        Endpoints {
            budget: format!("{}/billing/budget", server.url()),
            subscription: format!("{}/billing/subscription", server.url()),
        }
    }

    /// Write a warm cache carrying the same identity a fetch would store.
    fn prime(cache: &Cache, endpoints: &Endpoints, key: &str) {
        let payload = CachedPayload {
            target: target_key(endpoints, key),
            subscription: Some(serde_json::from_str(SUBSCRIPTION).unwrap()),
            budget: serde_json::from_str(BUDGET).unwrap(),
        };
        cache
            .write_payload(&serde_json::to_vec(&payload).unwrap())
            .unwrap();
    }

    #[tokio::test]
    async fn sends_bearer_key_and_parses_both_budgets() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/billing/budget")
            .match_header("authorization", "Bearer mst-test")
            .match_header("accept", "application/json")
            .with_status(200)
            .with_body(BUDGET)
            .create_async()
            .await;
        server
            .mock("GET", "/billing/subscription")
            .match_header("authorization", "Bearer mst-test")
            .with_status(200)
            .with_body(SUBSCRIPTION)
            .create_async()
            .await;
        let (_dir, cache) = cache();

        let outcome = fetch_snapshot(
            &reqwest::Client::new(),
            "mst-test",
            &cache,
            &endpoints(&server),
            Duration::ZERO,
        )
        .await
        .unwrap();

        assert_eq!(outcome.snapshot.plan.as_deref(), Some("EDU"));
        assert_eq!(
            outcome.snapshot.api.as_ref().map(|w| w.utilization_pct),
            Some(16)
        );
        assert_eq!(outcome.snapshot.vibe.utilization_pct, 0);
        assert!((outcome.snapshot.api_budget.unwrap() - 15.0).abs() < 1e-9);
        assert!((outcome.snapshot.vibe_budget - 150.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn a_failed_subscription_still_returns_the_budgets() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/billing/budget")
            .with_status(200)
            .with_body(BUDGET)
            .create_async()
            .await;
        server
            .mock("GET", "/billing/subscription")
            .with_status(500)
            .with_body("upstream error")
            .create_async()
            .await;
        let (_dir, cache) = cache();

        let outcome = fetch_snapshot(
            &reqwest::Client::new(),
            "mst-test",
            &cache,
            &endpoints(&server),
            Duration::ZERO,
        )
        .await
        .unwrap();

        assert_eq!(outcome.snapshot.plan, None);
        assert_eq!(
            outcome.snapshot.api.as_ref().map(|w| w.utilization_pct),
            Some(16)
        );
    }

    #[tokio::test]
    async fn a_missing_api_budget_still_returns_the_vibe_budget() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/billing/budget")
            .with_status(200)
            .with_body(
                r#"{"vibe_budget":{"usage_percentage":40.0,"initial_budget":100.0,"currency":"EUR"}}"#,
            )
            .create_async()
            .await;
        server
            .mock("GET", "/billing/subscription")
            .with_status(500)
            .create_async()
            .await;
        let (_dir, cache) = cache();

        let outcome = fetch_snapshot(
            &reqwest::Client::new(),
            "mst-test",
            &cache,
            &endpoints(&server),
            Duration::ZERO,
        )
        .await
        .unwrap();

        assert!(outcome.snapshot.api.is_none());
        assert!(outcome.snapshot.api_budget.is_none());
        assert_eq!(outcome.snapshot.vibe.utilization_pct, 40);
        assert_eq!(outcome.snapshot.currency, "EUR");
    }

    #[tokio::test]
    async fn a_server_error_keeps_the_last_good_snapshot_stale() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/billing/budget")
            .with_status(500)
            .with_body("upstream error")
            .create_async()
            .await;
        server
            .mock("GET", "/billing/subscription")
            .with_status(500)
            .create_async()
            .await;
        let (_dir, cache) = cache();
        let endpoints = endpoints(&server);
        prime(&cache, &endpoints, "mst-test");

        let outcome = fetch_snapshot(
            &reqwest::Client::new(),
            "mst-test",
            &cache,
            &endpoints,
            Duration::ZERO,
        )
        .await
        .unwrap();

        assert!(outcome.stale);
        assert_eq!(outcome.snapshot.api.unwrap().utilization_pct, 16);
        assert!(outcome.last_error.is_some());
    }

    #[tokio::test]
    async fn a_transport_failure_keeps_the_last_good_snapshot_stale() {
        // `.invalid` never resolves, so the request fails at the transport
        // layer while the cache target still matches.
        let endpoints = Endpoints {
            budget: "http://mistral.invalid/v1/billing/budget".into(),
            subscription: "http://mistral.invalid/v1/billing/subscription".into(),
        };
        let (_dir, cache) = cache();
        prime(&cache, &endpoints, "mst-test");

        let outcome = fetch_snapshot(
            &reqwest::Client::new(),
            "mst-test",
            &cache,
            &endpoints,
            Duration::ZERO,
        )
        .await
        .unwrap();

        assert!(outcome.stale);
        assert_eq!(outcome.snapshot.api.unwrap().utilization_pct, 16);
        assert!(outcome.last_error.is_none(), "{:?}", outcome.last_error);
    }

    #[tokio::test]
    async fn unauthorized_does_not_serve_the_cached_budgets() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/billing/budget")
            .with_status(401)
            .with_body(r#"{"message":"Unauthorized"}"#)
            .create_async()
            .await;
        server
            .mock("GET", "/billing/subscription")
            .with_status(401)
            .create_async()
            .await;
        let (_dir, cache) = cache();
        let endpoints = endpoints(&server);
        prime(&cache, &endpoints, "mst-test");

        let error = fetch_snapshot(
            &reqwest::Client::new(),
            "mst-test",
            &cache,
            &endpoints,
            Duration::ZERO,
        )
        .await
        .unwrap_err();

        assert!(
            matches!(error, AppError::Http { status: 401, .. }),
            "{error:?}"
        );
        assert!(cache.is_stale());
    }

    #[tokio::test]
    async fn forbidden_is_a_permission_error_and_does_not_serve_the_cache() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/billing/budget")
            .with_status(403)
            .with_body(r#"{"message":"forbidden"}"#)
            .create_async()
            .await;
        server
            .mock("GET", "/billing/subscription")
            .with_status(403)
            .create_async()
            .await;
        let (_dir, cache) = cache();
        let endpoints = endpoints(&server);
        prime(&cache, &endpoints, "mst-test");

        let error = fetch_snapshot(
            &reqwest::Client::new(),
            "mst-test",
            &cache,
            &endpoints,
            Duration::ZERO,
        )
        .await
        .unwrap_err();

        assert!(matches!(error, AppError::Permission(_)), "{error:?}");
        assert!(error.to_string().contains("see_org_inference_usage"));
        assert!(!error.to_string().contains("forbidden"));
        assert!(cache.is_stale());
        let (code, message) = cache.read_last_error().unwrap();
        assert_eq!(code, 0);
        assert!(message.contains("see_org_inference_usage"));
    }

    #[tokio::test]
    async fn a_warm_cache_avoids_the_network() {
        let mut server = mockito::Server::new_async().await;
        let budget = server
            .mock("GET", "/billing/budget")
            .with_status(200)
            .with_body(BUDGET)
            .expect(1)
            .create_async()
            .await;
        server
            .mock("GET", "/billing/subscription")
            .with_status(200)
            .with_body(SUBSCRIPTION)
            .expect(1)
            .create_async()
            .await;
        let (_dir, cache) = cache();
        let endpoints = endpoints(&server);

        let first = fetch_snapshot(
            &reqwest::Client::new(),
            "mst-test",
            &cache,
            &endpoints,
            Duration::from_secs(60),
        )
        .await
        .unwrap();
        assert!(!first.stale);
        assert_eq!(first.snapshot.vibe_budget, 150.0);

        let second = fetch_snapshot(
            &reqwest::Client::new(),
            "mst-test",
            &cache,
            &endpoints,
            Duration::from_secs(60),
        )
        .await
        .unwrap();
        assert!(!second.stale);
        assert_eq!(second.snapshot.vibe_budget, 150.0);
        budget.assert_async().await;
    }

    #[tokio::test]
    async fn a_cache_from_another_key_is_ignored() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/billing/budget")
            .match_header("authorization", "Bearer key-b")
            .with_status(200)
            .with_body(OTHER_BUDGET)
            .create_async()
            .await;
        server
            .mock("GET", "/billing/subscription")
            .with_status(500)
            .create_async()
            .await;
        let (_dir, cache) = cache();
        let endpoints = endpoints(&server);
        prime(&cache, &endpoints, "key-a");

        let outcome = fetch_snapshot(
            &reqwest::Client::new(),
            "key-b",
            &cache,
            &endpoints,
            Duration::from_secs(60),
        )
        .await
        .unwrap();

        assert!(!outcome.stale);
        assert_eq!(outcome.snapshot.api.unwrap().utilization_pct, 42);
        assert_eq!(outcome.snapshot.currency, "EUR");
    }

    #[tokio::test]
    async fn a_cache_from_another_key_is_not_a_stale_fallback() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/billing/budget")
            .match_header("authorization", "Bearer key-b")
            .with_status(500)
            .create_async()
            .await;
        server
            .mock("GET", "/billing/subscription")
            .with_status(500)
            .create_async()
            .await;
        let (_dir, cache) = cache();
        let endpoints = endpoints(&server);
        prime(&cache, &endpoints, "key-a");

        let error = fetch_snapshot(
            &reqwest::Client::new(),
            "key-b",
            &cache,
            &endpoints,
            Duration::from_secs(60),
        )
        .await
        .unwrap_err();

        assert!(
            matches!(error, AppError::Http { status: 500, .. }),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn a_malformed_budget_is_a_schema_error() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/billing/budget")
            .with_status(200)
            .with_body(r#"{"api_budget":{},"vibe_budget":{}}"#)
            .create_async()
            .await;
        let (_dir, cache) = cache();

        let error = fetch_snapshot(
            &reqwest::Client::new(),
            "mst-test",
            &cache,
            &endpoints(&server),
            Duration::ZERO,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, AppError::Schema(_)), "{error:?}");
    }
}
