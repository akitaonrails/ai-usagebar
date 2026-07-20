//! Novita AI fetch — reads the account balance from
//! `/openapi/v1/billing/balance/detail` under the shared cache + flock
//! primitives. Single-endpoint balance, same shape as `kilo::fetch`.

use std::time::Duration;

use crate::cache::{Cache, acquire_lock};
use crate::error::{AppError, Result};
use crate::usage::{NovitaSnapshot, finite_amount};

use super::types::{BalanceData, to_snapshot};

pub const BASE_URL: &str = "https://api.novita.ai";
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const LOCK_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub balance: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            balance: format!("{BASE_URL}/openapi/v1/billing/balance/detail"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FetchOutcome {
    pub snapshot: NovitaSnapshot,
    pub stale: bool,
    pub last_error: Option<(u16, String)>,
    pub cache_age: Option<Duration>,
}

pub async fn fetch_snapshot(
    client: &reqwest::Client,
    api_key: &str,
    cache: &Cache,
    endpoints: &Endpoints,
    cache_ttl: Duration,
) -> Result<FetchOutcome> {
    cache.ensure_dir()?;
    let _lock = acquire_lock(&cache.lock_path(), LOCK_TIMEOUT)?;

    if let Some(bytes) = cache.fresh_payload(cache_ttl)?
        && let Ok(outcome) = reuse_cache(&bytes, cache, false)
    {
        return Ok(outcome);
    }

    match fetch_live(client, endpoints, api_key).await {
        Ok(balance) => {
            let snap = to_snapshot(balance)?;
            let bytes = serde_json::to_vec(&serde_json::json!({ "snapshot": serde_repr(&snap) }))
                .unwrap_or_default();
            cache.write_payload(&bytes)?;
            Ok(FetchOutcome {
                snapshot: snap,
                stale: false,
                last_error: None,
                cache_age: Some(Duration::ZERO),
            })
        }
        Err(e) if e.is_transient() => fallback_silent(cache, e),
        Err(AppError::Http { status, body }) => {
            cache.mark_stale();
            cache.write_last_error(status, &body);
            let diag = (status, body.clone());
            fallback_with_error(cache, Some(diag), AppError::Http { status, body })
        }
        Err(e) => {
            cache.mark_stale();
            cache.write_last_error(0, &e.to_string());
            let diag = (0, e.to_string());
            fallback_with_error(cache, Some(diag), e)
        }
    }
}

fn fallback_silent(cache: &Cache, original: AppError) -> Result<FetchOutcome> {
    let Some(bytes) = cache.maybe_payload()? else {
        return Err(original);
    };
    reuse_cache(&bytes, cache, true)
}

/// On failure we show the last good figure with the error alongside it. With
/// nothing usable cached there is nothing to show, so the **original** error is
/// returned rather than a generic "no usable cache" that hides what went wrong.
fn fallback_with_error(
    cache: &Cache,
    last_error: Option<(u16, String)>,
    original: AppError,
) -> Result<FetchOutcome> {
    let Some(bytes) = cache.maybe_payload()? else {
        return Err(original);
    };
    let Ok(mut outcome) = reuse_cache(&bytes, cache, true) else {
        return Err(original);
    };
    outcome.last_error = last_error;
    Ok(outcome)
}

fn reuse_cache(bytes: &[u8], cache: &Cache, stale: bool) -> Result<FetchOutcome> {
    let snap = parse_cache(bytes)?;
    Ok(FetchOutcome {
        snapshot: snap,
        stale,
        last_error: cache.read_last_error(),
        cache_age: cache.payload_age(),
    })
}

fn serde_repr(snap: &NovitaSnapshot) -> serde_json::Value {
    serde_json::json!({
        "available": snap.available,
        "cash": snap.cash,
        "credit_limit": snap.credit_limit,
        "outstanding": snap.outstanding,
    })
}

fn parse_cache(bytes: &[u8]) -> Result<NovitaSnapshot> {
    let v: serde_json::Value = serde_json::from_slice(bytes)?;
    let s = v
        .get("snapshot")
        .ok_or_else(|| AppError::Schema("novita cache missing 'snapshot' field".into()))?;
    // Cached money is required, not optional: a truncated or half-written
    // payload must be refetched rather than rendered as $0.00.
    let field = |name: &str| -> Result<f64> {
        let v = s[name]
            .as_f64()
            .ok_or_else(|| AppError::Schema(format!("novita cache missing '{name}'")))?;
        finite_amount("novita cache", name, v)
    };
    Ok(NovitaSnapshot {
        available: field("available")?,
        cash: field("cash")?,
        credit_limit: field("credit_limit")?,
        outstanding: field("outstanding")?,
    })
}

async fn fetch_live(
    client: &reqwest::Client,
    endpoints: &Endpoints,
    api_key: &str,
) -> Result<BalanceData> {
    let resp = tokio::time::timeout(
        HTTP_TIMEOUT,
        client
            .get(&endpoints.balance)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Content-Type", "application/json")
            .send(),
    )
    .await
    .map_err(|_| AppError::Transport(format!("novita timeout: {}", endpoints.balance)))??;

    let status = resp.status();
    let bytes = resp.bytes().await?;

    if !status.is_success() {
        let body = String::from_utf8_lossy(&bytes).chars().take(200).collect();
        return Err(AppError::Http {
            status: status.as_u16(),
            body,
        });
    }
    let data: BalanceData = serde_json::from_slice(&bytes)
        .map_err(|e| AppError::Schema(format!("novita {}: {e}", endpoints.balance)))?;
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn cache_fixture() -> (TempDir, Cache) {
        let td = TempDir::new().unwrap();
        let cache = Cache::at(td.path().join("novita"));
        cache.ensure_dir().unwrap();
        (td, cache)
    }

    #[tokio::test]
    async fn live_fetch_parses_available_balance() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/openapi/v1/billing/balance/detail")
            .match_header("authorization", "Bearer nv-test")
            .with_status(200)
            .with_body(
                r#"{"availableBalance":"1000000","cashBalance":"800000",
                    "creditLimit":"200000","pendingCharges":"0","outstandingInvoices":"0"}"#,
            )
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            balance: format!("{}/openapi/v1/billing/balance/detail", server.url()),
        };
        let out = fetch_snapshot(
            &client,
            "nv-test",
            &cache,
            &endpoints,
            Duration::from_secs(0),
        )
        .await
        .unwrap();
        assert_eq!(out.snapshot.available, 100.0);
        assert_eq!(out.snapshot.cash, 80.0);
        assert!(!out.stale);
    }

    #[tokio::test]
    async fn http_error_falls_back_to_cache_when_present() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/openapi/v1/billing/balance/detail")
            .with_status(401)
            .with_body(r#"{"error":"unauthorized"}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let seed = serde_json::json!({ "snapshot": {
            "available": 42.0, "cash": 40.0, "credit_limit": 20.0, "outstanding": 0.0
        }});
        cache.write_payload(seed.to_string().as_bytes()).unwrap();

        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            balance: format!("{}/openapi/v1/billing/balance/detail", server.url()),
        };
        let out = fetch_snapshot(&client, "k", &cache, &endpoints, Duration::from_secs(0))
            .await
            .unwrap();
        assert!(out.stale);
        assert_eq!(out.snapshot.available, 42.0);
        assert_eq!(out.last_error.as_ref().map(|(c, _)| *c), Some(401));
    }

    #[tokio::test]
    async fn malformed_200_body_does_not_become_a_zero_balance() {
        // Novita answering 200 with an error envelope must surface as a schema
        // error, never as a fresh "$0.00 available" snapshot.
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/openapi/v1/billing/balance/detail")
            .with_status(200)
            .with_body(r#"{"code":401,"message":"invalid api key"}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            balance: format!("{}/openapi/v1/billing/balance/detail", server.url()),
        };
        let out = fetch_snapshot(&client, "k", &cache, &endpoints, Duration::from_secs(0)).await;
        assert!(out.is_err(), "expected a schema error, got {out:?}");
    }

    #[tokio::test]
    async fn corrupt_cache_is_not_served_as_zero() {
        // A truncated payload must be refetched, not rendered as $0.00.
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/openapi/v1/billing/balance/detail")
            .with_status(200)
            .with_body(
                r#"{"availableBalance":"55000","cashBalance":"55000",
                    "creditLimit":"0","outstandingInvoices":"0"}"#,
            )
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        // Fresh, but missing the monetary fields.
        let seed = serde_json::json!({ "snapshot": { "available": 42.0 } });
        cache.write_payload(seed.to_string().as_bytes()).unwrap();

        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            balance: format!("{}/openapi/v1/billing/balance/detail", server.url()),
        };
        let out = fetch_snapshot(&client, "k", &cache, &endpoints, Duration::from_secs(3600))
            .await
            .unwrap();
        assert_eq!(out.snapshot.available, 5.5);
        assert!(!out.stale);
    }
}
