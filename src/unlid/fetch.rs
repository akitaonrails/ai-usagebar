//! Unlid fetch — reads the account balance from `GET /v1/me` under the shared
//! cache + flock primitives. Single-endpoint balance, same shape as
//! `novita::fetch`.

use std::time::Duration;

use crate::cache::{Cache, acquire_lock_async};
use crate::error::{AppError, Result};
use crate::usage::{UnlidSnapshot, finite_amount};
use crate::vendor::{MAX_BODY_BYTES, read_body_capped};

use super::types::{MeData, to_snapshot};

pub const BASE_URL: &str = "https://api.unlid.ai";
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const LOCK_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub me: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            me: format!("{BASE_URL}/v1/me"),
        }
    }
}

/// This vendor's [`Outcome`](crate::outcome::Outcome) — the shared shape,
/// specialised to its snapshot.
pub type FetchOutcome = crate::outcome::Outcome<UnlidSnapshot>;

pub async fn fetch_snapshot(
    client: &reqwest::Client,
    api_key: &str,
    cache: &Cache,
    endpoints: &Endpoints,
    cache_ttl: Duration,
) -> Result<FetchOutcome> {
    cache.ensure_dir()?;
    let _lock = acquire_lock_async(&cache.lock_path(), LOCK_TIMEOUT).await?;

    if let Some(bytes) = cache.fresh_payload(cache_ttl)?
        && let Ok(snap) = parse_cache(&bytes)
    {
        return Ok(crate::outcome::Outcome::cached(snap, cache, false));
    }

    match fetch_live(client, endpoints, api_key).await {
        Ok(me) => {
            let snap = to_snapshot(me)?;
            let bytes = serde_json::to_vec(&serde_json::json!({
                "snapshot": {
                    "balance": snap.balance,
                    "held": snap.held,
                    "month_spend": snap.month_spend,
                }
            }))?;
            cache.write_payload(&bytes)?;
            Ok(crate::outcome::Outcome::fresh(snap))
        }
        // A transient failure records nothing, so the previous error stays.
        Err(e) if e.is_transient() => crate::outcome::fallback(cache, None, e, parse_cache),
        // Including 401/403 (#589): the last good reading is served stale with
        // the fixed auth diagnostic; with no cache the original error surfaces.
        Err(AppError::Http { status, body }) => {
            cache.mark_stale();
            let diag = cache.write_last_error(status, &body);
            crate::outcome::fallback(
                cache,
                Some(diag),
                AppError::Http { status, body },
                parse_cache,
            )
        }
        Err(e) => {
            cache.mark_stale();
            let diag = cache.write_last_error(0, &e.to_string());
            crate::outcome::fallback(cache, Some(diag), e, parse_cache)
        }
    }
}

fn parse_cache(bytes: &[u8]) -> Result<UnlidSnapshot> {
    let v: serde_json::Value = serde_json::from_slice(bytes)?;
    let s = v
        .get("snapshot")
        .ok_or_else(|| AppError::Schema("unlid cache missing 'snapshot' field".into()))?;
    // Cached money is required, not optional: a truncated or half-written
    // payload must be refetched rather than rendered as $0.00.
    let field = |name: &str| -> Result<f64> {
        let v = s[name]
            .as_f64()
            .ok_or_else(|| AppError::Schema(format!("unlid cache missing '{name}'")))?;
        finite_amount("unlid cache", name, v)
    };
    Ok(UnlidSnapshot {
        balance: field("balance")?,
        held: field("held")?,
        month_spend: field("month_spend")?,
    })
}

async fn fetch_live(
    client: &reqwest::Client,
    endpoints: &Endpoints,
    api_key: &str,
) -> Result<MeData> {
    let resp = tokio::time::timeout(
        HTTP_TIMEOUT,
        client
            .get(&endpoints.me)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Accept", "application/json")
            .send(),
    )
    .await
    .map_err(|_| AppError::Transport(format!("unlid timeout: {}", endpoints.me)))??;

    let status = resp.status();
    let bytes = read_body_capped(resp, MAX_BODY_BYTES).await?;

    if !status.is_success() {
        let body = String::from_utf8_lossy(&bytes).chars().take(200).collect();
        return Err(AppError::Http {
            status: status.as_u16(),
            body,
        });
    }
    serde_json::from_slice(&bytes)
        .map_err(|e| AppError::Schema(format!("unlid {}: {e}", endpoints.me)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const OK_BODY: &str = r#"{"key":{"id":"k-1","name":"test","prefix":"ul-test"},
        "balance_usd":12.34,"held_usd":0.5,"month_spend_usd":6.78}"#;

    fn cache_fixture() -> (TempDir, Cache) {
        let td = TempDir::new().unwrap();
        let cache = Cache::at(td.path().join("unlid"));
        cache.ensure_dir().unwrap();
        (td, cache)
    }

    fn endpoints(server: &mockito::Server) -> Endpoints {
        Endpoints {
            me: format!("{}/v1/me", server.url()),
        }
    }

    fn seed(cache: &Cache) {
        let warm = serde_json::json!({
            "snapshot": { "balance": 42.0, "held": 1.0, "month_spend": 3.0 }
        });
        cache.write_payload(warm.to_string().as_bytes()).unwrap();
    }

    async fn fetch_fresh(
        endpoints: &Endpoints,
        cache: &Cache,
        ttl: Duration,
    ) -> Result<FetchOutcome> {
        fetch_snapshot(&reqwest::Client::new(), "ul-test", cache, endpoints, ttl).await
    }

    #[tokio::test]
    async fn live_fetch_parses_the_balance() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/v1/me")
            .match_header("authorization", "Bearer ul-test")
            .with_status(200)
            .with_body(OK_BODY)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let out = fetch_fresh(&endpoints(&server), &cache, Duration::from_secs(0))
            .await
            .unwrap();
        assert_eq!(out.snapshot.balance, 12.34);
        assert_eq!(out.snapshot.held, 0.5);
        assert_eq!(out.snapshot.month_spend, 6.78);
        assert!(!out.stale);
    }

    /// #589: a rejected key keeps the last good reading, stale, with the fixed
    /// diagnostic — the 401 body (which may echo the key) never reaches it.
    #[tokio::test]
    async fn a_401_and_a_403_keep_the_cached_balance_stale_and_redacted() {
        for status in [401, 403] {
            let mut server = mockito::Server::new_async().await;
            server
                .mock("GET", "/v1/me")
                .with_status(status)
                .with_body(
                    r#"{"error":{"message":"UNLID user@example.test <credential>",
                        "type":"authentication_error","code":"invalid_api_key"}}"#,
                )
                .create_async()
                .await;

            let (_td, cache) = cache_fixture();
            seed(&cache);
            let out = fetch_fresh(&endpoints(&server), &cache, Duration::from_secs(0))
                .await
                .expect("a warm cache must still produce an outcome");

            assert!(out.stale);
            assert_eq!(out.snapshot.balance, 42.0);
            let (code, msg) = out.last_error.expect("the rejection must be reported");
            assert_eq!(code, status as u16);
            assert_eq!(msg, crate::error::AUTH_FAILURE_MESSAGE);
            assert!(!msg.contains("UNLID"), "{msg}");
            assert!(!msg.contains("<credential>"), "{msg}");
        }
    }

    #[tokio::test]
    async fn a_401_with_no_cache_returns_the_original_error() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/v1/me")
            .with_status(401)
            .with_body(r#"{"error":{"code":"invalid_api_key"}}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let err = fetch_fresh(&endpoints(&server), &cache, Duration::from_secs(0))
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Http { status: 401, .. }), "{err:?}");
    }

    #[tokio::test]
    async fn a_401_body_quoting_the_key_never_reaches_the_diagnostics() {
        let body = r#"{"error":{"message":"invalid key ul-test-secret-123",
            "code":"invalid_api_key"}}"#;

        // Warm cache: stale outcome with the fixed message.
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/v1/me")
            .with_status(401)
            .with_body(body)
            .create_async()
            .await;
        let (_td, cache) = cache_fixture();
        seed(&cache);
        let out = fetch_fresh(&endpoints(&server), &cache, Duration::from_secs(0))
            .await
            .expect("a warm cache must still produce an outcome");
        let (_, msg) = out.last_error.expect("the rejection must be reported");
        assert!(!msg.contains("ul-test"), "{msg}");

        // No cache: the user-facing message must not echo the key either.
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/v1/me")
            .with_status(401)
            .with_body(body)
            .create_async()
            .await;
        let (_td, cache) = cache_fixture();
        let err = fetch_fresh(&endpoints(&server), &cache, Duration::from_secs(0))
            .await
            .unwrap_err();
        assert!(!err.user_message().contains("ul-test"), "{err:?}");
    }

    #[tokio::test]
    async fn malformed_200_body_does_not_become_a_zero_balance() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/v1/me")
            .with_status(200)
            .with_body(r#"{"key":{"id":"k-1","name":"test","prefix":"ul-test"}}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let out = fetch_fresh(&endpoints(&server), &cache, Duration::from_secs(0)).await;
        assert!(out.is_err(), "expected a schema error, got {out:?}");
    }

    #[tokio::test]
    async fn corrupt_cache_is_not_served_as_zero() {
        // A truncated payload must be refetched, not rendered as $0.00.
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/v1/me")
            .with_status(200)
            .with_body(OK_BODY)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let partial = serde_json::json!({ "snapshot": { "balance": 42.0 } });
        cache.write_payload(partial.to_string().as_bytes()).unwrap();

        let out = fetch_fresh(&endpoints(&server), &cache, Duration::from_secs(3600))
            .await
            .unwrap();
        assert_eq!(out.snapshot.balance, 12.34);
        assert!(!out.stale);
    }
}
