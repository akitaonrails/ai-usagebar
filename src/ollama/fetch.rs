//! Ollama Cloud fetch. Bearer-token auth against `https://ollama.com/api/usage`.
//! Single request, small body; `Outcome` plumbing mirrors the other single-shot
//! API-key providers (cache the validated wire body, stale-fallback on errors).

use std::time::Duration;

use crate::cache::{Cache, acquire_lock_async};
use crate::error::{AppError, Result};
use crate::usage::OllamaSnapshot;

use super::types::Body;

pub const USAGE_URL: &str = "https://ollama.com/api/usage";
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const LOCK_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub usage: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            usage: USAGE_URL.into(),
        }
    }
}

/// This vendor's [`Outcome`](crate::outcome::Outcome), specialised to its snapshot.
pub type FetchOutcome = crate::outcome::Outcome<OllamaSnapshot>;

pub async fn fetch_snapshot(
    client: &reqwest::Client,
    api_key: &str,
    plan: &str,
    cache: &Cache,
    endpoints: &Endpoints,
    cache_ttl: Duration,
) -> Result<FetchOutcome> {
    cache.ensure_dir()?;
    let _lock = acquire_lock_async(&cache.lock_path(), LOCK_TIMEOUT).await?;

    if let Some(bytes) = cache.fresh_payload(cache_ttl)?
        && let Ok(outcome) = reuse(bytes, cache, false, plan)
    {
        return Ok(outcome);
    }
    // Corrupt fresh cache: fall through to live fetch rather than fabricate.

    match fetch_live(client, &endpoints.usage, api_key).await {
        Ok((bytes, body)) => {
            // Only a validated body reaches the cache.
            cache.write_payload(&bytes)?;
            Ok(crate::outcome::Outcome::fresh(
                body.into_snapshot(plan.into()),
            ))
        }
        Err(e) if e.is_transient() => fallback_silent(cache, plan, e),
        Err(AppError::Http { status, body }) => {
            cache.mark_stale();
            let last_error = Some(cache.write_last_error(status, &body));
            fallback_with_error(cache, last_error, plan, AppError::Http { status, body })
        }
        Err(e) => {
            cache.mark_stale();
            let last_error = Some(cache.write_last_error(0, &e.to_string()));
            fallback_with_error(cache, last_error, plan, e)
        }
    }
}

fn reuse(bytes: Vec<u8>, cache: &Cache, stale: bool, plan: &str) -> Result<FetchOutcome> {
    Ok(crate::outcome::Outcome::cached(
        parse_cache(&bytes, plan)?,
        cache,
        stale,
    ))
}

fn parse_cache(bytes: &[u8], plan: &str) -> Result<OllamaSnapshot> {
    let body: Body = serde_json::from_slice(bytes)
        .map_err(|e| AppError::Schema(format!("ollama cache: {e}")))?;
    Ok(body.into_snapshot(plan.to_string()))
}

fn fallback_silent(cache: &Cache, plan: &str, original: AppError) -> Result<FetchOutcome> {
    crate::outcome::fallback(cache, None, original, |bytes| parse_cache(bytes, plan))
}

fn fallback_with_error(
    cache: &Cache,
    last_error: Option<(u16, String)>,
    plan: &str,
    original: AppError,
) -> Result<FetchOutcome> {
    crate::outcome::fallback(cache, last_error, original, |bytes| {
        parse_cache(bytes, plan)
    })
}

async fn fetch_live(client: &reqwest::Client, url: &str, api_key: &str) -> Result<(Vec<u8>, Body)> {
    let resp = tokio::time::timeout(
        HTTP_TIMEOUT,
        client
            .get(url)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Accept", "application/json")
            .send(),
    )
    .await
    .map_err(|_| AppError::Transport(format!("ollama timeout: {url}")))??;

    let status = resp.status();
    let bytes = crate::vendor::read_body_capped(resp, crate::vendor::MAX_BODY_BYTES).await?;

    if !status.is_success() {
        let body = String::from_utf8_lossy(&bytes).chars().take(200).collect();
        return Err(AppError::Http {
            status: status.as_u16(),
            body,
        });
    }

    let body: Body = serde_json::from_slice(&bytes)
        .map_err(|e| AppError::Schema(format!("ollama usage response: {e}")))?;
    Ok((bytes.to_vec(), body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn cache_fixture() -> (TempDir, Cache) {
        let td = TempDir::new().unwrap();
        let cache = Cache::at(td.path().join("ollama"));
        cache.ensure_dir().unwrap();
        (td, cache)
    }

    const SAMPLE: &str = r#"{
      "limits": {
        "session": {"usage": 0.5, "models": [{"name":"kimi-k3","request_count":9}]},
        "weekly": {"usage": 0.1, "models": []}
      },
      "activity": {"cost": "1.25", "period": {"type": "last_4_weeks"}}
    }"#;

    #[test]
    fn cached_payload_round_trips() {
        let snap = parse_cache(SAMPLE.as_bytes(), "pro").unwrap();
        assert_eq!(snap.plan, "pro");
        assert_eq!(snap.session.as_ref().unwrap().utilization_pct, 50);
        assert_eq!(snap.weekly.as_ref().unwrap().utilization_pct, 10);
        assert_eq!(snap.session_models[0].request_count, 9);
        assert_eq!(snap.activity_cost.as_deref(), Some("1.25"));
    }

    #[test]
    fn corrupt_cache_is_rejected() {
        let err = parse_cache(b"not-json", "pro").unwrap_err();
        assert!(err.to_string().contains("ollama cache"), "{err}");
    }

    #[tokio::test]
    async fn live_200_returns_snapshot() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/api/usage")
            .match_header("Authorization", "Bearer test-key")
            .with_status(200)
            .with_body(SAMPLE)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            usage: format!("{}/api/usage", server.url()),
        };
        let outcome = fetch_snapshot(
            &client,
            "test-key",
            "pro",
            &cache,
            &endpoints,
            Duration::from_secs(60),
        )
        .await
        .unwrap();
        mock.assert_async().await;

        let snap = outcome.snapshot;
        assert_eq!(snap.session.as_ref().unwrap().utilization_pct, 50);
        assert_eq!(snap.activity_cost.as_deref(), Some("1.25"));
        assert!(!outcome.stale);
    }

    #[tokio::test]
    async fn live_401_falls_back_to_cache() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/api/usage")
            .with_status(401)
            .with_body(r#"{"error":"invalid credentials"}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        cache.write_payload(SAMPLE.as_bytes()).unwrap();
        // Age the cache past TTL so the live path is taken.
        std::thread::sleep(Duration::from_millis(20));

        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            usage: format!("{}/api/usage", server.url()),
        };
        let outcome = fetch_snapshot(
            &client,
            "bad-key",
            "pro",
            &cache,
            &endpoints,
            Duration::from_millis(1),
        )
        .await
        .unwrap();
        assert!(outcome.stale, "401 should serve stale cache");
        assert_eq!(
            outcome.snapshot.session.as_ref().unwrap().utilization_pct,
            50
        );
        assert!(outcome.last_error.is_some());
    }
}
