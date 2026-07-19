//! Moonshot / Kimi fetch — reads the account balance from
//! `/v1/users/me/balance` under the shared cache + flock primitives. The host
//! (and therefore the currency) is region-dependent: `api.moonshot.ai` → USD,
//! `api.moonshot.cn` → CNY. The caller passes the matching currency label.

use std::time::Duration;

use crate::cache::{Cache, acquire_lock};
use crate::error::{AppError, Result};
use crate::usage::MoonshotSnapshot;

use super::types::{BalanceEnvelope, to_snapshot};

pub const BASE_GLOBAL: &str = "https://api.moonshot.ai";
pub const BASE_CN: &str = "https://api.moonshot.cn";
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const LOCK_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub balance: String,
}

impl Endpoints {
    /// Pick the host by region. Returns the endpoints plus the currency label
    /// implied by that host (`"CNY"` for `cn`, `"USD"` otherwise).
    pub fn for_region(region: &str) -> (Self, &'static str) {
        if region.eq_ignore_ascii_case("cn") {
            (
                Self {
                    balance: format!("{BASE_CN}/v1/users/me/balance"),
                },
                "CNY",
            )
        } else {
            (
                Self {
                    balance: format!("{BASE_GLOBAL}/v1/users/me/balance"),
                },
                "USD",
            )
        }
    }
}

impl Default for Endpoints {
    fn default() -> Self {
        Self::for_region("global").0
    }
}

#[derive(Debug, Clone)]
pub struct FetchOutcome {
    pub snapshot: MoonshotSnapshot,
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
    currency: &str,
) -> Result<FetchOutcome> {
    cache.ensure_dir()?;
    let _lock = acquire_lock(&cache.lock_path(), LOCK_TIMEOUT)?;

    if let Some(bytes) = cache.fresh_payload(cache_ttl)? {
        return Ok(reuse_cache(bytes, cache, false));
    }

    match fetch_live(client, endpoints, api_key, currency).await {
        Ok(snap) => {
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
        Err(e) if e.is_transient() => fallback_silent(cache),
        Err(AppError::Http { status, body }) => {
            cache.mark_stale();
            cache.write_last_error(status, &body);
            fallback_with_error(cache, Some((status, body)))
        }
        Err(e) => {
            cache.mark_stale();
            cache.write_last_error(0, &e.to_string());
            fallback_with_error(cache, Some((0, e.to_string())))
        }
    }
}

fn fallback_silent(cache: &Cache) -> Result<FetchOutcome> {
    let Some(bytes) = cache.maybe_payload()? else {
        return Err(AppError::Transport(
            "moonshot: no cache and network unreachable".into(),
        ));
    };
    Ok(reuse_cache(bytes, cache, true))
}

fn fallback_with_error(cache: &Cache, last_error: Option<(u16, String)>) -> Result<FetchOutcome> {
    let Some(bytes) = cache.maybe_payload()? else {
        return Err(AppError::Other("moonshot: no usable cache".into()));
    };
    let mut outcome = reuse_cache(bytes, cache, true);
    outcome.last_error = last_error;
    Ok(outcome)
}

fn reuse_cache(bytes: Vec<u8>, cache: &Cache, stale: bool) -> FetchOutcome {
    let snap = parse_cache(&bytes).unwrap_or(MoonshotSnapshot {
        available: 0.0,
        voucher: 0.0,
        cash: 0.0,
        currency: "USD".into(),
    });
    FetchOutcome {
        snapshot: snap,
        stale,
        last_error: cache.read_last_error(),
        cache_age: cache.payload_age(),
    }
}

fn serde_repr(snap: &MoonshotSnapshot) -> serde_json::Value {
    serde_json::json!({
        "available": snap.available,
        "voucher": snap.voucher,
        "cash": snap.cash,
        "currency": snap.currency,
    })
}

fn parse_cache(bytes: &[u8]) -> Result<MoonshotSnapshot> {
    let v: serde_json::Value = serde_json::from_slice(bytes)?;
    let s = v
        .get("snapshot")
        .ok_or_else(|| AppError::Schema("moonshot cache missing 'snapshot' field".into()))?;
    Ok(MoonshotSnapshot {
        available: s["available"].as_f64().unwrap_or(0.0),
        voucher: s["voucher"].as_f64().unwrap_or(0.0),
        cash: s["cash"].as_f64().unwrap_or(0.0),
        currency: s["currency"].as_str().unwrap_or("USD").to_string(),
    })
}

async fn fetch_live(
    client: &reqwest::Client,
    endpoints: &Endpoints,
    api_key: &str,
    currency: &str,
) -> Result<MoonshotSnapshot> {
    let resp = tokio::time::timeout(
        HTTP_TIMEOUT,
        client
            .get(&endpoints.balance)
            .header("Authorization", format!("Bearer {api_key}"))
            .send(),
    )
    .await
    .map_err(|_| AppError::Transport(format!("moonshot timeout: {}", endpoints.balance)))??;

    let status = resp.status();
    let bytes = resp.bytes().await?;

    if !status.is_success() {
        let body = String::from_utf8_lossy(&bytes).chars().take(200).collect();
        return Err(AppError::Http {
            status: status.as_u16(),
            body,
        });
    }
    let env: BalanceEnvelope = serde_json::from_slice(&bytes)
        .map_err(|e| AppError::Schema(format!("moonshot {}: {e}", endpoints.balance)))?;
    Ok(to_snapshot(env.data, currency))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn cache_fixture() -> (TempDir, Cache) {
        let td = TempDir::new().unwrap();
        let cache = Cache::at(td.path().join("moonshot"));
        cache.ensure_dir().unwrap();
        (td, cache)
    }

    #[test]
    fn region_picks_host_and_currency() {
        let (global, cur) = Endpoints::for_region("global");
        assert!(global.balance.starts_with("https://api.moonshot.ai"));
        assert_eq!(cur, "USD");
        let (cn, cur_cn) = Endpoints::for_region("cn");
        assert!(cn.balance.starts_with("https://api.moonshot.cn"));
        assert_eq!(cur_cn, "CNY");
    }

    #[tokio::test]
    async fn live_fetch_reads_available_balance() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/v1/users/me/balance")
            .match_header("authorization", "Bearer ms-test")
            .with_status(200)
            .with_body(
                r#"{"code":0,"data":{"available_balance":49.58894,
                    "voucher_balance":46.58893,"cash_balance":3.00001},
                    "scode":"0x0","status":true}"#,
            )
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            balance: format!("{}/v1/users/me/balance", server.url()),
        };
        let out = fetch_snapshot(&client, "ms-test", &cache, &endpoints, Duration::from_secs(0), "USD")
            .await
            .unwrap();
        assert!((out.snapshot.available - 49.58894).abs() < 1e-6);
        assert_eq!(out.snapshot.currency, "USD");
        assert!(!out.stale);
    }

    #[tokio::test]
    async fn http_error_falls_back_to_cache_when_present() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/v1/users/me/balance")
            .with_status(401)
            .with_body(r#"{"error":"auth"}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let seed = serde_json::json!({ "snapshot": {
            "available": 49.0, "voucher": 46.0, "cash": 3.0, "currency": "USD"
        }});
        cache.write_payload(seed.to_string().as_bytes()).unwrap();

        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            balance: format!("{}/v1/users/me/balance", server.url()),
        };
        let out = fetch_snapshot(&client, "k", &cache, &endpoints, Duration::from_secs(0), "USD")
            .await
            .unwrap();
        assert!(out.stale);
        assert_eq!(out.snapshot.available, 49.0);
        assert_eq!(out.last_error.as_ref().map(|(c, _)| *c), Some(401));
    }
}
