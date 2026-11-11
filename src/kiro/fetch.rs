//! Orchestrates a Kiro CLI snapshot: read kiro-cli's own cached AWS SSO OIDC
//! or social-login token (`db.rs`), refresh it if it's close to expiry
//! (`oauth.rs` — kiro-cli's own live WAL database is never written back to and
//! `kiro-cli` offers no non-interactive renewal command), persist any rotated
//! token pair in ai-usagebar's atomic, mode-`0600`, account-fingerprinted
//! `kiro/oauth.json` sidecar, then call
//! `AmazonCodeWhispererService.GetUsageLimits` — the exact operation kiro-cli's
//! own `/usage` slash command invokes. Cache/stale/error-fallback shape
//! mirrors `cursor::fetch`.

use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::cache::{Cache, acquire_lock_async, atomic_write};
use crate::error::{AppError, Result};
use crate::usage::KiroSnapshot;
use crate::vendor::{MAX_BODY_BYTES, read_body_capped};

use super::db::{self, KiroAuthKind, KiroCredentials};
use super::oauth;
use super::types::{self, UsageLimitsResponse};

const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const REFRESH_TIMEOUT: Duration = Duration::from_secs(15);
const LOCK_TIMEOUT: Duration = Duration::from_secs(15);
const REQUEST_TARGET: &str = "AmazonCodeWhispererService.GetUsageLimits";
const OAUTH_CACHE_FILE: &str = "oauth.json";

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub usage_limits: String,
    pub token: String,
}

impl Endpoints {
    /// AWS's regional convention for the public CodeWhisperer Runtime
    /// endpoint (`<service>.<region>.amazonaws.com`) — confirmed live against
    /// this account. Enterprise IAM Identity Center accounts may instead be
    /// proxied through `management.<region>.kiro.dev` (observed in kiro-cli's
    /// own request trace for the same operation); both speak the same
    /// request/response shape, so either is a config override away.
    pub fn for_region(region: &str) -> Result<Self> {
        oauth::validate_region(region)?;
        Ok(Self {
            usage_limits: format!("https://codewhisperer.{region}.amazonaws.com/"),
            token: oauth::token_endpoint(region)?,
        })
    }

    pub fn for_credentials(creds: &KiroCredentials) -> Result<Self> {
        oauth::validate_region(&creds.region)?;
        let token = match &creds.auth_kind {
            KiroAuthKind::Oidc { .. } => oauth::token_endpoint(&creds.region)?,
            KiroAuthKind::Social => oauth::social_token_endpoint(&creds.region)?,
        };
        Ok(Self {
            usage_limits: format!("https://codewhisperer.{}.amazonaws.com/", creds.region),
            token,
        })
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct PersistedOAuth {
    account: String,
    access_token: String,
    refresh_token: String,
    expires_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile_arn: Option<String>,
}

/// This vendor's [`Outcome`](crate::outcome::Outcome) — the shared shape,
/// specialised to its snapshot.
pub type FetchOutcome = crate::outcome::Outcome<KiroSnapshot>;

/// The client refuses every redirect, so Kiro's refresh and access tokens are
/// never forwarded or re-sent through one, including a same-origin redirect.
fn build_client(timeout: Duration) -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
}

/// Kiro keeps its own client instead of the process-wide one that the widget
/// and TUI share: that client follows same-origin redirects, and this provider
/// must never send refresh or access tokens through one.
static CLIENT: LazyLock<Option<reqwest::Client>> =
    LazyLock::new(|| build_client(REFRESH_TIMEOUT).ok());

pub(crate) fn http_client() -> Result<&'static reqwest::Client> {
    CLIENT
        .as_ref()
        .ok_or_else(|| AppError::Other("Kiro HTTP client initialization failed".into()))
}

/// Cache-aware fetch. `db_path` is kiro-cli's `data.sqlite3` — the caller
/// resolves `[kiro] db_path` (config override) vs [`db::default_db_path`],
/// the same override pattern as `cursor.db_path`.
pub async fn fetch_snapshot(
    _client: &reqwest::Client,
    db_path: &Path,
    cache: &Cache,
    cache_ttl: Duration,
) -> Result<FetchOutcome> {
    fetch_snapshot_at(http_client()?, db_path, cache, cache_ttl, None, Utc::now()).await
}

/// Clock seam for cache rollover tests, mirroring `cursor::fetch::fetch_snapshot_at`.
///
/// `endpoints_override` exists only for tests: unlike every fixed-URL vendor
/// here, Kiro's real endpoint is derived from the account's own region (read
/// from `db_path`, not known until inside this function), so there is no
/// fixed default a test could point at mockito. Production always passes
/// `None` and gets `Endpoints::for_region(&creds.region)`.
async fn fetch_snapshot_at(
    client: &reqwest::Client,
    db_path: &Path,
    cache: &Cache,
    cache_ttl: Duration,
    endpoints_override: Option<&Endpoints>,
    now: DateTime<Utc>,
) -> Result<FetchOutcome> {
    cache.ensure_dir()?;
    let _lock = acquire_lock_async(&cache.lock_path(), LOCK_TIMEOUT).await?;

    // Resolve identity before accepting a cache hit — a `kiro-cli login`
    // switch mid-session must not keep serving the previous account's cache.
    let mut creds = db::read_credentials(db_path)?;

    if let Some(bytes) = cache.fresh_payload(cache_ttl)?
        && let Ok(outcome) = reuse_cache(&bytes, cache, false, &creds.account_key, now)
    {
        return Ok(outcome);
    }
    // #515: during a backoff `fresh_payload` returns bytes this refresh can
    // refuse, such as a payload past its credit-cycle reset. The shared
    // backoff still applies, as it does for the Anthropic API since #496:
    // surface the countdown instead of contacting the server, or a refresh
    // interval shorter than the backoff would re-request and re-arm it forever.
    if let Some(remaining) = cache.backoff_remaining() {
        return Err(AppError::Http {
            status: 429,
            body: format!(
                "rate limited; next attempt in {}",
                crate::cache::human_backoff(remaining)
            ),
        });
    }

    apply_persisted_oauth(cache, &mut creds)?;

    let derived;
    let endpoints = match endpoints_override {
        Some(e) => e,
        None => {
            derived = Endpoints::for_credentials(&creds)?;
            &derived
        }
    };
    match fetch_live(
        client,
        endpoints,
        cache,
        &creds,
        now,
        endpoints_override.is_none(),
    )
    .await
    {
        Ok(snap) => {
            let bytes = serde_json::to_vec(&snap_to_json(&snap, &creds.account_key))?;
            cache.write_payload(&bytes)?;
            Ok(crate::outcome::Outcome::fresh(snap))
        }
        Err(e) if e.is_transient() => fallback_silent(cache, &creds.account_key, now, e),
        Err(e) => {
            cache.mark_stale();
            if let Some((code, msg)) = error_to_pair(&e) {
                cache.write_last_error(code, &msg);
            }
            fallback_with_error(cache, &creds.account_key, now, e)
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

/// Never surface upstream bodies for auth failures: a 401/403 from an
/// internal Bearer-token endpoint is not guaranteed not to echo something
/// account-identifying back. Mirrors `cursor::fetch::error_to_pair`.
fn error_to_pair(e: &AppError) -> Option<(u16, String)> {
    match e {
        AppError::Http { status, .. } if matches!(status, 401 | 403) => {
            Some((*status, "Kiro CLI authentication failed".into()))
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
    account: &str,
    now: DateTime<Utc>,
) -> Result<FetchOutcome> {
    let snap = parse_cache_at(bytes, account, now)?;
    Ok(crate::outcome::Outcome::cached(snap, cache, stale))
}

fn parse_cache_at(bytes: &[u8], account: &str, now: DateTime<Utc>) -> Result<KiroSnapshot> {
    let v: serde_json::Value = serde_json::from_slice(bytes)?;
    if v.get("account").and_then(serde_json::Value::as_str) != Some(account) {
        return Err(AppError::Schema(
            "kiro cache belongs to a different account; refetching".into(),
        ));
    }
    let plan = v["plan"]
        .as_str()
        .filter(|plan| !plan.trim().is_empty())
        .ok_or_else(|| AppError::Schema("kiro cache: invalid plan".into()))?
        .to_string();
    let used = v["used"]
        .as_f64()
        .filter(|n| n.is_finite() && *n >= 0.0)
        .ok_or_else(|| AppError::Schema("kiro cache: invalid used".into()))?;
    let limit = v["limit"]
        .as_f64()
        .filter(|n| n.is_finite() && *n >= 0.0)
        .ok_or_else(|| AppError::Schema("kiro cache: invalid limit".into()))?;
    let reset_at = match &v["reset_at"] {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) => Some(
            DateTime::parse_from_rfc3339(s)
                .map_err(|e| AppError::Schema(format!("kiro cache: invalid reset_at: {e}")))?
                .with_timezone(&Utc),
        ),
        _ => return Err(AppError::Schema("kiro cache: invalid reset_at".into())),
    };
    // Only reject a *known-past* reset — the cycle rolled over while cached.
    // A missing reset (None) is a legitimate shape, not staleness.
    if let Some(reset_at) = reset_at
        && reset_at <= now
    {
        return Err(AppError::Schema(
            "kiro cache is past its credit-cycle reset; refetching".into(),
        ));
    }
    Ok(KiroSnapshot {
        plan,
        used,
        limit,
        reset_at,
    })
}

fn snap_to_json(snap: &KiroSnapshot, account: &str) -> serde_json::Value {
    serde_json::json!({
        "account": account,
        "plan": snap.plan,
        "used": snap.used,
        "limit": snap.limit,
        "reset_at": snap.reset_at.map(|dt| dt.to_rfc3339()),
    })
}

fn oauth_cache_path(cache: &Cache) -> std::path::PathBuf {
    cache.dir().join(OAUTH_CACHE_FILE)
}

fn read_persisted_oauth(cache: &Cache, account: &str) -> Result<Option<PersistedOAuth>> {
    let path = oauth_cache_path(cache);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(AppError::io_at(&path, e)),
    };
    let persisted: PersistedOAuth = serde_json::from_slice(&bytes).map_err(|e| {
        AppError::Credentials(format!(
            "ai-usagebar's cached Kiro credentials at {} are malformed ({e}); remove that file and try again",
            path.display()
        ))
    })?;
    if persisted.account != account {
        return Ok(None);
    }
    if persisted.access_token.trim().is_empty() || persisted.refresh_token.trim().is_empty() {
        return Err(AppError::Credentials(format!(
            "ai-usagebar's cached Kiro credentials at {} are incomplete; remove that file and try again",
            path.display()
        )));
    }
    if let Some(ref arn) = persisted.profile_arn {
        db::region_from_profile_arn(arn).map_err(|e| {
            AppError::Credentials(format!(
                "ai-usagebar's cached Kiro credentials at {} contain an invalid profile ARN ({e}); remove that file and try again",
                path.display()
            ))
        })?;
    }
    Ok(Some(persisted))
}

fn apply_persisted_oauth(cache: &Cache, creds: &mut KiroCredentials) -> Result<()> {
    let Some(persisted) = read_persisted_oauth(cache, &creds.account_key)? else {
        return Ok(());
    };
    // Kiro may refresh its own database independently. Prefer whichever
    // account-matched credential has the later expiry. Keep `creds.account_key`
    // pinned to the database identity so a canonical `profileArn` from refresh
    // cannot cross-pollinate or orphan the account's cache.
    if persisted.expires_at > creds.expires_at {
        creds.access_token = persisted.access_token;
        creds.refresh_token = persisted.refresh_token;
        creds.expires_at = persisted.expires_at;
        if let Some(profile_arn) = persisted.profile_arn {
            creds.region = db::region_from_profile_arn(&profile_arn)?;
            creds.profile_arn = profile_arn;
        }
    }
    Ok(())
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
    creds: &KiroCredentials,
    now: DateTime<Utc>,
    derive_usage_endpoint_from_profile: bool,
) -> Result<KiroSnapshot> {
    let (access_token, profile_arn) = if oauth::needs_refresh(
        creds.expires_at.timestamp(),
        now.timestamp(),
    ) {
        let refreshed = tokio::time::timeout(REFRESH_TIMEOUT, async {
            match &creds.auth_kind {
                KiroAuthKind::Oidc {
                    client_id,
                    client_secret,
                } => {
                    oauth::refresh(
                        client,
                        &endpoints.token,
                        client_id,
                        client_secret,
                        &creds.refresh_token,
                    )
                    .await
                }
                KiroAuthKind::Social => {
                    oauth::refresh_social(client, &endpoints.token, &creds.refresh_token).await
                }
            }
        })
        .await
        .map_err(|_| {
            AppError::Transport(format!("kiro token refresh timeout: {}", endpoints.token))
        })?
        .map_err(|e| {
            // A refresh that never reached the token endpoint says nothing
            // about the login: keep it transient, so the cache covers it the
            // way it covers the usage call's own network failures.
            if e.is_transient() {
                e
            } else {
                AppError::Credentials(format!(
                    "Kiro CLI token refresh failed ({e}). Run `kiro-cli login` again."
                ))
            }
        })?;
        let expires_in = i64::try_from(refreshed.expires_in)
            .map_err(|_| AppError::Schema("kiro token refresh expiry is out of range".into()))?;
        let expires_at_secs = now
            .timestamp()
            .checked_add(expires_in)
            .ok_or_else(|| AppError::Schema("kiro token refresh expiry overflowed".into()))?;
        let expires_at = DateTime::from_timestamp(expires_at_secs, 0)
            .ok_or_else(|| AppError::Schema("kiro token refresh expiry is out of range".into()))?;
        let profile_arn = refreshed
            .profile_arn
            .clone()
            .unwrap_or_else(|| creds.profile_arn.clone());
        let persisted = PersistedOAuth {
            account: creds.account_key.clone(),
            access_token: refreshed.access_token,
            refresh_token: refreshed
                .refresh_token
                .unwrap_or_else(|| creds.refresh_token.clone()),
            expires_at,
            profile_arn: refreshed.profile_arn,
        };
        write_persisted_oauth(cache, &persisted).map_err(|e| {
                AppError::Credentials(format!(
                    "refreshed Kiro CLI credentials could not be saved ({e}); run `kiro-cli login` again if the refresh token was rotated"
                ))
            })?;
        (persisted.access_token, profile_arn)
    } else {
        (creds.access_token.clone(), creds.profile_arn.clone())
    };

    let usage_limits =
        usage_limits_endpoint(endpoints, &profile_arn, derive_usage_endpoint_from_profile)?;
    let body = serde_json::json!({
        "origin": "AI_EDITOR",
        "profileArn": profile_arn,
        "resourceType": "AGENTIC_REQUEST",
    });

    let resp = tokio::time::timeout(
        HTTP_TIMEOUT,
        client
            .post(&usage_limits)
            .header("Content-Type", "application/x-amz-json-1.0")
            .header("x-amz-target", REQUEST_TARGET)
            .header("Authorization", format!("Bearer {access_token}"))
            .json(&body)
            .send(),
    )
    .await
    .map_err(|_| AppError::Transport(format!("kiro timeout: {usage_limits}")))??;

    let status = resp.status();
    if !status.is_success() {
        let body = if matches!(status.as_u16(), 401 | 403) {
            "Kiro CLI authentication failed".into()
        } else {
            format!("Kiro CLI API returned HTTP {}", status.as_u16())
        };
        return Err(AppError::Http {
            status: status.as_u16(),
            body,
        });
    }

    let bytes = read_body_capped(resp, MAX_BODY_BYTES).await?;
    let parsed: UsageLimitsResponse = serde_json::from_slice(&bytes)
        .map_err(|e| AppError::Schema(format!("kiro usage-limits response: {e}")))?;
    types::to_snapshot(parsed)
}

fn usage_limits_endpoint(
    endpoints: &Endpoints,
    profile_arn: &str,
    derive_from_profile: bool,
) -> Result<String> {
    // Production follows the canonical ARN returned by refresh immediately;
    // hermetic endpoint overrides must remain pointed at their mock server.
    if !derive_from_profile || profile_arn.trim().is_empty() {
        return Ok(endpoints.usage_limits.clone());
    }
    let region = db::region_from_profile_arn(profile_arn)?;
    Ok(format!("https://codewhisperer.{region}.amazonaws.com/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use tempfile::TempDir;

    fn cache_fixture() -> (TempDir, Cache) {
        let td = TempDir::new().unwrap();
        let cache = Cache::at(td.path().join("kiro"));
        cache.ensure_dir().unwrap();
        (td, cache)
    }

    fn seed_db(dir: &TempDir, expires_at: &str) -> std::path::PathBuf {
        let path = dir.path().join("data.sqlite3");
        let conn = Connection::open(&path).unwrap();
        conn.execute("CREATE TABLE auth_kv (key TEXT, value TEXT)", [])
            .unwrap();
        conn.execute("CREATE TABLE state (key TEXT, value TEXT)", [])
            .unwrap();
        let token = serde_json::json!({
            "access_token": "AT", "refresh_token": "RT",
            "expires_at": expires_at, "region": "us-east-1",
        })
        .to_string();
        let device =
            serde_json::json!({"client_id": "CID", "client_secret": "CSECRET"}).to_string();
        let profile =
            serde_json::json!({"arn": "arn:aws:codewhisperer:us-east-1:1:profile/A"}).to_string();
        conn.execute(
            "INSERT INTO auth_kv (key, value) VALUES ('kirocli:odic:token', ?1)",
            [&token],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO auth_kv (key, value) VALUES ('kirocli:odic:device-registration', ?1)",
            [&device],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO state (key, value) VALUES ('api.codewhisperer.profile', ?1)",
            [&profile],
        )
        .unwrap();
        path
    }

    fn account_key(dir: &TempDir) -> String {
        let path = dir.path().join("data.sqlite3");
        db::read_credentials(&path).unwrap().account_key
    }

    fn usage_json() -> String {
        r#"{
            "nextDateReset": 4102444800.0,
            "subscriptionInfo": { "subscriptionTitle": "KIRO POWER" },
            "usageBreakdownList": [{
                "resourceType": "CREDIT",
                "currentUsageWithPrecision": 40.0,
                "usageLimitWithPrecision": 100.0
            }]
        }"#
        .to_string()
    }

    #[test]
    fn canonical_profile_region_rebinds_only_the_production_usage_endpoint() {
        let endpoints = Endpoints {
            usage_limits: "https://codewhisperer.us-east-1.amazonaws.com/".into(),
            token: "unused".into(),
        };
        let canonical = "arn:aws:codewhisperer:eu-central-1:123:profile/CANONICAL";

        assert_eq!(
            usage_limits_endpoint(&endpoints, canonical, true).unwrap(),
            "https://codewhisperer.eu-central-1.amazonaws.com/"
        );
        assert_eq!(
            usage_limits_endpoint(&endpoints, canonical, false).unwrap(),
            endpoints.usage_limits
        );
        assert_eq!(
            usage_limits_endpoint(&endpoints, "", true).unwrap(),
            endpoints.usage_limits
        );
    }

    #[tokio::test]
    async fn live_fetch_reads_token_from_db_and_calls_get_usage_limits() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/")
            .match_header("x-amz-target", "AmazonCodeWhispererService.GetUsageLimits")
            .match_header("authorization", "Bearer AT")
            .with_status(200)
            .with_body(usage_json())
            .create_async()
            .await;

        let db_dir = TempDir::new().unwrap();
        // Far future — no refresh needed.
        let db_path = seed_db(&db_dir, "2099-01-01T00:00:00Z");
        let client = reqwest::Client::new();
        let endpoints_url = server.url();

        let creds = db::read_credentials(&db_path).unwrap();
        let (_cache_dir, cache) = cache_fixture();
        let out = fetch_live(
            &client,
            &Endpoints {
                usage_limits: endpoints_url,
                token: "unused".into(),
            },
            &cache,
            &creds,
            Utc::now(),
            false,
        )
        .await
        .unwrap();

        assert_eq!(out.plan, "KIRO POWER");
        assert_eq!(out.used, 40.0);
        assert_eq!(out.limit, 100.0);
        m.assert_async().await;
    }

    #[tokio::test]
    async fn expired_token_is_refreshed_before_the_usage_call() {
        let mut server = mockito::Server::new_async().await;
        let token_mock = server
            .mock("POST", "/token")
            .with_status(200)
            .with_body(r#"{"accessToken":"NEW-AT","expiresIn":3600}"#)
            .create_async()
            .await;
        let usage_mock = server
            .mock("POST", "/")
            .match_header("authorization", "Bearer NEW-AT")
            .with_status(200)
            .with_body(usage_json())
            .create_async()
            .await;

        let db_dir = TempDir::new().unwrap();
        // Already expired — must trigger a refresh.
        let db_path = seed_db(&db_dir, "2000-01-01T00:00:00Z");
        let creds = db::read_credentials(&db_path).unwrap();
        let (_cache_dir, cache) = cache_fixture();
        let client = reqwest::Client::new();

        let out = fetch_live(
            &client,
            &Endpoints {
                usage_limits: server.url(),
                token: format!("{}/token", server.url()),
            },
            &cache,
            &creds,
            Utc::now(),
            false,
        )
        .await
        .unwrap();

        assert_eq!(out.used, 40.0);
        token_mock.assert_async().await;
        usage_mock.assert_async().await;
    }

    #[tokio::test]
    async fn refreshed_credentials_are_reused_and_rotated_token_is_retained() {
        let mut server = mockito::Server::new_async().await;
        let token_mock = server
            .mock("POST", "/token")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "clientId": "CID",
                "clientSecret": "CSECRET",
                "grantType": "refresh_token",
                "refreshToken": "RT",
            })))
            .with_status(200)
            .with_body(r#"{"accessToken":"NEW-AT","refreshToken":"ROTATED-RT","expiresIn":3600}"#)
            .expect(1)
            .create_async()
            .await;
        let usage_mock = server
            .mock("POST", "/")
            .match_header("authorization", "Bearer NEW-AT")
            .with_status(200)
            .with_body(usage_json())
            .expect(2)
            .create_async()
            .await;

        let db_dir = TempDir::new().unwrap();
        let db_path = seed_db(&db_dir, "2000-01-01T00:00:00Z");
        let (_cache_dir, cache) = cache_fixture();
        let endpoints = Endpoints {
            usage_limits: server.url(),
            token: format!("{}/token", server.url()),
        };
        let now = DateTime::parse_from_rfc3339("2026-08-03T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        for _ in 0..2 {
            let out = fetch_snapshot_at(
                &reqwest::Client::new(),
                &db_path,
                &cache,
                Duration::ZERO,
                Some(&endpoints),
                now,
            )
            .await
            .unwrap();
            assert_eq!(out.snapshot.used, 40.0);
        }

        let persisted: serde_json::Value =
            serde_json::from_slice(&std::fs::read(oauth_cache_path(&cache)).unwrap()).unwrap();
        assert_eq!(persisted["access_token"], "NEW-AT");
        assert_eq!(persisted["refresh_token"], "ROTATED-RT");
        assert_eq!(persisted["account"], account_key(&db_dir));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(oauth_cache_path(&cache))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0);
        }

        token_mock.assert_async().await;
        usage_mock.assert_async().await;
    }

    #[test]
    fn persisted_credentials_from_another_account_are_ignored() {
        let db_dir = TempDir::new().unwrap();
        let db_path = seed_db(&db_dir, "2099-01-01T00:00:00Z");
        let mut creds = db::read_credentials(&db_path).unwrap();
        let (_cache_dir, cache) = cache_fixture();
        write_persisted_oauth(
            &cache,
            &PersistedOAuth {
                account: "another-account".into(),
                access_token: "OTHER-AT".into(),
                refresh_token: "OTHER-RT".into(),
                expires_at: DateTime::parse_from_rfc3339("2100-01-01T00:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
                profile_arn: Some(
                    "arn:aws:codewhisperer:us-east-1:999999999999:profile/OTHER".into(),
                ),
            },
        )
        .unwrap();

        apply_persisted_oauth(&cache, &mut creds).unwrap();

        assert_eq!(creds.access_token, "AT");
        assert_eq!(creds.refresh_token, "RT");
        assert_eq!(
            creds.profile_arn,
            "arn:aws:codewhisperer:us-east-1:1:profile/A"
        );
    }

    #[tokio::test]
    async fn refresh_failure_is_a_credentials_error() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/token")
            .with_status(400)
            .with_body(r#"{"error":"invalid_grant"}"#)
            .create_async()
            .await;

        let db_dir = TempDir::new().unwrap();
        let db_path = seed_db(&db_dir, "2000-01-01T00:00:00Z");
        let creds = db::read_credentials(&db_path).unwrap();
        let (_cache_dir, cache) = cache_fixture();
        let client = reqwest::Client::new();

        let err = fetch_live(
            &client,
            &Endpoints {
                usage_limits: server.url(),
                token: format!("{}/token", server.url()),
            },
            &cache,
            &creds,
            Utc::now(),
            false,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, AppError::Credentials(_)));
    }

    /// A refresh that cannot reach the token endpoint is a network failure,
    /// not a rejected login: it must not record "run `kiro-cli login`" as the
    /// refresh error, and with no cache to fall back on it stays transient.
    #[tokio::test]
    async fn a_refresh_that_cannot_connect_is_not_a_sign_in_error() {
        let db_dir = TempDir::new().unwrap();
        let db_path = seed_db(&db_dir, "2000-01-01T00:00:00Z");
        let (_cache_dir, cache) = cache_fixture();

        let err = fetch_snapshot_at(
            &reqwest::Client::new(),
            &db_path,
            &cache,
            Duration::ZERO,
            Some(&Endpoints {
                usage_limits: "http://127.0.0.1:1".into(),
                token: "http://127.0.0.1:1/token".into(),
            }),
            Utc::now(),
        )
        .await
        .unwrap_err();

        assert!(err.is_transient(), "{err:?}");
        assert_eq!(cache.read_last_error(), None);
    }

    #[tokio::test]
    async fn a_stale_cache_falls_back_when_the_live_call_fails() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/")
            .with_status(500)
            .with_body("boom")
            .create_async()
            .await;

        let db_dir = TempDir::new().unwrap();
        let db_path = seed_db(&db_dir, "2099-01-01T00:00:00Z");
        let (_cache_dir, cache) = cache_fixture();
        let account = account_key(&db_dir);
        cache
            .write_payload(
                serde_json::to_vec(&serde_json::json!({
                    "account": account,
                    "plan": "KIRO POWER",
                    "used": 10.0,
                    "limit": 100.0,
                    "reset_at": null,
                }))
                .unwrap()
                .as_slice(),
            )
            .unwrap();
        // Force the cache to look old enough to require a live refetch.
        std::thread::sleep(std::time::Duration::from_millis(5));

        let client = reqwest::Client::new();
        let out = fetch_snapshot_at(
            &client,
            &db_path,
            &cache,
            Duration::from_millis(1),
            Some(&Endpoints {
                usage_limits: server.url(),
                token: format!("{}/token", server.url()),
            }),
            Utc::now(),
        )
        .await
        .unwrap();

        assert!(out.stale);
        assert_eq!(out.snapshot.used, 10.0);
        assert!(out.last_error.is_some());
    }

    #[tokio::test]
    async fn fresh_cache_is_served_without_a_network_call() {
        let db_dir = TempDir::new().unwrap();
        let db_path = seed_db(&db_dir, "2099-01-01T00:00:00Z");
        let (_cache_dir, cache) = cache_fixture();
        let account = account_key(&db_dir);
        cache
            .write_payload(
                serde_json::to_vec(&serde_json::json!({
                    "account": account,
                    "plan": "KIRO POWER",
                    "used": 10.0,
                    "limit": 100.0,
                    "reset_at": null,
                }))
                .unwrap()
                .as_slice(),
            )
            .unwrap();

        let client = reqwest::Client::new();
        // Deliberately no mock server configured: a fresh-cache hit must
        // never reach the network, so pointing the override at a bogus
        // "http://127.0.0.1:1" endpoint is a canary — the test would fail
        // with a transport error if the cache short-circuit stopped working.
        let out = fetch_snapshot_at(
            &client,
            &db_path,
            &cache,
            Duration::from_secs(60),
            Some(&Endpoints {
                usage_limits: "http://127.0.0.1:1".into(),
                token: "http://127.0.0.1:1".into(),
            }),
            Utc::now(),
        )
        .await
        .unwrap();

        assert!(!out.stale);
        assert_eq!(out.snapshot.used, 10.0);
    }

    #[tokio::test]
    async fn cache_from_a_different_account_is_not_reused() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(usage_json())
            .create_async()
            .await;

        let db_dir = TempDir::new().unwrap();
        let db_path = seed_db(&db_dir, "2099-01-01T00:00:00Z");
        let (_cache_dir, cache) = cache_fixture();
        cache
            .write_payload(
                serde_json::to_vec(&serde_json::json!({
                    "account": "some-other-account",
                    "plan": "STALE PLAN",
                    "used": 10.0,
                    "limit": 100.0,
                    "reset_at": null,
                }))
                .unwrap()
                .as_slice(),
            )
            .unwrap();

        let client = reqwest::Client::new();
        let out = fetch_snapshot_at(
            &client,
            &db_path,
            &cache,
            Duration::from_secs(60),
            Some(&Endpoints {
                usage_limits: server.url(),
                token: format!("{}/token", server.url()),
            }),
            Utc::now(),
        )
        .await
        .unwrap();

        // The account-mismatched cache was rejected, so this came from the
        // live mock (used=40, plan="KIRO POWER"), not the stale cache entry.
        assert_eq!(out.snapshot.used, 40.0);
        assert_eq!(out.snapshot.plan, "KIRO POWER");
        assert!(!out.stale);
        m.assert_async().await;
    }

    /// The credit cycle rolled over while cached: serving that cache during
    /// an outage would show last cycle's usage as if it were current. The
    /// parse must reject it, surfacing the live error instead. Mirrors
    /// `cursor::fetch`'s `cache_past_its_billing_reset_is_not_served_during_an_outage`.
    #[tokio::test]
    async fn cache_past_its_credit_reset_is_not_served_during_an_outage() {
        let db_dir = TempDir::new().unwrap();
        let db_path = seed_db(&db_dir, "2099-01-01T00:00:00Z");
        let (_cache_dir, cache) = cache_fixture();
        let account = account_key(&db_dir);
        cache
            .write_payload(
                serde_json::to_vec(&serde_json::json!({
                    "account": account,
                    "plan": "KIRO POWER",
                    "used": 10.0,
                    "limit": 100.0,
                    "reset_at": "2026-08-04T00:00:00Z",
                }))
                .unwrap()
                .as_slice(),
            )
            .unwrap();

        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/")
            .with_status(503)
            .create_async()
            .await;
        let now = DateTime::parse_from_rfc3339("2026-08-05T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let err = fetch_snapshot_at(
            &reqwest::Client::new(),
            &db_path,
            &cache,
            Duration::from_secs(0),
            Some(&Endpoints {
                usage_limits: server.url(),
                token: format!("{}/token", server.url()),
            }),
            now,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, AppError::Http { status: 503, .. }));
    }

    /// #515: the cached payload is past its credit-cycle reset, so
    /// `reuse_cache` refuses it, but a 429 backoff armed moments ago still
    /// applies. The refresh must surface the shared countdown without
    /// contacting the server. Mirrors `cursor::fetch`'s
    /// `an_active_backoff_is_not_bypassed_after_the_billing_reset`.
    #[tokio::test]
    async fn an_active_backoff_is_not_bypassed_after_the_credit_reset() {
        let db_dir = TempDir::new().unwrap();
        let db_path = seed_db(&db_dir, "2099-01-01T00:00:00Z");
        let (_cache_dir, cache) = cache_fixture();
        let account = account_key(&db_dir);
        cache
            .write_payload(
                serde_json::to_vec(&serde_json::json!({
                    "account": account,
                    "plan": "KIRO POWER",
                    "used": 10.0,
                    "limit": 100.0,
                    "reset_at": "2026-08-04T00:00:00Z",
                }))
                .unwrap()
                .as_slice(),
            )
            .unwrap();
        // Armed against the real clock, like a genuine 429 would be; the
        // payload above is seconds fresh, so `fresh_payload` returns it and
        // the reset check refuses it.
        cache.note_rate_limit_at(std::time::SystemTime::now());

        let mut server = mockito::Server::new_async().await;
        let usage = server
            .mock("POST", "/")
            .with_status(500)
            .with_body("must not be contacted during backoff")
            .expect(0)
            .create_async()
            .await;
        let now = DateTime::parse_from_rfc3339("2026-08-05T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let err = fetch_snapshot_at(
            &reqwest::Client::new(),
            &db_path,
            &cache,
            Duration::from_secs(600),
            Some(&Endpoints {
                usage_limits: server.url(),
                token: format!("{}/token", server.url()),
            }),
            now,
        )
        .await
        .unwrap_err();
        usage.assert_async().await;
        match &err {
            AppError::Http { status, body } => {
                assert_eq!(status, &429, "body: {body}");
                assert!(body.contains("rate limited"), "body: {body}");
            }
            other => panic!("expected the shared backoff 429, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn expired_social_token_is_refreshed_and_persisted_before_usage_call() {
        let mut server = mockito::Server::new_async().await;
        let token_mock = server
            .mock("POST", "/refreshToken")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "refreshToken": "SOCIAL-RT",
            })))
            .with_status(200)
            .with_body(
                r#"{"accessToken":"SOCIAL-NEW-AT","refreshToken":"SOCIAL-ROTATED-RT","expiresIn":3600}"#,
            )
            .expect(1)
            .create_async()
            .await;
        let usage_mock = server
            .mock("POST", "/")
            .match_header("authorization", "Bearer SOCIAL-NEW-AT")
            .with_status(200)
            .with_body(usage_json())
            .expect(1)
            .create_async()
            .await;

        let db_dir = TempDir::new().unwrap();
        let db_path = db_dir.path().join("data.sqlite3");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute("CREATE TABLE auth_kv (key TEXT, value TEXT)", [])
            .unwrap();
        conn.execute("CREATE TABLE state (key TEXT, value TEXT)", [])
            .unwrap();
        let social = serde_json::json!({
            "access_token": "SOCIAL-OLD-AT",
            "refresh_token": "SOCIAL-RT",
            "expires_at": "2000-01-01T00:00:00Z",
            "provider": "google",
            "profile_arn": "arn:aws:codewhisperer:us-east-1:699475941385:profile/EHGA3GRVQMUK",
        })
        .to_string();
        conn.execute(
            "INSERT INTO auth_kv (key, value) VALUES ('kirocli:social:token', ?1)",
            [&social],
        )
        .unwrap();

        let (_cache_dir, cache) = cache_fixture();
        let endpoints = Endpoints {
            usage_limits: server.url(),
            token: format!("{}/refreshToken", server.url()),
        };
        let now = DateTime::parse_from_rfc3339("2026-08-03T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let out = fetch_snapshot_at(
            &reqwest::Client::new(),
            &db_path,
            &cache,
            Duration::ZERO,
            Some(&endpoints),
            now,
        )
        .await
        .unwrap();
        assert_eq!(out.snapshot.used, 40.0);

        let persisted: serde_json::Value =
            serde_json::from_slice(&std::fs::read(oauth_cache_path(&cache)).unwrap()).unwrap();
        assert_eq!(persisted["access_token"], "SOCIAL-NEW-AT");
        assert_eq!(persisted["refresh_token"], "SOCIAL-ROTATED-RT");

        token_mock.assert_async().await;
        usage_mock.assert_async().await;
    }

    #[tokio::test]
    async fn usage_limits_does_not_follow_same_origin_or_cross_origin_redirects() {
        let db_dir = TempDir::new().unwrap();
        let db_path = seed_db(&db_dir, "2099-01-01T00:00:00Z");
        let client = http_client().unwrap();

        for cross_origin in [false, true] {
            let mut server = mockito::Server::new_async().await;
            let location = if cross_origin {
                "https://attacker.invalid/collect".to_string()
            } else {
                format!("{}/redirected", server.url())
            };
            let initial = server
                .mock("POST", "/")
                .with_status(307)
                .with_header("location", &location)
                .create_async()
                .await;
            let redirected = server
                .mock("POST", "/redirected")
                .expect(0)
                .create_async()
                .await;
            let (_cache_dir, cache) = cache_fixture();

            let err = fetch_snapshot_at(
                client,
                &db_path,
                &cache,
                Duration::ZERO,
                Some(&Endpoints {
                    usage_limits: server.url(),
                    token: format!("{}/token", server.url()),
                }),
                Utc::now(),
            )
            .await
            .unwrap_err();

            initial.assert_async().await;
            redirected.assert_async().await;
            assert!(matches!(err, AppError::Http { status: 307, .. }));
            assert!(!err.to_string().contains("AT"));
        }
    }

    #[tokio::test]
    async fn social_refresh_adopts_canonical_profile_arn_without_cross_account_cache_bleed() {
        let canonical_arn = "arn:aws:codewhisperer:us-east-1:699475941385:profile/CANONICAL";
        let mut server = mockito::Server::new_async().await;
        let token_mock = server
            .mock("POST", "/refreshToken")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "refreshToken": "SOCIAL-RT-1",
            })))
            .with_status(200)
            .with_body(
                serde_json::json!({
                    "accessToken": "SOCIAL-NEW-AT-1",
                    "refreshToken": "SOCIAL-ROTATED-RT-1",
                    "expiresIn": 3600,
                    "profileArn": canonical_arn,
                })
                .to_string(),
            )
            .expect(1)
            .create_async()
            .await;
        let usage_mock = server
            .mock("POST", "/")
            .match_header("authorization", "Bearer SOCIAL-NEW-AT-1")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "origin": "AI_EDITOR",
                "profileArn": canonical_arn,
                "resourceType": "AGENTIC_REQUEST",
            })))
            .with_status(200)
            .with_body(usage_json())
            .expect(2)
            .create_async()
            .await;

        let db_dir = TempDir::new().unwrap();
        let db_path = db_dir.path().join("data.sqlite3");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute("CREATE TABLE auth_kv (key TEXT, value TEXT)", [])
            .unwrap();
        conn.execute("CREATE TABLE state (key TEXT, value TEXT)", [])
            .unwrap();
        let social = serde_json::json!({
            "access_token": "SOCIAL-OLD-AT-1",
            "refresh_token": "SOCIAL-RT-1",
            "expires_at": "2000-01-01T00:00:00Z",
            "provider": "google",
            "profile_arn": "arn:aws:codewhisperer:us-east-1:699475941385:profile/INITIAL",
        })
        .to_string();
        conn.execute(
            "INSERT INTO auth_kv (key, value) VALUES ('kirocli:social:token', ?1)",
            [&social],
        )
        .unwrap();

        let (_cache_dir, cache) = cache_fixture();
        let endpoints = Endpoints {
            usage_limits: server.url(),
            token: format!("{}/refreshToken", server.url()),
        };
        let now = DateTime::parse_from_rfc3339("2026-08-03T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        for _ in 0..2 {
            let out = fetch_snapshot_at(
                http_client().unwrap(),
                &db_path,
                &cache,
                Duration::ZERO,
                Some(&endpoints),
                now,
            )
            .await
            .unwrap();
            assert_eq!(out.snapshot.used, 40.0);
        }
        token_mock.assert_async().await;
        usage_mock.assert_async().await;

        // Switching to a second social account in data.sqlite3 must not reuse
        // the first account's cached quota or persisted canonical profileArn.
        let second_arn = "arn:aws:codewhisperer:us-east-1:699475941385:profile/SECOND";
        let social_two = serde_json::json!({
            "access_token": "SOCIAL-AT-2",
            "refresh_token": "SOCIAL-RT-2",
            "expires_at": "2099-01-01T00:00:00Z",
            "provider": "github",
            "profile_arn": second_arn,
        })
        .to_string();
        conn.execute(
            "UPDATE auth_kv SET value = ?1 WHERE key = 'kirocli:social:token'",
            [&social_two],
        )
        .unwrap();
        let second_usage = server
            .mock("POST", "/")
            .match_header("authorization", "Bearer SOCIAL-AT-2")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "origin": "AI_EDITOR",
                "profileArn": second_arn,
                "resourceType": "AGENTIC_REQUEST",
            })))
            .with_status(200)
            .with_body(usage_json())
            .expect(1)
            .create_async()
            .await;

        fetch_snapshot_at(
            http_client().unwrap(),
            &db_path,
            &cache,
            Duration::from_secs(3600),
            Some(&endpoints),
            now,
        )
        .await
        .unwrap();
        second_usage.assert_async().await;
    }
}
