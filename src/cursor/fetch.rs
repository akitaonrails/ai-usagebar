//! Fetch Cursor's included-usage summary from `GET /api/usage-summary`,
//! authenticated with the session token read out of the local `state.vscdb`
//! (see `db.rs`). Cache/stale/error-fallback shape mirrors `kimi::fetch`.

use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::cache::{Cache, acquire_lock_async};
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

/// This vendor's [`Outcome`](crate::outcome::Outcome) — the shared shape,
/// specialised to its snapshot.
pub type FetchOutcome = crate::outcome::Outcome<CursorSnapshot>;

/// Cache-aware fetch. `db_path` is Cursor's `state.vscdb` — the caller resolves
/// `[cursor] db_path` (config override) vs [`db::default_db_path`], the same
/// override pattern as `openai.codex_auth_path`. `agent_auth_path` is the
/// headless `cursor-agent` CLI's own `auth.json`, tried when `db_path` is
/// missing — see `db::resolve_access_token`.
pub async fn fetch_snapshot(
    client: &reqwest::Client,
    db_path: &Path,
    agent_auth_path: &Path,
    cache: &Cache,
    endpoints: &Endpoints,
    cache_ttl: Duration,
) -> Result<FetchOutcome> {
    fetch_snapshot_at(
        client,
        db_path,
        agent_auth_path,
        cache,
        endpoints,
        cache_ttl,
        Utc::now(),
    )
    .await
}

/// Clock seam for cache rollover tests. Cursor's payload describes one billing
/// cycle, so serving it after `reset_at` would knowingly show the prior cycle.
async fn fetch_snapshot_at(
    client: &reqwest::Client,
    db_path: &Path,
    agent_auth_path: &Path,
    cache: &Cache,
    endpoints: &Endpoints,
    cache_ttl: Duration,
    now: DateTime<Utc>,
) -> Result<FetchOutcome> {
    cache.ensure_dir()?;
    let _lock = acquire_lock_async(&cache.lock_path(), LOCK_TIMEOUT).await?;

    // Resolve the local identity before accepting a cache hit. Cursor can switch
    // accounts in-place in this database; returning the cache first would show
    // the previous account's private usage until the TTL elapsed.
    let token = db::resolve_access_token(db_path, agent_auth_path)?;
    let auth = db::session_auth(&token)?;

    if let Some(bytes) = cache.fresh_payload(cache_ttl)?
        && let Ok(outcome) = reuse_cache(&bytes, cache, false, &auth.account_key, now)
    {
        return Ok(outcome);
    }

    match fetch_live(client, endpoints, &auth).await {
        Ok(snap) => {
            let bytes = serde_json::to_vec(&snap_to_json(&snap, &auth.account_key))?;
            cache.write_payload(&bytes)?;
            Ok(crate::outcome::Outcome::fresh(snap))
        }
        Err(e) if e.is_transient() => fallback_silent(cache, &auth.account_key, now, e),
        Err(e) => {
            cache.mark_stale();
            if let Some((code, msg)) = error_to_pair(&e) {
                cache.write_last_error(code, &msg);
            }
            fallback_with_error(cache, &auth.account_key, now, e)
        }
    }
}

fn fallback_silent(
    cache: &Cache,
    account: &str,
    now: DateTime<Utc>,
    original: AppError,
) -> Result<FetchOutcome> {
    crate::outcome::fallback(cache, None, original, |bytes| {
        parse_cache_at(bytes, account, now)
    })
}

fn fallback_with_error(
    cache: &Cache,
    account: &str,
    now: DateTime<Utc>,
    original: AppError,
) -> Result<FetchOutcome> {
    let last_error = error_to_pair(&original);
    crate::outcome::fallback(cache, last_error, original, |bytes| {
        parse_cache_at(bytes, account, now)
    })
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

fn reuse_cache(
    bytes: &[u8],
    cache: &Cache,
    stale: bool,
    account: &str,
    now: DateTime<Utc>,
) -> Result<FetchOutcome> {
    let snap = parse_cache_at(bytes, account, now)?;
    Ok(crate::outcome::Outcome::cached(snap, cache, stale))
}

fn parse_cache_at(bytes: &[u8], account: &str, now: DateTime<Utc>) -> Result<CursorSnapshot> {
    let v: serde_json::Value = serde_json::from_slice(bytes)?;
    if v.get("account").and_then(serde_json::Value::as_str) != Some(account) {
        return Err(AppError::Schema(
            "cursor cache belongs to a different account; refetching".into(),
        ));
    }
    let int = |key: &str| -> Result<i32> {
        v[key]
            .as_i64()
            .filter(|n| *n >= 0)
            .and_then(|n| i32::try_from(n).ok())
            .ok_or_else(|| AppError::Schema(format!("cursor cache: invalid {key}")))
    };
    let plan = v["plan"]
        .as_str()
        .filter(|plan| !plan.trim().is_empty())
        .ok_or_else(|| AppError::Schema("cursor cache: invalid plan".into()))?
        .to_string();
    let reset_at = parse_cache_datetime(&v["reset_at"])?
        .ok_or_else(|| AppError::Schema("cursor cache: missing reset timestamp".into()))?;
    if reset_at <= now {
        return Err(AppError::Schema(
            "cursor cache is past its billing-cycle reset; refetching".into(),
        ));
    }
    Ok(CursorSnapshot {
        plan,
        auto_pct: int("auto_pct")?,
        api_pct: int("api_pct")?,
        total_pct: int("total_pct")?,
        unlimited: v["unlimited"]
            .as_bool()
            .ok_or_else(|| AppError::Schema("cursor cache: invalid unlimited flag".into()))?,
        on_demand_enabled: v["on_demand_enabled"]
            .as_bool()
            .ok_or_else(|| AppError::Schema("cursor cache: invalid on-demand flag".into()))?,
        reset_at: Some(reset_at),
        cycle_start: v
            .get("cycle_start")
            .and_then(|c| parse_cache_datetime(c).ok())
            .flatten(),
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

fn snap_to_json(snap: &CursorSnapshot, account: &str) -> serde_json::Value {
    serde_json::json!({
        "account": account,
        "plan": snap.plan,
        "auto_pct": snap.auto_pct,
        "api_pct": snap.api_pct,
        "total_pct": snap.total_pct,
        "unlimited": snap.unlimited,
        "on_demand_enabled": snap.on_demand_enabled,
        "reset_at": snap.reset_at.map(|dt| dt.to_rfc3339()),
        "cycle_start": snap.cycle_start.map(|dt| dt.to_rfc3339()),
    })
}

async fn fetch_live(
    client: &reqwest::Client,
    endpoints: &Endpoints,
    auth: &db::SessionAuth,
) -> Result<CursorSnapshot> {
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

    fn account_key(token: &str) -> String {
        db::session_auth(token).unwrap().account_key
    }

    /// A path that never exists, for tests that only care about the IDE
    /// `db_path` and want the agent fallback to stay out of the way.
    fn no_agent_auth() -> std::path::PathBuf {
        std::path::PathBuf::from("/nonexistent/cursor-agent-auth.json")
    }

    fn cached_snapshot(account: &str, reset_at: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "account": account,
            "plan": "Ultra",
            "auto_pct": 40,
            "api_pct": 10,
            "total_pct": 30,
            "unlimited": false,
            "on_demand_enabled": false,
            "reset_at": reset_at,
        }))
        .unwrap()
    }

    fn sample_json() -> String {
        r#"{
            "billingCycleEnd": "2099-08-04T00:35:51.000Z",
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
            &no_agent_auth(),
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

        let err = fetch_snapshot(
            &client,
            db_path,
            &no_agent_auth(),
            &cache,
            &endpoints,
            Duration::from_secs(0),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, AppError::Credentials(_)));
    }

    #[tokio::test]
    async fn agent_auth_file_is_used_when_the_ide_db_is_missing() {
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

        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("state.vscdb"); // deliberately never seeded
        let agent_path = dir.path().join("auth.json");
        std::fs::write(
            &agent_path,
            serde_json::json!({"accessToken": token, "refreshToken": "r"}).to_string(),
        )
        .unwrap();
        let (_cache_dir, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            summary: format!("{}/api/usage-summary", server.url()),
        };

        let out = fetch_snapshot(
            &client,
            &db_path,
            &agent_path,
            &cache,
            &endpoints,
            Duration::from_secs(0),
        )
        .await
        .unwrap();
        m.assert_async().await;
        assert_eq!(out.snapshot.plan, "Ultra");
        assert!(!out.stale);
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
            .write_payload(&cached_snapshot(
                &account_key(&token),
                "2099-08-04T00:00:00Z",
            ))
            .unwrap();

        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            summary: format!("{}/api/usage-summary", server.url()),
        };
        let out = fetch_snapshot(
            &client,
            &db_path,
            &no_agent_auth(),
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
    async fn fresh_cache_is_used_after_verifying_the_current_account() {
        let token = fake_token("user_123");
        let db_dir = TempDir::new().unwrap();
        let db_path = seed_state_db(&db_dir, &token);
        let (_cache_dir, cache) = cache_fixture();
        cache
            .write_payload(
                serde_json::json!({
                    "account": account_key(&token),
                    "plan": "Pro", "auto_pct": 7, "api_pct": 3, "total_pct": 5,
                    "unlimited": false, "on_demand_enabled": true,
                    "reset_at": "2099-08-04T00:00:00Z",
                })
                .to_string()
                .as_bytes(),
            )
            .unwrap();

        let client = reqwest::Client::new();
        let endpoints = Endpoints::default();
        let out = fetch_snapshot(
            &client,
            &db_path,
            &no_agent_auth(),
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

    #[tokio::test]
    async fn switching_accounts_rejects_a_fresh_cache_and_refetches() {
        let old_token = fake_token("old_account");
        let new_token = fake_token("new_account");
        let db_dir = TempDir::new().unwrap();
        let db_path = seed_state_db(&db_dir, &new_token);
        let (_cache_dir, cache) = cache_fixture();
        cache
            .write_payload(&cached_snapshot(
                &account_key(&old_token),
                "2099-08-04T00:00:00Z",
            ))
            .unwrap();

        let mut server = mockito::Server::new_async().await;
        let request = server
            .mock("GET", "/api/usage-summary")
            .match_header(
                "cookie",
                format!("WorkosCursorSessionToken=new_account%3A%3A{new_token}").as_str(),
            )
            .with_status(200)
            .with_body(sample_json())
            .expect(1)
            .create_async()
            .await;
        let endpoints = Endpoints {
            summary: format!("{}/api/usage-summary", server.url()),
        };

        let out = fetch_snapshot(
            &reqwest::Client::new(),
            &db_path,
            &no_agent_auth(),
            &cache,
            &endpoints,
            Duration::from_secs(3600),
        )
        .await
        .unwrap();
        request.assert_async().await;
        assert_eq!(out.snapshot.auto_pct, 98);
        assert!(!out.stale);
    }

    #[tokio::test]
    async fn cache_past_its_billing_reset_is_not_served_during_an_outage() {
        let token = fake_token("user_123");
        let db_dir = TempDir::new().unwrap();
        let db_path = seed_state_db(&db_dir, &token);
        let (_cache_dir, cache) = cache_fixture();
        cache
            .write_payload(&cached_snapshot(
                &account_key(&token),
                "2026-08-04T00:00:00Z",
            ))
            .unwrap();

        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/api/usage-summary")
            .with_status(503)
            .create_async()
            .await;
        let endpoints = Endpoints {
            summary: format!("{}/api/usage-summary", server.url()),
        };
        let now = DateTime::parse_from_rfc3339("2026-08-05T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let err = fetch_snapshot_at(
            &reqwest::Client::new(),
            &db_path,
            &no_agent_auth(),
            &cache,
            &endpoints,
            Duration::from_secs(0),
            now,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, AppError::Http { status: 503, .. }));
    }

    #[test]
    fn cached_percentages_are_range_checked_before_narrowing() {
        let now = DateTime::parse_from_rfc3339("2026-08-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut payload: serde_json::Value =
            serde_json::from_slice(&cached_snapshot("account", "2026-08-04T00:00:00Z")).unwrap();
        payload["auto_pct"] = serde_json::json!(i64::MAX);
        let err =
            parse_cache_at(&serde_json::to_vec(&payload).unwrap(), "account", now).unwrap_err();
        assert!(matches!(err, AppError::Schema(_)));
    }
}
