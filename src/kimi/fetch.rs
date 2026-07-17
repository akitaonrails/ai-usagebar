//! Fetch Kimi usage from `/coding/v1/usages`.

use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::cache::{Cache, acquire_lock};
use crate::error::{AppError, Result};
use crate::usage::KimiSnapshot;

use super::types::UsagesResponse;

pub const BASE_URL: &str = "https://api.kimi.com";
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const LOCK_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub usages: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            usages: format!("{BASE_URL}/coding/v1/usages"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FetchOutcome {
    pub snapshot: KimiSnapshot,
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
        && let Ok(outcome) = reuse_cache(bytes, cache, false)
    {
        return Ok(outcome);
    }
    // Corrupt fresh cache: fall through to live fetch rather than return a
    // fabricated zero snapshot.

    match fetch_live(client, &endpoints.usages, api_key).await {
        Ok(snap) => {
            let bytes = serde_json::to_vec(&snap_to_json(&snap)).unwrap_or_default();
            cache.write_payload(&bytes)?;
            Ok(FetchOutcome {
                snapshot: snap,
                stale: false,
                last_error: None,
                cache_age: Some(Duration::ZERO),
            })
        }
        Err(e) if e.is_transient() => fallback_silent(cache, e),
        Err(e) => {
            cache.mark_stale();
            if let Some((code, msg)) = error_to_pair(&e) {
                cache.write_last_error(code, &msg);
            }
            fallback_with_error(cache, e)
        }
    }
}

fn fallback_silent(cache: &Cache, original: AppError) -> Result<FetchOutcome> {
    let Some(bytes) = cache.maybe_payload()? else {
        return Err(original);
    };
    match reuse_cache(bytes, cache, true) {
        Ok(outcome) => Ok(outcome),
        Err(_) => Err(original),
    }
}

fn fallback_with_error(cache: &Cache, original: AppError) -> Result<FetchOutcome> {
    let Some(bytes) = cache.maybe_payload()? else {
        return Err(original);
    };
    match reuse_cache(bytes, cache, true) {
        Ok(mut outcome) => {
            outcome.last_error = error_to_pair(&original);
            Ok(outcome)
        }
        Err(_) => Err(original),
    }
}

fn error_to_pair(e: &AppError) -> Option<(u16, String)> {
    match e {
        AppError::Http { status, body } => Some((*status, body.clone())),
        // A 2xx response with an unknown shape is not an HTTP 422 response.
        AppError::Schema(_) => Some((0, "Kimi API schema drift".into())),
        e => Some((0, e.to_string())),
    }
}

fn reuse_cache(bytes: Vec<u8>, cache: &Cache, stale: bool) -> Result<FetchOutcome> {
    let snap = parse_cache(&bytes)?;
    Ok(FetchOutcome {
        snapshot: snap,
        stale,
        last_error: cache.read_last_error(),
        cache_age: cache.payload_age(),
    })
}

fn parse_cache(bytes: &[u8]) -> Result<KimiSnapshot> {
    let v: serde_json::Value = serde_json::from_slice(bytes)?;
    Ok(KimiSnapshot {
        plan: v["plan"].as_str().map(|s| s.to_string()),
        weekly_limit: parse_cache_u64(&v["weekly_limit"], "weekly_limit")?,
        weekly_used: parse_cache_u64(&v["weekly_used"], "weekly_used")?,
        weekly_remaining: parse_cache_u64(&v["weekly_remaining"], "weekly_remaining")?,
        weekly_reset_at: parse_cache_datetime(&v["weekly_reset_at"])?,
        window_limit: parse_cache_u64(&v["window_limit"], "window_limit")?,
        window_used: parse_cache_u64(&v["window_used"], "window_used")?,
        window_remaining: parse_cache_u64(&v["window_remaining"], "window_remaining")?,
        window_reset_at: parse_cache_datetime(&v["window_reset_at"])?,
    })
}

fn parse_cache_u64(v: &serde_json::Value, name: &str) -> Result<u64> {
    v.as_u64()
        .ok_or_else(|| AppError::Schema(format!("kimi cache: invalid {name}")))
}

fn parse_cache_datetime(v: &serde_json::Value) -> Result<Option<DateTime<Utc>>> {
    match v {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(s) => DateTime::parse_from_rfc3339(s)
            .map(|dt| Some(dt.into()))
            .map_err(|e| AppError::Schema(format!("kimi cache: invalid reset timestamp: {e}"))),
        _ => Err(AppError::Schema(
            "kimi cache: invalid reset timestamp".into(),
        )),
    }
}

fn snap_to_json(snap: &KimiSnapshot) -> serde_json::Value {
    serde_json::json!({
        "plan": snap.plan,
        "weekly_limit": snap.weekly_limit,
        "weekly_used": snap.weekly_used,
        "weekly_remaining": snap.weekly_remaining,
        "weekly_reset_at": snap.weekly_reset_at.map(|dt| dt.to_rfc3339()),
        "window_limit": snap.window_limit,
        "window_used": snap.window_used,
        "window_remaining": snap.window_remaining,
        "window_reset_at": snap.window_reset_at.map(|dt| dt.to_rfc3339()),
    })
}

async fn fetch_live(client: &reqwest::Client, url: &str, api_key: &str) -> Result<KimiSnapshot> {
    let resp = tokio::time::timeout(
        HTTP_TIMEOUT,
        client
            .get(url)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Accept", "application/json")
            .send(),
    )
    .await
    .map_err(|_| AppError::Transport(format!("kimi timeout: {url}")))??;

    let status = resp.status();

    if !status.is_success() {
        // Never surface upstream/proxy bodies: they can contain credentials or
        // arbitrary markup. Keep the cached diagnostic useful but generic.
        let body = if matches!(status.as_u16(), 401 | 403) {
            "Kimi authentication failed".into()
        } else {
            format!("Kimi API returned HTTP {}", status.as_u16())
        };
        return Err(AppError::Http {
            status: status.as_u16(),
            body,
        });
    }

    let bytes = resp.bytes().await?;
    let r: UsagesResponse = serde_json::from_slice(&bytes)
        .map_err(|e| AppError::Schema(format!("kimi usages response: {e}")))?;
    r.into_snapshot()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn cache_fixture() -> (TempDir, Cache) {
        let td = TempDir::new().unwrap();
        let cache = Cache::at(td.path().join("kimi"));
        cache.ensure_dir().unwrap();
        (td, cache)
    }

    fn sample_json() -> &'static str {
        r#"{
            "user": { "membership": { "level": "LEVEL_INTERMEDIATE" } },
            "usage": { "limit": "100", "used": "26", "remaining": "74", "resetTime": "2026-02-11T17:32:50.757941Z" },
            "limits": [
                {
                    "window": { "duration": 300, "timeUnit": "TIME_UNIT_MINUTE" },
                    "detail": { "limit": "100", "used": "15", "remaining": "85", "resetTime": "2026-02-07T12:32:50.757941Z" }
                }
            ]
        }"#
    }

    fn sample_seed() -> serde_json::Value {
        serde_json::json!({
            "plan": "LEVEL_INTERMEDIATE",
            "weekly_limit": 100,
            "weekly_used": 30,
            "weekly_remaining": 70,
            "weekly_reset_at": "2026-02-11T17:32:50.757941Z",
            "window_limit": 100,
            "window_used": 20,
            "window_remaining": 80,
            "window_reset_at": "2026-02-07T12:32:50.757941Z"
        })
    }

    #[tokio::test]
    async fn live_200_returns_snapshot_and_sends_headers() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("GET", "/coding/v1/usages")
            .with_status(200)
            .with_body(sample_json())
            .match_header("authorization", "Bearer sk-test")
            .match_header("accept", "application/json")
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            usages: format!("{}/coding/v1/usages", server.url()),
        };
        let out = fetch_snapshot(
            &client,
            "sk-test",
            &cache,
            &endpoints,
            Duration::from_secs(0),
        )
        .await
        .unwrap();
        m.assert_async().await;
        assert_eq!(out.snapshot.plan, Some("LEVEL_INTERMEDIATE".into()));
        assert_eq!(out.snapshot.weekly_limit, 100);
        assert_eq!(out.snapshot.weekly_used, 26);
        assert_eq!(out.snapshot.weekly_remaining, 74);
        assert_eq!(out.snapshot.window_limit, 100);
        assert_eq!(out.snapshot.window_used, 15);
        assert!(!out.stale);
    }

    #[tokio::test]
    async fn http_401_falls_back_to_cache() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/coding/v1/usages")
            .with_status(401)
            .with_body(r#"{"error": "invalid api key"}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        cache
            .write_payload(sample_seed().to_string().as_bytes())
            .unwrap();

        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            usages: format!("{}/coding/v1/usages", server.url()),
        };
        let out = fetch_snapshot(
            &client,
            "bad-key",
            &cache,
            &endpoints,
            Duration::from_secs(0),
        )
        .await
        .unwrap();
        assert!(out.stale);
        assert_eq!(out.snapshot.weekly_used, 30);
        assert_eq!(out.last_error.as_ref().map(|(c, _)| *c), Some(401));
    }

    #[tokio::test]
    async fn http_500_falls_back_to_cache() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/coding/v1/usages")
            .with_status(500)
            .with_body(r#"{"error": "internal server error"}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        cache
            .write_payload(sample_seed().to_string().as_bytes())
            .unwrap();

        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            usages: format!("{}/coding/v1/usages", server.url()),
        };
        let out = fetch_snapshot(
            &client,
            "sk-test",
            &cache,
            &endpoints,
            Duration::from_secs(0),
        )
        .await
        .unwrap();
        assert!(out.stale);
        assert_eq!(out.last_error.as_ref().map(|(c, _)| *c), Some(500));
    }

    #[tokio::test]
    async fn http_401_without_cache_returns_http_error() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/coding/v1/usages")
            .with_status(401)
            .with_body(r#"{"error": "invalid api key"}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            usages: format!("{}/coding/v1/usages", server.url()),
        };
        let err = fetch_snapshot(
            &client,
            "bad-key",
            &cache,
            &endpoints,
            Duration::from_secs(0),
        )
        .await
        .unwrap_err();
        match err {
            AppError::Http { status, .. } => assert_eq!(status, 401),
            other => panic!("expected Http 401, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn malformed_numeric_200_returns_schema_error() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/coding/v1/usages")
            .with_status(200)
            .with_body(r#"{"usage": {"limit": "100", "used": "garbage"}}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            usages: format!("{}/coding/v1/usages", server.url()),
        };
        let err = fetch_snapshot(
            &client,
            "sk-test",
            &cache,
            &endpoints,
            Duration::from_secs(0),
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string().contains("used") || err.to_string().contains("Schema"),
            "expected schema error, got {err}"
        );
    }

    #[tokio::test]
    async fn malformed_numeric_200_with_seeded_cache_returns_stale_snapshot_and_preserves_cache() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/coding/v1/usages")
            .with_status(200)
            .with_body(r#"{"usage": {"limit": "100", "used": "garbage"}}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let seeded = sample_seed().to_string();
        cache.write_payload(seeded.as_bytes()).unwrap();

        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            usages: format!("{}/coding/v1/usages", server.url()),
        };
        let out = fetch_snapshot(
            &client,
            "sk-test",
            &cache,
            &endpoints,
            Duration::from_secs(0),
        )
        .await
        .unwrap();

        assert!(out.stale);
        assert_eq!(out.snapshot.weekly_used, 30);
        assert_eq!(out.snapshot.window_used, 20);
        assert_eq!(out.last_error, Some((0, "Kimi API schema drift".into())));

        // The payload file must still contain the original seeded snapshot.
        let payload = std::fs::read_to_string(cache.payload_path()).unwrap();
        assert_eq!(payload, seeded);
    }

    #[tokio::test]
    async fn error_object_200_returns_schema_error() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/coding/v1/usages")
            .with_status(200)
            .with_body(r#"{"error": "invalid token"}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            usages: format!("{}/coding/v1/usages", server.url()),
        };
        let err = fetch_snapshot(
            &client,
            "sk-test",
            &cache,
            &endpoints,
            Duration::from_secs(0),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("usage block"), "got {err}");
    }

    #[tokio::test]
    async fn corrupt_fresh_cache_ignored() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/coding/v1/usages")
            .with_status(200)
            .with_body(sample_json())
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        cache.write_payload(b"not valid json".as_slice()).unwrap();

        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            usages: format!("{}/coding/v1/usages", server.url()),
        };
        let out = fetch_snapshot(
            &client,
            "sk-test",
            &cache,
            &endpoints,
            Duration::from_secs(60),
        )
        .await
        .unwrap();
        assert_eq!(out.snapshot.weekly_used, 26);
        assert!(!out.stale);
    }

    #[tokio::test]
    async fn corrupt_stale_cache_returns_error() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/coding/v1/usages")
            .with_status(401)
            .with_body(r#"{"error": "invalid api key"}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        cache.write_payload(b"not valid json".as_slice()).unwrap();

        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            usages: format!("{}/coding/v1/usages", server.url()),
        };
        let err = fetch_snapshot(
            &client,
            "bad-key",
            &cache,
            &endpoints,
            Duration::from_secs(0),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, AppError::Http { status, .. } if status == 401),
            "expected 401, got {err:?}"
        );
    }

    #[tokio::test]
    async fn transport_error_with_stale_cache_uses_cache() {
        // Use a URL that will not resolve to trigger a transport error.
        let (_td, cache) = cache_fixture();
        cache
            .write_payload(sample_seed().to_string().as_bytes())
            .unwrap();

        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            usages: "http://localhost:1/coding/v1/usages".into(),
        };
        let out = fetch_snapshot(
            &client,
            "sk-test",
            &cache,
            &endpoints,
            Duration::from_secs(0),
        )
        .await
        .unwrap();
        assert!(out.stale);
        assert_eq!(out.snapshot.weekly_used, 30);
    }

    #[tokio::test]
    async fn missing_counters_with_seeded_cache_preserves_snapshot() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/coding/v1/usages")
            .with_status(200)
            .with_body(r#"{"usage":{"limit":100}}"#)
            .create_async()
            .await;
        let (_td, cache) = cache_fixture();
        let seeded = sample_seed().to_string();
        cache.write_payload(seeded.as_bytes()).unwrap();
        let out = fetch_snapshot(
            &reqwest::Client::new(),
            "sk-test",
            &cache,
            &Endpoints {
                usages: format!("{}/coding/v1/usages", server.url()),
            },
            Duration::ZERO,
        )
        .await
        .unwrap();
        assert!(out.stale);
        assert_eq!(out.snapshot.weekly_used, 30);
        assert_eq!(
            std::fs::read_to_string(cache.payload_path()).unwrap(),
            seeded
        );
    }

    #[tokio::test]
    async fn unrecognized_window_with_seeded_cache_preserves_snapshot() {
        let mut server = mockito::Server::new_async().await;
        server.mock("GET", "/coding/v1/usages").with_status(200)
            .with_body(r#"{"usage":{"limit":100,"used":10},"limits":[{"window":{"duration":4,"timeUnit":"TIME_UNIT_HOUR"},"detail":{"limit":100,"used":10}}]}"#).create_async().await;
        let (_td, cache) = cache_fixture();
        let seeded = sample_seed().to_string();
        cache.write_payload(seeded.as_bytes()).unwrap();
        let out = fetch_snapshot(
            &reqwest::Client::new(),
            "sk-test",
            &cache,
            &Endpoints {
                usages: format!("{}/coding/v1/usages", server.url()),
            },
            Duration::ZERO,
        )
        .await
        .unwrap();
        assert!(out.stale);
        assert_eq!(out.snapshot.window_used, 20);
        assert_eq!(
            std::fs::read_to_string(cache.payload_path()).unwrap(),
            seeded
        );
    }

    #[tokio::test]
    async fn http_error_body_is_redacted() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/coding/v1/usages")
            .with_status(500)
            .with_body("proxy secret: <token>")
            .create_async()
            .await;
        let (_td, cache) = cache_fixture();
        let err = fetch_snapshot(
            &reqwest::Client::new(),
            "sk-test",
            &cache,
            &Endpoints {
                usages: format!("{}/coding/v1/usages", server.url()),
            },
            Duration::ZERO,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, AppError::Http { status: 500, ref body } if body == "Kimi API returned HTTP 500")
        );
    }
}
