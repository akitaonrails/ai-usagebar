//! Orchestrate: read ~/.codex/auth.json → maybe refresh → fetch usage → cache.
//!
//! Mirrors `anthropic::fetch::fetch_snapshot` but for the Codex OAuth flow.

use std::path::Path;
use std::time::Duration;

use chrono::Utc;

use crate::cache::{Cache, acquire_lock_async};
use crate::error::{AppError, Result};
use crate::usage::OpenAiSnapshot;

use super::creds::{self, Tokens};
use super::oauth;
use super::types::UsageResponse;

pub const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const REFRESH_TIMEOUT: Duration = Duration::from_secs(25);
const LOCK_TIMEOUT: Duration = Duration::from_secs(45);

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

/// This vendor's [`Outcome`](crate::outcome::Outcome) — the shared shape,
/// specialised to its snapshot.
pub type FetchOutcome = crate::outcome::Outcome<OpenAiSnapshot>;

pub async fn fetch_snapshot(
    client: &reqwest::Client,
    creds_path: &Path,
    cache: &Cache,
    endpoints: &Endpoints,
    cache_ttl: Duration,
) -> Result<FetchOutcome> {
    cache.ensure_dir()?;
    let _lock = acquire_lock_async(&cache.lock_path(), LOCK_TIMEOUT).await?;

    let mut auth = creds::read_from(creds_path)?;
    let plan_hint = auth.tokens.plan_type_from_id_token();

    // Corrupt fresh cache falls through to a live fetch rather than returning
    // an all-zero snapshot.
    if let Some(bytes) = cache.fresh_payload(cache_ttl)?
        && let Ok(outcome) = reuse(bytes, cache, false, plan_hint.as_deref())
    {
        return Ok(outcome);
    }

    // Maybe refresh — Codex CLI doesn't always populate expires_at, so we use
    // the id_token's exp claim.
    let now = Utc::now().timestamp();
    if oauth::needs_refresh(auth.tokens.expires_at_secs(), now) {
        match tokio::time::timeout(
            REFRESH_TIMEOUT,
            oauth::refresh(client, &endpoints.token, &auth.tokens.refresh_token),
        )
        .await
        {
            Ok(Ok(rr)) => {
                auth.tokens.access_token = rr.access_token;
                // A rotated refresh token exists only in memory until it is
                // persisted; losing it silently logs the user out on the next
                // run. See the matching comment in `anthropic::fetch`.
                let rotated = rr.refresh_token.is_some();
                if let Some(rt) = rr.refresh_token {
                    auth.tokens.refresh_token = rt;
                }
                if let Some(id) = rr.id_token {
                    auth.tokens.id_token = id;
                }
                // Expiry is normally read from the id_token's exp claim, so a
                // refresh that returns no new id_token would leave the old
                // (expired) claim in place and make every later run refresh
                // again. Record the response's own `expires_in` as the explicit
                // `expires_at` so the expiry is known either way.
                if let Some(secs) = rr.expires_in
                    && let Some(dt) = chrono::DateTime::from_timestamp(now + secs as i64, 0)
                {
                    auth.tokens.expires_at = Some(dt.to_rfc3339());
                }
                if let Err(e) = creds::write_back(creds_path, &auth)
                    && rotated
                {
                    let msg = format!(
                        "refreshed token could not be saved ({e}); the rotated \
                         refresh token is lost — re-run `codex login`"
                    );
                    cache.write_last_error(0, &msg);
                    return handle_auth_failure(cache, plan_hint.as_deref(), false);
                }
            }
            Ok(Err(AppError::Http { status, body })) => {
                cache.write_last_error(status, &body);
                return handle_auth_failure(cache, plan_hint.as_deref(), false);
            }
            Ok(Err(e)) if e.is_transient() => {
                return handle_auth_failure(cache, plan_hint.as_deref(), true);
            }
            Ok(Err(e)) => {
                cache.write_last_error(0, &e.to_string());
                return handle_auth_failure(cache, plan_hint.as_deref(), false);
            }
            Err(_) => return handle_auth_failure(cache, plan_hint.as_deref(), true),
        }
    }

    match tokio::time::timeout(
        HTTP_TIMEOUT,
        fetch_usage(client, &endpoints.usage, &auth.tokens),
    )
    .await
    {
        Ok(Ok(response)) => {
            // Cache what we parse, not what arrived. The raw body carries the
            // account's `user_id`, `account_id` and `email`, none of which any
            // renderer reads — writing the parsed response is an allowlist by
            // construction, so a field OpenAI adds later cannot quietly start
            // living on disk. Same rule Command Code follows.
            cache.write_payload(&serde_json::to_vec(&response)?)?;
            let snap = response.into_snapshot(plan_hint.as_deref())?;
            Ok(crate::outcome::Outcome::fresh(snap))
        }
        Ok(Err(AppError::Http { status, body })) => {
            cache.mark_stale();
            let last_error = Some(cache.write_last_error(status, &body));
            fallback(
                cache,
                plan_hint.as_deref(),
                last_error,
                AppError::Http { status, body },
            )
        }
        Ok(Err(e)) if e.is_transient() => fallback_silent(cache, plan_hint.as_deref(), e),
        Ok(Err(e)) => {
            cache.mark_stale();
            let last_error = Some(cache.write_last_error(0, &e.to_string()));
            fallback(cache, plan_hint.as_deref(), last_error, e)
        }
        Err(_) => fallback_silent(
            cache,
            plan_hint.as_deref(),
            AppError::Transport("openai: usage request timed out".into()),
        ),
    }
}

fn reuse(
    bytes: Vec<u8>,
    cache: &Cache,
    stale: bool,
    plan_hint: Option<&str>,
) -> Result<FetchOutcome> {
    let snap = parse_payload(&bytes, plan_hint)?;
    Ok(crate::outcome::Outcome::cached(snap, cache, stale))
}

fn fallback(
    cache: &Cache,
    plan_hint: Option<&str>,
    last_error: Option<(u16, String)>,
    original: AppError,
) -> Result<FetchOutcome> {
    crate::outcome::fallback(cache, last_error, original, |bytes| {
        parse_payload(bytes, plan_hint)
    })
}

fn fallback_silent(
    cache: &Cache,
    plan_hint: Option<&str>,
    original: AppError,
) -> Result<FetchOutcome> {
    crate::outcome::fallback(cache, None, original, |bytes| {
        parse_payload(bytes, plan_hint)
    })
}

/// The one place a *synthesized* error beats the original: the refresh failed,
/// and "run `codex login` to re-auth" tells the user what to do about it,
/// which the underlying OAuth error does not.
fn handle_auth_failure(
    cache: &Cache,
    plan_hint: Option<&str>,
    transient: bool,
) -> Result<FetchOutcome> {
    let original = if transient {
        AppError::Transport("openai: no cache and refresh failed transiently".into())
    } else {
        AppError::Credentials("openai: token refresh failed; run `codex login` to re-auth".into())
    };
    crate::outcome::fallback(cache, None, original, |bytes| {
        parse_payload(bytes, plan_hint)
    })
}

fn parse_payload(bytes: &[u8], plan_hint: Option<&str>) -> Result<OpenAiSnapshot> {
    parse_response(bytes)?.into_snapshot(plan_hint)
}

/// The wire response, before it becomes a snapshot. Split out so the live path
/// can cache the parsed form rather than the raw body.
fn parse_response(bytes: &[u8]) -> Result<UsageResponse> {
    Ok(serde_json::from_slice(bytes)?)
}

fn authorized(client: &reqwest::Client, url: String, t: &Tokens) -> reqwest::RequestBuilder {
    let mut req = client
        .get(url)
        .header("Authorization", format!("Bearer {}", t.access_token))
        .header("User-Agent", "codex-cli");
    if let Some(aid) = t.account_id.as_deref() {
        req = req.header("ChatGPT-Account-Id", aid);
    }
    req
}

/// Cache the usage payload, grafting on the reset-credit expiries from the
/// second endpoint when they exist. The graft is a field insert into the
/// original JSON: re-serializing our typed `UsageResponse` would drop every
/// unknown key the API still sends (`user_id`, tomorrow's new window, …),
/// and a cache holding only the usage bytes would keep the count while the
/// deadline beside it vanished for the rest of the TTL. The inserted
/// `credits` array is our typed projection — status + expiry, never the
/// redemption `id`.
async fn fetch_usage(client: &reqwest::Client, url: &str, t: &Tokens) -> Result<UsageResponse> {
    let resp = authorized(client, url.to_string(), t).send().await?;
    let status = resp.status();
    let bytes = crate::vendor::read_body_capped(resp, crate::vendor::MAX_BODY_BYTES).await?;

    if !status.is_success() {
        let body: String = String::from_utf8_lossy(&bytes).chars().take(200).collect();
        return Err(AppError::Http {
            status: status.as_u16(),
            body,
        });
    }
    let mut parsed: UsageResponse = serde_json::from_slice(&bytes)
        .map_err(|e| AppError::Schema(format!("openai usage response: {e}")))?;
    parsed.rate_limit_reset_credits = enrich_reset_credits(client, url, t, &parsed).await;
    // Reject drift here, while the caller can still fall back to a good cache.
    parsed.clone().into_snapshot(None)?;
    Ok(parsed)
}

/// The usage endpoint reports how many banked resets exist but not when they
/// expire; a second call carries the per-credit detail. That call is strictly
/// additive: its failure leaves the count exactly as the usage endpoint
/// reported it, because a count with no deadline is still true, and it is not
/// worth failing a whole refresh over the deadline alone. The count itself
/// always stays the usage endpoint's — the two responses can disagree across a
/// redemption, and the one that also carries the quota figures is the one the
/// rest of the snapshot is consistent with.
async fn enrich_reset_credits(
    client: &reqwest::Client,
    usage_url: &str,
    t: &Tokens,
    parsed: &UsageResponse,
) -> Option<super::types::ResetCreditsBlock> {
    let mut block = parsed.rate_limit_reset_credits.clone()?;
    if block.available_count == 0 {
        return Some(block);
    }
    if let Ok(details) = fetch_reset_credits(client, usage_url, t).await {
        block.credits = details.credits;
    }
    Some(block)
}

async fn fetch_reset_credits(
    client: &reqwest::Client,
    usage_url: &str,
    t: &Tokens,
) -> Result<super::types::ResetCreditsBlock> {
    let base = usage_url.strip_suffix("/usage").unwrap_or(usage_url);
    let resp = authorized(client, format!("{base}/rate-limit-reset-credits"), t)
        .send()
        .await?;
    let status = resp.status();
    let bytes = crate::vendor::read_body_capped(resp, crate::vendor::MAX_BODY_BYTES).await?;
    if !status.is_success() {
        // The body is discarded rather than reported: this call is optional,
        // its failure never reaches the user, and it would only carry an
        // account-identifying error into a log.
        return Err(AppError::Http {
            status: status.as_u16(),
            body: String::new(),
        });
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| AppError::Schema("openai reset credits response is invalid".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use std::io::Write;
    use tempfile::{NamedTempFile, TempDir};

    fn fake_jwt(claims: serde_json::Value) -> String {
        let h = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"alg":"none","typ":"JWT"}"#);
        let p =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string().as_bytes());
        format!("{h}.{p}.sig")
    }

    fn future_creds() -> NamedTempFile {
        // exp 1h in the future.
        let exp = Utc::now().timestamp() + 3600;
        let jwt = fake_jwt(serde_json::json!({
            "exp": exp,
            "https://api.openai.com/auth": {"chatgpt_plan_type": "plus"}
        }));
        let body = format!(
            r#"{{"tokens":{{"access_token":"AT","refresh_token":"RT","id_token":"{jwt}",
                "account_id":"acc"}}}}"#
        );
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(body.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    fn cache_fixture() -> (TempDir, Cache) {
        let td = TempDir::new().unwrap();
        let c = Cache::at(td.path().join("openai"));
        c.ensure_dir().unwrap();
        (td, c)
    }

    #[tokio::test]
    async fn live_200_returns_snapshot_with_plan_from_id_token() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/backend-api/wham/usage")
            .with_status(200)
            .with_body(
                r#"{"plan_type":"plus","rate_limit":{
                "primary_window":{"used_percent":1,"limit_window_seconds":18000,"reset_at":1779597324},
                "secondary_window":{"used_percent":0,"limit_window_seconds":604800,"reset_at":1780184124}
            }}"#,
            )
            .create_async()
            .await;
        let (_td, cache) = cache_fixture();
        let creds = future_creds();
        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            usage: format!("{}/backend-api/wham/usage", server.url()),
            token: format!("{}/oauth/token", server.url()),
        };
        let out = fetch_snapshot(
            &client,
            creds.path(),
            &cache,
            &endpoints,
            Duration::from_secs(0),
        )
        .await
        .unwrap();
        assert_eq!(out.snapshot.plan, "ChatGPT Plus");
        assert_eq!(out.snapshot.session.as_ref().unwrap().utilization_pct, 1);
        assert!(!out.stale);
    }

    #[tokio::test]
    async fn weekly_only_primary_returns_weekly_snapshot() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/backend-api/wham/usage")
            .with_status(200)
            .with_body(
                r#"{"plan_type":"prolite","rate_limit":{
                "primary_window":{"used_percent":66,"limit_window_seconds":604800,"reset_at":1785261834},
                "secondary_window":null
            }}"#,
            )
            .create_async()
            .await;
        let (_td, cache) = cache_fixture();
        let creds = future_creds();
        let endpoints = Endpoints {
            usage: format!("{}/backend-api/wham/usage", server.url()),
            token: format!("{}/oauth/token", server.url()),
        };
        let out = fetch_snapshot(
            &reqwest::Client::new(),
            creds.path(),
            &cache,
            &endpoints,
            Duration::from_secs(0),
        )
        .await
        .unwrap();
        assert!(out.snapshot.session.is_none());
        assert_eq!(out.snapshot.weekly.unwrap().utilization_pct, 66);
    }

    #[tokio::test]
    async fn corrupt_fresh_cache_refetches_instead_of_showing_an_empty_snapshot() {
        // `reuse` used to swallow a parse failure into `empty(plan_hint)` and
        // serve that all-zero snapshot for the rest of the TTL.
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/backend-api/wham/usage")
            .with_status(200)
            .with_body(
                r#"{"plan_type":"pro","rate_limit":{"primary_window":{"used_percent":37,"limit_window_seconds":18000}}}"#,
            )
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        cache.write_payload(b"{ truncated").unwrap();

        let creds = future_creds();
        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            usage: format!("{}/backend-api/wham/usage", server.url()),
            token: format!("{}/oauth/token", server.url()),
        };
        // A long TTL: the payload IS fresh, it is simply unusable.
        let out = fetch_snapshot(
            &client,
            creds.path(),
            &cache,
            &endpoints,
            Duration::from_secs(3600),
        )
        .await
        .unwrap();
        assert_eq!(out.snapshot.session.as_ref().unwrap().utilization_pct, 37);
        assert!(!out.stale);
    }

    /// The expiry lives behind a second endpoint. It has to reach the cache
    /// with the usage figures, or the deadline disappears for the rest of the
    /// TTL while the count beside it stays on screen.
    #[tokio::test]
    async fn banked_reset_expiries_are_fetched_and_cached_with_the_usage_figures() {
        let mut server = mockito::Server::new_async().await;
        let usage = server
            .mock("GET", "/backend-api/wham/usage")
            .with_body(
                r#"{"plan_type":"plus","future_field":true,"rate_limit":{
                    "primary_window":{"used_percent":81,"limit_window_seconds":18000,"reset_at":1786536977}},
                    "rate_limit_reset_credits":{"available_count":2}}"#,
            )
            .create_async()
            .await;
        let details = server
            .mock("GET", "/backend-api/wham/rate-limit-reset-credits")
            .with_body(
                r#"{"available_count":2,"credits":[
                    {"id":"c1","status":"available","title":"Full reset (Weekly + 5 hr)","expires_at":"2026-07-17T00:00:00Z"},
                    {"id":"c2","status":"available","title":"Full reset (Weekly + 5 hr)","expires_at":"2026-08-01T00:00:00Z"}]}"#,
            )
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let creds = future_creds();
        let endpoints = Endpoints {
            usage: format!("{}/backend-api/wham/usage", server.url()),
            token: format!("{}/oauth/token", server.url()),
        };
        let out = fetch_snapshot(
            &reqwest::Client::new(),
            creds.path(),
            &cache,
            &endpoints,
            Duration::from_secs(0),
        )
        .await
        .unwrap();
        usage.assert_async().await;
        details.assert_async().await;
        assert_eq!(out.snapshot.reset_credits.available, 2);
        assert_eq!(
            out.snapshot.reset_credits.next_expiry(),
            Some("2026-07-17T00:00:00Z".parse().unwrap())
        );

        // The redemption id is what spends a credit; it must not be written to
        // disk just because it shared a response with the expiry.
        let cached = std::fs::read_to_string(cache.payload_path()).unwrap();
        assert!(!cached.contains("\"c1\""), "{cached}");
        // The cache holds the parsed response, so it holds only what a
        // renderer reads. An unknown field is dropped rather than kept — the
        // cache is a short-lived copy of something refetchable, and keeping
        // the whole body is how the account's identity ended up on disk.
        assert!(
            !cached.contains("future_field"),
            "the cache must not carry fields nothing parses: {cached}"
        );
        let reused = parse_payload(cached.as_bytes(), None).unwrap();
        assert_eq!(reused.reset_credits, out.snapshot.reset_credits);
    }

    /// The response carries the account's identity — `user_id`, `account_id`
    /// and `email` — and no renderer reads any of it. Caching the raw body put
    /// all three on disk for the life of the TTL. Caching the *parsed*
    /// response is an allowlist by construction: a field OpenAI adds later
    /// cannot start living there without someone adding it to the type first.
    #[tokio::test]
    async fn the_cache_holds_no_account_identity() {
        let mut server = mockito::Server::new_async().await;
        let usage = server
            .mock("GET", "/backend-api/wham/usage")
            .with_body(
                r#"{"plan_type":"pro",
                    "user_id":"user_abc123",
                    "account_id":"acct_abc123",
                    "email":"person@example.test",
                    "rate_limit":{"primary_window":{"used_percent":5,
                                  "limit_window_seconds":604800}}}"#,
            )
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let creds = future_creds();
        let endpoints = Endpoints {
            usage: format!("{}/backend-api/wham/usage", server.url()),
            token: format!("{}/oauth/token", server.url()),
        };
        let out = fetch_snapshot(
            &reqwest::Client::new(),
            creds.path(),
            &cache,
            &endpoints,
            Duration::from_secs(0),
        )
        .await
        .unwrap();
        usage.assert_async().await;

        // The figures still arrive.
        assert_eq!(out.snapshot.weekly.as_ref().unwrap().utilization_pct, 5);

        let cached = std::fs::read_to_string(cache.payload_path()).unwrap();
        for identity in ["user_abc123", "acct_abc123", "person@example.test"] {
            assert!(
                !cached.contains(identity),
                "{identity} reached the cache: {cached}"
            );
        }
        for key in ["user_id", "account_id", "email"] {
            assert!(!cached.contains(key), "{key} reached the cache: {cached}");
        }
    }

    /// The detail call is an extra. When it fails, the count the usage
    /// endpoint reported is still true and still worth showing — refusing the
    /// whole refresh over a missing deadline would cost the quota figures too.
    #[tokio::test]
    async fn a_failed_detail_call_keeps_the_count_from_the_usage_response() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/backend-api/wham/usage")
            .with_body(
                r#"{"plan_type":"plus","rate_limit":{
                    "primary_window":{"used_percent":10,"limit_window_seconds":18000}},
                    "rate_limit_reset_credits":{"available_count":1}}"#,
            )
            .create_async()
            .await;
        let details = server
            .mock("GET", "/backend-api/wham/rate-limit-reset-credits")
            .with_status(404)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let creds = future_creds();
        let endpoints = Endpoints {
            usage: format!("{}/backend-api/wham/usage", server.url()),
            token: format!("{}/oauth/token", server.url()),
        };
        let out = fetch_snapshot(
            &reqwest::Client::new(),
            creds.path(),
            &cache,
            &endpoints,
            Duration::from_secs(0),
        )
        .await
        .unwrap();
        details.assert_async().await;
        assert!(!out.stale);
        assert_eq!(out.snapshot.session.as_ref().unwrap().utilization_pct, 10);
        assert_eq!(out.snapshot.reset_credits.available, 1);
        assert!(out.snapshot.reset_credits.credits.is_empty());
    }

    /// With nothing banked there is nothing to detail. Asking anyway would
    /// double every refresh's request count for every account that has none.
    #[tokio::test]
    async fn no_banked_resets_means_no_second_request() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/backend-api/wham/usage")
            .with_body(
                r#"{"plan_type":"plus","rate_limit":{
                    "primary_window":{"used_percent":10,"limit_window_seconds":18000}},
                    "rate_limit_reset_credits":{"available_count":0}}"#,
            )
            .create_async()
            .await;
        let details = server
            .mock("GET", "/backend-api/wham/rate-limit-reset-credits")
            .expect(0)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let creds = future_creds();
        let endpoints = Endpoints {
            usage: format!("{}/backend-api/wham/usage", server.url()),
            token: format!("{}/oauth/token", server.url()),
        };
        let out = fetch_snapshot(
            &reqwest::Client::new(),
            creds.path(),
            &cache,
            &endpoints,
            Duration::from_secs(0),
        )
        .await
        .unwrap();
        details.assert_async().await;
        assert!(out.snapshot.reset_credits.is_empty());
    }

    #[tokio::test]
    async fn http_500_falls_back_to_cache_when_present() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/backend-api/wham/usage")
            .with_status(500)
            .with_body(r#"{"error":{"message":"upstream"}}"#)
            .create_async()
            .await;
        let (_td, cache) = cache_fixture();
        cache
            .write_payload(
                br#"{"plan_type":"pro","rate_limit":{"primary_window":{"used_percent":50,"limit_window_seconds":18000}}}"#,
            )
            .unwrap();
        let creds = future_creds();
        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            usage: format!("{}/backend-api/wham/usage", server.url()),
            token: format!("{}/oauth/token", server.url()),
        };
        let out = fetch_snapshot(
            &client,
            creds.path(),
            &cache,
            &endpoints,
            Duration::from_secs(0),
        )
        .await
        .unwrap();
        assert!(out.stale);
        assert_eq!(out.snapshot.session.as_ref().unwrap().utilization_pct, 50);
        assert_eq!(out.last_error.as_ref().map(|(c, _)| *c), Some(500));
    }
}
