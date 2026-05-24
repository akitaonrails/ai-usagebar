//! Z.AI fetch. Note the auth-header quirk — the API key is passed as
//! `Authorization: <KEY>` WITHOUT the `Bearer` prefix. Sending `Bearer …`
//! returns 401.

use std::time::Duration;

use crate::cache::{Cache, acquire_lock};
use crate::error::{AppError, Result};
use crate::usage::ZaiSnapshot;

use super::types::Envelope;

pub const QUOTA_URL: &str = "https://api.z.ai/api/monitor/usage/quota/limit";
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const LOCK_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub quota: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            quota: QUOTA_URL.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FetchOutcome {
    pub snapshot: ZaiSnapshot,
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
    config_plan_tier: Option<&str>,
) -> Result<FetchOutcome> {
    cache.ensure_dir()?;
    let _lock = acquire_lock(&cache.lock_path(), LOCK_TIMEOUT)?;

    if let Some(bytes) = cache.fresh_payload(cache_ttl)? {
        return Ok(reuse(bytes, cache, false, config_plan_tier));
    }

    match fetch_live(client, &endpoints.quota, api_key).await {
        Ok(bytes) => {
            cache.write_payload(&bytes)?;
            let env: Envelope = serde_json::from_slice(&bytes)?;
            Ok(FetchOutcome {
                snapshot: env.into_snapshot(config_plan_tier),
                stale: false,
                last_error: None,
                cache_age: Some(Duration::ZERO),
            })
        }
        Err(e) if e.is_transient() => fallback_silent(cache, config_plan_tier),
        Err(AppError::Http { status, body }) => {
            cache.mark_stale();
            cache.write_last_error(status, &body);
            fallback_with_error(cache, Some((status, body)), config_plan_tier)
        }
        Err(e) => {
            cache.mark_stale();
            cache.write_last_error(0, &e.to_string());
            fallback_with_error(cache, Some((0, e.to_string())), config_plan_tier)
        }
    }
}

fn reuse(bytes: Vec<u8>, cache: &Cache, stale: bool, tier: Option<&str>) -> FetchOutcome {
    let snapshot = serde_json::from_slice::<Envelope>(&bytes)
        .map(|e| e.into_snapshot(tier))
        .unwrap_or_else(|_| ZaiSnapshot {
            plan: "GLM Coding Unknown".into(),
            session: None,
            weekly: None,
            mcp: None,
        });
    FetchOutcome {
        snapshot,
        stale,
        last_error: cache.read_last_error(),
        cache_age: cache.payload_age(),
    }
}

fn fallback_silent(cache: &Cache, tier: Option<&str>) -> Result<FetchOutcome> {
    let Some(bytes) = cache.maybe_payload()? else {
        return Err(AppError::Transport(
            "zai: no cache and network unreachable".into(),
        ));
    };
    Ok(reuse(bytes, cache, true, tier))
}

fn fallback_with_error(
    cache: &Cache,
    last_error: Option<(u16, String)>,
    tier: Option<&str>,
) -> Result<FetchOutcome> {
    let Some(bytes) = cache.maybe_payload()? else {
        return Err(AppError::Other("zai: no usable cache".into()));
    };
    let mut out = reuse(bytes, cache, true, tier);
    out.last_error = last_error;
    Ok(out)
}

async fn fetch_live(client: &reqwest::Client, url: &str, api_key: &str) -> Result<Vec<u8>> {
    let resp = tokio::time::timeout(
        HTTP_TIMEOUT,
        client
            .get(url)
            .header("Authorization", api_key) // NO `Bearer ` prefix.
            .header("Accept-Language", "en-US,en")
            .header("Content-Type", "application/json")
            .send(),
    )
    .await
    .map_err(|_| AppError::Transport(format!("zai timeout: {url}")))?
    ?;

    let status = resp.status();
    let bytes = resp.bytes().await?.to_vec();

    if !status.is_success() {
        let body = String::from_utf8_lossy(&bytes).chars().take(200).collect();
        return Err(AppError::Http {
            status: status.as_u16(),
            body,
        });
    }

    // Sanity check we got a valid envelope. Schema drift surfaces here.
    let _: Envelope = serde_json::from_slice(&bytes).map_err(|e| {
        AppError::Schema(format!("zai quota response: {e}"))
    })?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn cache_fixture() -> (TempDir, Cache) {
        let td = TempDir::new().unwrap();
        let cache = Cache::at(td.path().join("zai"));
        cache.ensure_dir().unwrap();
        (td, cache)
    }

    #[tokio::test]
    async fn live_200_parses_real_shape() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/api/monitor/usage/quota/limit")
            .with_status(200)
            .with_body(
                r#"{"code":200,"msg":"Operation successful","data":{
                    "limits":[
                        {"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":42},
                        {"type":"TOKENS_LIMIT","unit":6,"number":1,"percentage":15,"nextResetTime":1779792169974}
                    ],"level":"pro"
                },"success":true}"#,
            )
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            quota: format!("{}/api/monitor/usage/quota/limit", server.url()),
        };
        let out = fetch_snapshot(&client, "fake-key", &cache, &endpoints, Duration::from_secs(0), None)
            .await
            .unwrap();
        assert_eq!(out.snapshot.plan, "GLM Coding Pro");
        assert_eq!(out.snapshot.session.as_ref().unwrap().utilization_pct, 42);
        assert_eq!(out.snapshot.weekly.as_ref().unwrap().utilization_pct, 15);
    }

    #[tokio::test]
    async fn http_401_falls_back_to_cache_when_present() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/api/monitor/usage/quota/limit")
            .with_status(401)
            .with_body(r#"{"code":401,"msg":"Unauthorized"}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let seed = r#"{"code":200,"data":{"limits":[
            {"type":"TOKENS_LIMIT","percentage":10}
        ],"level":"lite"},"success":true}"#;
        cache.write_payload(seed.as_bytes()).unwrap();

        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            quota: format!("{}/api/monitor/usage/quota/limit", server.url()),
        };
        let out = fetch_snapshot(&client, "k", &cache, &endpoints, Duration::from_secs(0), None)
            .await
            .unwrap();
        assert!(out.stale);
        assert_eq!(out.snapshot.session.as_ref().unwrap().utilization_pct, 10);
        assert_eq!(out.last_error.as_ref().map(|(c, _)| *c), Some(401));
    }
}
