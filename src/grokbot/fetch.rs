//! Fetch Grok Bot usage from
//! `aiserver.v1.DashboardService/GetSandUsageStatus` (Connect-RPC), with the
//! desktop app's own OAuth session (`creds.rs`). The session's access token
//! has no locally recorded expiry, so the flow is: try it, and on a 401/403
//! refresh through Cursor's public OAuth client, persist the rotated pair in
//! ai-usagebar's vendor cache (`oauth.json`, scoped by a fingerprint of the
//! refresh token, mode 0600 — never back to the app's file), and retry once.
//! Cache/stale/error-fallback shape mirrors `kiro::fetch`.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::cache::{Cache, acquire_lock_async, atomic_write};
use crate::error::{AppError, Result};
use crate::usage::GrokbotSnapshot;
use crate::vendor::{MAX_BODY_BYTES, read_body_capped};

use super::creds::GrokbotCredentials;
use super::types::SandUsageStatus;

const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const REFRESH_TIMEOUT: Duration = Duration::from_secs(15);
const LOCK_TIMEOUT: Duration = Duration::from_secs(15);
const OAUTH_CACHE_FILE: &str = "oauth.json";

/// The public OAuth client id the Cursor CLI itself uses for its installed-app
/// flow — public by definition (it ships in the CLI), embedded here for the
/// same reason the Kimi Code CLI's is.
pub const CLIENT_ID: &str = "KbZUR41cY7W6zRSdpSUJ7I7mLYBKOCmB";

#[derive(Debug, Clone)]
pub struct Endpoints {
    /// `GetSandUsageStatus`.
    pub usage: String,
    /// OAuth token endpoint for the refresh grant.
    pub token: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            usage: "https://api2.cursor.sh/aiserver.v1.DashboardService/GetSandUsageStatus".into(),
            token: "https://api2.cursor.sh/oauth/token".into(),
        }
    }
}

/// This vendor's [`Outcome`](crate::outcome::Outcome) — the shared shape,
/// specialised to its snapshot.
pub type FetchOutcome = crate::outcome::Outcome<GrokbotSnapshot>;

/// The rotated token pair, persisted in the vendor cache. Keyed by a
/// fingerprint of the refresh token, so a re-login in the app (which rewrites
/// `sand-secrets.json` with a new pair) never inherits the old session's
/// cached tokens — the kiro/minimax treatment.
#[derive(Debug, Clone, Deserialize, Serialize)]
struct PersistedOAuth {
    fingerprint: String,
    access_token: String,
    refresh_token: String,
}

/// Production entry: resolve the desktop app's session, then fetch.
pub async fn fetch_snapshot(
    client: &reqwest::Client,
    cfg: &crate::config::GrokbotConfig,
    cache: &Cache,
    cache_ttl: Duration,
) -> Result<FetchOutcome> {
    let creds = super::resolve_credentials(cfg)?;
    fetch_snapshot_with(client, &creds, cache, &Endpoints::default(), cache_ttl).await
}

/// Fetch with the credential and endpoints resolved by the caller.
pub async fn fetch_snapshot_with(
    client: &reqwest::Client,
    creds: &GrokbotCredentials,
    cache: &Cache,
    endpoints: &Endpoints,
    cache_ttl: Duration,
) -> Result<FetchOutcome> {
    cache.ensure_dir()?;
    let _lock = acquire_lock_async(&cache.lock_path(), LOCK_TIMEOUT).await?;

    // The cache is scoped to the sign-in: a re-login in the app must not keep
    // serving the previous session's payload.
    if let Some(bytes) = cache.fresh_payload(cache_ttl)?
        && let Ok(outcome) = reuse_cache(&bytes, cache, false, &creds.fingerprint)
    {
        return Ok(outcome);
    }

    match fetch_live(client, endpoints, cache, creds).await {
        Ok(snap) => {
            let bytes = serde_json::to_vec(&snap_to_json(&snap, &creds.fingerprint))?;
            cache.write_payload(&bytes)?;
            Ok(crate::outcome::Outcome::fresh(snap))
        }
        Err(e) if e.is_transient() => fallback_silent(cache, &creds.fingerprint, e),
        Err(e) => {
            cache.mark_stale();
            if let Some((code, msg)) = error_to_pair(&e) {
                cache.write_last_error(code, &msg);
            }
            fallback_with_error(cache, &creds.fingerprint, e)
        }
    }
}

fn fallback_silent(cache: &Cache, fingerprint: &str, original: AppError) -> Result<FetchOutcome> {
    crate::outcome::fallback(cache, None, original, |bytes| {
        parse_cache_at(bytes, fingerprint)
    })
}

fn fallback_with_error(
    cache: &Cache,
    fingerprint: &str,
    original: AppError,
) -> Result<FetchOutcome> {
    let last_error = error_to_pair(&original);
    crate::outcome::fallback(cache, last_error, original, |bytes| {
        parse_cache_at(bytes, fingerprint)
    })
}

/// Never surface upstream bodies for auth failures: a 401/403 from a
/// Bearer-token endpoint is not guaranteed not to echo something
/// account-identifying back. Mirrors `kiro::fetch::error_to_pair`.
fn error_to_pair(e: &AppError) -> Option<(u16, String)> {
    match e {
        AppError::Http { status, .. } if matches!(status, 401 | 403) => {
            Some((*status, "Grok Bot authentication failed".into()))
        }
        AppError::Http { status, body } => Some((*status, body.clone())),
        AppError::Credentials(msg) => Some((0, msg.clone())),
        e => Some((0, e.to_string())),
    }
}

fn reuse_cache(
    bytes: &[u8],
    cache: &Cache,
    stale: bool,
    fingerprint: &str,
) -> Result<FetchOutcome> {
    let snap = parse_cache_at(bytes, fingerprint)?;
    Ok(crate::outcome::Outcome::cached(snap, cache, stale))
}

fn parse_cache_at(bytes: &[u8], fingerprint: &str) -> Result<GrokbotSnapshot> {
    let v: serde_json::Value = serde_json::from_slice(bytes)?;
    if v.get("account").and_then(serde_json::Value::as_str) != Some(fingerprint) {
        return Err(AppError::Schema(
            "grokbot cache belongs to a different sign-in; refetching".into(),
        ));
    }
    let invalid = |field: &str| AppError::Schema(format!("grokbot cache: invalid {field}"));
    let plan = v["plan"]
        .as_str()
        .filter(|plan| !plan.trim().is_empty())
        .ok_or_else(|| invalid("plan"))?
        .to_string();
    let has_included_allowance = v["has_included_allowance"]
        .as_bool()
        .ok_or_else(|| invalid("has_included_allowance"))?;
    let weekly_pct = v["weekly_pct"]
        .as_i64()
        .filter(|pct| (0..=100).contains(pct))
        .ok_or_else(|| invalid("weekly_pct"))? as i32;
    let period_start = parse_cache_datetime(&v["period_start"])?;
    let reset_at = parse_cache_datetime(&v["reset_at"])?;
    let window = match (period_start, reset_at) {
        (Some(start), Some(reset)) if reset > start => Some(reset - start),
        _ => None,
    };
    Ok(GrokbotSnapshot {
        plan,
        has_included_allowance,
        weekly_pct,
        has_available_usage: v["has_available_usage"].as_bool().unwrap_or(false),
        on_demand_enabled: v["on_demand_enabled"].as_bool().unwrap_or(false),
        period_start,
        reset_at,
        window,
    })
}

fn parse_cache_datetime(v: &serde_json::Value) -> Result<Option<DateTime<Utc>>> {
    match v {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(s) => DateTime::parse_from_rfc3339(s)
            .map(|dt| Some(dt.with_timezone(&Utc)))
            .map_err(|e| AppError::Schema(format!("grokbot cache: invalid timestamp: {e}"))),
        _ => Err(AppError::Schema("grokbot cache: invalid timestamp".into())),
    }
}

fn snap_to_json(snap: &GrokbotSnapshot, fingerprint: &str) -> serde_json::Value {
    serde_json::json!({
        "account": fingerprint,
        "plan": snap.plan,
        "has_included_allowance": snap.has_included_allowance,
        "weekly_pct": snap.weekly_pct,
        "has_available_usage": snap.has_available_usage,
        "on_demand_enabled": snap.on_demand_enabled,
        "period_start": snap.period_start.map(|dt| dt.to_rfc3339()),
        "reset_at": snap.reset_at.map(|dt| dt.to_rfc3339()),
    })
}

fn oauth_cache_path(cache: &Cache) -> std::path::PathBuf {
    cache.dir().join(OAUTH_CACHE_FILE)
}

fn read_persisted_oauth(cache: &Cache, fingerprint: &str) -> Result<Option<PersistedOAuth>> {
    let path = oauth_cache_path(cache);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(AppError::io_at(&path, e)),
    };
    let persisted: PersistedOAuth = serde_json::from_slice(&bytes).map_err(|e| {
        AppError::Credentials(format!(
            "ai-usagebar's cached Grok Bot credentials at {} are malformed ({e}); remove that file and try again",
            crate::display::sanitize_untrusted_path(&path)
        ))
    })?;
    // The app re-signed-in since this was persisted: a different refresh
    // token means a different session, and the cached pair belongs to it.
    if persisted.fingerprint != fingerprint {
        return Ok(None);
    }
    if persisted.access_token.trim().is_empty() || persisted.refresh_token.trim().is_empty() {
        return Err(AppError::Credentials(format!(
            "ai-usagebar's cached Grok Bot credentials at {} are incomplete; remove that file and try again",
            crate::display::sanitize_untrusted_path(&path)
        )));
    }
    Ok(Some(persisted))
}

fn write_persisted_oauth(cache: &Cache, persisted: &PersistedOAuth) -> Result<()> {
    let path = oauth_cache_path(cache);
    let bytes = serde_json::to_vec_pretty(persisted)?;
    atomic_write(&path, &bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| AppError::io_at(&path, e))?;
    }
    Ok(())
}

async fn fetch_live(
    client: &reqwest::Client,
    endpoints: &Endpoints,
    cache: &Cache,
    creds: &GrokbotCredentials,
) -> Result<GrokbotSnapshot> {
    // A previously rotated pair wins over the app's file when it belongs to
    // the same sign-in: the app does not rewrite its own file on refresh, so
    // its access token is the older one.
    let (access_token, refresh_token) = match read_persisted_oauth(cache, &creds.fingerprint)? {
        Some(persisted) => (persisted.access_token, persisted.refresh_token),
        None => (creds.access_token.clone(), creds.refresh_token.clone()),
    };

    let resp = usage_call(client, endpoints, &access_token).await?;
    let status = resp.status();
    if matches!(status.as_u16(), 401 | 403) {
        // The access token died (no local expiry records when). Refresh with
        // the paired refresh token and retry exactly once.
        let refreshed = refresh(client, &endpoints.token, &refresh_token).await?;
        let persisted = PersistedOAuth {
            fingerprint: super::creds::fingerprint_of(&refreshed.refresh_token),
            access_token: refreshed.access_token,
            refresh_token: refreshed.refresh_token,
        };
        write_persisted_oauth(cache, &persisted).map_err(|e| {
            AppError::Credentials(format!(
                "the refreshed Grok Bot credentials could not be saved ({e}); sign in to the Grok Bot desktop app again if the refresh token was rotated"
            ))
        })?;
        let retry = usage_call(client, endpoints, &persisted.access_token).await?;
        return parse_usage_response(retry).await;
    }
    parse_usage_response(resp).await
}

/// One `GetSandUsageStatus` call: Connect-RPC with the app's client headers.
///
/// `x-cursor-checksum` is deliberately **not** sent: it is Cursor's machine
/// checksum, which we cannot reproduce. Residual risk: the server could start
/// requiring it, which would surface as a 4xx and the cached payload (or its
/// absence) is the honest answer — flagged for the reporter's live test
/// (#206).
async fn usage_call(
    client: &reqwest::Client,
    endpoints: &Endpoints,
    access_token: &str,
) -> Result<reqwest::Response> {
    tokio::time::timeout(
        HTTP_TIMEOUT,
        client
            .post(&endpoints.usage)
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {access_token}"))
            .header("Connect-Protocol-Version", "1")
            .header("x-cursor-client-type", "sand")
            .header("x-cursor-client-version", "0.1.0")
            .header("x-sand-box-namespace", "prod")
            .header("x-ghost-mode", "true")
            .header("x-request-id", request_id())
            .body("{}")
            .send(),
    )
    .await
    .map_err(|_| AppError::Transport(format!("grokbot timeout: {}", endpoints.usage)))?
    .map_err(|e| AppError::Transport(format!("grokbot transport: {e}")))
}

async fn parse_usage_response(resp: reqwest::Response) -> Result<GrokbotSnapshot> {
    let status = resp.status();
    if !status.is_success() {
        // Never surface upstream/proxy bodies: they can contain credentials
        // or arbitrary markup. The cache records the redacted form centrally.
        let body = if matches!(status.as_u16(), 401 | 403) {
            "Grok Bot authentication failed".into()
        } else {
            format!("Grok Bot API returned HTTP {}", status.as_u16())
        };
        return Err(AppError::Http {
            status: status.as_u16(),
            body,
        });
    }
    let bytes = read_body_capped(resp, MAX_BODY_BYTES).await?;
    let parsed: SandUsageStatus = serde_json::from_slice(&bytes)
        .map_err(|e| AppError::Schema(format!("grokbot usage response: {e}")))?;
    parsed.into_snapshot()
}

#[derive(Debug, Deserialize)]
struct RefreshResponse {
    access_token: String,
    /// Absent means the refresh token was not rotated; keep the current one.
    refresh_token: Option<String>,
}

async fn refresh(
    client: &reqwest::Client,
    token_url: &str,
    refresh_token: &str,
) -> Result<RefreshedPair> {
    let body = serde_json::json!({
        "client_id": CLIENT_ID,
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
    });
    let resp = tokio::time::timeout(REFRESH_TIMEOUT, client.post(token_url).json(&body).send())
        .await
        .map_err(|_| AppError::Transport(format!("grokbot token refresh timeout: {token_url}")))?
        .map_err(|e| AppError::Transport(format!("grokbot token refresh: {e}")))?;
    if !resp.status().is_success() {
        return Err(AppError::Credentials(
            "Grok Bot token refresh was rejected; sign in to the Grok Bot desktop app again".into(),
        ));
    }
    let bytes = read_body_capped(resp, MAX_BODY_BYTES).await?;
    let parsed: RefreshResponse = serde_json::from_slice(&bytes)
        .map_err(|e| AppError::Schema(format!("grokbot token refresh response: {e}")))?;
    if parsed.access_token.trim().is_empty() {
        return Err(AppError::Schema(
            "grokbot token refresh returned no access token".into(),
        ));
    }
    Ok(RefreshedPair {
        access_token: parsed.access_token,
        refresh_token: parsed
            .refresh_token
            .filter(|token| !token.trim().is_empty())
            .unwrap_or_else(|| refresh_token.to_string()),
    })
}

struct RefreshedPair {
    access_token: String,
    refresh_token: String,
}

/// A UUID-v4-shaped request id for the `x-request-id` header. `uuid` is
/// deliberately not a dependency: a Connect-RPC request id is a tracing
/// value, not a credential, so one is minted from process/time/counter
/// entropy mixed through SHA-256 (already in the tree) and shaped per
/// RFC 4122 §4.4.
fn request_id() -> String {
    use sha2::{Digest, Sha256};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let digest = Sha256::digest(format!("grokbot:{}:{nanos}:{seq}", std::process::id()).as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // variant 10
    let mut out = String::with_capacity(36);
    for (i, byte) in bytes.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            out.push('-');
        }
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn cache_fixture() -> (TempDir, Cache) {
        let td = TempDir::new().unwrap();
        let cache = Cache::at(td.path().join("grokbot"));
        cache.ensure_dir().unwrap();
        (td, cache)
    }

    fn test_endpoints(base: &str) -> Endpoints {
        Endpoints {
            usage: format!("{base}/aiserver.v1.DashboardService/GetSandUsageStatus"),
            token: format!("{base}/oauth/token"),
        }
    }

    fn test_creds() -> GrokbotCredentials {
        GrokbotCredentials {
            access_token: "at-stored".into(),
            refresh_token: "rt-stored".into(),
            fingerprint: super::super::creds::fingerprint_of("rt-stored"),
        }
    }

    /// The reporter's capture, lightly trimmed.
    fn usage_json() -> &'static str {
        r#"{"currentPeriodStart":"2026-09-11T18:43:19.645Z","nextResetTimestampUtc":"2026-09-18T18:43:19.645Z","usagePercent":12,"hasAvailableUsage":true,"hasNonZeroIncludedLimit":true,"onDemandSettings":{"visible":true,"eligible":true,"enabled":false},"grokPlanLabel":"Grok Bot Plan","cursorPlanName":"Pro"}"#
    }

    fn sample_seed(fingerprint: &str) -> serde_json::Value {
        serde_json::json!({
            "account": fingerprint,
            "plan": "Grok Bot Plan",
            "has_included_allowance": true,
            "weekly_pct": 30,
            "has_available_usage": true,
            "on_demand_enabled": false,
            "period_start": "2026-09-11T18:43:19.645Z",
            "reset_at": "2026-09-18T18:43:19.645Z",
        })
    }

    /// The mock usage endpoint, matching every header the client must send.
    fn usage_mock(server: &mut mockito::ServerGuard, bearer: &str) -> mockito::Mock {
        server
            .mock("POST", "/aiserver.v1.DashboardService/GetSandUsageStatus")
            .match_header("authorization", format!("Bearer {bearer}").as_str())
            .match_header("content-type", "application/json")
            .match_header("connect-protocol-version", "1")
            .match_header("x-cursor-client-type", "sand")
            .match_header("x-cursor-client-version", "0.1.0")
            .match_header("x-sand-box-namespace", "prod")
            .match_header("x-ghost-mode", "true")
            .match_header(
                "x-request-id",
                mockito::Matcher::Regex(r"^[0-9a-f-]{36}$".into()),
            )
            // We cannot reproduce Cursor's machine checksum; it must not go out.
            .match_header("x-cursor-checksum", mockito::Matcher::Missing)
            .match_body(mockito::Matcher::Json(serde_json::json!({})))
    }

    #[tokio::test]
    async fn live_200_returns_a_snapshot_and_sends_the_connect_rpc_headers() {
        let mut server = mockito::Server::new_async().await;
        let m = usage_mock(&mut server, "at-stored")
            .with_status(200)
            .with_body(usage_json())
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let out = fetch_snapshot_with(
            &reqwest::Client::new(),
            &test_creds(),
            &cache,
            &test_endpoints(&server.url()),
            Duration::ZERO,
        )
        .await
        .unwrap();

        m.assert_async().await;
        assert_eq!(out.snapshot.plan, "Grok Bot Plan");
        assert!(out.snapshot.has_included_allowance);
        assert_eq!(out.snapshot.weekly_pct, 12);
        assert_eq!(out.snapshot.window, Some(chrono::Duration::days(7)));
        assert!(!out.stale);
    }

    #[tokio::test]
    async fn a_401_refreshes_persists_and_retries_once() {
        let mut server = mockito::Server::new_async().await;
        let stale = usage_mock(&mut server, "at-stored")
            .with_status(401)
            .with_body(r#"{"error":"expired"}"#)
            .expect(1)
            .create_async()
            .await;
        let refresh = server
            .mock("POST", "/oauth/token")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "client_id": CLIENT_ID,
                "grant_type": "refresh_token",
                "refresh_token": "rt-stored",
            })))
            .with_status(200)
            .with_body(
                r#"{"access_token":"at-fresh","refresh_token":"rt-rotated","expires_in":3600}"#,
            )
            .expect(1)
            .create_async()
            .await;
        let retried = usage_mock(&mut server, "at-fresh")
            .with_status(200)
            .with_body(usage_json())
            .expect(1)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let out = fetch_snapshot_with(
            &reqwest::Client::new(),
            &test_creds(),
            &cache,
            &test_endpoints(&server.url()),
            Duration::ZERO,
        )
        .await
        .unwrap();

        stale.assert_async().await;
        refresh.assert_async().await;
        retried.assert_async().await;
        assert_eq!(out.snapshot.weekly_pct, 12);

        // The rotated pair persisted to the vendor cache, scoped by the new
        // refresh token's fingerprint — never back to the app's file.
        let persisted: serde_json::Value =
            serde_json::from_slice(&std::fs::read(oauth_cache_path(&cache)).unwrap()).unwrap();
        assert_eq!(persisted["access_token"], "at-fresh");
        assert_eq!(persisted["refresh_token"], "rt-rotated");
        assert_eq!(
            persisted["fingerprint"],
            super::super::creds::fingerprint_of("rt-rotated")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(oauth_cache_path(&cache))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0);
        }
    }

    #[tokio::test]
    async fn a_persisted_pair_is_used_in_place_of_the_apps_older_access_token() {
        let mut server = mockito::Server::new_async().await;
        let m = usage_mock(&mut server, "at-fresh")
            .with_status(200)
            .with_body(usage_json())
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        write_persisted_oauth(
            &cache,
            &PersistedOAuth {
                // Same sign-in as the app's file, so the rotation is honored.
                fingerprint: super::super::creds::fingerprint_of("rt-stored"),
                access_token: "at-fresh".into(),
                refresh_token: "rt-stored".into(),
            },
        )
        .unwrap();

        let out = fetch_snapshot_with(
            &reqwest::Client::new(),
            &test_creds(),
            &cache,
            &test_endpoints(&server.url()),
            Duration::ZERO,
        )
        .await
        .unwrap();

        m.assert_async().await;
        assert_eq!(out.snapshot.weekly_pct, 12);
    }

    #[tokio::test]
    async fn a_persisted_pair_from_a_previous_sign_in_is_ignored() {
        let mut server = mockito::Server::new_async().await;
        let m = usage_mock(&mut server, "at-stored")
            .with_status(200)
            .with_body(usage_json())
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        write_persisted_oauth(
            &cache,
            &PersistedOAuth {
                fingerprint: super::super::creds::fingerprint_of("rt-someone-else"),
                access_token: "at-stranger".into(),
                refresh_token: "rt-stranger".into(),
            },
        )
        .unwrap();

        let out = fetch_snapshot_with(
            &reqwest::Client::new(),
            &test_creds(),
            &cache,
            &test_endpoints(&server.url()),
            Duration::ZERO,
        )
        .await
        .unwrap();

        m.assert_async().await;
        assert_eq!(out.snapshot.weekly_pct, 12);
    }

    #[tokio::test]
    async fn a_rejected_refresh_is_a_credentials_error_naming_the_app() {
        let mut server = mockito::Server::new_async().await;
        usage_mock(&mut server, "at-stored")
            .with_status(401)
            .create_async()
            .await;
        server
            .mock("POST", "/oauth/token")
            .with_status(400)
            .with_body(r#"{"error":"invalid_grant"}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let err = fetch_snapshot_with(
            &reqwest::Client::new(),
            &test_creds(),
            &cache,
            &test_endpoints(&server.url()),
            Duration::ZERO,
        )
        .await
        .unwrap_err();

        assert!(matches!(err, AppError::Credentials(_)), "{err:?}");
        assert!(
            err.to_string()
                .contains("sign in to the Grok Bot desktop app"),
            "{err}"
        );
        // The rejected pair must not persist.
        assert!(!oauth_cache_path(&cache).exists());
    }

    #[tokio::test]
    async fn http_500_falls_back_to_the_cache_with_a_redacted_body() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/aiserver.v1.DashboardService/GetSandUsageStatus")
            .with_status(500)
            .with_body("proxy secret: <token>")
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        cache
            .write_payload(
                sample_seed(&test_creds().fingerprint)
                    .to_string()
                    .as_bytes(),
            )
            .unwrap();

        let out = fetch_snapshot_with(
            &reqwest::Client::new(),
            &test_creds(),
            &cache,
            &test_endpoints(&server.url()),
            Duration::ZERO,
        )
        .await
        .unwrap();

        assert!(out.stale);
        assert_eq!(out.snapshot.weekly_pct, 30);
        assert_eq!(
            out.last_error,
            Some((500, "Grok Bot API returned HTTP 500".into()))
        );
    }

    #[tokio::test]
    async fn http_401_after_refresh_also_falls_back_with_a_redacted_body() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/aiserver.v1.DashboardService/GetSandUsageStatus")
            .with_status(403)
            .with_body(r#"{"error":"the access token itself, echoed"}"#)
            .create_async()
            .await;
        server
            .mock("POST", "/oauth/token")
            .with_status(200)
            .with_body(r#"{"access_token":"at-fresh","refresh_token":"rt-rotated"}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let err = fetch_snapshot_with(
            &reqwest::Client::new(),
            &test_creds(),
            &cache,
            &test_endpoints(&server.url()),
            Duration::ZERO,
        )
        .await
        .unwrap_err();

        match err {
            AppError::Http { status, body } => {
                assert_eq!(status, 403);
                assert_eq!(body, "Grok Bot authentication failed");
            }
            other => panic!("expected Http 403, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_fresh_cache_is_served_without_a_network_call() {
        let (_td, cache) = cache_fixture();
        cache
            .write_payload(
                sample_seed(&test_creds().fingerprint)
                    .to_string()
                    .as_bytes(),
            )
            .unwrap();

        // No mock server at all: a cache hit must never reach the network.
        let out = fetch_snapshot_with(
            &reqwest::Client::new(),
            &test_creds(),
            &cache,
            &test_endpoints("http://127.0.0.1:1"),
            Duration::from_secs(60),
        )
        .await
        .unwrap();

        assert!(!out.stale);
        assert_eq!(out.snapshot.weekly_pct, 30);
        assert_eq!(out.snapshot.window, Some(chrono::Duration::days(7)));
    }

    #[tokio::test]
    async fn a_cache_from_a_previous_sign_in_is_not_reused() {
        let mut server = mockito::Server::new_async().await;
        let m = usage_mock(&mut server, "at-stored")
            .with_status(200)
            .with_body(usage_json())
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        cache
            .write_payload(sample_seed("some-other-fingerprint").to_string().as_bytes())
            .unwrap();

        let out = fetch_snapshot_with(
            &reqwest::Client::new(),
            &test_creds(),
            &cache,
            &test_endpoints(&server.url()),
            // A long TTL: only the fingerprint mismatch can explain a refetch.
            Duration::from_secs(3600),
        )
        .await
        .unwrap();

        m.assert_async().await;
        assert!(!out.stale);
        assert_eq!(out.snapshot.weekly_pct, 12);
    }

    #[tokio::test]
    async fn a_transport_error_with_a_stale_cache_uses_the_cache() {
        let (_td, cache) = cache_fixture();
        cache
            .write_payload(
                sample_seed(&test_creds().fingerprint)
                    .to_string()
                    .as_bytes(),
            )
            .unwrap();

        let out = fetch_snapshot_with(
            &reqwest::Client::new(),
            &test_creds(),
            &cache,
            &test_endpoints("http://127.0.0.1:1"),
            Duration::ZERO,
        )
        .await
        .unwrap();

        assert!(out.stale);
        assert_eq!(out.snapshot.weekly_pct, 30);
    }

    #[test]
    fn a_cache_with_an_out_of_range_percent_is_rejected() {
        let mut v = sample_seed("fp");
        v["weekly_pct"] = serde_json::json!(150);
        let err = parse_cache_at(v.to_string().as_bytes(), "fp").unwrap_err();
        assert!(err.to_string().contains("weekly_pct"), "{err}");
    }

    #[test]
    fn a_snapshot_survives_the_cache_round_trip() {
        let snap = GrokbotSnapshot {
            plan: "Grok Bot Plan".into(),
            has_included_allowance: true,
            weekly_pct: 42,
            has_available_usage: true,
            on_demand_enabled: true,
            period_start: DateTime::parse_from_rfc3339("2026-09-11T18:43:19.645Z")
                .map(|dt| dt.with_timezone(&Utc))
                .ok(),
            reset_at: DateTime::parse_from_rfc3339("2026-09-18T18:43:19.645Z")
                .map(|dt| dt.with_timezone(&Utc))
                .ok(),
            window: Some(chrono::Duration::days(7)),
        };
        let bytes = serde_json::to_vec(&snap_to_json(&snap, "fp")).unwrap();
        assert_eq!(parse_cache_at(&bytes, "fp").unwrap(), snap);
    }

    #[test]
    fn request_ids_are_uuid_v4_shaped_and_unique() {
        let a = request_id();
        let b = request_id();
        assert_ne!(a, b);
        for id in [&a, &b] {
            assert_eq!(id.len(), 36, "{id}");
            assert_eq!(&id[14..15], "4", "version nibble: {id}");
            assert!(
                matches!(&id[19..20], "8" | "9" | "a" | "b"),
                "variant bits: {id}"
            );
            assert!(
                id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
                "{id}"
            );
        }
    }
}
