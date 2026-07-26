//! Fetch Cursor's included-usage summary from `GET /api/usage-summary`,
//! authenticated with the session token read out of the local `state.vscdb`
//! (see `db.rs`). Cache/stale/error-fallback shape mirrors `kimi::fetch`.

use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::cache::{Cache, MAX_STALE, acquire_lock_async};
use crate::error::{AppError, Result};
use crate::usage::CursorSnapshot;
use crate::vendor::{MAX_BODY_BYTES, read_body_capped};

use super::db;
use super::types::{self, UsageSummary};

pub const BASE_URL: &str = "https://cursor.com";
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const LOCK_TIMEOUT: Duration = Duration::from_secs(15);
/// The dashboard endpoint gates on browser-looking headers; a plain
/// `reqwest` request with only the cookie is rejected by its CORS check.
const BROWSER_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
    AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub summary: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            summary: format!("{BASE_URL}/api/usage-summary"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FetchOutcome {
    pub snapshot: CursorSnapshot,
    pub stale: bool,
    pub last_error: Option<(u16, String)>,
    pub cache_age: Option<Duration>,
}

/// Cache-aware fetch. `db_path` is Cursor's `state.vscdb` — the caller resolves
/// `[cursor] db_path` (config override) vs [`db::default_db_path`], the same
/// override pattern as `openai.codex_auth_path`.
pub async fn fetch_snapshot(
    client: &reqwest::Client,
    db_path: &Path,
    cache: &Cache,
    endpoints: &Endpoints,
    cache_ttl: Duration,
) -> Result<FetchOutcome> {
    cache.ensure_dir()?;
    let _lock = acquire_lock_async(&cache.lock_path(), LOCK_TIMEOUT).await?;

    if let Some(bytes) = cache.fresh_payload(cache_ttl)?
        && let Ok(outcome) = reuse_cache(&bytes, cache, false)
    {
        return Ok(outcome);
    }

    match fetch_live(client, endpoints, db_path).await {
        Ok(snap) => {
            let bytes = serde_json::to_vec(&snap_to_json(&snap))?;
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
    let Some(bytes) = cache.fallback_payload(MAX_STALE)? else {
        return Err(original);
    };
    match reuse_cache(&bytes, cache, true) {
        Ok(outcome) => Ok(outcome),
        Err(_) => Err(original),
    }
}

fn fallback_with_error(cache: &Cache, original: AppError) -> Result<FetchOutcome> {
    let Some(bytes) = cache.fallback_payload(MAX_STALE)? else {
        return Err(original);
    };
    match reuse_cache(&bytes, cache, true) {
        Ok(mut outcome) => {
            outcome.last_error = error_to_pair(&original);
            Ok(outcome)
        }
        Err(_) => Err(original),
    }
}

/// Never surface upstream bodies for auth failures: the request carried a
/// session cookie derived from a signed-in token, and 401/403 bodies from a
/// scraped web endpoint are not guaranteed not to echo it back.
fn error_to_pair(e: &AppError) -> Option<(u16, String)> {
    match e {
        AppError::Http { status, .. } if matches!(status, 401 | 403) => {
            Some((*status, "Cursor authentication failed".into()))
        }
        AppError::Http { status, body } => Some((*status, body.clone())),
        e => Some((0, e.to_string())),
    }
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

fn parse_cache(bytes: &[u8]) -> Result<CursorSnapshot> {
    let v: serde_json::Value = serde_json::from_slice(bytes)?;
    let int = |key: &str| -> Result<i32> {
        v[key]
            .as_i64()
            .map(|n| n as i32)
            .ok_or_else(|| AppError::Schema(format!("cursor cache: invalid {key}")))
    };
    Ok(CursorSnapshot {
        plan: v["plan"].as_str().unwrap_or("Cursor").to_string(),
        auto_pct: int("auto_pct")?,
        api_pct: int("api_pct")?,
        total_pct: int("total_pct")?,
        unlimited: v["unlimited"].as_bool().unwrap_or(false),
        on_demand_enabled: v["on_demand_enabled"].as_bool().unwrap_or(false),
        reset_at: parse_cache_datetime(&v["reset_at"])?,
    })
}

fn parse_cache_datetime(v: &serde_json::Value) -> Result<Option<DateTime<Utc>>> {
    match v {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(s) => DateTime::parse_from_rfc3339(s)
            .map(|dt| Some(dt.into()))
            .map_err(|e| AppError::Schema(format!("cursor cache: invalid reset timestamp: {e}"))),
        _ => Err(AppError::Schema(
            "cursor cache: invalid reset timestamp".into(),
        )),
    }
}

fn snap_to_json(snap: &CursorSnapshot) -> serde_json::Value {
    serde_json::json!({
        "plan": snap.plan,
        "auto_pct": snap.auto_pct,
        "api_pct": snap.api_pct,
        "total_pct": snap.total_pct,
        "unlimited": snap.unlimited,
        "on_demand_enabled": snap.on_demand_enabled,
        "reset_at": snap.reset_at.map(|dt| dt.to_rfc3339()),
    })
}

async fn fetch_live(
    client: &reqwest::Client,
    endpoints: &Endpoints,
    db_path: &Path,
) -> Result<CursorSnapshot> {
    // Local, synchronous, and cheap — only reached on a cache miss, so a
    // running Cursor IDE isn't contending for the sqlite file every tick.
    let token = db::read_access_token(db_path)?;
    let auth = db::session_auth(&token)?;

    // usage-summary keys off the session cookie alone (no `?user=` param); the
    // browser-ish headers get past its CORS gate.
    let resp = tokio::time::timeout(
        HTTP_TIMEOUT,
        client
            .get(&endpoints.summary)
            .header(
                "Cookie",
                format!("WorkosCursorSessionToken={}", auth.cookie_value),
            )
            .header("Origin", BASE_URL)
            .header("Referer", format!("{BASE_URL}/dashboard"))
            .header("User-Agent", BROWSER_UA)
            .send(),
    )
    .await
    .map_err(|_| AppError::Transport(format!("cursor timeout: {}", endpoints.summary)))??;

    let status = resp.status();
    if !status.is_success() {
        let body = if matches!(status.as_u16(), 401 | 403) {
            "Cursor authentication failed".into()
        } else {
            format!("Cursor API returned HTTP {}", status.as_u16())
        };
        return Err(AppError::Http {
            status: status.as_u16(),
            body,
        });
    }

    let bytes = read_body_capped(resp, MAX_BODY_BYTES).await?;
    let parsed: UsageSummary = serde_json::from_slice(&bytes)
        .map_err(|e| AppError::Schema(format!("cursor usage-summary response: {e}")))?;
    types::to_snapshot(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use tempfile::TempDir;

    fn cache_fixture() -> (TempDir, Cache) {
        let td = TempDir::new().unwrap();
        let cache = Cache::at(td.path().join("cursor"));
        cache.ensure_dir().unwrap();
        (td, cache)
    }

    /// A minimal, unsigned JWT with `sub: "auth0|<user_id>"` — signature
    /// verification is never performed (see `db::parse_jwt_claims`).
    fn fake_token(user_id: &str) -> String {
        use base64::Engine;
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::json!({"sub": format!("auth0|{user_id}")}).to_string());
        format!("{header}.{payload}.sig")
    }

    fn seed_state_db(dir: &TempDir, token: &str) -> std::path::PathBuf {
        let path = dir.path().join("state.vscdb");
        let conn = Connection::open(&path).unwrap();
        conn.execute("CREATE TABLE ItemTable (key TEXT, value TEXT)", [])
            .unwrap();
        conn.execute(
            "INSERT INTO ItemTable (key, value) VALUES ('cursorAuth/accessToken', ?1)",
            [token],
        )
        .unwrap();
        path
    }

    fn sample_json() -> String {
        r#"{
            "billingCycleEnd": "2026-08-04T00:35:51.000Z",
            "membershipType": "ultra",
            "isUnlimited": false,
            "individualUsage": {
                "plan": { "autoPercentUsed": 98.109, "apiPercentUsed": 100, "totalPercentUsed": 98.5 },
                "onDemand": { "enabled": false }
            }
        }"#
        .to_string()
    }

    #[tokio::test]
    async fn live_fetch_reads_token_from_db_and_sends_the_session_cookie() {
        let mut server = mockito::Server::new_async().await;
        let token = fake_token("user_123");
        let m = server
            .mock("GET", "/api/usage-summary")
            .match_header(
                "cookie",
                format!("WorkosCursorSessionToken=user_123%3A%3A{token}").as_str(),
            )
            .with_status(200)
            .with_body(sample_json())
            .create_async()
            .await;

        let db_dir = TempDir::new().unwrap();
        let db_path = seed_state_db(&db_dir, &token);
        let (_cache_dir, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            summary: format!("{}/api/usage-summary", server.url()),
        };

        let out = fetch_snapshot(
            &client,
            &db_path,
            &cache,
            &endpoints,
            Duration::from_secs(0),
        )
        .await
        .unwrap();
        m.assert_async().await;
        assert_eq!(out.snapshot.plan, "Ultra");
        assert_eq!(out.snapshot.auto_pct, 98);
        assert_eq!(out.snapshot.api_pct, 100);
        assert!(!out.stale);
    }

    #[tokio::test]
    async fn missing_db_file_is_a_credentials_error_with_no_cache_to_fall_back_on() {
        let (_cache_dir, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let endpoints = Endpoints::default();
        let db_path = std::path::Path::new("/nonexistent/state.vscdb");

        let err = fetch_snapshot(&client, db_path, &cache, &endpoints, Duration::from_secs(0))
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Credentials(_)));
    }

    #[tokio::test]
    async fn http_error_falls_back_to_cache_and_hides_the_upstream_body() {
        let mut server = mockito::Server::new_async().await;
        let token = fake_token("user_123");
        server
            .mock("GET", "/api/usage-summary")
            .with_status(401)
            .with_body(r#"{"detail":"leaked-looking body"}"#)
            .create_async()
            .await;

        let db_dir = TempDir::new().unwrap();
        let db_path = seed_state_db(&db_dir, &token);
        let (_cache_dir, cache) = cache_fixture();
        cache
            .write_payload(
                serde_json::json!({
                    "plan": "Ultra", "auto_pct": 40, "api_pct": 10, "total_pct": 30,
                    "unlimited": false, "on_demand_enabled": false,
                    "reset_at": "2026-08-04T00:00:00Z",
                })
                .to_string()
                .as_bytes(),
            )
            .unwrap();

        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            summary: format!("{}/api/usage-summary", server.url()),
        };
        let out = fetch_snapshot(
            &client,
            &db_path,
            &cache,
            &endpoints,
            Duration::from_secs(0),
        )
        .await
        .unwrap();
        assert!(out.stale);
        assert_eq!(out.snapshot.auto_pct, 40);
        let (code, msg) = out.last_error.unwrap();
        assert_eq!(code, 401);
        assert_eq!(msg, "Cursor authentication failed");
        assert!(!msg.contains("leaked-looking"));
    }

    #[tokio::test]
    async fn fresh_cache_short_circuits_without_touching_the_db() {
        let (_cache_dir, cache) = cache_fixture();
        cache
            .write_payload(
                serde_json::json!({
                    "plan": "Pro", "auto_pct": 7, "api_pct": 3, "total_pct": 5,
                    "unlimited": false, "on_demand_enabled": true,
                    "reset_at": "2026-08-04T00:00:00Z",
                })
                .to_string()
                .as_bytes(),
            )
            .unwrap();

        let client = reqwest::Client::new();
        let endpoints = Endpoints::default();
        // A path that doesn't exist would error if the db were read — proving
        // the fresh-cache path never touches it.
        let db_path = std::path::Path::new("/nonexistent/state.vscdb");
        let out = fetch_snapshot(
            &client,
            db_path,
            &cache,
            &endpoints,
            Duration::from_secs(3600),
        )
        .await
        .unwrap();
        assert_eq!(out.snapshot.auto_pct, 7);
        assert!(out.snapshot.on_demand_enabled);
        assert!(!out.stale);
    }
}
