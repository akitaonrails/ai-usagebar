//! Fetch Kimi usage from `/coding/v1/usages`.
//!
//! One endpoint, two credentials: an **API key**, or the **Kimi Code CLI's
//! OAuth session** — the credential a subscriber already has locally, with no
//! key to create or paste. See [`Auth`] and `oauth.rs`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::cache::{Cache, acquire_lock_async};
use crate::error::{AppError, Result};
use crate::usage::KimiSnapshot;

use super::oauth::{self, Region};
use super::types::{UsagesResponse, UserInfoResponse, humanize_membership_level};

pub const BASE_URL: &str = "https://api.kimi.com";
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const REFRESH_TIMEOUT: Duration = Duration::from_secs(15);
const LOCK_TIMEOUT: Duration = Duration::from_secs(15);
/// Stable marker stored alongside code 0 for a successful HTTP response whose
/// payload no longer matches Kimi's undocumented usage schema.
pub const SCHEMA_DRIFT_MESSAGE: &str = "Kimi API schema drift";

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub usages: String,
    /// Profile endpoint, read solely for the subscription's own tier name.
    pub me: String,
    /// OAuth token endpoint for the same deployment. Unused by the API-key
    /// path, which never refreshes anything.
    pub token: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self::for_region(Region::MainlandCn)
    }
}

impl Endpoints {
    pub fn for_region(region: Region) -> Self {
        Self {
            usages: format!("{}/usages", region.api_base()),
            me: format!("{}/me", region.api_base()),
            token: oauth::token_endpoint(region.oauth_host()),
        }
    }
}

/// Where the bearer for `/coding/v1/usages` comes from.
#[derive(Debug, Clone)]
pub enum Auth {
    /// A platform API key (`KIMI_API_KEY` or `[kimi] api_key`).
    ApiKey(String),
    /// The Kimi Code CLI's own OAuth session — a subscription login.
    KimiCode(KimiCodeAuth),
}

/// The two paths inside a kimi-code home this vendor touches: the credential
/// file it reads and rewrites, and the lock target that serializes a refresh
/// against the CLI's own (see `lock.rs`).
#[derive(Debug, Clone)]
pub struct KimiCodeAuth {
    pub credentials_path: PathBuf,
    pub lock_target: PathBuf,
}

impl KimiCodeAuth {
    pub fn in_home(home: &Path) -> Self {
        Self {
            credentials_path: oauth::credentials_path_in(home),
            lock_target: oauth::lock_target_in(home),
        }
    }

    /// A credential file relocated by config (`[kimi] credentials_path`) still
    /// belongs to a kimi-code home; the lock target is derived from that home
    /// so both clients agree on which file the lock protects.
    pub fn with_credentials_path(home: &Path, credentials_path: PathBuf) -> Self {
        Self {
            credentials_path,
            lock_target: oauth::lock_target_in(home),
        }
    }
}

/// This vendor's [`Outcome`](crate::outcome::Outcome) — the shared shape,
/// specialised to its snapshot.
pub type FetchOutcome = crate::outcome::Outcome<KimiSnapshot>;

/// API-key fetch. Kept as-is for existing callers; the OAuth path goes
/// through [`fetch_snapshot_with_auth`].
pub async fn fetch_snapshot(
    client: &reqwest::Client,
    api_key: &str,
    cache: &Cache,
    endpoints: &Endpoints,
    cache_ttl: Duration,
) -> Result<FetchOutcome> {
    fetch_snapshot_with_auth(
        client,
        &Auth::ApiKey(api_key.to_string()),
        cache,
        endpoints,
        cache_ttl,
    )
    .await
}

pub async fn fetch_snapshot_with_auth(
    client: &reqwest::Client,
    auth: &Auth,
    cache: &Cache,
    endpoints: &Endpoints,
    cache_ttl: Duration,
) -> Result<FetchOutcome> {
    fetch_snapshot_at(client, auth, cache, endpoints, cache_ttl, Utc::now()).await
}

/// Clock seam for the OAuth expiry decision, mirroring
/// `kiro::fetch::fetch_snapshot_at`.
async fn fetch_snapshot_at(
    client: &reqwest::Client,
    auth: &Auth,
    cache: &Cache,
    endpoints: &Endpoints,
    cache_ttl: Duration,
    now: DateTime<Utc>,
) -> Result<FetchOutcome> {
    cache.ensure_dir()?;
    let _lock = acquire_lock_async(&cache.lock_path(), LOCK_TIMEOUT).await?;

    if let Some(bytes) = cache.fresh_payload(cache_ttl)? {
        // Releases before the profile lookup cached `/usages`' internal enum
        // verbatim. Do not let a still-fresh legacy entry postpone `/me` until
        // the normal TTL expires: refresh it once and replace it with Kimi's
        // own tier name. If the network is down, the fallback path below still
        // serves the quota after humanizing the enum.
        if !cache_has_legacy_plan(&bytes)
            && let Ok(outcome) = reuse_cache(bytes, cache, false)
        {
            return Ok(outcome);
        }
    }
    // Corrupt fresh cache: fall through to live fetch rather than return a
    // fabricated zero snapshot.

    match fetch_live(client, endpoints, auth, now).await {
        Ok(snap) => {
            let bytes = serde_json::to_vec(&snap_to_json(&snap))?;
            cache.write_payload(&bytes)?;
            Ok(crate::outcome::Outcome::fresh(snap))
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
    crate::outcome::fallback(cache, None, original, parse_cache)
}

fn fallback_with_error(cache: &Cache, original: AppError) -> Result<FetchOutcome> {
    let last_error = error_to_pair(&original);
    crate::outcome::fallback(cache, last_error, original, parse_cache)
}

fn error_to_pair(e: &AppError) -> Option<(u16, String)> {
    match e {
        AppError::Http { status, body } => Some((*status, body.clone())),
        // A 2xx response with an unknown shape is not an HTTP 422 response.
        AppError::Schema(_) => Some((0, SCHEMA_DRIFT_MESSAGE.into())),
        e => Some((0, e.to_string())),
    }
}

fn reuse_cache(bytes: Vec<u8>, cache: &Cache, stale: bool) -> Result<FetchOutcome> {
    let snap = parse_cache(&bytes)?;
    Ok(crate::outcome::Outcome::cached(snap, cache, stale))
}

fn parse_cache(bytes: &[u8]) -> Result<KimiSnapshot> {
    let v: serde_json::Value = serde_json::from_slice(bytes)?;
    let weekly_limit = parse_cache_u64(&v["weekly_limit"], "weekly_limit")?;
    let weekly_used = parse_cache_u64(&v["weekly_used"], "weekly_used")?;
    let weekly_remaining = parse_cache_u64(&v["weekly_remaining"], "weekly_remaining")?;
    // Caches written before the monthly shape have no `has_weekly` key; they
    // all carried weekly counters, so absence means the legacy shape.
    let has_weekly = match &v["has_weekly"] {
        serde_json::Value::Null => true,
        serde_json::Value::Bool(b) => *b,
        _ => return Err(AppError::Schema("kimi cache: invalid has_weekly".into())),
    };
    if !has_weekly && (weekly_limit > 0 || weekly_used > 0 || weekly_remaining > 0) {
        // A combination the fetch path can never produce: the newer shape has
        // no weekly counters, so nonzero ones here are a corrupt cache.
        return Err(AppError::Schema(
            "kimi cache: weekly counters on a monthly-shape snapshot".into(),
        ));
    }
    let monthly_pct = parse_cache_monthly_pct(&v["monthly_pct"])?;
    Ok(KimiSnapshot {
        plan: v["plan"].as_str().map(|plan| {
            if plan.starts_with("LEVEL_") {
                humanize_membership_level(plan)
            } else {
                plan.to_string()
            }
        }),
        weekly_limit,
        weekly_used,
        weekly_remaining,
        weekly_reset_at: parse_cache_datetime(&v["weekly_reset_at"])?,
        has_weekly,
        monthly_pct,
        monthly_reset_at: parse_cache_datetime(&v["monthly_reset_at"])?,
        window_limit: parse_cache_u64(&v["window_limit"], "window_limit")?,
        window_used: parse_cache_u64(&v["window_used"], "window_used")?,
        window_remaining: parse_cache_u64(&v["window_remaining"], "window_remaining")?,
        window_reset_at: parse_cache_datetime(&v["window_reset_at"])?,
    })
}

fn cache_has_legacy_plan(bytes: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(bytes)
        .ok()
        .and_then(|value| value["plan"].as_str().map(str::to_owned))
        .is_some_and(|plan| plan.starts_with("LEVEL_"))
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

/// A percentage the fetch path already clamped to 0..=100; anything else in
/// the cache is corruption, not a quota.
fn parse_cache_monthly_pct(v: &serde_json::Value) -> Result<Option<i32>> {
    match v {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Number(n) => {
            let pct = n
                .as_i64()
                .ok_or_else(|| AppError::Schema("kimi cache: invalid monthly_pct".into()))?;
            if !(0..=100).contains(&pct) {
                return Err(AppError::Schema(
                    "kimi cache: monthly_pct out of range".into(),
                ));
            }
            Ok(Some(pct as i32))
        }
        _ => Err(AppError::Schema("kimi cache: invalid monthly_pct".into())),
    }
}

fn snap_to_json(snap: &KimiSnapshot) -> serde_json::Value {
    serde_json::json!({
        "plan": snap.plan,
        "weekly_limit": snap.weekly_limit,
        "weekly_used": snap.weekly_used,
        "weekly_remaining": snap.weekly_remaining,
        "weekly_reset_at": snap.weekly_reset_at.map(|dt| dt.to_rfc3339()),
        "has_weekly": snap.has_weekly,
        "monthly_pct": snap.monthly_pct,
        "monthly_reset_at": snap.monthly_reset_at.map(|dt| dt.to_rfc3339()),
        "window_limit": snap.window_limit,
        "window_used": snap.window_used,
        "window_remaining": snap.window_remaining,
        "window_reset_at": snap.window_reset_at.map(|dt| dt.to_rfc3339()),
    })
}

/// Resolve the bearer for this fetch. The API key is already one; a Kimi Code
/// login may first need a refresh, which rotates the CLI's stored token pair
/// and is therefore serialized against the CLI itself.
async fn bearer_token(
    client: &reqwest::Client,
    endpoints: &Endpoints,
    auth: &Auth,
    now: DateTime<Utc>,
) -> Result<String> {
    let kimi_code = match auth {
        Auth::ApiKey(key) => return Ok(key.clone()),
        Auth::KimiCode(kimi_code) => kimi_code,
    };

    let creds = oauth::read_from(&kimi_code.credentials_path)?;
    if !oauth::needs_refresh(creds.expires_at, now.timestamp()) {
        return Ok(creds.access_token);
    }

    let _lock = super::lock::acquire(&kimi_code.lock_target).await?;
    // Re-read under the lock: the CLI (or another ai-usagebar process) may
    // have refreshed while we waited, and reusing our pre-lock copy would burn
    // an already-rotated refresh token.
    let creds = oauth::read_from(&kimi_code.credentials_path)?;
    if !oauth::needs_refresh(creds.expires_at, now.timestamp()) {
        return Ok(creds.access_token);
    }

    let refreshed = tokio::time::timeout(
        REFRESH_TIMEOUT,
        oauth::refresh(
            client,
            &endpoints.token,
            oauth::CLIENT_ID,
            &creds.refresh_token,
        ),
    )
    .await
    .map_err(|_| AppError::Transport(format!("kimi token refresh timeout: {}", endpoints.token)))?
    .map_err(|e| match e {
        AppError::Transport(msg) => AppError::Transport(msg),
        e => AppError::Credentials(format!(
            "Kimi Code CLI token refresh failed ({e}). Run `kimi` and log in again."
        )),
    })?;

    let next = oauth::apply_refresh(&creds, refreshed, now.timestamp());
    oauth::write_to(&kimi_code.credentials_path, &next).map_err(|e| {
        // The rotation already happened upstream, so a failed write-back means
        // the CLI is now holding a dead refresh token: say so plainly instead
        // of leaving the user to discover it at their next `kimi` run.
        AppError::Credentials(format!(
            "the refreshed Kimi Code CLI credentials could not be saved ({e}); run `kimi` and log in again"
        ))
    })?;
    Ok(next.access_token)
}

/// Ask `/coding/v1/me` for the subscription's own tier name ("Allegretto"),
/// which is the only place the vendor spells its plans the way its pricing
/// page does — `/usages` only carries the `LEVEL_*` wire enum.
///
/// Best-effort by construction: every failure returns `None` and leaves the
/// humanized enum in place. A missing plan label must never cost the user
/// their quota numbers, and this endpoint is documented (by kimi-code's own
/// error text) to 404 for accounts without a coding profile.
async fn plan_label(client: &reqwest::Client, url: &str, token: &str) -> Option<String> {
    let resp = tokio::time::timeout(
        HTTP_TIMEOUT,
        client
            .get(url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/json")
            .send(),
    )
    .await
    .ok()?
    .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let bytes = crate::vendor::read_body_capped(resp, crate::vendor::MAX_BODY_BYTES)
        .await
        .ok()?;
    serde_json::from_slice::<UserInfoResponse>(&bytes)
        .ok()?
        .plan_label()
}

async fn fetch_live(
    client: &reqwest::Client,
    endpoints: &Endpoints,
    auth: &Auth,
    now: DateTime<Utc>,
) -> Result<KimiSnapshot> {
    let url = &endpoints.usages;
    let token = bearer_token(client, endpoints, auth, now).await?;
    // Concurrent, not sequential: the plan label is a second request against
    // the same deployment, and a widget tick should cost one round-trip's
    // latency, not two.
    let (usages, label) = tokio::join!(
        tokio::time::timeout(
            HTTP_TIMEOUT,
            client
                .get(url)
                .header("Authorization", format!("Bearer {token}"))
                .header("Accept", "application/json")
                .send(),
        ),
        plan_label(client, &endpoints.me, &token),
    );
    let resp = usages.map_err(|_| AppError::Transport(format!("kimi timeout: {url}")))??;

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

    let bytes = crate::vendor::read_body_capped(resp, crate::vendor::MAX_BODY_BYTES).await?;
    let r: UsagesResponse = serde_json::from_slice(&bytes)
        .map_err(|e| AppError::Schema(format!("kimi usages response: {e}")))?;
    let mut snap = r.into_snapshot()?;
    if let Some(label) = label {
        snap.plan = Some(label);
    }
    Ok(snap)
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

    /// Both endpoints point at the same mock server, so an OAuth test can
    /// serve `/api/oauth/token` and `/coding/v1/usages` from one mockito.
    fn test_endpoints(base: &str) -> Endpoints {
        Endpoints {
            usages: format!("{base}/coding/v1/usages"),
            me: format!("{base}/coding/v1/me"),
            token: format!("{base}/api/oauth/token"),
        }
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
        let endpoints = test_endpoints(&server.url());
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
        // No /me mock on this server, so the humanized wire enum stands.
        assert_eq!(out.snapshot.plan, Some("Intermediate".into()));
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
        let endpoints = test_endpoints(&server.url());
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
        let endpoints = test_endpoints(&server.url());
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
        let endpoints = test_endpoints(&server.url());
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
        let endpoints = test_endpoints(&server.url());
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
        let endpoints = test_endpoints(&server.url());
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
        assert_eq!(out.last_error, Some((0, SCHEMA_DRIFT_MESSAGE.into())));

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
        let endpoints = test_endpoints(&server.url());
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
        let endpoints = test_endpoints(&server.url());
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
    async fn a_fresh_legacy_plan_cache_is_upgraded_through_the_profile_endpoint() {
        let mut server = mockito::Server::new_async().await;
        let usages = server
            .mock("GET", "/coding/v1/usages")
            .with_status(200)
            .with_body(sample_json())
            .create_async()
            .await;
        let me = server
            .mock("GET", "/coding/v1/me")
            .with_status(200)
            .with_body(r#"{"user_level_name":"Allegretto"}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        cache
            .write_payload(sample_seed().to_string().as_bytes())
            .unwrap();

        let out = fetch_snapshot(
            &reqwest::Client::new(),
            "sk-test",
            &cache,
            &test_endpoints(&server.url()),
            Duration::from_secs(60),
        )
        .await
        .unwrap();

        usages.assert_async().await;
        me.assert_async().await;
        assert_eq!(out.snapshot.plan, Some("Allegretto".into()));
        let cached: serde_json::Value =
            serde_json::from_slice(&std::fs::read(cache.payload_path()).unwrap()).unwrap();
        assert_eq!(cached["plan"], "Allegretto");
    }

    #[test]
    fn a_legacy_plan_is_humanized_when_only_fallback_cache_is_available() {
        let bytes = sample_seed().to_string();
        let snap = parse_cache(bytes.as_bytes()).unwrap();
        assert_eq!(snap.plan, Some("Intermediate".into()));
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
        let endpoints = test_endpoints(&server.url());
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
        let endpoints = test_endpoints("http://localhost:1");
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
            &test_endpoints(&server.url()),
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
            &test_endpoints(&server.url()),
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
            &test_endpoints(&server.url()),
            Duration::ZERO,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, AppError::Http { status: 500, ref body } if body == "Kimi API returned HTTP 500")
        );
    }

    // ---- Kimi Code CLI (subscription) credential path ----

    /// A kimi-code home with a stored login. `expires_at` is relative to
    /// `NOW_SECS`, the instant every OAuth test below passes in.
    const NOW_SECS: i64 = 1_800_000_000;

    fn kimi_code_home(td: &TempDir, expires_in: i64) -> (PathBuf, KimiCodeAuth) {
        let home = td.path().join(".kimi-code");
        let auth = KimiCodeAuth::in_home(&home);
        std::fs::create_dir_all(auth.credentials_path.parent().unwrap()).unwrap();
        std::fs::write(
            &auth.credentials_path,
            serde_json::json!({
                "access_token": "cli-at",
                "refresh_token": "cli-rt",
                "expires_at": NOW_SECS + expires_in,
                "expires_in": 900,
                "scope": "kimi-code",
                "token_type": "Bearer",
            })
            .to_string(),
        )
        .unwrap();
        (home, auth)
    }

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(NOW_SECS, 0).unwrap()
    }

    #[tokio::test]
    async fn a_valid_cli_token_is_used_as_is_and_never_refreshed() {
        let mut server = mockito::Server::new_async().await;
        let usages = server
            .mock("GET", "/coding/v1/usages")
            .match_header("authorization", "Bearer cli-at")
            .with_status(200)
            .with_body(sample_json())
            .create_async()
            .await;
        let refresh = server
            .mock("POST", "/api/oauth/token")
            .expect(0)
            .create_async()
            .await;

        let (td, cache) = cache_fixture();
        let (_home, auth) = kimi_code_home(&td, 600);
        let out = fetch_snapshot_at(
            &reqwest::Client::new(),
            &Auth::KimiCode(auth.clone()),
            &cache,
            &test_endpoints(&server.url()),
            Duration::ZERO,
            now(),
        )
        .await
        .unwrap();

        usages.assert_async().await;
        refresh.assert_async().await;
        assert_eq!(out.snapshot.weekly_used, 26);
        let stored = std::fs::read_to_string(&auth.credentials_path).unwrap();
        assert!(stored.contains("cli-rt"), "an unused token must not rotate");
    }

    #[tokio::test]
    async fn an_expiring_cli_token_is_refreshed_and_the_rotation_is_written_back() {
        let mut server = mockito::Server::new_async().await;
        let refresh = server
            .mock("POST", "/api/oauth/token")
            .match_body(mockito::Matcher::UrlEncoded(
                "refresh_token".into(),
                "cli-rt".into(),
            ))
            .with_status(200)
            .with_body(
                r#"{"access_token":"fresh-at","refresh_token":"fresh-rt","expires_in":900,
                    "scope":"kimi-code","token_type":"Bearer"}"#,
            )
            .create_async()
            .await;
        let usages = server
            .mock("GET", "/coding/v1/usages")
            .match_header("authorization", "Bearer fresh-at")
            .with_status(200)
            .with_body(sample_json())
            .create_async()
            .await;

        let (td, cache) = cache_fixture();
        // Inside the refresh buffer: still valid, but not for long enough.
        let (home, auth) = kimi_code_home(&td, 30);
        let out = fetch_snapshot_at(
            &reqwest::Client::new(),
            &Auth::KimiCode(auth.clone()),
            &cache,
            &test_endpoints(&server.url()),
            Duration::ZERO,
            now(),
        )
        .await
        .unwrap();

        refresh.assert_async().await;
        usages.assert_async().await;
        assert_eq!(out.snapshot.weekly_used, 26);

        let stored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&auth.credentials_path).unwrap()).unwrap();
        assert_eq!(stored["access_token"], "fresh-at");
        assert_eq!(
            stored["refresh_token"], "fresh-rt",
            "the CLI's own store must carry the rotated token, or its next run is dead"
        );
        assert_eq!(stored["expires_at"], NOW_SECS + 900);
        // The lock is the CLI's own target, and it must not be left behind.
        assert!(!super::super::lock::lock_dir_for(&auth.lock_target).exists());
        assert!(home.join("oauth").is_dir());
    }

    #[tokio::test]
    async fn a_rejected_refresh_reports_a_credential_error_naming_the_cli() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/api/oauth/token")
            .with_status(401)
            .with_body(r#"{"error":"invalid_grant"}"#)
            .create_async()
            .await;

        let (td, cache) = cache_fixture();
        let (_home, auth) = kimi_code_home(&td, -60);
        let err = fetch_snapshot_at(
            &reqwest::Client::new(),
            &Auth::KimiCode(auth),
            &cache,
            &test_endpoints(&server.url()),
            Duration::ZERO,
            now(),
        )
        .await
        .unwrap_err();
        let message = err.to_string();
        assert!(matches!(err, AppError::Credentials(_)), "{err:?}");
        assert!(message.contains("log in again"), "{message}");
    }

    #[tokio::test]
    async fn a_logged_out_cli_falls_back_to_cache_with_a_credential_warning() {
        let (td, cache) = cache_fixture();
        let (_home, auth) = kimi_code_home(&td, 600);
        std::fs::write(
            &auth.credentials_path,
            r#"{"access_token":"","refresh_token":"","expires_at":0}"#,
        )
        .unwrap();
        cache
            .write_payload(sample_seed().to_string().as_bytes())
            .unwrap();

        let out = fetch_snapshot_at(
            &reqwest::Client::new(),
            &Auth::KimiCode(auth),
            &cache,
            &test_endpoints("http://localhost:1"),
            Duration::ZERO,
            now(),
        )
        .await
        .unwrap();
        assert!(out.stale);
        assert_eq!(out.snapshot.weekly_used, 30);
        let (code, message) = out.last_error.unwrap();
        assert_eq!(code, 0);
        assert!(message.contains("logged out"), "{message}");
    }

    #[tokio::test]
    async fn a_peer_refresh_during_the_wait_is_picked_up_instead_of_rotating_again() {
        let mut server = mockito::Server::new_async().await;
        let refresh = server
            .mock("POST", "/api/oauth/token")
            .expect(0)
            .create_async()
            .await;
        let usages = server
            .mock("GET", "/coding/v1/usages")
            .match_header("authorization", "Bearer peer-at")
            .with_status(200)
            .with_body(sample_json())
            .create_async()
            .await;

        let (td, cache) = cache_fixture();
        let (_home, auth) = kimi_code_home(&td, -60);
        // Stand in for the CLI finishing its own refresh while we queued: the
        // re-read under the lock must win over the copy read before it.
        let peer = serde_json::json!({
            "access_token": "peer-at",
            "refresh_token": "peer-rt",
            "expires_at": NOW_SECS + 900,
            "expires_in": 900,
            "scope": "kimi-code",
            "token_type": "Bearer",
        });
        std::fs::write(&auth.credentials_path, peer.to_string()).unwrap();

        let out = fetch_snapshot_at(
            &reqwest::Client::new(),
            &Auth::KimiCode(auth),
            &cache,
            &test_endpoints(&server.url()),
            Duration::ZERO,
            now(),
        )
        .await
        .unwrap();
        refresh.assert_async().await;
        usages.assert_async().await;
        assert_eq!(out.snapshot.weekly_used, 26);
    }

    #[test]
    fn endpoints_follow_the_region() {
        let cn = Endpoints::for_region(Region::MainlandCn);
        assert_eq!(cn.usages, "https://api.kimi.com/coding/v1/usages");
        assert_eq!(cn.token, "https://auth.kimi.com/api/oauth/token");
        let global = Endpoints::for_region(Region::Global);
        assert_eq!(global.usages, "https://api.kimi.ai/coding/v1/usages");
        assert_eq!(global.token, "https://auth.kimi.ai/api/oauth/token");
        // The default stays the endpoint the API-key path has always used.
        assert_eq!(Endpoints::default().usages, cn.usages);
    }

    #[tokio::test]
    async fn the_vendors_own_tier_name_replaces_the_wire_enum() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/coding/v1/usages")
            .with_status(200)
            .with_body(sample_json())
            .create_async()
            .await;
        let me = server
            .mock("GET", "/coding/v1/me")
            .match_header("authorization", "Bearer sk-test")
            .with_status(200)
            .with_body(r#"{"user_id":"u-1","user_level":25,"user_level_name":"Allegretto"}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let out = fetch_snapshot(
            &reqwest::Client::new(),
            "sk-test",
            &cache,
            &test_endpoints(&server.url()),
            Duration::ZERO,
        )
        .await
        .unwrap();
        me.assert_async().await;
        assert_eq!(out.snapshot.plan, Some("Allegretto".into()));
        // …and it survives the cache round-trip, not just the live fetch.
        let cached: serde_json::Value =
            serde_json::from_slice(&std::fs::read(cache.payload_path()).unwrap()).unwrap();
        assert_eq!(cached["plan"], "Allegretto");
    }

    #[tokio::test]
    async fn a_profile_endpoint_that_fails_costs_the_label_and_nothing_else() {
        // 404 is documented by kimi-code's own error text for accounts with no
        // coding profile; the quota numbers must still come through.
        for status in [404, 401, 500] {
            let mut server = mockito::Server::new_async().await;
            server
                .mock("GET", "/coding/v1/usages")
                .with_status(200)
                .with_body(sample_json())
                .create_async()
                .await;
            server
                .mock("GET", "/coding/v1/me")
                .with_status(status)
                .with_body(r#"{"error":"nope"}"#)
                .create_async()
                .await;

            let (_td, cache) = cache_fixture();
            let out = fetch_snapshot(
                &reqwest::Client::new(),
                "sk-test",
                &cache,
                &test_endpoints(&server.url()),
                Duration::ZERO,
            )
            .await
            .unwrap();
            assert_eq!(out.snapshot.plan, Some("Intermediate".into()), "{status}");
            assert_eq!(out.snapshot.weekly_used, 26, "{status}");
            assert!(out.last_error.is_none(), "{status}: must not warn");
        }
    }

    #[tokio::test]
    async fn an_unreachable_profile_endpoint_does_not_fail_the_fetch() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/coding/v1/usages")
            .with_status(200)
            .with_body(sample_json())
            .create_async()
            .await;
        let mut endpoints = test_endpoints(&server.url());
        endpoints.me = "http://localhost:1/coding/v1/me".into();

        let (_td, cache) = cache_fixture();
        let out = fetch_snapshot(
            &reqwest::Client::new(),
            "sk-test",
            &cache,
            &endpoints,
            Duration::ZERO,
        )
        .await
        .unwrap();
        assert_eq!(out.snapshot.plan, Some("Intermediate".into()));
        assert!(!out.stale);
    }

    #[test]
    fn a_relocated_credential_file_keeps_the_homes_lock_target() {
        let home = Path::new("/home/u/.kimi-code");
        let auth =
            KimiCodeAuth::with_credentials_path(home, PathBuf::from("/elsewhere/kimi-code.json"));
        assert_eq!(
            auth.credentials_path,
            PathBuf::from("/elsewhere/kimi-code.json")
        );
        assert_eq!(auth.lock_target, oauth::lock_target_in(home));
    }

    fn monthly_shape_snap() -> KimiSnapshot {
        KimiSnapshot {
            plan: Some("Allegretto".into()),
            weekly_limit: 0,
            weekly_used: 0,
            weekly_remaining: 0,
            weekly_reset_at: None,
            has_weekly: false,
            monthly_pct: Some(42),
            monthly_reset_at: Some(
                DateTime::parse_from_rfc3339("2026-10-16T00:00:00Z")
                    .unwrap()
                    .into(),
            ),
            window_limit: 100,
            window_used: 15,
            window_remaining: 85,
            window_reset_at: Some(
                DateTime::parse_from_rfc3339("2026-09-16T20:11:32Z")
                    .unwrap()
                    .into(),
            ),
        }
    }

    #[test]
    fn a_monthly_shape_snapshot_survives_the_cache_round_trip() {
        let bytes = serde_json::to_vec(&snap_to_json(&monthly_shape_snap())).unwrap();
        let snap = parse_cache(&bytes).unwrap();
        assert!(!snap.has_weekly);
        assert_eq!(snap.monthly_pct, Some(42));
        assert_eq!(
            snap.monthly_reset_at.map(|dt| dt.to_rfc3339()),
            Some("2026-10-16T00:00:00+00:00".to_string())
        );
        assert_eq!(snap.weekly_used, 0);
        assert_eq!(snap.window_used, 15);
        assert_eq!(snap, monthly_shape_snap());
    }

    #[test]
    fn a_legacy_cache_without_the_new_keys_still_parses() {
        // sample_seed predates the monthly shape: no has_weekly/monthly keys.
        let bytes = sample_seed().to_string();
        let snap = parse_cache(bytes.as_bytes()).unwrap();
        assert!(snap.has_weekly);
        assert_eq!(snap.monthly_pct, None);
        assert_eq!(snap.monthly_reset_at, None);
        assert_eq!(snap.weekly_used, 30);
    }

    #[test]
    fn a_cache_with_weekly_counters_on_a_monthly_shape_is_rejected() {
        let mut v = sample_seed();
        v["has_weekly"] = serde_json::json!(false);
        let err = parse_cache(v.to_string().as_bytes()).unwrap_err();
        assert!(err.to_string().contains("monthly-shape"), "{err}");
    }

    #[test]
    fn a_cache_with_an_out_of_range_monthly_pct_is_rejected() {
        for bad in [150, -1] {
            let mut v: serde_json::Value = serde_json::from_slice(
                &serde_json::to_vec(&snap_to_json(&monthly_shape_snap())).unwrap(),
            )
            .unwrap();
            v["monthly_pct"] = serde_json::json!(bad);
            let err = parse_cache(v.to_string().as_bytes()).unwrap_err();
            assert!(err.to_string().contains("monthly_pct"), "{bad}: {err}");
        }
    }
}
