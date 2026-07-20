//! Kilo Code fetch — reads the credit balance from `/api/profile/balance`
//! under the shared cache + flock primitives. Mirrors `openrouter::fetch`
//! semantics (fresh cache short-circuits; on failure, fall back to cache +
//! mark stale), but Kilo is a single-endpoint balance.

use std::time::Duration;

use crate::cache::{Cache, MAX_STALE, acquire_lock_async};
use crate::error::{AppError, Result};
use crate::usage::{KiloSnapshot, finite_amount};
use crate::vendor::{MAX_BODY_BYTES, read_body_capped};

use super::types::{BalanceData, to_snapshot};

pub const BASE_URL: &str = "https://api.kilo.ai";
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const LOCK_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub balance: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            balance: format!("{BASE_URL}/api/profile/balance"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FetchOutcome {
    pub snapshot: KiloSnapshot,
    pub stale: bool,
    pub last_error: Option<(u16, String)>,
    pub cache_age: Option<Duration>,
}

/// Cache-aware fetch. `organization_id`, when set, scopes the balance to a team
/// (the `x-kilocode-organizationid` header); omitting it returns the personal
/// balance.
pub async fn fetch_snapshot(
    client: &reqwest::Client,
    api_key: &str,
    cache: &Cache,
    endpoints: &Endpoints,
    cache_ttl: Duration,
    organization_id: Option<&str>,
) -> Result<FetchOutcome> {
    cache.ensure_dir()?;
    let _lock = acquire_lock_async(&cache.lock_path(), LOCK_TIMEOUT).await?;

    let target = target_key(organization_id);

    if let Some(bytes) = cache.fresh_payload(cache_ttl)?
        && let Ok(outcome) = reuse_cache(&bytes, cache, false, &target)
    {
        return Ok(outcome);
    }

    match fetch_live(client, endpoints, api_key, organization_id).await {
        Ok(balance) => {
            let snap = to_snapshot(balance)?;
            let cache_repr = serde_json::json!({
                "target": target,
                "snapshot": { "label": snap.label, "balance": snap.balance },
            });
            let bytes = serde_json::to_vec(&cache_repr)?;
            cache.write_payload(&bytes)?;
            Ok(FetchOutcome {
                snapshot: snap,
                stale: false,
                last_error: None,
                cache_age: Some(Duration::ZERO),
            })
        }
        Err(e) if e.is_transient() => fallback_silent(cache, &target, e),
        Err(AppError::Http { status, body }) => {
            cache.mark_stale();
            cache.write_last_error(status, &body);
            let diag = (status, body.clone());
            fallback_with_error(cache, Some(diag), &target, AppError::Http { status, body })
        }
        Err(e) => {
            cache.mark_stale();
            cache.write_last_error(0, &e.to_string());
            let diag = (0, e.to_string());
            fallback_with_error(cache, Some(diag), &target, e)
        }
    }
}

/// Identity of the account the cached figure belongs to. A balance fetched for
/// one organization must never be displayed after the user switches to another
/// (or back to the personal account).
fn target_key(organization_id: Option<&str>) -> String {
    match organization_id.filter(|o| !o.is_empty()) {
        Some(org) => format!("org:{org}"),
        None => "personal".to_string(),
    }
}

fn fallback_silent(cache: &Cache, target: &str, original: AppError) -> Result<FetchOutcome> {
    let Some(bytes) = cache.fallback_payload(MAX_STALE)? else {
        return Err(original);
    };
    reuse_cache(&bytes, cache, true, target)
}

/// On failure we show the last good figure with the error alongside it. With
/// nothing usable cached there is nothing to show, so the **original** error is
/// returned rather than a generic "no usable cache" that hides what went wrong.
fn fallback_with_error(
    cache: &Cache,
    last_error: Option<(u16, String)>,
    target: &str,
    original: AppError,
) -> Result<FetchOutcome> {
    let Some(bytes) = cache.fallback_payload(MAX_STALE)? else {
        return Err(original);
    };
    // A cache we cannot attribute to this target is no better than no cache.
    let Ok(mut outcome) = reuse_cache(&bytes, cache, true, target) else {
        return Err(original);
    };
    outcome.last_error = last_error;
    Ok(outcome)
}

fn reuse_cache(bytes: &[u8], cache: &Cache, stale: bool, target: &str) -> Result<FetchOutcome> {
    let snap = parse_cache(bytes, target)?;
    Ok(FetchOutcome {
        snapshot: snap,
        stale,
        last_error: cache.read_last_error(),
        cache_age: cache.payload_age(),
    })
}

fn parse_cache(bytes: &[u8], target: &str) -> Result<KiloSnapshot> {
    let v: serde_json::Value = serde_json::from_slice(bytes)?;
    // Payloads written before the target was recorded are not attributable to
    // an account, so they are discarded rather than shown against this one.
    let cached_target = v.get("target").and_then(serde_json::Value::as_str);
    if cached_target != Some(target) {
        return Err(AppError::Schema(format!(
            "kilo cache belongs to a different account ({}); refetching",
            cached_target.unwrap_or("unknown")
        )));
    }
    let s = v
        .get("snapshot")
        .ok_or_else(|| AppError::Schema("kilo cache missing 'snapshot' field".into()))?;
    let balance = s["balance"]
        .as_f64()
        .ok_or_else(|| AppError::Schema("kilo cache missing 'balance'".into()))?;
    Ok(KiloSnapshot {
        label: s["label"].as_str().unwrap_or("Kilo").to_string(),
        balance: finite_amount("kilo", "balance", balance)?,
    })
}

async fn fetch_live(
    client: &reqwest::Client,
    endpoints: &Endpoints,
    api_key: &str,
    organization_id: Option<&str>,
) -> Result<BalanceData> {
    let mut req = client
        .get(&endpoints.balance)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json");
    if let Some(org) = organization_id
        && !org.is_empty()
    {
        req = req.header("x-kilocode-organizationid", org);
    }

    let resp = tokio::time::timeout(HTTP_TIMEOUT, req.send())
        .await
        .map_err(|_| AppError::Transport(format!("kilo timeout: {}", endpoints.balance)))??;

    let status = resp.status();
    let bytes = read_body_capped(resp, MAX_BODY_BYTES).await?;

    if !status.is_success() {
        let body = String::from_utf8_lossy(&bytes).chars().take(200).collect();
        return Err(AppError::Http {
            status: status.as_u16(),
            body,
        });
    }
    let data: BalanceData = serde_json::from_slice(&bytes)
        .map_err(|e| AppError::Schema(format!("kilo {}: {e}", endpoints.balance)))?;
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn cache_fixture() -> (TempDir, Cache) {
        let td = TempDir::new().unwrap();
        let cache = Cache::at(td.path().join("kilo"));
        cache.ensure_dir().unwrap();
        (td, cache)
    }

    #[tokio::test]
    async fn live_fetch_reads_balance() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/api/profile/balance")
            .match_header("authorization", "Bearer sk-kilo-test")
            .with_status(200)
            .with_body(r#"{"balance":8.42}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            balance: format!("{}/api/profile/balance", server.url()),
        };
        let out = fetch_snapshot(
            &client,
            "sk-kilo-test",
            &cache,
            &endpoints,
            Duration::from_secs(0),
            None,
        )
        .await
        .unwrap();
        assert_eq!(out.snapshot.balance, 8.42);
        assert_eq!(out.snapshot.label, "Kilo");
        assert!(!out.stale);
    }

    #[tokio::test]
    async fn org_header_is_sent_when_configured() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/api/profile/balance")
            .match_header("x-kilocode-organizationid", "org_1")
            .with_status(200)
            .with_body(r#"{"balance":100.0}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            balance: format!("{}/api/profile/balance", server.url()),
        };
        let out = fetch_snapshot(
            &client,
            "k",
            &cache,
            &endpoints,
            Duration::from_secs(0),
            Some("org_1"),
        )
        .await
        .unwrap();
        assert_eq!(out.snapshot.balance, 100.0);
    }

    #[tokio::test]
    async fn http_error_falls_back_to_cache_when_present() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/api/profile/balance")
            .with_status(401)
            .with_body(r#"{"error":"unauthorized"}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let seed = serde_json::json!({
            "target": "personal",
            "snapshot": { "label": "Kilo", "balance": 20.0 },
        });
        cache.write_payload(seed.to_string().as_bytes()).unwrap();

        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            balance: format!("{}/api/profile/balance", server.url()),
        };
        let out = fetch_snapshot(
            &client,
            "k",
            &cache,
            &endpoints,
            Duration::from_secs(0),
            None,
        )
        .await
        .unwrap();
        assert!(out.stale);
        assert_eq!(out.snapshot.balance, 20.0);
        assert_eq!(out.last_error.as_ref().map(|(c, _)| *c), Some(401));
    }

    #[tokio::test]
    async fn switching_organization_refetches_instead_of_reusing_the_cache() {
        // A fresh cache for the personal account must not be shown as the
        // organization's balance just because the TTL has not expired.
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/api/profile/balance")
            .match_header("x-kilocode-organizationid", "org_9")
            .with_status(200)
            .with_body(r#"{"balance":7.5}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let seed = serde_json::json!({
            "target": "personal",
            "snapshot": { "label": "Kilo", "balance": 999.0 },
        });
        cache.write_payload(seed.to_string().as_bytes()).unwrap();

        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            balance: format!("{}/api/profile/balance", server.url()),
        };
        // A long TTL: the payload IS fresh, it just belongs to another target.
        let out = fetch_snapshot(
            &client,
            "k",
            &cache,
            &endpoints,
            Duration::from_secs(3600),
            Some("org_9"),
        )
        .await
        .unwrap();
        assert_eq!(out.snapshot.balance, 7.5);
        assert!(!out.stale);
    }

    #[tokio::test]
    async fn malformed_200_body_does_not_become_a_zero_balance() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/api/profile/balance")
            .with_status(200)
            .with_body(r#"{"error":"quota exceeded"}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            balance: format!("{}/api/profile/balance", server.url()),
        };
        // No cache to fall back on: the schema error must surface.
        let err = fetch_snapshot(
            &client,
            "k",
            &cache,
            &endpoints,
            Duration::from_secs(0),
            None,
        )
        .await;
        assert!(err.is_err(), "expected a schema error, got {err:?}");
    }
}
