//! xAI (Grok) fetch — reads the prepaid balance from the Management API. Two
//! steps when no team id is configured: resolve the team from the management
//! key (`/auth/management-keys/validation`), then read
//! `/v1/billing/teams/{team}/prepaid/balance`.

use std::time::Duration;

use crate::cache::{Cache, acquire_lock};
use crate::error::{AppError, Result};
use crate::usage::GrokSnapshot;

use super::types::{BalanceResp, Validation, to_snapshot};

pub const BASE_URL: &str = "https://management-api.x.ai";
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const LOCK_TIMEOUT: Duration = Duration::from_secs(15);

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
    fn validation_url(&self) -> String {
        format!("{}/auth/management-keys/validation", self.base)
    }
    fn balance_url(&self, team: &str) -> String {
        format!("{}/v1/billing/teams/{team}/prepaid/balance", self.base)
    }
}

#[derive(Debug, Clone)]
pub struct FetchOutcome {
    pub snapshot: GrokSnapshot,
    pub stale: bool,
    pub last_error: Option<(u16, String)>,
    pub cache_age: Option<Duration>,
}

/// `management_key` is the xAI Management API key (distinct from the inference
/// key). `team_id`, when set, skips the validation round-trip.
pub async fn fetch_snapshot(
    client: &reqwest::Client,
    management_key: &str,
    cache: &Cache,
    endpoints: &Endpoints,
    cache_ttl: Duration,
    team_id: Option<&str>,
) -> Result<FetchOutcome> {
    cache.ensure_dir()?;
    let _lock = acquire_lock(&cache.lock_path(), LOCK_TIMEOUT)?;

    if let Some(bytes) = cache.fresh_payload(cache_ttl)? {
        return Ok(reuse_cache(bytes, cache, false));
    }

    match fetch_live(client, endpoints, management_key, team_id).await {
        Ok(snap) => {
            let bytes =
                serde_json::to_vec(&serde_json::json!({ "snapshot": { "balance": snap.balance } }))
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
            "grok: no cache and network unreachable".into(),
        ));
    };
    Ok(reuse_cache(bytes, cache, true))
}

fn fallback_with_error(cache: &Cache, last_error: Option<(u16, String)>) -> Result<FetchOutcome> {
    let Some(bytes) = cache.maybe_payload()? else {
        return Err(AppError::Other("grok: no usable cache".into()));
    };
    let mut outcome = reuse_cache(bytes, cache, true);
    outcome.last_error = last_error;
    Ok(outcome)
}

fn reuse_cache(bytes: Vec<u8>, cache: &Cache, stale: bool) -> FetchOutcome {
    let snap = parse_cache(&bytes).unwrap_or(GrokSnapshot { balance: 0.0 });
    FetchOutcome {
        snapshot: snap,
        stale,
        last_error: cache.read_last_error(),
        cache_age: cache.payload_age(),
    }
}

fn parse_cache(bytes: &[u8]) -> Result<GrokSnapshot> {
    let v: serde_json::Value = serde_json::from_slice(bytes)?;
    let s = v
        .get("snapshot")
        .ok_or_else(|| AppError::Schema("grok cache missing 'snapshot' field".into()))?;
    Ok(GrokSnapshot {
        balance: s["balance"].as_f64().unwrap_or(0.0),
    })
}

async fn fetch_live(
    client: &reqwest::Client,
    endpoints: &Endpoints,
    key: &str,
    team_id: Option<&str>,
) -> Result<GrokSnapshot> {
    let team = match team_id {
        Some(t) if !t.is_empty() => t.to_string(),
        _ => {
            let v: Validation = get_json(client, &endpoints.validation_url(), key).await?;
            v.resolved_team().ok_or_else(|| {
                AppError::Other(
                    "grok: could not resolve team_id from the management key; \
                     set `team_id` under [grok] in config"
                        .into(),
                )
            })?
        }
    };
    let resp: BalanceResp = get_json(client, &endpoints.balance_url(&team), key).await?;
    Ok(to_snapshot(resp))
}

async fn get_json<T: for<'de> serde::Deserialize<'de>>(
    client: &reqwest::Client,
    url: &str,
    key: &str,
) -> Result<T> {
    let resp = tokio::time::timeout(
        HTTP_TIMEOUT,
        client
            .get(url)
            .header("Authorization", format!("Bearer {key}"))
            .send(),
    )
    .await
    .map_err(|_| AppError::Transport(format!("grok timeout: {url}")))??;

    let status = resp.status();
    let bytes = resp.bytes().await?;
    if !status.is_success() {
        let body = String::from_utf8_lossy(&bytes).chars().take(200).collect();
        return Err(AppError::Http {
            status: status.as_u16(),
            body,
        });
    }
    serde_json::from_slice(&bytes).map_err(|e| AppError::Schema(format!("grok {url}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn cache_fixture() -> (TempDir, Cache) {
        let td = TempDir::new().unwrap();
        let cache = Cache::at(td.path().join("grok"));
        cache.ensure_dir().unwrap();
        (td, cache)
    }

    #[tokio::test]
    async fn resolves_team_then_reads_balance() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/auth/management-keys/validation")
            .with_status(200)
            .with_body(r#"{"scopeId":"team-xyz","teamId":"team-xyz"}"#)
            .create_async()
            .await;
        server
            .mock("GET", "/v1/billing/teams/team-xyz/prepaid/balance")
            .match_header("authorization", "Bearer xai-mgmt")
            .with_status(200)
            .with_body(r#"{"changes":[],"total":{"val":"-2500"}}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let endpoints = Endpoints { base: server.url() };
        let out = fetch_snapshot(
            &client,
            "xai-mgmt",
            &cache,
            &endpoints,
            Duration::from_secs(0),
            None,
        )
        .await
        .unwrap();
        assert!((out.snapshot.balance - 25.0).abs() < 1e-9);
        assert!(!out.stale);
    }

    #[tokio::test]
    async fn configured_team_skips_validation() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/v1/billing/teams/my-team/prepaid/balance")
            .with_status(200)
            .with_body(r#"{"total":{"val":"-500"}}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let endpoints = Endpoints { base: server.url() };
        let out = fetch_snapshot(
            &client,
            "xai-mgmt",
            &cache,
            &endpoints,
            Duration::from_secs(0),
            Some("my-team"),
        )
        .await
        .unwrap();
        assert!((out.snapshot.balance - 5.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn http_404_falls_back_to_cache_when_present() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/v1/billing/teams/t/prepaid/balance")
            .with_status(404)
            .with_body(r#"{"error":"no team"}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        cache
            .write_payload(serde_json::json!({ "snapshot": { "balance": 12.0 } }).to_string().as_bytes())
            .unwrap();

        let client = reqwest::Client::new();
        let endpoints = Endpoints { base: server.url() };
        let out = fetch_snapshot(
            &client,
            "k",
            &cache,
            &endpoints,
            Duration::from_secs(0),
            Some("t"),
        )
        .await
        .unwrap();
        assert!(out.stale);
        assert_eq!(out.snapshot.balance, 12.0);
        assert_eq!(out.last_error.as_ref().map(|(c, _)| *c), Some(404));
    }
}
