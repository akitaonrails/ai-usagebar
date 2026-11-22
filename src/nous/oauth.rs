//! Nous OAuth device flow and refresh state machine.

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
use crate::vendor::command_without_vendor_secrets;
use std::fmt;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde_json::Value;
use thiserror::Error;

use super::credentials::{CredentialDocument, CredentialStore, NousCredential};
use super::types::{DeviceCode, TokenResponse, parse_device_code, parse_token};

pub const CLIENT_ID: &str = "hermes-cli";
pub const SCOPE: &str = "inference:invoke";
pub const DEVICE_CODE_URL: &str = "https://portal.nousresearch.com/api/oauth/device/code";
pub const TOKEN_URL: &str = "https://portal.nousresearch.com/api/oauth/token";
pub const REFRESH_SKEW_SECONDS: i64 = 120;
pub const SLOW_DOWN_INCREMENT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    pub device_code: String,
    pub token: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            device_code: DEVICE_CODE_URL.into(),
            token: TOKEN_URL.into(),
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum OAuthError {
    #[error("OAuth transport failure")]
    Transport,
    #[error("OAuth HTTP request returned status {0}")]
    HttpStatus(u16),
    #[error("OAuth response schema mismatch")]
    Schema,
    #[error("OAuth server returned an unknown error")]
    UnknownOAuthError,
    #[error("authorization was denied")]
    AccessDenied,
    #[error("device authorization expired")]
    ExpiredToken,
    #[error("device authorization deadline elapsed")]
    Deadline,
    #[error("refresh authorization was rejected; login is required again")]
    RefreshTokenRejected,
    #[error("credential store failure")]
    Credentials,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollState {
    AuthorizationPending,
    SlowDown,
    Success,
    AccessDenied,
    ExpiredToken,
}

/// A browser launcher is injected so login tests never spawn a real browser.
pub trait BrowserOpener {
    fn open(&self, url: &str) -> std::io::Result<()>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemBrowserOpener;

impl BrowserOpener for SystemBrowserOpener {
    fn open(&self, url: &str) -> std::io::Result<()> {
        #[cfg(target_os = "linux")]
        {
            let _child = command_without_vendor_secrets("xdg-open")
                .arg(url)
                .spawn()?;
            Ok(())
        }
        #[cfg(target_os = "macos")]
        {
            let _child = command_without_vendor_secrets("/usr/bin/open")
                .arg(url)
                .spawn()?;
            Ok(())
        }
        #[cfg(target_os = "windows")]
        {
            // Keep the remotely supplied URL out of `cmd.exe`; metacharacters
            // such as `&` and `%` are data to Explorer, not shell syntax.
            let _child = command_without_vendor_secrets("explorer.exe")
                .arg(url)
                .spawn()?;
            Ok(())
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            let _ = url;
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "browser opening is unsupported",
            ))
        }
    }
}

/// Browser opening is intentionally best effort.  The CLI always prints the
/// sanitized verification URL separately, so a missing desktop opener is not a
/// failed authorization.
pub fn open_verification_url(url: &str, opener: &dyn BrowserOpener) -> bool {
    if !is_safe_portal_url(url) {
        return false;
    }
    opener.open(url).is_ok()
}

pub async fn request_device_code(
    client: &reqwest::Client,
    endpoints: &Endpoints,
) -> Result<DeviceCode, OAuthError> {
    let response = client
        .post(&endpoints.device_code)
        .header("content-type", "application/x-www-form-urlencoded")
        .form(&[("client_id", CLIENT_ID), ("scope", SCOPE)])
        .send()
        .await
        .map_err(|_| OAuthError::Transport)?;
    let status = response.status();
    if !status.is_success() {
        return Err(OAuthError::HttpStatus(status.as_u16()));
    }
    let body = crate::vendor::read_body_capped(response, crate::vendor::MAX_BODY_BYTES)
        .await
        .map_err(|_| OAuthError::Transport)?;
    let value: Value = serde_json::from_slice(&body).map_err(|_| OAuthError::Schema)?;
    parse_device_code(&value).map_err(|_| OAuthError::Schema)
}

pub fn classify_poll_response(status: u16, value: &Value) -> Result<PollState, OAuthError> {
    if (200..300).contains(&status) {
        parse_token(value).map_err(|_| OAuthError::Schema)?;
        return Ok(PollState::Success);
    }
    let code = value
        .get("error")
        .and_then(Value::as_str)
        .ok_or(OAuthError::UnknownOAuthError)?;
    match code {
        "authorization_pending" => Ok(PollState::AuthorizationPending),
        "slow_down" => Ok(PollState::SlowDown),
        "access_denied" => Ok(PollState::AccessDenied),
        "expired_token" => Ok(PollState::ExpiredToken),
        _ => Err(OAuthError::UnknownOAuthError),
    }
}

pub fn next_poll_interval(current: Duration, state: PollState) -> Duration {
    match state {
        PollState::SlowDown => current.saturating_add(SLOW_DOWN_INCREMENT),
        _ => current,
    }
}

/// Poll the token endpoint no faster than the server-provided interval.
pub async fn poll_for_token(
    client: &reqwest::Client,
    endpoint: &str,
    device: &DeviceCode,
) -> Result<TokenResponse, OAuthError> {
    let deadline = Instant::now().checked_add(Duration::from_secs(device.expires_in));
    let Some(deadline) = deadline else {
        return Err(OAuthError::Deadline);
    };
    let mut interval = Duration::from_secs(device.interval);
    loop {
        if Instant::now()
            .checked_add(interval)
            .is_none_or(|at| at > deadline)
        {
            return Err(OAuthError::Deadline);
        }
        tokio::time::sleep(interval).await;
        if Instant::now() > deadline {
            return Err(OAuthError::Deadline);
        }
        let (status, value) = poll_request(client, endpoint, &device.device_code).await?;
        match classify_poll_response(status, &value)? {
            PollState::AuthorizationPending => {}
            PollState::SlowDown => interval = next_poll_interval(interval, PollState::SlowDown),
            PollState::Success => return parse_token(&value).map_err(|_| OAuthError::Schema),
            PollState::AccessDenied => return Err(OAuthError::AccessDenied),
            PollState::ExpiredToken => return Err(OAuthError::ExpiredToken),
        }
    }
}

async fn poll_request(
    client: &reqwest::Client,
    endpoint: &str,
    device_code: &str,
) -> Result<(u16, Value), OAuthError> {
    let response = client
        .post(endpoint)
        .header("content-type", "application/x-www-form-urlencoded")
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("client_id", CLIENT_ID),
            ("device_code", device_code),
        ])
        .send()
        .await
        .map_err(|_| OAuthError::Transport)?;
    let status = response.status().as_u16();
    let body = crate::vendor::read_body_capped(response, crate::vendor::MAX_BODY_BYTES)
        .await
        .map_err(|_| OAuthError::Transport)?;
    let value = serde_json::from_slice(&body).map_err(|_| OAuthError::Schema)?;
    Ok((status, value))
}

pub async fn refresh_access_token(
    client: &reqwest::Client,
    endpoint: &str,
    refresh_token: &str,
) -> Result<TokenResponse, OAuthError> {
    if refresh_token.trim().is_empty() {
        return Err(OAuthError::Credentials);
    }
    let response = client
        .post(endpoint)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("x-nous-refresh-token", refresh_token)
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", CLIENT_ID),
            ("refresh_token", refresh_token),
        ])
        .send()
        .await
        .map_err(|_| OAuthError::Transport)?;
    let status = response.status();
    if !status.is_success() {
        // Portal uses 400 for an expired, revoked, reused, or otherwise invalid
        // refresh grant. All require a clean login; no body text is surfaced.
        if status.as_u16() == 400 {
            return Err(OAuthError::RefreshTokenRejected);
        }
        return Err(OAuthError::HttpStatus(status.as_u16()));
    }
    let body = crate::vendor::read_body_capped(response, crate::vendor::MAX_BODY_BYTES)
        .await
        .map_err(|_| OAuthError::Transport)?;
    let value: Value = serde_json::from_slice(&body).map_err(|_| OAuthError::Schema)?;
    parse_token(&value).map_err(|_| OAuthError::Schema)
}

pub fn needs_refresh(now: DateTime<Utc>, expires_at: DateTime<Utc>) -> bool {
    now.checked_add_signed(chrono::Duration::seconds(REFRESH_SKEW_SECONDS))
        .is_none_or(|threshold| expires_at <= threshold)
}

/// Lock, re-read, refresh at most once, and persist the complete rotated pair
/// before returning the access credential to an account fetcher.
pub async fn refresh_if_needed(
    client: &reqwest::Client,
    store: &CredentialStore,
    endpoint: &str,
    now: DateTime<Utc>,
) -> Result<NousCredential, OAuthError> {
    let lock = store
        .acquire_lock_async()
        .await
        .map_err(|_| OAuthError::Credentials)?;
    let document = store
        .read_unlocked()
        .map_err(|_| OAuthError::Credentials)?
        .ok_or(OAuthError::Credentials)?;
    let current = document
        .nous
        .as_ref()
        .ok_or(OAuthError::Credentials)?
        .clone();
    if !needs_refresh(now, current.expires_at) {
        drop(lock);
        return Ok(current);
    }
    let token = refresh_access_token(client, endpoint, &current.refresh_token).await?;
    let expires_at = token_expiration(now, token.expires_in)?;
    let replacement = NousCredential {
        client_id: CLIENT_ID.into(),
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        expires_at,
    };
    replacement.validate().map_err(|_| OAuthError::Schema)?;
    let mut replacement_document = document;
    replacement_document.nous = Some(replacement.clone());
    store
        .write_locked(&lock, &replacement_document)
        .map_err(|_| OAuthError::Credentials)?;
    drop(lock);
    Ok(replacement)
}

pub fn credential_from_token(
    token: TokenResponse,
    now: DateTime<Utc>,
) -> Result<NousCredential, OAuthError> {
    let credential = NousCredential {
        client_id: CLIENT_ID.into(),
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        expires_at: token_expiration(now, token.expires_in)?,
    };
    credential.validate().map_err(|_| OAuthError::Schema)?;
    Ok(credential)
}

pub fn persist_credential(
    store: &CredentialStore,
    credential: NousCredential,
) -> Result<(), OAuthError> {
    let lock = store.acquire_lock().map_err(|_| OAuthError::Credentials)?;
    let mut document = store
        .read_unlocked()
        .map_err(|_| OAuthError::Credentials)?
        .unwrap_or_else(|| CredentialDocument::new(None));
    document.nous = Some(credential);
    store
        .write_locked(&lock, &document)
        .map_err(|_| OAuthError::Credentials)
}

fn token_expiration(now: DateTime<Utc>, expires_in: u64) -> Result<DateTime<Utc>, OAuthError> {
    let seconds = i64::try_from(expires_in).map_err(|_| OAuthError::Schema)?;
    now.checked_add_signed(chrono::Duration::seconds(seconds))
        .ok_or(OAuthError::Schema)
}

pub(crate) fn is_safe_portal_url(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    parsed.scheme() == "https"
        && parsed.host_str() == Some("portal.nousresearch.com")
        && parsed.port_or_known_default() == Some(443)
        && parsed.username().is_empty()
        && parsed.password().is_none()
}

impl fmt::Display for PollState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::AuthorizationPending => "authorization_pending",
            Self::SlowDown => "slow_down",
            Self::Success => "success",
            Self::AccessDenied => "access_denied",
            Self::ExpiredToken => "expired_token",
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::{Duration as ChronoDuration, TimeZone, Utc};
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn device_request_uses_the_exact_portal_form_and_parses_response() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/device")
            .match_header(
                "content-type",
                mockito::Matcher::Regex("application/x-www-form-urlencoded.*".into()),
            )
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("client_id".into(), "hermes-cli".into()),
                mockito::Matcher::UrlEncoded("scope".into(), "inference:invoke".into()),
            ]))
            .with_status(200)
            .with_body(r#"{"device_code":"test-device","user_code":"TEST","verification_uri":"https://portal.nousresearch.com/device","verification_uri_complete":"https://portal.nousresearch.com/device?user_code=TEST","expires_in":900,"interval":5}"#)
            .create_async()
            .await;
        let endpoints = Endpoints {
            device_code: format!("{}/device", server.url()),
            token: format!("{}/token", server.url()),
        };

        let result = request_device_code(&reqwest::Client::new(), &endpoints)
            .await
            .unwrap();
        assert_eq!(result.device_code, "test-device");
        mock.assert_async().await;
    }

    #[test]
    fn poll_state_classifies_pending_slowdown_success_denial_expiry_and_unknown_errors() {
        assert_eq!(
            classify_poll_response(400, &json!({"error":"authorization_pending"})).unwrap(),
            PollState::AuthorizationPending
        );
        assert_eq!(
            classify_poll_response(400, &json!({"error":"slow_down"})).unwrap(),
            PollState::SlowDown
        );
        assert_eq!(
            classify_poll_response(400, &json!({"error":"access_denied"})).unwrap(),
            PollState::AccessDenied
        );
        assert_eq!(
            classify_poll_response(400, &json!({"error":"expired_token"})).unwrap(),
            PollState::ExpiredToken
        );
        assert_eq!(
            classify_poll_response(
                200,
                &json!({"access_token":"test-a","refresh_token":"test-r","token_type":"Bearer","expires_in":3600})
            )
            .unwrap(),
            PollState::Success
        );
        assert!(matches!(
            classify_poll_response(400, &json!({"error":"made_up"})),
            Err(OAuthError::UnknownOAuthError)
        ));
    }

    #[test]
    fn slow_down_adds_five_seconds_but_pending_keeps_the_authorized_interval() {
        assert_eq!(
            next_poll_interval(Duration::from_secs(5), PollState::AuthorizationPending),
            Duration::from_secs(5)
        );
        assert_eq!(
            next_poll_interval(Duration::from_secs(5), PollState::SlowDown),
            Duration::from_secs(10)
        );
    }

    #[test]
    fn refresh_threshold_is_exactly_120_seconds() {
        let now = Utc.with_ymd_and_hms(2026, 8, 16, 12, 0, 0).unwrap();
        assert!(!needs_refresh(now, now + ChronoDuration::seconds(121)));
        assert!(needs_refresh(now, now + ChronoDuration::seconds(120)));
        assert!(needs_refresh(now, now + ChronoDuration::seconds(119)));
        assert!(needs_refresh(now, now - ChronoDuration::seconds(1)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_contended_refresh_lock_keeps_the_runtime_responsive() {
        use std::future::{Future, poll_fn};
        use std::task::Poll;

        let root = tempfile::tempdir().unwrap();
        let store = CredentialStore::at(root.path().join("config/credentials.json"));
        let now = Utc.with_ymd_and_hms(2026, 10, 28, 12, 0, 0).unwrap();
        let credential = NousCredential {
            client_id: CLIENT_ID.into(),
            access_token: "test-access".into(),
            refresh_token: "test-refresh".into(),
            expires_at: now + ChronoDuration::hours(1),
        };
        store
            .write(&CredentialDocument::new(Some(credential.clone())))
            .unwrap();
        let client = reqwest::Client::new();
        let held = store.acquire_lock().unwrap();
        let (release, released) = std::sync::mpsc::channel();
        let holder = std::thread::spawn(move || {
            // A watchdog releases the lock even on the broken implementation,
            // so a regression fails instead of hanging the entire test suite.
            let runtime_progressed = released.recv_timeout(Duration::from_secs(10)).is_ok();
            drop(held);
            runtime_progressed
        });
        let mut refresh = Box::pin(refresh_if_needed(
            &client,
            &store,
            "http://127.0.0.1:1/unused",
            now,
        ));
        // A contended async lock must yield on its first poll. Release the
        // holder only after observing that yield, not after a timing estimate.
        let first_poll = poll_fn(|cx| Poll::Ready(refresh.as_mut().poll(cx))).await;
        let _ = release.send(());
        assert!(
            holder.join().unwrap(),
            "credential lock blocked the runtime until the watchdog released it"
        );
        assert!(
            first_poll.is_pending(),
            "refresh must wait for the existing holder"
        );
        assert_eq!(refresh.await.unwrap(), credential);
    }

    #[tokio::test]
    async fn cancelling_a_lock_wait_does_not_leave_a_waiter_or_release_the_holder() {
        let root = tempfile::tempdir().unwrap();
        let store = CredentialStore::at(root.path().join("config/credentials.json"));
        let held = store.acquire_lock().unwrap();
        for _ in 0..2 {
            assert!(
                tokio::time::timeout(Duration::from_millis(20), store.acquire_lock_async())
                    .await
                    .is_err()
            );
        }
        drop(held);
        let guard = tokio::time::timeout(Duration::from_secs(2), store.acquire_lock_async())
            .await
            .unwrap()
            .unwrap();
        drop(guard);
    }

    #[tokio::test]
    async fn refresh_request_uses_header_and_required_form_without_secret_in_url() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/token")
            .match_header("x-nous-refresh-token", "test-old-refresh")
            .match_header(
                "content-type",
                mockito::Matcher::Regex("application/x-www-form-urlencoded.*".into()),
            )
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("grant_type".into(), "refresh_token".into()),
                mockito::Matcher::UrlEncoded("client_id".into(), "hermes-cli".into()),
                mockito::Matcher::UrlEncoded(
                    "refresh_token".into(),
                    "test-old-refresh".into(),
                ),
            ]))
            .with_status(200)
            .with_body(r#"{"access_token":"test-new-access","refresh_token":"test-new-refresh","token_type":"Bearer","expires_in":3600}"#)
            .create_async()
            .await;
        let token = refresh_access_token(
            &reqwest::Client::new(),
            &format!("{}/token", server.url()),
            "test-old-refresh",
        )
        .await
        .unwrap();
        assert_eq!(token.access_token, "test-new-access");
        mock.assert_async().await;
    }

    #[test]
    fn browser_open_failure_is_nonfatal_and_error_debug_is_redacted() {
        struct FailingBrowser;
        impl BrowserOpener for FailingBrowser {
            fn open(&self, _url: &str) -> std::io::Result<()> {
                Err(std::io::Error::other("test failure"))
            }
        }
        assert!(!open_verification_url(
            "https://portal.nousresearch.com/device",
            &FailingBrowser
        ));
        let error = OAuthError::HttpStatus(401);
        assert!(!format!("{error:?}").contains("test-access-token"));
    }

    #[test]
    fn browser_opener_accepts_only_the_production_portal_origin() {
        use std::cell::Cell;

        struct RecordingBrowser(Cell<usize>);
        impl BrowserOpener for RecordingBrowser {
            fn open(&self, _url: &str) -> std::io::Result<()> {
                self.0.set(self.0.get() + 1);
                Ok(())
            }
        }

        let browser = RecordingBrowser(Cell::new(0));
        assert!(open_verification_url(
            "https://portal.nousresearch.com/device?user_code=TEST",
            &browser
        ));
        for unsafe_url in [
            "https://portal.nousresearch.com.evil.test/device",
            "https://evil.test/device&calc.exe",
            "http://portal.nousresearch.com/device",
            "https://user@portal.nousresearch.com/device",
        ] {
            assert!(!open_verification_url(unsafe_url, &browser));
        }
        assert_eq!(browser.0.get(), 1, "rejected URLs must never reach the OS");
    }

    #[test]
    fn token_expiration_rejects_overflow_instead_of_wrapping_or_panicking() {
        let now = Utc.with_ymd_and_hms(2026, 8, 16, 12, 0, 0).unwrap();
        assert_eq!(
            token_expiration(now, 3600).unwrap(),
            now + ChronoDuration::hours(1)
        );
        assert_eq!(token_expiration(now, u64::MAX), Err(OAuthError::Schema));
        assert_eq!(
            token_expiration(DateTime::<Utc>::MAX_UTC, 1),
            Err(OAuthError::Schema)
        );
    }

    #[tokio::test]
    async fn any_bad_refresh_grant_requires_a_clean_login() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/token")
            .with_status(400)
            .with_body("non-json error body that must not affect classification")
            .create_async()
            .await;

        let error = refresh_access_token(
            &reqwest::Client::new(),
            &format!("{}/token", server.url()),
            "test-old-refresh",
        )
        .await
        .unwrap_err();
        assert_eq!(error, OAuthError::RefreshTokenRejected);
    }
}
