//! Read the Factory subscription limits and keep only normalized metrics in
//! the local cache.
//!
//! The JWT travels only as the `Authorization: Bearer` header of a GET to the
//! fixed Factory host, over a client that rejects every redirect. Neither the
//! raw response nor the credential is persisted — the cache holds the
//! projected windows and the login's truncated fingerprint.

use std::sync::LazyLock;
use std::time::Duration;

use crate::cache::{Cache, acquire_lock_async};
use crate::error::{AppError, Result};
use crate::usage::{DroidSnapshot, UsageWindow};
use crate::vendor::{MAX_BODY_BYTES, read_body_capped};

use super::creds::{self, Credentials};
use super::{renew, types};

const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const LOCK_TIMEOUT: Duration = Duration::from_secs(15);
const LIMITS_PATH: &str = "/api/billing/limits";
pub const API_BASE: &str = "https://api.factory.ai";

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: String,
}

impl Endpoints {
    pub fn limits_url(&self) -> String {
        format!("{}{}", self.base.trim_end_matches('/'), LIMITS_PATH)
    }
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: API_BASE.to_string(),
        }
    }
}

pub type FetchOutcome = crate::outcome::Outcome<DroidSnapshot>;

/// The client refuses every redirect, so the CLI token is never forwarded or
/// re-sent through one, including a same-origin redirect.
fn build_client(timeout: Duration) -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
}

/// Droid keeps its own client instead of the process-wide one the widget and
/// TUI share: that client follows redirects, and this provider must never
/// send the CLI token through one.
static CLIENT: LazyLock<Option<reqwest::Client>> =
    LazyLock::new(|| build_client(HTTP_TIMEOUT).ok());

fn http_client() -> Result<&'static reqwest::Client> {
    CLIENT
        .as_ref()
        .ok_or_else(|| AppError::Other("Droid HTTP client initialization failed".into()))
}

/// Production entry point; the CLI login is resolved on every call — before
/// any client or cache directory is touched — so a changed login is reported
/// before a stale cache can answer for it. A JWT whose `exp` has passed is
/// never sent to the API: it gets the same cache fallback a 401 does.
pub async fn fetch_snapshot(
    cfg: &crate::config::DroidConfig,
    cache: &Cache,
    cache_ttl: Duration,
) -> Result<FetchOutcome> {
    let mut credentials = super::resolve_credentials(cfg)?;
    if cfg.cli_renewal && credentials.expires_within(renew::RENEW_MARGIN, chrono::Utc::now()) {
        credentials = renew_login(cfg, cache, credentials).await?;
    }
    fetch_snapshot_with(
        http_client()?,
        &credentials,
        cache,
        &Endpoints::default(),
        cache_ttl,
    )
    .await
}

/// Let the `droid` CLI renew a login that is expiring, then read it again.
/// Under the vendor lock, so two bars refreshing together spawn the CLI once;
/// the login is re-read first because the other one may already have done it.
async fn renew_login(
    cfg: &crate::config::DroidConfig,
    cache: &Cache,
    credentials: Credentials,
) -> Result<Credentials> {
    cache.ensure_dir()?;
    let _lock = acquire_lock_async(&cache.lock_path(), LOCK_TIMEOUT).await?;
    let current = super::resolve_credentials(cfg)?;
    if !current.expires_within(renew::RENEW_MARGIN, chrono::Utc::now()) {
        return Ok(current);
    }
    if renew::renew(cfg, cache).await {
        // A renewal the CLI could not finish leaves the old login in place,
        // which the expired path below answers like before.
        return super::resolve_credentials(cfg).or(Ok(credentials));
    }
    Ok(current)
}

async fn fetch_snapshot_with(
    client: &reqwest::Client,
    credentials: &Credentials,
    cache: &Cache,
    endpoints: &Endpoints,
    cache_ttl: Duration,
) -> Result<FetchOutcome> {
    cache.ensure_dir()?;
    let _lock = acquire_lock_async(&cache.lock_path(), LOCK_TIMEOUT).await?;

    // A JWT whose `exp` already passed would 401: skip the wire call and run
    // the same cache fallback a rejected token gets — the last good payload
    // plus the re-login note when a cache holds one.
    if credentials.expired {
        let error = AppError::Credentials(creds::EXPIRED_LOGIN_MESSAGE.into());
        cache.mark_stale();
        let (code, message) = error_to_pair(&error);
        let diagnostic = cache.write_last_error(code, &message);
        return crate::outcome::fallback(cache, Some(diagnostic), error, |bytes| {
            parse_cache_at(bytes, &credentials.fingerprint)
        });
    }

    if let Some(bytes) = cache.fresh_payload(cache_ttl)?
        && let Ok(outcome) = reuse_cache(&bytes, cache, &credentials.fingerprint)
    {
        return Ok(outcome);
    }

    match limits_call(client, credentials, endpoints).await {
        Ok(snapshot) => {
            cache.write_payload(&serde_json::to_vec(&snapshot_to_cache(
                &snapshot,
                &credentials.fingerprint,
            ))?)?;
            Ok(crate::outcome::Outcome::fresh(snapshot))
        }
        Err(error) if error.is_transient() => {
            crate::outcome::fallback(cache, None, error, |bytes| {
                parse_cache_at(bytes, &credentials.fingerprint)
            })
        }
        Err(error) => {
            cache.mark_stale();
            let (code, message) = error_to_pair(&error);
            let diagnostic = cache.write_last_error(code, &message);
            crate::outcome::fallback(cache, Some(diagnostic), error, |bytes| {
                parse_cache_at(bytes, &credentials.fingerprint)
            })
        }
    }
}

async fn limits_call(
    client: &reqwest::Client,
    credentials: &Credentials,
    endpoints: &Endpoints,
) -> Result<DroidSnapshot> {
    let response = client
        .get(endpoints.limits_url())
        .header(reqwest::header::ACCEPT, "application/json")
        .bearer_auth(&credentials.access_token)
        .send()
        .await
        .map_err(|error| {
            AppError::Transport(if error.is_timeout() {
                "Droid limits request timed out".into()
            } else {
                "Droid limits request failed".into()
            })
        })?;

    let status = response.status();
    let bytes = read_body_capped(response, MAX_BODY_BYTES).await?;
    if status.as_u16() == 401 {
        // A rejected token means the CLI's JWT lapsed between the exp check
        // and the request. The `droid` CLI is what renews it — say so. Only
        // 401 gets this: a 403 is an ordinary HTTP error, like elsewhere.
        return Err(AppError::Credentials(creds::EXPIRED_LOGIN_MESSAGE.into()));
    }
    if !status.is_success() {
        // The response body is never copied into the error: an error from a
        // token-bearing endpoint may echo account detail.
        return Err(AppError::Http {
            status: status.as_u16(),
            body: format!("Droid limits endpoint returned HTTP {}", status.as_u16()),
        });
    }
    types::parse_response(&bytes)
}

/// Only an HTTP status or a schema message survives into `.last_error` and
/// the tooltip. Everything else collapses to fixed text on purpose: those
/// errors can carry a path or a request detail, and this is a token-bearing
/// request. A credentials error keeps its own message — it is a fixed string
/// that tells the user the one useful thing (run `droid` once).
fn error_to_pair(error: &AppError) -> (u16, String) {
    match error {
        AppError::Http { status, body } => (*status, body.clone()),
        AppError::Schema(message) => (0, message.clone()),
        AppError::Credentials(message) => (0, message.clone()),
        _ => (0, "Droid limits request failed; details suppressed".into()),
    }
}

fn reuse_cache(bytes: &[u8], cache: &Cache, fingerprint: &str) -> Result<FetchOutcome> {
    let snapshot = parse_cache_at(bytes, fingerprint)?;
    Ok(crate::outcome::Outcome::cached(snapshot, cache, false))
}

fn snapshot_to_cache(snapshot: &DroidSnapshot, fingerprint: &str) -> serde_json::Value {
    let window = |window: &Option<UsageWindow>| match window {
        Some(window) => serde_json::json!({
            "used_pct": window.utilization_pct,
            "reset_unix": window.resets_at.map(|at| at.timestamp()),
        }),
        None => serde_json::Value::Null,
    };
    serde_json::json!({
        "account": fingerprint,
        "session": window(&snapshot.session),
        "weekly": window(&snapshot.weekly),
        "monthly": window(&snapshot.monthly),
        "core_session": window(&snapshot.core_session),
        "core_weekly": window(&snapshot.core_weekly),
        "core_monthly": window(&snapshot.core_monthly),
        "extra_balance_cents": snapshot.extra_balance_cents,
    })
}

fn parse_cache_at(bytes: &[u8], fingerprint: &str) -> Result<DroidSnapshot> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|_| AppError::Schema("Droid cache was invalid JSON".into()))?;
    if value.get("account").and_then(serde_json::Value::as_str) != Some(fingerprint) {
        return Err(AppError::Schema(
            "Droid cache belongs to a different login; refetching".into(),
        ));
    }
    let invalid = |field: &str| AppError::Schema(format!("Droid cache: invalid {field}"));
    let window = |field: &str, duration| -> Result<Option<UsageWindow>> {
        let Some(value) = value.get(field) else {
            return Err(invalid(field));
        };
        if value.is_null() {
            return Ok(None);
        }
        let pct = value
            .get("used_pct")
            .and_then(serde_json::Value::as_i64)
            .filter(|pct| (0..=100).contains(pct))
            .ok_or_else(|| invalid(field))? as i32;
        let reset = match value.get("reset_unix") {
            None | Some(serde_json::Value::Null) => None,
            Some(value) => {
                let seconds = value
                    .as_i64()
                    .filter(|seconds| *seconds >= 0)
                    .ok_or_else(|| invalid(field))?;
                Some(chrono::DateTime::from_timestamp(seconds, 0).ok_or_else(|| invalid(field))?)
            }
        };
        Ok(Some(UsageWindow {
            utilization_pct: pct,
            resets_at: reset,
            window_duration: duration,
        }))
    };
    let extra_balance_cents = match value.get("extra_balance_cents") {
        Some(serde_json::Value::Null) => None,
        Some(value) => Some(value.as_i64().ok_or_else(|| invalid("extra balance"))?),
        None => return Err(invalid("extra balance")),
    };
    Ok(DroidSnapshot {
        session: window("session", types::SESSION_WINDOW)?,
        weekly: window("weekly", types::WEEKLY_WINDOW)?,
        monthly: window("monthly", types::MONTHLY_WINDOW)?,
        core_session: window("core_session", types::SESSION_WINDOW)?,
        core_weekly: window("core_weekly", types::WEEKLY_WINDOW)?,
        core_monthly: window("core_monthly", types::MONTHLY_WINDOW)?,
        extra_balance_cents,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const CANARY: &str = "DroidTokenCanary-748201";

    fn credentials() -> Credentials {
        Credentials {
            access_token: CANARY.to_string(),
            fingerprint: crate::cache::fingerprint_of(CANARY),
            expires_at: None,
            expired: false,
        }
    }

    fn cache_fixture() -> (TempDir, Cache) {
        let dir = TempDir::new().unwrap();
        let cache = Cache::at(dir.path().join("droid"));
        cache.ensure_dir().unwrap();
        (dir, cache)
    }

    fn safe_client() -> reqwest::Client {
        http_client().unwrap().clone()
    }

    fn response_body() -> String {
        serde_json::json!({
            "limits": {
                "standard": {
                    "fiveHour": {"usedPercent": 42, "windowEnd": "2026-10-22T01:38:45Z"},
                    "weekly": {"usedPercent": 31, "windowEnd": "2026-10-28T20:38:45Z"},
                    "monthly": {"usedPercent": 11, "windowEnd": null}
                },
                "core": {
                    "fiveHour": {"usedPercent": 0, "windowEnd": null},
                    "weekly": {"usedPercent": 0, "windowEnd": null},
                    "monthly": {"usedPercent": 0, "windowEnd": null}
                }
            },
            "extraUsageBalanceCents": 250
        })
        .to_string()
    }

    fn request_mock(server: &mut mockito::ServerGuard) -> mockito::Mock {
        server
            .mock("GET", LIMITS_PATH)
            .match_header("authorization", format!("Bearer {CANARY}").as_str())
            .match_header("accept", "application/json")
    }

    fn snapshot() -> DroidSnapshot {
        let window = |pct, reset, duration| {
            Some(UsageWindow {
                utilization_pct: pct,
                resets_at: reset,
                window_duration: duration,
            })
        };
        DroidSnapshot {
            session: window(
                42,
                chrono::DateTime::from_timestamp(1_791_014_400, 0),
                types::SESSION_WINDOW,
            ),
            weekly: window(31, None, types::WEEKLY_WINDOW),
            monthly: None,
            core_session: window(0, None, types::SESSION_WINDOW),
            core_weekly: None,
            core_monthly: None,
            extra_balance_cents: Some(250),
        }
    }

    #[tokio::test]
    async fn request_sends_the_bearer_and_cache_keeps_only_normalized_values() {
        let credentials = credentials();
        let mut server = mockito::Server::new_async().await;
        let request = request_mock(&mut server)
            .with_status(200)
            .with_body(response_body())
            .create_async()
            .await;
        let (_cache_dir, cache) = cache_fixture();
        let outcome = fetch_snapshot_with(
            &safe_client(),
            &credentials,
            &cache,
            &Endpoints { base: server.url() },
            Duration::ZERO,
        )
        .await
        .unwrap();
        request.assert_async().await;
        assert_eq!(
            outcome.snapshot.session.as_ref().unwrap().utilization_pct,
            42
        );
        assert_eq!(
            outcome.snapshot.monthly.as_ref().unwrap().utilization_pct,
            11
        );
        assert_eq!(outcome.snapshot.monthly.as_ref().unwrap().resets_at, None);
        assert_eq!(outcome.snapshot.extra_balance_cents, Some(250));
        let stored = std::fs::read_to_string(cache.payload_path()).unwrap();
        assert!(!stored.contains(CANARY));
        assert!(!stored.contains("usedPercent"));
        assert_eq!(
            parse_cache_at(stored.as_bytes(), &credentials.fingerprint).unwrap(),
            outcome.snapshot
        );
    }

    #[tokio::test]
    async fn a_rejected_token_reports_the_droid_refresh_fix() {
        let credentials = credentials();
        let mut server = mockito::Server::new_async().await;
        let request = request_mock(&mut server)
            .with_status(401)
            .with_body(format!("credential echo: {CANARY}"))
            .create_async()
            .await;
        let (_cache_dir, cache) = cache_fixture();

        let error = fetch_snapshot_with(
            &safe_client(),
            &credentials,
            &cache,
            &Endpoints { base: server.url() },
            Duration::ZERO,
        )
        .await
        .unwrap_err();

        request.assert_async().await;
        assert!(matches!(error, AppError::Credentials(_)), "{error:?}");
        assert!(error.to_string().contains(creds::EXPIRED_LOGIN_MESSAGE));
        assert!(!error.to_string().contains(CANARY));
    }

    #[tokio::test]
    async fn a_forbidden_response_is_a_plain_http_error() {
        let credentials = credentials();
        let mut server = mockito::Server::new_async().await;
        let request = request_mock(&mut server)
            .with_status(403)
            .with_body(format!("credential echo: {CANARY}"))
            .create_async()
            .await;
        let (_cache_dir, cache) = cache_fixture();

        let error = fetch_snapshot_with(
            &safe_client(),
            &credentials,
            &cache,
            &Endpoints { base: server.url() },
            Duration::ZERO,
        )
        .await
        .unwrap_err();

        request.assert_async().await;
        assert!(
            matches!(error, AppError::Http { status: 403, .. }),
            "{error:?}"
        );
        assert!(!error.to_string().contains(creds::EXPIRED_LOGIN_MESSAGE));
        assert!(!error.to_string().contains(CANARY));
    }

    #[tokio::test]
    async fn an_expired_jwt_with_a_cache_serves_the_last_good_payload() {
        let mut credentials = credentials();
        credentials.expired = true;
        let mut server = mockito::Server::new_async().await;
        // The wire is never touched: an expired token is not even sent.
        let request = request_mock(&mut server).expect(0).create_async().await;
        let (_cache_dir, cache) = cache_fixture();
        cache
            .write_payload(
                &serde_json::to_vec(&snapshot_to_cache(&snapshot(), &credentials.fingerprint))
                    .unwrap(),
            )
            .unwrap();

        let outcome = fetch_snapshot_with(
            &safe_client(),
            &credentials,
            &cache,
            &Endpoints { base: server.url() },
            Duration::ZERO,
        )
        .await
        .unwrap();

        request.assert_async().await;
        assert!(outcome.stale);
        assert_eq!(outcome.snapshot, snapshot());
        let (_, message) = outcome.last_error.unwrap();
        assert!(message.contains(creds::EXPIRED_LOGIN_MESSAGE));
        assert!(!message.contains(CANARY));
    }

    #[tokio::test]
    async fn an_expired_jwt_without_a_cache_reports_the_relogin_fix() {
        let mut credentials = credentials();
        credentials.expired = true;
        let mut server = mockito::Server::new_async().await;
        let request = request_mock(&mut server).expect(0).create_async().await;
        let (_cache_dir, cache) = cache_fixture();

        let error = fetch_snapshot_with(
            &safe_client(),
            &credentials,
            &cache,
            &Endpoints { base: server.url() },
            Duration::ZERO,
        )
        .await
        .unwrap_err();

        request.assert_async().await;
        assert!(matches!(error, AppError::Credentials(_)), "{error:?}");
        assert!(error.to_string().contains(creds::EXPIRED_LOGIN_MESSAGE));
    }

    #[tokio::test]
    async fn a_rejected_token_with_a_cache_serves_the_last_good_payload() {
        let credentials = credentials();
        let mut server = mockito::Server::new_async().await;
        let request = request_mock(&mut server)
            .with_status(401)
            .with_body(format!("credential echo: {CANARY}"))
            .create_async()
            .await;
        let (_cache_dir, cache) = cache_fixture();
        cache
            .write_payload(
                &serde_json::to_vec(&snapshot_to_cache(&snapshot(), &credentials.fingerprint))
                    .unwrap(),
            )
            .unwrap();

        let outcome = fetch_snapshot_with(
            &safe_client(),
            &credentials,
            &cache,
            &Endpoints { base: server.url() },
            Duration::ZERO,
        )
        .await
        .unwrap();

        request.assert_async().await;
        assert!(outcome.stale);
        assert_eq!(outcome.snapshot, snapshot());
        let (_, message) = outcome.last_error.unwrap();
        assert!(message.contains(creds::EXPIRED_LOGIN_MESSAGE));
        assert!(!message.contains(CANARY));
        for item in std::fs::read_dir(cache.dir()).unwrap() {
            let item = item.unwrap();
            if item.file_type().unwrap().is_file() {
                let bytes = std::fs::read(item.path()).unwrap();
                assert!(!String::from_utf8_lossy(&bytes).contains(CANARY));
            }
        }
    }

    #[tokio::test]
    async fn redirects_are_not_followed_with_the_cli_token() {
        let credentials = credentials();
        let mut server = mockito::Server::new_async().await;
        let request = request_mock(&mut server)
            .with_status(307)
            .with_header("location", "https://attacker.invalid/collect")
            .create_async()
            .await;
        let redirected = server
            .mock("GET", "/collect")
            .expect(0)
            .create_async()
            .await;
        let (_cache_dir, cache) = cache_fixture();
        let error = fetch_snapshot_with(
            &safe_client(),
            &credentials,
            &cache,
            &Endpoints { base: server.url() },
            Duration::ZERO,
        )
        .await
        .unwrap_err();
        request.assert_async().await;
        redirected.assert_async().await;
        assert!(matches!(error, AppError::Http { status: 307, .. }));
        assert!(!error.to_string().contains(CANARY));
    }

    #[tokio::test]
    async fn a_cache_for_another_login_is_refetched_not_reused() {
        let mut server = mockito::Server::new_async().await;
        let request = request_mock(&mut server)
            .with_status(200)
            .with_body(response_body())
            .create_async()
            .await;
        let (_cache_dir, cache) = cache_fixture();
        let old = Credentials {
            access_token: "old-account-token".to_string(),
            fingerprint: crate::cache::fingerprint_of("old-account-token"),
            expires_at: None,
            expired: false,
        };
        let mut old_snapshot = snapshot();
        old_snapshot.session.as_mut().unwrap().utilization_pct = 99;
        cache
            .write_payload(
                &serde_json::to_vec(&snapshot_to_cache(&old_snapshot, &old.fingerprint)).unwrap(),
            )
            .unwrap();

        let outcome = fetch_snapshot_with(
            &safe_client(),
            &credentials(),
            &cache,
            &Endpoints { base: server.url() },
            Duration::from_secs(3600),
        )
        .await
        .unwrap();

        request.assert_async().await;
        assert_eq!(
            outcome.snapshot.session.as_ref().unwrap().utilization_pct,
            42
        );
    }

    #[tokio::test]
    async fn a_429_arms_the_backoff_and_the_next_refresh_stays_off_the_wire() {
        let credentials = credentials();
        let mut server = mockito::Server::new_async().await;
        let request = request_mock(&mut server)
            .with_status(429)
            .with_body(format!("credential echo: {CANARY}"))
            .expect(1)
            .create_async()
            .await;
        let (_cache_dir, cache) = cache_fixture();
        let endpoints = Endpoints { base: server.url() };

        let error = fetch_snapshot_with(
            &safe_client(),
            &credentials,
            &cache,
            &endpoints,
            Duration::ZERO,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(error, AppError::Http { status: 429, .. }),
            "{error:?}"
        );
        assert!(!error.to_string().contains(CANARY));
        assert!(
            cache
                .backoff_remaining_at(std::time::SystemTime::now())
                .is_some()
        );

        // Armed: the second refresh is refused before any request.
        let error = fetch_snapshot_with(
            &safe_client(),
            &credentials,
            &cache,
            &endpoints,
            Duration::ZERO,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(error, AppError::Http { status: 429, .. }),
            "{error:?}"
        );
        request.assert_async().await;
    }

    #[tokio::test]
    async fn a_429_with_a_cache_keeps_serving_the_last_good_payload() {
        let credentials = credentials();
        let mut server = mockito::Server::new_async().await;
        let request = request_mock(&mut server)
            .with_status(429)
            .expect(1)
            .create_async()
            .await;
        let (_cache_dir, cache) = cache_fixture();
        cache
            .write_payload(
                &serde_json::to_vec(&snapshot_to_cache(&snapshot(), &credentials.fingerprint))
                    .unwrap(),
            )
            .unwrap();
        let endpoints = Endpoints { base: server.url() };

        for _ in 0..2 {
            let outcome = fetch_snapshot_with(
                &safe_client(),
                &credentials,
                &cache,
                &endpoints,
                Duration::ZERO,
            )
            .await
            .unwrap();
            assert_eq!(outcome.snapshot, snapshot());
        }
        request.assert_async().await;
    }

    #[tokio::test]
    async fn an_oversized_body_is_refused_without_echoing_it() {
        let credentials = credentials();
        let mut server = mockito::Server::new_async().await;
        let request = request_mock(&mut server)
            .with_status(200)
            .with_body(format!("{CANARY}{}", "x".repeat(MAX_BODY_BYTES + 1)))
            .create_async()
            .await;
        let (_cache_dir, cache) = cache_fixture();

        let error = fetch_snapshot_with(
            &safe_client(),
            &credentials,
            &cache,
            &Endpoints { base: server.url() },
            Duration::ZERO,
        )
        .await
        .unwrap_err();

        request.assert_async().await;
        assert!(matches!(error, AppError::Schema(_)), "{error:?}");
        assert!(!error.to_string().contains(CANARY));
        assert!(!cache.payload_path().exists());
    }

    #[tokio::test]
    async fn a_silent_server_times_out_and_falls_back_to_the_cache() {
        // Accepts the connection and never answers.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let _server = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                held.push(socket);
            }
        });
        let client = build_client(Duration::from_millis(300)).unwrap();
        let credentials = credentials();
        let (_cache_dir, cache) = cache_fixture();
        let endpoints = Endpoints {
            base: format!("http://{address}"),
        };

        let error = fetch_snapshot_with(&client, &credentials, &cache, &endpoints, Duration::ZERO)
            .await
            .unwrap_err();
        assert!(
            matches!(&error, AppError::Transport(message) if message.contains("timed out")),
            "{error:?}"
        );

        cache
            .write_payload(
                &serde_json::to_vec(&snapshot_to_cache(&snapshot(), &credentials.fingerprint))
                    .unwrap(),
            )
            .unwrap();
        let outcome =
            fetch_snapshot_with(&client, &credentials, &cache, &endpoints, Duration::ZERO)
                .await
                .unwrap();
        assert!(outcome.stale);
        assert_eq!(outcome.snapshot, snapshot());
    }

    #[tokio::test]
    async fn a_rotated_token_for_the_same_organization_reuses_the_cache() {
        let mut server = mockito::Server::new_async().await;
        let request = server
            .mock("GET", LIMITS_PATH)
            .expect(0)
            .create_async()
            .await;
        let (_cache_dir, cache) = cache_fixture();
        let scope = crate::cache::fingerprint_of("droid-org:org-1");
        cache
            .write_payload(&serde_json::to_vec(&snapshot_to_cache(&snapshot(), &scope)).unwrap())
            .unwrap();
        let rotated = Credentials {
            access_token: "rotated-token".to_string(),
            fingerprint: scope,
            expires_at: None,
            expired: false,
        };

        let outcome = fetch_snapshot_with(
            &safe_client(),
            &rotated,
            &cache,
            &Endpoints { base: server.url() },
            Duration::from_secs(3600),
        )
        .await
        .unwrap();

        request.assert_async().await;
        assert_eq!(outcome.snapshot, snapshot());
    }
}
