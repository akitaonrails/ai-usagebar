//! Stitches together: read creds → maybe-refresh → GET usage → cache result.
//!
//! Mirrors claudebar:402-491 — the lock + refresh + fetch state machine.

use std::time::Duration;

use chrono::Utc;

use crate::cache::{Cache, LockGuard, acquire_lock_async};
use crate::error::{AppError, Result};
use crate::usage::AnthropicSnapshot;

use super::creds::{self, OauthCreds};
use super::oauth;
use super::types::UsageResponse;

pub const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
pub const USAGE_BETA_HEADER: &str = "oauth-2025-04-20";
/// The usage endpoint rate-limits hard unless the request carries a Claude Code
/// `User-Agent`. The exact patch version isn't validated, so a stable recent
/// `claude-code/<version>` (what the official client sends) is fine.
pub const USAGE_USER_AGENT: &str = "claude-code/2.1.183";
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const REFRESH_TIMEOUT: Duration = Duration::from_secs(25);
const LOCK_TIMEOUT: Duration = Duration::from_secs(45);

/// Endpoints (parameterized for tests).
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub usage: String,
    pub token: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            usage: USAGE_URL.into(),
            token: oauth::TOKEN_URL.into(),
        }
    }
}

/// What we ultimately hand back to the renderer.
/// This vendor's [`Outcome`](crate::outcome::Outcome) — the shared shape,
/// specialised to its snapshot.
pub type FetchOutcome = crate::outcome::Outcome<AnthropicSnapshot>;

/// High-level entry point. Reads creds, refreshes if needed, fetches usage,
/// writes back the cache, and returns the snapshot — falling back to cache on
/// failure. All under a flock so multi-monitor Waybar instances coexist.
pub async fn fetch_snapshot(
    client: &reqwest::Client,
    creds_target: &creds::CredsTarget,
    cache: &Cache,
    endpoints: &Endpoints,
    cache_ttl: Duration,
) -> Result<FetchOutcome> {
    cache.ensure_dir()?;
    let _lock = acquire_lock_async(&cache.lock_path(), LOCK_TIMEOUT).await?;
    // Desktop snapshots and account switching can mutate the same rotating
    // credential. Hold their shared lock from source resolution through any
    // refresh write-back; the live config source is read-only but also uses the
    // lock so it cannot be read halfway through our own switch transaction.
    let credential_lock = acquire_credential_lock(creds_target, LOCK_TIMEOUT).await?;

    // Fast path: cache is fresh, no work needed. We still need creds for the
    // plan label though, so read them either way. `resolve` also reports where
    // the creds actually came from (file vs macOS Keychain) so the refresh
    // write-back follows the same source instead of forking a stale copy.
    let (mut creds, creds_source) = creds::resolve(creds_target)?;
    let plan_label = creds.claude_ai_oauth.plan_label();

    // Corrupt fresh cache falls through to a live fetch rather than returning
    // an all-zero snapshot labelled "Unknown".
    if let Some(bytes) = cache.fresh_payload(cache_ttl)?
        && let Ok(outcome) = reuse_cache(bytes, plan_label.clone(), cache, false)
    {
        return Ok(outcome);
    }

    // Maybe refresh.
    let now = Utc::now().timestamp();
    let stale_token = oauth::needs_refresh(creds.claude_ai_oauth.expires_at_secs(), now);
    let have_refresh = oauth::can_refresh(&creds.claude_ai_oauth.refresh_token);
    if stale_token && !have_refresh {
        // No refresh token to refresh with — the Claude Code client owns token
        // rotation (trusted-device flow) and leaves `refreshToken` empty. Don't
        // POST an empty grant (the token endpoint answers 400 "Invalid request
        // format" and we'd cache a zeroed snapshot). Also clear any stale
        // token-endpoint error from older builds, then continue with the current
        // access token: only the real usage request decides whether to fall
        // back to cache.
        cache.clear_last_error();
    } else if stale_token {
        match tokio::time::timeout(
            REFRESH_TIMEOUT,
            oauth::refresh(
                client,
                &endpoints.token,
                &creds.claude_ai_oauth.refresh_token,
            ),
        )
        .await
        {
            Ok(Ok(rr)) => {
                creds.claude_ai_oauth.access_token = rr.access_token;
                // A rotated refresh token exists *only* in memory until it is
                // persisted. If the server rotated it and the write-back fails,
                // the old token on disk is already spent: the next run cannot
                // refresh and the user is silently logged out. That is a hard
                // failure, not a best-effort detail.
                let rotated = rr.refresh_token.is_some();
                if let Some(new_rt) = rr.refresh_token {
                    creds.claude_ai_oauth.refresh_token = new_rt;
                }
                creds.claude_ai_oauth.expires_at_ms =
                    Utc::now().timestamp_millis() + (rr.expires_in as i64) * 1000;
                // When only the access token changed, a failed write loses
                // nothing — the next run just refreshes again — so carry on.
                if let Err(e) = creds::write_back_to(&creds_source, &creds.claude_ai_oauth)
                    && rotated
                {
                    let msg = format!(
                        "refreshed token could not be saved ({e}); the rotated \
                         refresh token is lost — re-run `claude` to log in again"
                    );
                    cache.write_last_error(0, &msg);
                    return with_oauth_plan(
                        &plan_label,
                        handle_auth_failure(cache, plan_label.clone(), false),
                    );
                }
            }
            Ok(Err(AppError::Http { status, body })) => {
                cache.write_last_error(status, &body);
                return with_oauth_plan(
                    &plan_label,
                    handle_auth_failure(cache, plan_label.clone(), false),
                );
            }
            Ok(Err(e)) if e.is_transient() => {
                return with_oauth_plan(
                    &plan_label,
                    handle_auth_failure(cache, plan_label.clone(), true),
                );
            }
            Ok(Err(e)) => {
                cache.write_last_error(0, &e.to_string());
                return with_oauth_plan(
                    &plan_label,
                    handle_auth_failure(cache, plan_label.clone(), false),
                );
            }
            Err(_elapsed) => {
                return with_oauth_plan(
                    &plan_label,
                    handle_auth_failure(cache, plan_label.clone(), true),
                );
            }
        }
    }

    // Usage fetches do not mutate credentials and should not make an account
    // switch wait on the network once refresh/write-back is complete.
    drop(credential_lock);

    // Fetch usage.
    match tokio::time::timeout(
        HTTP_TIMEOUT,
        fetch_usage(client, &endpoints.usage, &creds.claude_ai_oauth),
    )
    .await
    {
        Ok(Ok(bytes)) => {
            cache
                .write_payload(&bytes)
                .map_err(|e| attach_plan(&plan_label, e))?;
            let snap = parse_payload(&bytes, plan_label.clone())
                .map_err(|e| attach_plan(&plan_label, e))?;
            Ok(crate::outcome::Outcome::fresh(snap))
        }
        Ok(Err(AppError::Http { status, body })) => {
            cache.mark_stale();
            let last_error = Some(cache.write_last_error(status, &body));
            with_oauth_plan(
                &plan_label,
                fallback_to_cache(
                    cache,
                    plan_label.clone(),
                    last_error,
                    AppError::Http { status, body },
                ),
            )
        }
        Ok(Err(e)) if e.is_transient() => {
            // Reuse cache silently; no last_error write.
            with_oauth_plan(
                &plan_label,
                fallback_to_cache_silent(cache, plan_label.clone(), e),
            )
        }
        Ok(Err(e)) => {
            cache.mark_stale();
            let last_error = Some(cache.write_last_error(0, &e.to_string()));
            with_oauth_plan(
                &plan_label,
                fallback_to_cache(cache, plan_label.clone(), last_error, e),
            )
        }
        Err(_elapsed) => with_oauth_plan(
            &plan_label,
            fallback_to_cache_silent(
                cache,
                plan_label.clone(),
                AppError::Transport("usage request timed out".into()),
            ),
        ),
    }
}

async fn acquire_credential_lock(
    target: &creds::CredsTarget,
    timeout: Duration,
) -> Result<Option<LockGuard>> {
    let creds::CredsTarget::Desktop(desktop) = target else {
        return Ok(None);
    };
    let Some(path) = desktop.coordination_lock() else {
        return Ok(None);
    };
    acquire_lock_async(path, timeout).await.map(Some)
}

fn reuse_cache(
    bytes: Vec<u8>,
    plan_label: String,
    cache: &Cache,
    stale: bool,
) -> Result<FetchOutcome> {
    let snap = parse_payload(&bytes, plan_label)?;
    Ok(crate::outcome::Outcome::cached(snap, cache, stale))
}

fn fallback_to_cache(
    cache: &Cache,
    plan_label: String,
    last_error: Option<(u16, String)>,
    original: AppError,
) -> Result<FetchOutcome> {
    crate::outcome::fallback(cache, last_error, original, |bytes| {
        parse_payload(bytes, plan_label)
    })
}

fn fallback_to_cache_silent(
    cache: &Cache,
    plan_label: String,
    original: AppError,
) -> Result<FetchOutcome> {
    crate::outcome::fallback(cache, None, original, |bytes| {
        parse_payload(bytes, plan_label)
    })
}

/// The one place a *synthesized* error beats the original: the refresh failed,
/// and "run `claude` to re-auth" tells the user what to do about it, which the
/// underlying OAuth error does not.
fn handle_auth_failure(cache: &Cache, plan_label: String, transient: bool) -> Result<FetchOutcome> {
    let original = if transient {
        AppError::Transport("no cache and refresh failed transiently".into())
    } else {
        AppError::Credentials("token refresh failed; run `claude` to re-auth".into())
    };
    crate::outcome::fallback(cache, None, original, |bytes| {
        parse_payload(bytes, plan_label)
    })
}

fn parse_payload(bytes: &[u8], plan_label: String) -> Result<AnthropicSnapshot> {
    let resp: UsageResponse = serde_json::from_slice(bytes)?;
    Ok(resp.into_snapshot(plan_label))
}

/// Keep the OAuth plan on a failed fetch so a 401/429 card can still name
/// Max/Pro. "Unknown" is the empty-subscription fallback and is not a plan.
fn attach_plan(plan_label: &str, err: AppError) -> AppError {
    let name = plan_label.trim();
    if name.is_empty() || name.eq_ignore_ascii_case("unknown") {
        err
    } else {
        err.with_plan(format!("Claude {name}"))
    }
}

fn with_oauth_plan<T>(plan_label: &str, result: Result<T>) -> Result<T> {
    result.map_err(|err| attach_plan(plan_label, err))
}

async fn fetch_usage(client: &reqwest::Client, url: &str, creds: &OauthCreds) -> Result<Vec<u8>> {
    let resp = client
        .get(url)
        .header("Authorization", format!("Bearer {}", creds.access_token))
        .header("anthropic-beta", USAGE_BETA_HEADER)
        // These four headers are exactly what the endpoint accepts — the
        // `User-Agent` is load-bearing (without it the endpoint 429s hard).
        .header("User-Agent", USAGE_USER_AGENT)
        .header("Content-Type", "application/json")
        .send()
        .await?;

    let status = resp.status();
    let bytes = crate::vendor::read_body_capped(resp, crate::vendor::MAX_BODY_BYTES).await?;

    if status.is_success() {
        // Validate it's a usage shape — keep claudebar's "must have five_hour"
        // sanity check (claudebar:385).
        let _: UsageResponse = serde_json::from_slice(&bytes)
            .map_err(|e| AppError::Schema(format!("usage response unparseable: {e}")))?;
        Ok(bytes.to_vec())
    } else {
        let body = String::from_utf8_lossy(&bytes).into_owned();
        let msg =
            oauth::parse_error_body(&body).unwrap_or_else(|| body.chars().take(200).collect());
        Err(AppError::Http {
            status: status.as_u16(),
            body: msg,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::{NamedTempFile, TempDir};

    fn future_creds() -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        // Expires 1 hour from now → no refresh needed in tests.
        let expires_ms = (Utc::now().timestamp_millis()) + 3_600_000;
        let s = format!(
            r#"{{"claudeAiOauth":{{
                "accessToken":"AT","refreshToken":"RT",
                "expiresAt": {expires_ms},
                "subscriptionType":"max","rateLimitTier":"default_claude_max_5x"
            }}}}"#
        );
        f.write_all(s.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    /// Expired access token AND an empty `refreshToken` — the trusted-device
    /// shape recent Claude Code builds leave in the shared credential blob.
    fn expired_creds_no_refresh() -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        let expires_ms = (Utc::now().timestamp_millis()) - 3_600_000; // 1h ago
        let s = format!(
            r#"{{"claudeAiOauth":{{
                "accessToken":"AT","refreshToken":"",
                "expiresAt": {expires_ms},
                "subscriptionType":"max","rateLimitTier":"default_claude_max_5x"
            }}}}"#
        );
        f.write_all(s.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    fn cache_fixture() -> (TempDir, Cache) {
        let td = TempDir::new().unwrap();
        let cache = Cache::at(td.path().join("anthropic"));
        cache.ensure_dir().unwrap();
        (td, cache)
    }

    #[tokio::test]
    async fn desktop_refresh_waits_for_the_account_switch_lock() {
        let tmp = TempDir::new().unwrap();
        let lock_path = tmp.path().join(".account-switch.lock");
        let held = crate::cache::acquire_lock(&lock_path, Duration::from_secs(1)).unwrap();
        let desktop = crate::anthropic::desktop_creds::source_for(
            &tmp.path().join("config.json"),
            &tmp.path().join("profile"),
            false,
            [0; 16],
        )
        .with_coordination_lock(lock_path);
        let target = creds::CredsTarget::Desktop(desktop);

        let waiter = tokio::spawn(async move {
            acquire_credential_lock(&target, Duration::from_secs(2))
                .await
                .unwrap()
                .is_some()
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiter.is_finished(), "refresh bypassed the switch lock");

        drop(held);
        assert!(waiter.await.unwrap());
    }

    #[tokio::test]
    async fn corrupt_fresh_cache_refetches_instead_of_showing_unknown() {
        // `reuse_cache` used to swallow a parse failure into an all-zero
        // snapshot labelled "Unknown" and serve it for the rest of the TTL —
        // a fabricated reading presented as current.
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/api/oauth/usage")
            .with_status(200)
            .with_body(r#"{"five_hour":{"utilization":42},"seven_day":{"utilization":15}}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        cache.write_payload(b"{ truncated").unwrap();

        let creds = future_creds();
        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            usage: format!("{}/api/oauth/usage", server.url()),
            token: format!("{}/token", server.url()),
        };
        // A long TTL: the payload IS fresh, it is simply unusable.
        let outcome = fetch_snapshot(
            &client,
            &creds::CredsTarget::Explicit(creds.path().to_path_buf()),
            &cache,
            &endpoints,
            Duration::from_secs(3600),
        )
        .await
        .unwrap();
        assert_eq!(outcome.snapshot.session.utilization_pct, 42);
        assert_ne!(outcome.snapshot.plan, "Unknown");
        assert!(!outcome.stale);
    }

    #[tokio::test]
    async fn fresh_cache_skips_network() {
        let (_td, cache) = cache_fixture();
        cache
            .write_payload(
                br#"{"five_hour":{"utilization":42,"resets_at":"2026-05-23T17:30:00Z"},
                     "seven_day":{"utilization":15,"resets_at":"2026-05-30T12:00:00Z"}}"#,
            )
            .unwrap();

        let creds = future_creds();
        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            usage: "http://localhost:1/should-not-be-called".into(),
            token: "http://localhost:1/should-not-be-called".into(),
        };
        let outcome = fetch_snapshot(
            &client,
            &creds::CredsTarget::Explicit(creds.path().to_path_buf()),
            &cache,
            &endpoints,
            Duration::from_secs(60),
        )
        .await
        .unwrap();
        assert_eq!(outcome.snapshot.session.utilization_pct, 42);
        assert!(!outcome.stale);
    }

    #[tokio::test]
    async fn live_fetch_writes_cache_and_returns_snapshot() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("GET", "/api/oauth/usage")
            .with_status(200)
            .with_body(
                r#"{"five_hour":{"utilization":50,"resets_at":"2026-05-23T17:30:00Z"},
                    "seven_day":{"utilization":25,"resets_at":"2026-05-30T12:00:00Z"}}"#,
            )
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let creds = future_creds();
        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            usage: format!("{}/api/oauth/usage", server.url()),
            token: format!("{}/v1/oauth/token", server.url()),
        };
        let outcome = fetch_snapshot(
            &client,
            &creds::CredsTarget::Explicit(creds.path().to_path_buf()),
            &cache,
            &endpoints,
            Duration::from_secs(0),
        )
        .await
        .unwrap();
        assert_eq!(outcome.snapshot.session.utilization_pct, 50);
        assert!(!outcome.stale);
        m.assert_async().await;
        // Cache should now exist.
        assert!(cache.maybe_payload().unwrap().is_some());
    }

    #[tokio::test]
    async fn http_429_falls_back_to_stale_cache() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/api/oauth/usage")
            .with_status(429)
            .with_body(r#"{"error":{"type":"rate_limit_error","message":"slow down"}}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        cache
            .write_payload(
                br#"{"five_hour":{"utilization":12,"resets_at":"2026-05-23T17:30:00Z"},
                     "seven_day":{"utilization":5,"resets_at":"2026-05-30T12:00:00Z"}}"#,
            )
            .unwrap();
        // Force the cache to be considered stale by setting TTL = 0.
        let creds = future_creds();
        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            usage: format!("{}/api/oauth/usage", server.url()),
            token: format!("{}/v1/oauth/token", server.url()),
        };
        let outcome = fetch_snapshot(
            &client,
            &creds::CredsTarget::Explicit(creds.path().to_path_buf()),
            &cache,
            &endpoints,
            Duration::from_secs(0),
        )
        .await
        .unwrap();
        assert!(outcome.stale);
        assert_eq!(outcome.snapshot.session.utilization_pct, 12);
        assert_eq!(outcome.last_error.as_ref().map(|(c, _)| *c), Some(429));
        assert_eq!(
            outcome.last_error.as_ref().map(|(_, m)| m.as_str()),
            Some("slow down")
        );
    }

    #[tokio::test]
    async fn empty_refresh_token_skips_refresh_and_fetches_usage() {
        // Expired token, empty refresh token. `.expect(0)` is the assertion: an
        // empty grant must never be POSTed (it would 400 and poison the cache).
        let mut server = mockito::Server::new_async().await;
        let refresh = server
            .mock("POST", "/v1/oauth/token")
            .with_status(400)
            .with_body(
                r#"{"error":{"type":"invalid_request_error","message":"Invalid request format"}}"#,
            )
            .expect(0)
            .create_async()
            .await;
        let usage = server
            .mock("GET", "/api/oauth/usage")
            .match_header("authorization", "Bearer AT")
            .match_header("user-agent", USAGE_USER_AGENT)
            .match_header("anthropic-beta", USAGE_BETA_HEADER)
            .with_status(200)
            .with_body(
                r#"{"five_hour":{"utilization":61,"resets_at":"2026-06-25T17:30:00Z"},
                    "seven_day":{"utilization":31,"resets_at":"2026-06-26T12:00:00Z"}}"#,
            )
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        cache
            .write_payload(
                br#"{"five_hour":{"utilization":17,"resets_at":"2026-06-25T17:30:00Z"},
                     "seven_day":{"utilization":77,"resets_at":"2026-06-26T12:00:00Z"}}"#,
            )
            .unwrap();

        let creds = expired_creds_no_refresh();
        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            usage: format!("{}/api/oauth/usage", server.url()),
            token: format!("{}/v1/oauth/token", server.url()),
        };
        let outcome = fetch_snapshot(
            &client,
            &creds::CredsTarget::Explicit(creds.path().to_path_buf()),
            &cache,
            &endpoints,
            Duration::from_secs(0),
        )
        .await
        .unwrap();

        assert!(!outcome.stale);
        assert_eq!(outcome.snapshot.session.utilization_pct, 61);
        assert!(
            outcome.last_error.is_none(),
            "empty-refresh path must not poison .last_error, got {:?}",
            outcome.last_error
        );
        refresh.assert_async().await; // refresh endpoint was never called
        usage.assert_async().await; // usage endpoint was still called
    }

    #[tokio::test]
    async fn empty_refresh_token_clears_old_last_error_on_transient_fallback() {
        let mut server = mockito::Server::new_async().await;
        let refresh = server
            .mock("POST", "/v1/oauth/token")
            .expect(0)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        cache
            .write_payload(
                br#"{"five_hour":{"utilization":17,"resets_at":"2026-06-25T17:30:00Z"},
                     "seven_day":{"utilization":77,"resets_at":"2026-06-26T12:00:00Z"}}"#,
            )
            .unwrap();
        cache.write_last_error(400, "Invalid request format");

        let creds = expired_creds_no_refresh();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(200))
            .build()
            .unwrap();
        let endpoints = Endpoints {
            usage: "http://127.0.0.1:1/api/oauth/usage".into(),
            token: format!("{}/v1/oauth/token", server.url()),
        };
        let outcome = fetch_snapshot(
            &client,
            &creds::CredsTarget::Explicit(creds.path().to_path_buf()),
            &cache,
            &endpoints,
            Duration::from_secs(0),
        )
        .await
        .unwrap();

        assert!(outcome.stale);
        assert_eq!(outcome.snapshot.session.utilization_pct, 17);
        assert!(outcome.last_error.is_none());
        assert!(cache.read_last_error().is_none());
        refresh.assert_async().await;
    }

    #[tokio::test]
    async fn no_cache_and_no_network_returns_error() {
        // Point at a closed port so we get a transport error.
        let (_td, cache) = cache_fixture();
        let creds = future_creds();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(200))
            .build()
            .unwrap();
        let endpoints = Endpoints {
            usage: "http://127.0.0.1:1/api/oauth/usage".into(),
            token: "http://127.0.0.1:1/v1/oauth/token".into(),
        };
        let err = fetch_snapshot(
            &client,
            &creds::CredsTarget::Explicit(creds.path().to_path_buf()),
            &cache,
            &endpoints,
            Duration::from_secs(0),
        )
        .await
        .unwrap_err();
        assert!(err.is_transient(), "expected transient error, got {err:?}");
        assert_eq!(err.plan(), Some("Claude Max 5x"));
    }

    #[tokio::test]
    async fn usage_401_without_cache_keeps_oauth_plan_on_the_error() {
        // Frontends that read `usage --json` drop the plan when the tab is
        // an Error with no snapshot. The OAuth blob still knows Max/Pro, so a
        // 401 must carry that label rather than a plan-less error card.
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/api/oauth/usage")
            .with_status(401)
            .with_body(r#"{"error":{"type":"authentication_error","message":"invalid token"}}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let creds = expired_creds_no_refresh();
        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            usage: format!("{}/api/oauth/usage", server.url()),
            token: format!("{}/v1/oauth/token", server.url()),
        };
        let err = fetch_snapshot(
            &client,
            &creds::CredsTarget::Explicit(creds.path().to_path_buf()),
            &cache,
            &endpoints,
            Duration::from_secs(0),
        )
        .await
        .unwrap_err();
        assert_eq!(err.plan(), Some("Claude Max 5x"));
        let message = err.user_message();
        assert!(message.contains("401"), "{message}");
        assert!(
            message.contains(crate::error::AUTH_FAILURE_MESSAGE),
            "{message}"
        );
        assert!(!message.contains("invalid token"), "{message}");
    }
}
