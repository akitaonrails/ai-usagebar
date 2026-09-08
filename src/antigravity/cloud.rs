//! Google Cloud Code client for the Antigravity "app closed" fallback.
//!
//! When no Antigravity product is running there is no loopback RPC to ask, but
//! the Google session Antigravity saved in the OS keyring (`credential.rs`)
//! is still valid. The same Cloud Code endpoints the product itself calls —
//! `retrieveUserQuotaSummary` for the quota windows and `loadCodeAssist` for
//! the plan tier — accept that bearer token directly. Access tokens live about
//! an hour; the refresh token is exchanged at Google's standard token endpoint
//! and the refreshed access token is persisted in ai-usagebar's own cache,
//! never written back to the keyring.
//!
//! Upstream error bodies are never surfaced: a rejected bearer token or an
//! `invalid_grant` response can carry account detail, so every non-2xx maps to
//! a fixed message — same rule as `kiro::oauth::refresh`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::cache::{Cache, atomic_write};
use crate::error::{AppError, Result};
use crate::vendor::{MAX_BODY_BYTES, read_body_capped};

const DAILY_BASE: &str = "https://daily-cloudcode-pa.googleapis.com";
const PROD_BASE: &str = "https://cloudcode-pa.googleapis.com";
const QUOTA_PATH: &str = "/v1internal:retrieveUserQuotaSummary";
const LOAD_CODE_ASSIST_PATH: &str = "/v1internal:loadCodeAssist";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const QUOTA_USER_AGENT: &str = "antigravity";
const LOAD_CODE_ASSIST_USER_AGENT: &str = "agy";
const OAUTH_CACHE_FILE: &str = "oauth.json";

/// Refresh this far ahead of the recorded expiry so a slow round-trip never
/// races the token's death. Mirrors `kiro::oauth::REFRESH_BUFFER_SECS`.
pub const REFRESH_BUFFER_SECS: i64 = 300;

/// Longest plan label kept from an unrecognised tier name.
const MAX_PLAN_CHARS: usize = 32;

const SESSION_REJECTED: &str = "Antigravity's Google session was rejected";
const REFRESH_FAILED: &str = "Antigravity token refresh failed";
const SESSION_EXPIRED: &str =
    "Antigravity's saved Google session expired; open Antigravity to sign in again";

/// Where the fallback talks to. Quota and plan each try every base in order
/// (the daily channel first, as the product does); tests point them at mockito.
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub quota: Vec<String>,
    pub load_code_assist: Vec<String>,
    pub token: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            quota: vec![
                format!("{DAILY_BASE}{QUOTA_PATH}"),
                format!("{PROD_BASE}{QUOTA_PATH}"),
            ],
            load_code_assist: vec![
                format!("{DAILY_BASE}{LOAD_CODE_ASSIST_PATH}"),
                format!("{PROD_BASE}{LOAD_CODE_ASSIST_PATH}"),
            ],
            token: TOKEN_URL.to_string(),
        }
    }
}

/// The OAuth client used for refresh: config overrides or the defaults.
#[derive(Debug, Clone)]
pub struct OauthClient {
    pub id: String,
    pub secret: String,
}

impl OauthClient {
    /// `[antigravity] oauth_client_id` + `oauth_client_secret`. Both are
    /// required for a refresh; nothing is embedded in source, so without them
    /// the saved access token is used only while it lasts. The pair is
    /// Antigravity's own public installed-app client (RFC 8252 §8.5: such a
    /// secret grants nothing on its own — the refresh token in the keyring is
    /// the credential), but a secret-shaped literal in a tracked file trips
    /// every secret scanner, so it is the user's line to write.
    pub fn from_config(id: Option<&str>, secret: Option<&str>) -> Option<Self> {
        let pick = |value: Option<&str>| {
            value
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        Some(Self {
            id: pick(id)?,
            secret: pick(secret)?,
        })
    }
}

/// A freshly exchanged access token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refreshed {
    pub access_token: String,
    pub expires_at: DateTime<Utc>,
}

/// ai-usagebar's own record of a refreshed access token, scoped to the
/// keyring session it was minted from (`credential::StoredToken::fingerprint`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PersistedOAuth {
    pub fingerprint: String,
    pub access_token: String,
    pub expires_at: DateTime<Utc>,
}

async fn post_json(
    client: &reqwest::Client,
    url: &str,
    access_token: &str,
    user_agent: &str,
) -> std::result::Result<reqwest::Response, reqwest::Error> {
    client
        .post(url)
        .timeout(HTTP_TIMEOUT)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("User-Agent", user_agent)
        .body("{}")
        .send()
        .await
}

/// Ask each quota base in turn for the user's quota summary. The raw JSON is
/// returned as-is — either the bare summary or the `{"response": …}` wrapper,
/// depending on the channel — for the caller to project.
///
/// A 401/403 stops immediately: the token is the problem, not the base. Any
/// other status or a transport failure moves on to the next base; when every
/// base failed the last error is returned.
pub async fn fetch_quota(
    client: &reqwest::Client,
    endpoints: &Endpoints,
    access_token: &str,
) -> Result<serde_json::Value> {
    let mut last_error = AppError::Transport("no Antigravity quota endpoint configured".into());
    for url in &endpoints.quota {
        let resp = match post_json(client, url, access_token, QUOTA_USER_AGENT).await {
            Ok(resp) => resp,
            Err(e) => {
                last_error = e.into();
                continue;
            }
        };
        let status = resp.status();
        if matches!(status.as_u16(), 401 | 403) {
            return Err(AppError::Http {
                status: status.as_u16(),
                body: SESSION_REJECTED.into(),
            });
        }
        if !status.is_success() {
            last_error = AppError::Http {
                status: status.as_u16(),
                body: format!(
                    "Antigravity quota endpoint returned HTTP {}",
                    status.as_u16()
                ),
            };
            continue;
        }
        match read_body_capped(resp, MAX_BODY_BYTES).await {
            Ok(bytes) => match serde_json::from_slice::<serde_json::Value>(&bytes) {
                Ok(value) => return Ok(value),
                Err(e) => {
                    last_error = AppError::Schema(format!("antigravity quota response: {e}"));
                }
            },
            Err(e) => last_error = e,
        }
    }
    Err(last_error)
}

/// Best-effort plan label from `loadCodeAssist`: the first base that answers
/// 2xx wins, `paidTier.name` beats `currentTier.name`, and any failure is
/// simply `None` — the quota figures are the product, the tier is a garnish.
pub async fn fetch_plan(
    client: &reqwest::Client,
    endpoints: &Endpoints,
    access_token: &str,
) -> Option<String> {
    for url in &endpoints.load_code_assist {
        let Ok(resp) = post_json(client, url, access_token, LOAD_CODE_ASSIST_USER_AGENT).await
        else {
            continue;
        };
        let status = resp.status();
        if matches!(status.as_u16(), 401 | 403) {
            return None;
        }
        if !status.is_success() {
            continue;
        }
        let Ok(bytes) = read_body_capped(resp, MAX_BODY_BYTES).await else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        return plan_from_value(&value);
    }
    None
}

fn plan_from_value(value: &serde_json::Value) -> Option<String> {
    let body = value.get("response").unwrap_or(value);
    ["paidTier", "currentTier"]
        .iter()
        .filter_map(|tier| body.get(tier)?.get("name")?.as_str())
        .map(format_plan)
        .find(|plan| !plan.is_empty())
}

/// Google's tier ids and display names both reach here (`google_ai_ultra`,
/// `Google AI Pro`, `free-tier`); the widget shows one short word.
pub fn format_plan(raw: &str) -> String {
    let words: Vec<String> = raw
        .split(|c: char| c.is_whitespace() || c == '_' || c == '-')
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect();
    for (needle, label) in [("ultra", "Ultra"), ("pro", "Pro"), ("free", "Free")] {
        if words.iter().any(|word| word == needle) {
            return label.to_string();
        }
    }
    let title: Vec<String> = words
        .iter()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect(),
                None => String::new(),
            }
        })
        .collect();
    title
        .join(" ")
        .chars()
        .take(MAX_PLAN_CHARS)
        .collect::<String>()
        .trim_end()
        .to_string()
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: Option<serde_json::Value>,
}

/// Exchange the keyring's refresh token for a new access token at `token_url`
/// ([`Endpoints::token`] in production; mockito in tests).
///
/// A definitive 4xx (anything but 408/429) means the session is gone —
/// revoked, or the refresh token rotated by a newer sign-in — and is reported
/// as a `Credentials` error that tells the user what to do. Everything else is
/// an `Http` error with a fixed body.
pub async fn refresh(
    client: &reqwest::Client,
    token_url: &str,
    oauth: &OauthClient,
    refresh_token: &str,
) -> Result<Refreshed> {
    let form = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", oauth.id.as_str()),
        ("client_secret", oauth.secret.as_str()),
    ];
    let resp = client
        .post(token_url)
        .timeout(HTTP_TIMEOUT)
        .header("Accept", "application/json")
        .form(&form)
        .send()
        .await?;
    let status = resp.status();
    let body = read_body_capped(resp, MAX_BODY_BYTES).await?;
    if !status.is_success() {
        let code = status.as_u16();
        if (400..500).contains(&code) && !matches!(code, 408 | 429) {
            return Err(AppError::Credentials(SESSION_EXPIRED.into()));
        }
        return Err(AppError::Http {
            status: code,
            body: REFRESH_FAILED.into(),
        });
    }
    let parsed: TokenResponse = serde_json::from_slice(&body)
        .map_err(|e| AppError::Schema(format!("antigravity token refresh response: {e}")))?;
    if parsed.access_token.trim().is_empty() {
        return Err(AppError::Schema(
            "antigravity token refresh response: access_token is empty".into(),
        ));
    }
    let expires_in = parsed
        .expires_in
        .as_ref()
        .and_then(expires_in_secs)
        .ok_or_else(|| {
            AppError::Schema("antigravity token refresh response: invalid expires_in".into())
        })?;
    let expires_at_secs = Utc::now()
        .timestamp()
        .checked_add(expires_in)
        .ok_or_else(|| AppError::Schema("antigravity token refresh expiry overflowed".into()))?;
    let expires_at = DateTime::from_timestamp(expires_at_secs, 0).ok_or_else(|| {
        AppError::Schema("antigravity token refresh expiry is out of range".into())
    })?;
    Ok(Refreshed {
        access_token: parsed.access_token,
        expires_at,
    })
}

/// Google returns `expires_in` as a JSON number; some proxies stringify it.
/// Only a positive integer within a sane range is accepted.
fn expires_in_secs(value: &serde_json::Value) -> Option<i64> {
    const MAX_SECS: i64 = 366 * 24 * 60 * 60;
    let secs = match value {
        serde_json::Value::Number(number) => number.as_i64()?,
        serde_json::Value::String(text) => text.trim().parse::<i64>().ok()?,
        _ => return None,
    };
    (1..=MAX_SECS).contains(&secs).then_some(secs)
}

/// `None` (no recorded expiry) always refreshes; otherwise refresh once the
/// token is within [`REFRESH_BUFFER_SECS`] of dying.
pub fn needs_refresh(expires_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    match expires_at {
        None => true,
        Some(expires_at) => expires_at.timestamp() < now.timestamp() + REFRESH_BUFFER_SECS,
    }
}

/// Where the refreshed access token is persisted, inside the vendor cache.
pub fn oauth_cache_path(cache: &Cache) -> PathBuf {
    cache.dir().join(OAUTH_CACHE_FILE)
}

/// The persisted token for exactly this keyring session. Absent, unreadable,
/// malformed, empty, or minted from a different session all read as `None`:
/// the worst case is one extra refresh round-trip.
pub fn read_persisted(path: &Path, fingerprint: &str) -> Option<PersistedOAuth> {
    let bytes = std::fs::read(path).ok()?;
    let persisted: PersistedOAuth = serde_json::from_slice(&bytes).ok()?;
    if persisted.fingerprint != fingerprint || persisted.access_token.trim().is_empty() {
        return None;
    }
    Some(persisted)
}

/// Atomically write the persisted token, owner-only on unix (kiro pattern).
pub fn write_persisted(path: &Path, value: &PersistedOAuth) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    atomic_write(path, &bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| AppError::io_at(path, e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Matcher;
    use tempfile::TempDir;

    fn endpoints(server: &mockito::Server) -> Endpoints {
        let base = server.url();
        Endpoints {
            quota: vec![format!("{base}/daily/quota"), format!("{base}/prod/quota")],
            load_code_assist: vec![format!("{base}/daily/plan"), format!("{base}/prod/plan")],
            token: format!("{base}/token"),
        }
    }

    #[test]
    fn default_endpoints_try_daily_then_prod() {
        let e = Endpoints::default();
        assert_eq!(
            e.quota,
            vec![
                "https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary",
                "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary",
            ]
        );
        assert_eq!(
            e.load_code_assist,
            vec![
                "https://daily-cloudcode-pa.googleapis.com/v1internal:loadCodeAssist",
                "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist",
            ]
        );
        assert_eq!(e.token, "https://oauth2.googleapis.com/token");
    }

    fn test_client() -> OauthClient {
        OauthClient {
            id: "test-client".into(),
            secret: "test-client-secret".into(),
        }
    }

    #[test]
    fn oauth_client_needs_both_halves_and_trims_them() {
        assert!(OauthClient::from_config(None, None).is_none());
        assert!(OauthClient::from_config(Some("id"), Some("")).is_none());
        assert!(OauthClient::from_config(Some(" "), Some("s")).is_none());
        let both = OauthClient::from_config(Some(" my-id "), Some(" my-secret ")).unwrap();
        assert_eq!(both.id, "my-id");
        assert_eq!(both.secret, "my-secret");
    }

    #[tokio::test]
    async fn quota_falls_through_from_daily_to_prod() {
        let mut server = mockito::Server::new_async().await;
        let daily = server
            .mock("POST", "/daily/quota")
            .match_header("authorization", "Bearer AT")
            .match_header("content-type", "application/json")
            .match_header("accept", "application/json")
            .match_header("user-agent", "antigravity")
            .match_body("{}")
            .with_status(500)
            .with_body("boom")
            .expect(1)
            .create_async()
            .await;
        let prod = server
            .mock("POST", "/prod/quota")
            .match_header("authorization", "Bearer AT")
            .match_header("user-agent", "antigravity")
            .with_status(200)
            .with_body(r#"{"response":{"buckets":[{"modelId":"gemini","remainingFraction":0.5}]}}"#)
            .expect(1)
            .create_async()
            .await;

        let value = fetch_quota(&reqwest::Client::new(), &endpoints(&server), "AT")
            .await
            .unwrap();
        assert_eq!(value["response"]["buckets"][0]["remainingFraction"], 0.5);
        daily.assert_async().await;
        prod.assert_async().await;
    }

    #[tokio::test]
    async fn quota_401_stops_without_trying_the_next_base() {
        let mut server = mockito::Server::new_async().await;
        let daily = server
            .mock("POST", "/daily/quota")
            .with_status(401)
            .with_body(r#"{"error":{"message":"sensitive detail"}}"#)
            .expect(1)
            .create_async()
            .await;
        let prod = server
            .mock("POST", "/prod/quota")
            .with_status(200)
            .with_body("{}")
            .expect(0)
            .create_async()
            .await;

        let err = fetch_quota(&reqwest::Client::new(), &endpoints(&server), "AT")
            .await
            .unwrap_err();
        match err {
            AppError::Http { status, body } => {
                assert_eq!(status, 401);
                assert_eq!(body, SESSION_REJECTED);
            }
            other => panic!("expected Http, got {other:?}"),
        }
        daily.assert_async().await;
        prod.assert_async().await;
    }

    #[tokio::test]
    async fn quota_reports_the_last_error_when_every_base_fails() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/daily/quota")
            .with_status(500)
            .create_async()
            .await;
        server
            .mock("POST", "/prod/quota")
            .with_status(503)
            .with_body("private upstream text")
            .create_async()
            .await;

        let err = fetch_quota(&reqwest::Client::new(), &endpoints(&server), "AT")
            .await
            .unwrap_err();
        match err {
            AppError::Http { status, body } => {
                assert_eq!(status, 503);
                assert!(!body.contains("private upstream text"));
            }
            other => panic!("expected Http, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn plan_prefers_paid_tier_and_sends_the_agy_user_agent() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/daily/plan")
            .match_header("authorization", "Bearer AT")
            .match_header("user-agent", "agy")
            .match_body("{}")
            .with_status(200)
            .with_body(
                r#"{"currentTier":{"id":"free-tier","name":"Free"},"paidTier":{"id":"google_ai_pro","name":"Google AI Pro"}}"#,
            )
            .create_async()
            .await;

        let plan = fetch_plan(&reqwest::Client::new(), &endpoints(&server), "AT").await;
        assert_eq!(plan.as_deref(), Some("Pro"));
        m.assert_async().await;
    }

    #[tokio::test]
    async fn plan_falls_back_to_current_tier_and_to_the_next_base() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/daily/plan")
            .with_status(500)
            .create_async()
            .await;
        server
            .mock("POST", "/prod/plan")
            .with_status(200)
            .with_body(r#"{"response":{"currentTier":{"name":"GOOGLE_AI_ULTRA"}}}"#)
            .create_async()
            .await;

        let plan = fetch_plan(&reqwest::Client::new(), &endpoints(&server), "AT").await;
        assert_eq!(plan.as_deref(), Some("Ultra"));
    }

    #[tokio::test]
    async fn plan_is_none_when_nothing_answers_usefully() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/daily/plan")
            .with_status(200)
            .with_body("not json")
            .create_async()
            .await;
        server
            .mock("POST", "/prod/plan")
            .with_status(200)
            .with_body(r#"{"currentTier":{"id":"x"}}"#)
            .create_async()
            .await;
        assert_eq!(
            fetch_plan(&reqwest::Client::new(), &endpoints(&server), "AT").await,
            None
        );
    }

    #[test]
    fn format_plan_normalises_known_tiers() {
        assert_eq!(format_plan("Google AI Pro"), "Pro");
        assert_eq!(format_plan("google_ai_ultra"), "Ultra");
        assert_eq!(format_plan("GOOGLE_AI_ULTRA"), "Ultra");
        assert_eq!(format_plan("free-tier"), "Free");
        assert_eq!(format_plan("Free"), "Free");
        assert_eq!(format_plan("  legacy_team plan "), "Legacy Team Plan");
        assert_eq!(format_plan(""), "");
        let long = format_plan(&"word ".repeat(20));
        assert!(long.chars().count() <= MAX_PLAN_CHARS);
        assert!(!long.ends_with(' '));
    }

    #[tokio::test]
    async fn refresh_sends_the_form_body_and_parses_the_token() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/token")
            .match_header("content-type", "application/x-www-form-urlencoded")
            .match_body(Matcher::AllOf(vec![
                Matcher::UrlEncoded("grant_type".into(), "refresh_token".into()),
                Matcher::UrlEncoded("refresh_token".into(), "old-rt".into()),
                Matcher::UrlEncoded("client_id".into(), "cid".into()),
                Matcher::UrlEncoded("client_secret".into(), "csecret".into()),
            ]))
            .with_status(200)
            .with_body(r#"{"access_token":"new-at","expires_in":3599,"token_type":"Bearer"}"#)
            .create_async()
            .await;
        let oauth = OauthClient {
            id: "cid".into(),
            secret: "csecret".into(),
        };
        let before = Utc::now();
        let refreshed = refresh(
            &reqwest::Client::new(),
            &format!("{}/token", server.url()),
            &oauth,
            "old-rt",
        )
        .await
        .unwrap();
        assert_eq!(refreshed.access_token, "new-at");
        let delta = refreshed.expires_at.timestamp() - before.timestamp();
        assert!((3590..=3610).contains(&delta), "{delta}");
        m.assert_async().await;
    }

    #[tokio::test]
    async fn refresh_400_is_a_credentials_error_that_does_not_echo_the_body() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/token")
            .with_status(400)
            .with_body(r#"{"error":"invalid_grant","error_description":"sensitive detail"}"#)
            .create_async()
            .await;
        let err = refresh(
            &reqwest::Client::new(),
            &format!("{}/token", server.url()),
            &test_client(),
            "old-rt",
        )
        .await
        .unwrap_err();
        assert!(matches!(err, AppError::Credentials(_)), "{err:?}");
        let text = err.to_string();
        assert!(!text.contains("sensitive detail"));
        assert!(!text.contains("invalid_grant"));
        assert!(text.contains("sign in again"));
    }

    #[tokio::test]
    async fn refresh_5xx_and_429_are_http_errors_with_a_fixed_body() {
        for status in [429u16, 503] {
            let mut server = mockito::Server::new_async().await;
            server
                .mock("POST", "/token")
                .with_status(status.into())
                .with_body("private upstream text")
                .create_async()
                .await;
            let err = refresh(
                &reqwest::Client::new(),
                &format!("{}/token", server.url()),
                &test_client(),
                "old-rt",
            )
            .await
            .unwrap_err();
            match err {
                AppError::Http { status: got, body } => {
                    assert_eq!(got, status);
                    assert_eq!(body, REFRESH_FAILED);
                }
                other => panic!("expected Http, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn refresh_rejects_malformed_success_bodies() {
        for body in [
            r#"{"access_token":"","expires_in":3600}"#,
            r#"{"access_token":"new","expires_in":0}"#,
            r#"{"access_token":"new","expires_in":"soon"}"#,
            r#"{"access_token":"new"}"#,
            "not json",
        ] {
            let mut server = mockito::Server::new_async().await;
            server
                .mock("POST", "/token")
                .with_status(200)
                .with_body(body)
                .create_async()
                .await;
            let err = refresh(
                &reqwest::Client::new(),
                &format!("{}/token", server.url()),
                &test_client(),
                "old-rt",
            )
            .await
            .unwrap_err();
            assert!(matches!(err, AppError::Schema(_)), "{body}: {err:?}");
        }
    }

    #[test]
    fn needs_refresh_threshold() {
        let now = DateTime::parse_from_rfc3339("2026-08-03T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(needs_refresh(None, now));
        assert!(needs_refresh(
            Some(now + chrono::Duration::seconds(REFRESH_BUFFER_SECS - 1)),
            now
        ));
        assert!(needs_refresh(Some(now - chrono::Duration::hours(1)), now));
        assert!(!needs_refresh(
            Some(now + chrono::Duration::seconds(REFRESH_BUFFER_SECS + 60)),
            now
        ));
    }

    #[test]
    fn persisted_token_round_trips_and_is_scoped_to_its_fingerprint() {
        let td = TempDir::new().unwrap();
        let cache = Cache::at(td.path().join("antigravity"));
        cache.ensure_dir().unwrap();
        let path = oauth_cache_path(&cache);
        assert_eq!(read_persisted(&path, "abcd"), None);

        let value = PersistedOAuth {
            fingerprint: "abcd".into(),
            access_token: "AT".into(),
            expires_at: DateTime::parse_from_rfc3339("2030-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        };
        write_persisted(&path, &value).unwrap();

        assert_eq!(read_persisted(&path, "abcd"), Some(value.clone()));
        assert_eq!(read_persisted(&path, "other"), None);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0);
        }
    }

    #[test]
    fn malformed_or_empty_persisted_token_reads_as_none() {
        let td = TempDir::new().unwrap();
        let path = td.path().join("oauth.json");
        std::fs::write(&path, b"{not json").unwrap();
        assert_eq!(read_persisted(&path, "abcd"), None);
        std::fs::write(
            &path,
            br#"{"fingerprint":"abcd","access_token":"  ","expires_at":"2030-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!(read_persisted(&path, "abcd"), None);
    }
}
