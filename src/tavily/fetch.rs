//! Fetch Tavily usage from the documented `GET /usage` endpoint.

use std::fmt::Write as _;
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::cache::{Cache, acquire_lock_async};
use crate::error::{AppError, Result};
use crate::usage::TavilySnapshot;
use crate::vendor::{MAX_BODY_BYTES, read_body_capped};

use super::types::UsageResponse;

pub const BASE_URL: &str = "https://api.tavily.com/usage";
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const LOCK_TIMEOUT: Duration = Duration::from_secs(15);
/// Stable marker stored alongside code 0 for a successful HTTP response whose
/// payload no longer matches Tavily's documented usage schema.
pub const SCHEMA_DRIFT_MESSAGE: &str = "Tavily API schema drift";

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub usage: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            usage: BASE_URL.to_string(),
        }
    }
}

pub type FetchOutcome = crate::outcome::Outcome<TavilySnapshot>;

pub async fn fetch_snapshot(
    client: &reqwest::Client,
    api_key: &str,
    project_id: Option<&str>,
    cache: &Cache,
    endpoints: &Endpoints,
    cache_ttl: Duration,
) -> Result<FetchOutcome> {
    cache.ensure_dir()?;
    let _lock = acquire_lock_async(&cache.lock_path(), LOCK_TIMEOUT).await?;
    let target = target_key(endpoints, api_key, project_id);

    if let Some(bytes) = cache.fresh_payload(cache_ttl)?
        && let Ok(snapshot) = parse_cache(&bytes, &target)
    {
        return Ok(crate::outcome::Outcome::cached(snapshot, cache, false));
    }
    // A corrupt payload or a scope-fingerprint mismatch falls through to a
    // live fetch rather than serving stale or another project's numbers.

    match fetch_live(client, &endpoints.usage, api_key, project_id).await {
        Ok(snapshot) => {
            let body = serde_json::to_vec(&serde_json::json!({
                "target": target,
                "response": snapshot_repr(&snapshot),
            }))?;
            cache.write_payload(&body)?;
            Ok(crate::outcome::Outcome::fresh(snapshot))
        }
        Err(AppError::Transport(e)) => fallback_silent(cache, target, AppError::Transport(e)),
        Err(e) => {
            cache.mark_stale();
            if let Some((code, msg)) = error_to_pair(&e) {
                cache.write_last_error(code, &msg);
            }
            fallback_with_error(cache, target, e)
        }
    }
}

fn fallback_silent(cache: &Cache, target: String, original: AppError) -> Result<FetchOutcome> {
    crate::outcome::fallback(cache, None, original, |bytes| parse_cache(bytes, &target))
        .map_err(|_| AppError::Transport("Tavily request timed out".into()))
}

fn fallback_with_error(cache: &Cache, target: String, original: AppError) -> Result<FetchOutcome> {
    crate::outcome::fallback(cache, error_to_pair(&original), original, |bytes| {
        parse_cache(bytes, &target)
    })
}

fn error_to_pair(e: &AppError) -> Option<(u16, String)> {
    match e {
        AppError::Http { status, body } => Some((*status, body.clone())),
        AppError::Schema(_) => Some((0, SCHEMA_DRIFT_MESSAGE.into())),
        _ => Some((0, e.to_string())),
    }
}

/// Stable, non-secret identity for the endpoint, account, and optional project
/// selected by the caller. The usage endpoint resolves the key to an account
/// (and `X-Project-ID` narrows it to a project), so cache reuse must fail
/// closed when any input changes — otherwise one project's usage could be
/// served for another.
fn target_key(endpoints: &Endpoints, api_key: &str, project_id: Option<&str>) -> String {
    let digest = Sha256::digest(api_key.as_bytes());
    let mut fingerprint = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(fingerprint, "{byte:02x}");
    }
    let project = project_id.map(str::trim).unwrap_or("");
    format!("{}|key:{fingerprint}|project:{project}", endpoints.usage)
}

fn snapshot_repr(snapshot: &TavilySnapshot) -> serde_json::Value {
    serde_json::json!({
        "plan": snapshot.plan,
        "plan_used": snapshot.plan_used,
        "plan_limit": snapshot.plan_limit,
        "payg_used": snapshot.payg_used,
        "payg_limit": snapshot.payg_limit,
        "key_used": snapshot.key_used,
        "key_limit": snapshot.key_limit,
        "search": snapshot.search,
        "extract": snapshot.extract,
        "crawl": snapshot.crawl,
        "map": snapshot.map,
        "research": snapshot.research,
    })
}

fn parse_cache(bytes: &[u8], target: &str) -> Result<TavilySnapshot> {
    let v: serde_json::Value = serde_json::from_slice(bytes)?;
    if v["target"].as_str() != Some(target) {
        return Err(AppError::Schema(
            "tavily cache: scope fingerprint mismatch; refetching".into(),
        ));
    }
    let r = &v["response"];
    Ok(TavilySnapshot {
        plan: r["plan"]
            .as_str()
            .ok_or_else(|| AppError::Schema("tavily cache: missing plan".into()))?
            .to_string(),
        plan_used: parse_cache_u64(r, "plan_used")?,
        plan_limit: parse_cache_opt_u64(r, "plan_limit")?,
        payg_used: parse_cache_u64(r, "payg_used")?,
        payg_limit: parse_cache_opt_u64(r, "payg_limit")?,
        key_used: parse_cache_u64(r, "key_used")?,
        key_limit: parse_cache_opt_u64(r, "key_limit")?,
        search: parse_cache_u64(r, "search")?,
        extract: parse_cache_u64(r, "extract")?,
        crawl: parse_cache_u64(r, "crawl")?,
        map: parse_cache_u64(r, "map")?,
        research: parse_cache_u64(r, "research")?,
        scope_fingerprint: target.to_string(),
    })
}

fn parse_cache_u64(v: &serde_json::Value, name: &str) -> Result<u64> {
    v[name]
        .as_u64()
        .ok_or_else(|| AppError::Schema(format!("tavily cache: invalid {name}")))
}

fn parse_cache_opt_u64(v: &serde_json::Value, name: &str) -> Result<Option<u64>> {
    match v.get(name) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| AppError::Schema(format!("tavily cache: invalid {name}"))),
    }
}

async fn fetch_live(
    client: &reqwest::Client,
    url: &str,
    api_key: &str,
    project_id: Option<&str>,
) -> Result<TavilySnapshot> {
    let mut request = client
        .get(url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Accept", "application/json");
    if let Some(project) = project_id.map(str::trim).filter(|p| !p.is_empty()) {
        request = request.header("X-Project-ID", project);
    }
    let resp = tokio::time::timeout(HTTP_TIMEOUT, request.send())
        .await
        .map_err(|_| AppError::Transport(format!("tavily timeout: {url}")))??;

    let status = resp.status();
    let body = read_body_capped(resp, MAX_BODY_BYTES).await?;
    if !status.is_success() {
        // Never surface upstream bodies: they can echo the key or arbitrary
        // markup. The 401/403 case is redacted again at cache-persist time.
        let message = if matches!(status.as_u16(), 401 | 403) {
            "Tavily authentication failed".to_string()
        } else {
            format!("Tavily API returned HTTP {}", status.as_u16())
        };
        return Err(AppError::Http {
            status: status.as_u16(),
            body: message,
        });
    }

    let parsed: UsageResponse = serde_json::from_slice(&body)
        .map_err(|e| AppError::Schema(format!("tavily usage response: {e}")))?;
    // The fingerprint is recomputed on the next fresh-cache read; storing it in
    // the snapshot keeps the render path honest about which scope it carries.
    let target = target_key(
        &Endpoints {
            usage: url.to_string(),
        },
        api_key,
        project_id,
    );
    parsed.into_snapshot(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn cache_fixture() -> (TempDir, Cache) {
        let td = TempDir::new().unwrap();
        let cache = Cache::at(td.path().join("tavily"));
        cache.ensure_dir().unwrap();
        (td, cache)
    }

    fn sample_json() -> &'static str {
        r#"{
            "key": { "usage": 150, "limit": 1000,
                     "search_usage": 100, "extract_usage": 25,
                     "crawl_usage": 15, "map_usage": 7, "research_usage": 3 },
            "account": { "current_plan": "Bootstrap", "plan_usage": 500, "plan_limit": 15000,
                         "paygo_usage": 25, "paygo_limit": 100,
                         "search_usage": 350, "extract_usage": 75,
                         "crawl_usage": 50, "map_usage": 15, "research_usage": 10 }
        }"#
    }

    fn seed_for(endpoint: &str, key: &str, project: Option<&str>) -> String {
        let endpoints = Endpoints {
            usage: endpoint.to_string(),
        };
        let snapshot = TavilySnapshot {
            plan: "Bootstrap".into(),
            plan_used: 500,
            plan_limit: Some(15000),
            payg_used: 25,
            payg_limit: Some(100),
            key_used: 150,
            key_limit: Some(1000),
            search: 350,
            extract: 75,
            crawl: 50,
            map: 15,
            research: 10,
            scope_fingerprint: target_key(&endpoints, key, project),
        };
        serde_json::json!({
            "target": target_key(&endpoints, key, project),
            "response": snapshot_repr(&snapshot),
        })
        .to_string()
    }

    fn endpoints(url: &str) -> Endpoints {
        Endpoints {
            usage: url.to_string(),
        }
    }

    #[tokio::test]
    async fn live_200_returns_snapshot_and_sends_headers() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("GET", "/usage")
            .with_status(200)
            .with_body(sample_json())
            .match_header("authorization", "Bearer tvly-test")
            .match_header("accept", "application/json")
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let out = fetch_snapshot(
            &client,
            "tvly-test",
            None,
            &cache,
            &endpoints(&format!("{}/usage", server.url())),
            Duration::from_secs(0),
        )
        .await
        .unwrap();
        m.assert_async().await;
        assert_eq!(out.snapshot.plan, "Bootstrap");
        assert_eq!(out.snapshot.plan_used, 500);
        assert_eq!(out.snapshot.plan_limit, Some(15000));
        assert_eq!(out.snapshot.key_used, 150);
        assert!(!out.stale);
    }

    #[tokio::test]
    async fn project_id_is_sent_as_header_and_scopes_the_cache() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("GET", "/usage")
            .with_status(200)
            .with_body(sample_json())
            .match_header("x-project-id", "prj_123")
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let url = format!("{}/usage", server.url());
        let out = fetch_snapshot(
            &client,
            "tvly-test",
            Some("prj_123"),
            &cache,
            &endpoints(&url),
            Duration::from_secs(0),
        )
        .await
        .unwrap();
        m.assert_async().await;
        assert!(
            out.snapshot.scope_fingerprint.contains("|project:prj_123"),
            "{}",
            out.snapshot.scope_fingerprint
        );

        // A different project must not reuse this payload.
        let seeded = seed_for(&url, "tvly-test", Some("prj_123"));
        cache.write_payload(seeded.as_bytes()).unwrap();
        let mut server2 = mockito::Server::new_async().await;
        server2
            .mock("GET", "/usage")
            .with_status(200)
            .with_body(sample_json())
            .create_async()
            .await;
        let out2 = fetch_snapshot(
            &client,
            "tvly-test",
            Some("prj_other"),
            &cache,
            &endpoints(&format!("{}/usage", server2.url())),
            Duration::from_secs(60),
        )
        .await
        .unwrap();
        assert!(!out2.stale);
        assert_eq!(out2.snapshot.plan_used, 500);
    }

    #[tokio::test]
    async fn changed_key_forces_refetch_and_never_serves_other_scope() {
        let mut server = mockito::Server::new_async().await;
        let url = format!("{}/usage", server.url());
        server
            .mock("GET", "/usage")
            .with_status(200)
            .with_body(sample_json())
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        // Seed with a payload bound to a *different* key. A fresh read with the
        // new key must refuse it and refetch, never serving the old scope.
        cache
            .write_payload(seed_for(&url, "tvly-old", None).as_bytes())
            .unwrap();
        let client = reqwest::Client::new();
        let out = fetch_snapshot(
            &client,
            "tvly-new",
            None,
            &cache,
            &endpoints(&url),
            Duration::from_secs(60),
        )
        .await
        .unwrap();
        assert!(!out.stale);
        assert_eq!(out.snapshot.plan, "Bootstrap");
    }

    #[tokio::test]
    async fn fresh_cache_is_served_without_a_network_request() {
        let mut server = mockito::Server::new_async().await;
        let url = format!("{}/usage", server.url());
        let missed = server.mock("GET", "/usage").expect(0).create_async().await;
        let (_td, cache) = cache_fixture();
        cache
            .write_payload(seed_for(&url, "tvly-test", None).as_bytes())
            .unwrap();

        let client = reqwest::Client::new();
        let out = fetch_snapshot(
            &client,
            "tvly-test",
            None,
            &cache,
            &endpoints(&url),
            Duration::from_secs(60),
        )
        .await
        .unwrap();
        missed.assert_async().await;
        assert!(!out.stale);
        assert_eq!(out.snapshot.plan_used, 500);
    }

    #[tokio::test]
    async fn http_401_falls_back_to_cache() {
        let mut server = mockito::Server::new_async().await;
        let url = format!("{}/usage", server.url());
        server
            .mock("GET", "/usage")
            .with_status(401)
            .with_body(r#"{"detail":{"error":"invalid api key"}}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        cache
            .write_payload(seed_for(&url, "bad-key", None).as_bytes())
            .unwrap();

        let client = reqwest::Client::new();
        let out = fetch_snapshot(
            &client,
            "bad-key",
            None,
            &cache,
            &endpoints(&url),
            Duration::from_secs(0),
        )
        .await
        .unwrap();
        assert!(out.stale);
        assert_eq!(out.snapshot.plan_used, 500);
        assert_eq!(out.last_error.as_ref().map(|(c, _)| *c), Some(401));
    }

    #[tokio::test]
    async fn http_401_without_cache_returns_http_error() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/usage")
            .with_status(401)
            .with_body(r#"{"detail":{"error":"invalid api key"}}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let err = fetch_snapshot(
            &client,
            "bad-key",
            None,
            &cache,
            &endpoints(&format!("{}/usage", server.url())),
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
    async fn malformed_200_with_seeded_cache_returns_stale_snapshot_and_preserves_cache() {
        let mut server = mockito::Server::new_async().await;
        let url = format!("{}/usage", server.url());
        server
            .mock("GET", "/usage")
            .with_status(200)
            .with_body(r#"{"key":{"usage":0},"account":{}}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let seeded = seed_for(&url, "tvly-test", None);
        cache.write_payload(seeded.as_bytes()).unwrap();

        let client = reqwest::Client::new();
        let out = fetch_snapshot(
            &client,
            "tvly-test",
            None,
            &cache,
            &endpoints(&url),
            Duration::from_secs(0),
        )
        .await
        .unwrap();
        assert!(out.stale);
        assert_eq!(out.snapshot.plan_used, 500);
        assert_eq!(out.last_error, Some((0, SCHEMA_DRIFT_MESSAGE.into())));
        assert_eq!(
            std::fs::read_to_string(cache.payload_path()).unwrap(),
            seeded
        );
    }

    #[tokio::test]
    async fn transport_error_with_stale_cache_uses_cache() {
        let (_td, cache) = cache_fixture();
        cache
            .write_payload(seed_for("http://localhost:1/usage", "tvly-test", None).as_bytes())
            .unwrap();
        let client = reqwest::Client::new();
        let out = fetch_snapshot(
            &client,
            "tvly-test",
            None,
            &cache,
            &endpoints("http://localhost:1/usage"),
            Duration::from_secs(0),
        )
        .await
        .unwrap();
        assert!(out.stale);
        assert_eq!(out.snapshot.plan_used, 500);
    }

    #[tokio::test]
    async fn corrupt_fresh_cache_is_ignored_and_refetched() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/usage")
            .with_status(200)
            .with_body(sample_json())
            .create_async()
            .await;
        let (_td, cache) = cache_fixture();
        cache.write_payload(b"not valid json".as_slice()).unwrap();

        let client = reqwest::Client::new();
        let out = fetch_snapshot(
            &client,
            "tvly-test",
            None,
            &cache,
            &endpoints(&format!("{}/usage", server.url())),
            Duration::from_secs(60),
        )
        .await
        .unwrap();
        assert_eq!(out.snapshot.plan_used, 500);
        assert!(!out.stale);
    }

    #[tokio::test]
    async fn http_error_body_is_redacted() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/usage")
            .with_status(500)
            .with_body("proxy secret: <token>")
            .create_async()
            .await;
        let (_td, cache) = cache_fixture();
        let err = fetch_snapshot(
            &reqwest::Client::new(),
            "tvly-test",
            None,
            &cache,
            &endpoints(&format!("{}/usage", server.url())),
            Duration::from_secs(0),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, AppError::Http { status: 500, ref body } if body == "Tavily API returned HTTP 500"),
            "got {err:?}"
        );
    }
}
