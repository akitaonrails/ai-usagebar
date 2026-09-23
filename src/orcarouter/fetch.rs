//! OrcaRouter fetch — combines `/v1/dashboard/billing/usage` and
//! `/v1/dashboard/billing/subscription` under the shared cache + flock
//! primitives. Mirrors `openrouter::fetch` semantics (fresh cache
//! short-circuits; on failure, fall back to cache + mark stale), with the
//! minimax-style key-fingerprint cache scope: a single key selects the
//! account, so a cache written for one key must never be shown for another.

use std::time::Duration;

use crate::cache::{Cache, acquire_lock_async};
use crate::error::{AppError, Result};
use crate::usage::OrcaRouterSnapshot;
use crate::vendor::{MAX_BODY_BYTES, read_body_capped};

use super::types::{SubscriptionResponse, UsageResponse, combine};

pub const BASE_URL: &str = "https://api.orcarouter.ai/v1";
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const LOCK_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub usage: String,
    pub subscription: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            usage: format!("{BASE_URL}/dashboard/billing/usage"),
            subscription: format!("{BASE_URL}/dashboard/billing/subscription"),
        }
    }
}

/// This vendor's [`Outcome`](crate::outcome::Outcome) — the shared shape,
/// specialised to its snapshot.
pub type FetchOutcome = crate::outcome::Outcome<OrcaRouterSnapshot>;

/// Cache-aware fetch.
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

    if let Some(bytes) = cache.fresh_payload(cache_ttl)?
        && let Ok(outcome) = reuse_cache(&bytes, cache, false, &target)
    {
        return Ok(outcome);
    }

    match fetch_live(client, endpoints, api_key).await {
        Ok((usage, subscription)) => {
            let snap = combine(&usage, &subscription);
            let cache_repr = serde_json::json!({
                "target": target,
                "snapshot": serde_repr(&snap),
            });
            let bytes = serde_json::to_vec(&cache_repr)?;
            cache.write_payload(&bytes)?;
            Ok(crate::outcome::Outcome::fresh(snap))
        }
        Err(e) if e.is_transient() => fallback_silent(cache, &target, e),
        Err(AppError::Http { status, body }) => {
            cache.mark_stale();
            let diag = cache.write_last_error(status, &body);
            fallback_with_error(cache, Some(diag), &target, AppError::Http { status, body })
        }
        Err(e) => {
            cache.mark_stale();
            let diag = cache.write_last_error(0, &e.to_string());
            fallback_with_error(cache, Some(diag), &target, e)
        }
    }
}

/// Identity of the key the cached card belongs to. Replacing the configured
/// key can select another OrcaRouter account, so store only a fingerprint of
/// it: a cache change detector, not an authentication secret (minimax
/// pattern).
fn target_key(endpoints: &Endpoints, api_key: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    api_key.hash(&mut hasher);
    format!("{}|key:{:016x}", endpoints.usage, hasher.finish())
}

fn fallback_silent(cache: &Cache, target: &str, original: AppError) -> Result<FetchOutcome> {
    crate::outcome::fallback(cache, None, original, |bytes| parse_cache(bytes, target))
}

fn fallback_with_error(
    cache: &Cache,
    last_error: Option<(u16, String)>,
    target: &str,
    original: AppError,
) -> Result<FetchOutcome> {
    crate::outcome::fallback(cache, last_error, original, |bytes| {
        parse_cache(bytes, target)
    })
}

fn reuse_cache(bytes: &[u8], cache: &Cache, stale: bool, target: &str) -> Result<FetchOutcome> {
    let snap = parse_cache(bytes, target)?;
    Ok(crate::outcome::Outcome::cached(snap, cache, stale))
}

fn serde_repr(snap: &OrcaRouterSnapshot) -> serde_json::Value {
    serde_json::json!({
        "spent_cents": snap.spent_cents,
        "limit_cents": snap.limit_cents,
        "access_until": snap.access_until.map(|t| t.timestamp()),
    })
}

/// Cached spend is required, not optional: a truncated or half-written payload
/// must be refetched rather than rendered as $0.00 spent. `limit_cents` stays
/// optional — the wire itself can omit it (or send the unlimited sentinel).
fn parse_cache(bytes: &[u8], target: &str) -> Result<OrcaRouterSnapshot> {
    let v: serde_json::Value = serde_json::from_slice(bytes)?;
    let cached_target = v.get("target").and_then(serde_json::Value::as_str);
    if cached_target != Some(target) {
        return Err(AppError::Schema(format!(
            "orcarouter cache belongs to a different key ({}); refetching",
            cached_target.unwrap_or("unknown")
        )));
    }
    let s = v
        .get("snapshot")
        .ok_or_else(|| AppError::Schema("orcarouter cache missing 'snapshot' field".into()))?;
    let spent_cents = s["spent_cents"]
        .as_i64()
        .ok_or_else(|| AppError::Schema("orcarouter cache missing 'spent_cents'".into()))?;
    if spent_cents < 0 {
        return Err(AppError::Schema(
            "orcarouter cache 'spent_cents' cannot be negative".into(),
        ));
    }
    let limit_cents = match s.get("limit_cents") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => {
            let cents = value.as_i64().ok_or_else(|| {
                AppError::Schema("orcarouter cache 'limit_cents' is not an integer".into())
            })?;
            if cents <= 0 {
                return Err(AppError::Schema(
                    "orcarouter cache 'limit_cents' must be positive".into(),
                ));
            }
            Some(cents)
        }
    };
    let access_until = match s.get("access_until") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => {
            let secs = value.as_i64().ok_or_else(|| {
                AppError::Schema("orcarouter cache 'access_until' is not an integer".into())
            })?;
            if secs <= 0 {
                return Err(AppError::Schema(
                    "orcarouter cache 'access_until' must be a positive Unix timestamp".into(),
                ));
            }
            Some(chrono::DateTime::from_timestamp(secs, 0).ok_or_else(|| {
                AppError::Schema("orcarouter cache 'access_until' is out of range".into())
            })?)
        }
    };
    Ok(OrcaRouterSnapshot {
        spent_cents,
        limit_cents,
        access_until,
    })
}

async fn fetch_live(
    client: &reqwest::Client,
    endpoints: &Endpoints,
    api_key: &str,
) -> Result<(UsageResponse, SubscriptionResponse)> {
    let usage_fut = fetch_one::<UsageResponse>(client, &endpoints.usage, api_key);
    let subscription_fut =
        fetch_one::<SubscriptionResponse>(client, &endpoints.subscription, api_key);
    let (usage, subscription) = tokio::join!(usage_fut, subscription_fut);
    Ok((usage?, subscription?))
}

async fn fetch_one<T: for<'de> serde::Deserialize<'de>>(
    client: &reqwest::Client,
    url: &str,
    api_key: &str,
) -> Result<T> {
    let resp = tokio::time::timeout(
        HTTP_TIMEOUT,
        client
            .get(url)
            .header("Authorization", format!("Bearer {api_key}"))
            .send(),
    )
    .await
    .map_err(|_| AppError::Transport(format!("orcarouter timeout: {url}")))??;

    let status = resp.status();
    let bytes = read_body_capped(resp, MAX_BODY_BYTES).await?;

    if !status.is_success() {
        let body = String::from_utf8_lossy(&bytes).chars().take(200).collect();
        return Err(AppError::Http {
            status: status.as_u16(),
            body,
        });
    }
    // one-api lineage errors arrive as HTTP 200 wrapped in an OpenAI error
    // envelope. Detected before the typed parse so it surfaces as a failure
    // carrying the vendor's own message — never as a confident zero.
    if let Some(message) = error_envelope(&bytes) {
        return Err(AppError::Schema(format!(
            "orcarouter {url}: error envelope: {message}"
        )));
    }
    serde_json::from_slice(&bytes).map_err(|e| AppError::Schema(format!("orcarouter {url}: {e}")))
}

/// `{"error":{"message":…}}` at HTTP 200, or `None` for any other body. The
/// message is capped the way error bodies are elsewhere.
fn error_envelope(bytes: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let message = v.get("error")?.get("message")?.as_str()?;
    Some(message.chars().take(200).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const USAGE_BODY: &str = r#"{"object":"list","total_usage":275}"#;
    const SUBSCRIPTION_BODY: &str = r#"{
        "object":"billing_subscription",
        "has_payment_method":true,
        "soft_limit_usd":12.5,
        "hard_limit_usd":12.5,
        "system_hard_limit_usd":12.5,
        "access_until":1790000000
    }"#;

    fn cache_fixture() -> (TempDir, Cache) {
        let td = TempDir::new().unwrap();
        let cache = Cache::at(td.path().join("orcarouter"));
        cache.ensure_dir().unwrap();
        (td, cache)
    }

    fn endpoints_for(server: &mockito::Server) -> Endpoints {
        Endpoints {
            usage: format!("{}/v1/dashboard/billing/usage", server.url()),
            subscription: format!("{}/v1/dashboard/billing/subscription", server.url()),
        }
    }

    async fn mock_both(server: &mut mockito::Server, usage: &str, subscription: &str) {
        server
            .mock("GET", "/v1/dashboard/billing/usage")
            .match_header("authorization", "Bearer sk-orca-test")
            .with_status(200)
            .with_body(usage)
            .create_async()
            .await;
        server
            .mock("GET", "/v1/dashboard/billing/subscription")
            .match_header("authorization", "Bearer sk-orca-test")
            .with_status(200)
            .with_body(subscription)
            .create_async()
            .await;
    }

    #[tokio::test]
    async fn live_fetch_combines_both_endpoints() {
        let mut server = mockito::Server::new_async().await;
        mock_both(&mut server, USAGE_BODY, SUBSCRIPTION_BODY).await;

        let (_td, cache) = cache_fixture();
        let out = fetch_snapshot(
            &reqwest::Client::new(),
            "sk-orca-test",
            &cache,
            &endpoints_for(&server),
            Duration::from_secs(0),
        )
        .await
        .unwrap();
        // Cents, not dollars: 275 cents = $2.75 of a $12.50 limit.
        assert_eq!(out.snapshot.spent_cents, 275);
        assert_eq!(out.snapshot.limit_cents, Some(1250));
        assert_eq!(out.snapshot.remaining_cents(), Some(975));
        assert!(out.snapshot.access_until.is_some());
        assert!(!out.stale);
    }

    /// The unlimited sentinel must arrive as a spend-only card, never a
    /// $100M wallet.
    #[tokio::test]
    async fn unlimited_sentinel_yields_a_spend_only_snapshot() {
        let mut server = mockito::Server::new_async().await;
        mock_both(
            &mut server,
            USAGE_BODY,
            r#"{"soft_limit_usd":100000000,"hard_limit_usd":100000000,
                "system_hard_limit_usd":100000000,"access_until":0}"#,
        )
        .await;

        let (_td, cache) = cache_fixture();
        let out = fetch_snapshot(
            &reqwest::Client::new(),
            "sk-orca-test",
            &cache,
            &endpoints_for(&server),
            Duration::from_secs(0),
        )
        .await
        .unwrap();
        assert_eq!(out.snapshot.limit_cents, None);
        assert_eq!(out.snapshot.remaining_cents(), None);
        assert_eq!(out.snapshot.consumed_pct(), None);
        assert_eq!(out.snapshot.spent_cents, 275);
        assert!(out.snapshot.access_until.is_none());
    }

    /// one-api deployments report auth failures as HTTP 200 + an OpenAI error
    /// envelope. That must surface as a failure, not deserialize into a
    /// zero-spend card that overwrites a good cache.
    #[tokio::test]
    async fn http_200_error_envelope_is_a_failure_not_a_zero() {
        let mut server = mockito::Server::new_async().await;
        mock_both(
            &mut server,
            r#"{"error":{"message":"Invalid API key provided","type":"invalid_request_error"}}"#,
            SUBSCRIPTION_BODY,
        )
        .await;

        let (_td, cache) = cache_fixture();
        let err = fetch_snapshot(
            &reqwest::Client::new(),
            "sk-orca-test",
            &cache,
            &endpoints_for(&server),
            Duration::from_secs(0),
        )
        .await
        .unwrap_err();
        let message = err.to_string();
        assert!(
            matches!(err, AppError::Schema(_)),
            "expected schema failure, got {err:?}"
        );
        assert!(message.contains("error envelope"), "{message}");
        assert!(message.contains("Invalid API key"), "{message}");
    }

    /// With a cold cache there is no figure to show, so the real status must
    /// survive — the openrouter lesson.
    #[tokio::test]
    async fn an_http_error_with_no_cache_surfaces_the_status() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/v1/dashboard/billing/usage")
            .with_status(401)
            .with_body(r#"{"error":"unauthorized"}"#)
            .create_async()
            .await;
        server
            .mock("GET", "/v1/dashboard/billing/subscription")
            .with_status(401)
            .with_body(r#"{"error":"unauthorized"}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let err = fetch_snapshot(
            &reqwest::Client::new(),
            "sk-orca-test",
            &cache,
            &endpoints_for(&server),
            Duration::from_secs(0),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, AppError::Http { status: 401, .. }),
            "expected the 401 to survive, got {err:?}"
        );
    }

    /// 401 with a warm cache: the last good card is shown stale, the status
    /// rides along via `write_last_error`, and the 401 body is replaced by the
    /// shared auth-failure message — never the upstream text.
    #[tokio::test]
    async fn http_401_falls_back_to_cache_with_a_redacted_body() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/v1/dashboard/billing/usage")
            .with_status(401)
            .with_body(r#"{"error":{"message":"sk-orca-test leaked"}}"#)
            .create_async()
            .await;
        server
            .mock("GET", "/v1/dashboard/billing/subscription")
            .with_status(401)
            .with_body(r#"{"error":"unauthorized"}}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let endpoints = endpoints_for(&server);
        let seed = serde_json::json!({
            "target": target_key(&endpoints, "sk-orca-test"),
            "snapshot": { "spent_cents": 900, "limit_cents": 1250, "access_until": null },
        });
        cache
            .write_payload(&serde_json::to_vec(&seed).unwrap())
            .unwrap();

        let out = fetch_snapshot(
            &reqwest::Client::new(),
            "sk-orca-test",
            &cache,
            &endpoints,
            Duration::from_secs(0),
        )
        .await
        .unwrap();
        assert!(out.stale);
        assert_eq!(out.snapshot.spent_cents, 900);
        assert_eq!(out.snapshot.remaining_cents(), Some(350));
        let (code, body) = out.last_error.expect("error recorded alongside the figure");
        assert_eq!(code, 401);
        assert!(
            !body.contains("sk-orca-test") && !body.contains("leaked"),
            "401 body must be redacted, got {body:?}"
        );
    }

    /// Replacing the configured key can select another account. A fresh cache
    /// from the previous key must not cross that boundary.
    #[tokio::test]
    async fn cache_from_another_key_is_rejected() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/v1/dashboard/billing/usage")
            .match_header("authorization", "Bearer sk-orca-new")
            .with_status(200)
            .with_body(USAGE_BODY)
            .expect(1)
            .create_async()
            .await;
        server
            .mock("GET", "/v1/dashboard/billing/subscription")
            .match_header("authorization", "Bearer sk-orca-new")
            .with_status(200)
            .with_body(SUBSCRIPTION_BODY)
            .expect(1)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let endpoints = endpoints_for(&server);
        let seed = serde_json::json!({
            "target": target_key(&endpoints, "sk-orca-old"),
            "snapshot": { "spent_cents": 99999, "limit_cents": 100000, "access_until": null },
        });
        cache
            .write_payload(&serde_json::to_vec(&seed).unwrap())
            .unwrap();

        // A long TTL: the payload IS fresh, it just belongs to another key.
        let out = fetch_snapshot(
            &reqwest::Client::new(),
            "sk-orca-new",
            &cache,
            &endpoints,
            Duration::from_secs(3600),
        )
        .await
        .unwrap();
        assert_eq!(out.snapshot.spent_cents, 275, "refetched, not 99999");

        let stored = std::fs::read_to_string(cache.payload_path()).unwrap();
        assert!(!stored.contains("sk-orca-new"), "cache leaked the API key");
    }

    #[test]
    fn cache_round_trips_and_validates() {
        let endpoints = Endpoints::default();
        let target = target_key(&endpoints, "k");
        let snap = OrcaRouterSnapshot {
            spent_cents: 275,
            limit_cents: Some(1250),
            access_until: chrono::DateTime::from_timestamp(1_790_000_000, 0),
        };
        let bytes = serde_json::to_vec(&serde_json::json!({
            "target": target,
            "snapshot": serde_repr(&snap),
        }))
        .unwrap();
        assert_eq!(parse_cache(&bytes, &target).unwrap(), snap);

        // A spend-only card round-trips too (sentinel or absent limit).
        let spend_only = OrcaRouterSnapshot {
            spent_cents: 275,
            limit_cents: None,
            access_until: None,
        };
        let bytes = serde_json::to_vec(&serde_json::json!({
            "target": target,
            "snapshot": serde_repr(&spend_only),
        }))
        .unwrap();
        assert_eq!(parse_cache(&bytes, &target).unwrap(), spend_only);

        // Missing or negative spend is drift, not zero.
        for bad in [
            serde_json::json!({"target": target, "snapshot": {}}),
            serde_json::json!({"target": target, "snapshot": {"spent_cents": -1}}),
            serde_json::json!({"target": target, "snapshot": {"spent_cents": "275"}}),
        ] {
            let err = parse_cache(&serde_json::to_vec(&bad).unwrap(), &target);
            assert!(err.is_err(), "{bad} must not parse");
        }
        // A foreign target is refused.
        let foreign = serde_json::to_vec(&serde_json::json!({
            "target": "somewhere-else",
            "snapshot": {"spent_cents": 1, "limit_cents": null, "access_until": null},
        }))
        .unwrap();
        assert!(parse_cache(&foreign, &target).is_err());
    }
}
