//! Orchestrate: read ~/.codex/auth.json → maybe refresh → fetch usage → cache.
//!
//! Mirrors `anthropic::fetch::fetch_snapshot` but for the Codex OAuth flow.

use std::path::Path;
use std::time::Duration;

use chrono::Utc;

use crate::cache::{Cache, MAX_STALE, acquire_lock_async};
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

#[derive(Debug, Clone)]
pub struct FetchOutcome {
    pub snapshot: OpenAiSnapshot,
    pub stale: bool,
    pub last_error: Option<(u16, String)>,
    pub cache_age: Option<Duration>,
}

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

    if let Some(bytes) = cache.fresh_payload(cache_ttl)? {
        return Ok(reuse(bytes, cache, false, plan_hint.as_deref()));
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
        Ok(Ok(bytes)) => {
            cache.write_payload(&bytes)?;
            let snap = parse_payload(&bytes, plan_hint.as_deref())?;
            Ok(FetchOutcome {
                snapshot: snap,
                stale: false,
                last_error: None,
                cache_age: Some(Duration::ZERO),
            })
        }
        Ok(Err(AppError::Http { status, body })) => {
            cache.mark_stale();
            cache.write_last_error(status, &body);
            fallback(cache, plan_hint.as_deref(), Some((status, body)))
        }
        Ok(Err(e)) if e.is_transient() => fallback_silent(cache, plan_hint.as_deref()),
        Ok(Err(e)) => {
            cache.mark_stale();
            cache.write_last_error(0, &e.to_string());
            fallback(cache, plan_hint.as_deref(), Some((0, e.to_string())))
        }
        Err(_) => fallback_silent(cache, plan_hint.as_deref()),
    }
}

fn reuse(bytes: Vec<u8>, cache: &Cache, stale: bool, plan_hint: Option<&str>) -> FetchOutcome {
    let snap = parse_payload(&bytes, plan_hint).unwrap_or_else(|_| empty(plan_hint));
    FetchOutcome {
        snapshot: snap,
        stale,
        last_error: cache.read_last_error(),
        cache_age: cache.payload_age(),
    }
}

fn fallback(
    cache: &Cache,
    plan_hint: Option<&str>,
    last_error: Option<(u16, String)>,
) -> Result<FetchOutcome> {
    let Some(bytes) = cache.fallback_payload(MAX_STALE)? else {
        return Err(AppError::Other("openai: no usable cache".into()));
    };
    let mut out = reuse(bytes, cache, true, plan_hint);
    out.last_error = last_error;
    Ok(out)
}

fn fallback_silent(cache: &Cache, plan_hint: Option<&str>) -> Result<FetchOutcome> {
    let Some(bytes) = cache.fallback_payload(MAX_STALE)? else {
        return Err(AppError::Transport(
            "openai: no cache and network unreachable".into(),
        ));
    };
    Ok(reuse(bytes, cache, true, plan_hint))
}

fn handle_auth_failure(
    cache: &Cache,
    plan_hint: Option<&str>,
    transient: bool,
) -> Result<FetchOutcome> {
    let Some(bytes) = cache.fallback_payload(MAX_STALE)? else {
        return if transient {
            Err(AppError::Transport(
                "openai: no cache and refresh failed transiently".into(),
            ))
        } else {
            Err(AppError::Credentials(
                "openai: token refresh failed; run `codex login` to re-auth".into(),
            ))
        };
    };
    Ok(reuse(bytes, cache, true, plan_hint))
}

fn parse_payload(bytes: &[u8], plan_hint: Option<&str>) -> Result<OpenAiSnapshot> {
    let r: UsageResponse = serde_json::from_slice(bytes)?;
    Ok(r.into_snapshot(plan_hint))
}

fn empty(plan_hint: Option<&str>) -> OpenAiSnapshot {
    UsageResponse::default().into_snapshot(plan_hint)
}

async fn fetch_usage(client: &reqwest::Client, url: &str, t: &Tokens) -> Result<Vec<u8>> {
    let mut req = client
        .get(url)
        .header("Authorization", format!("Bearer {}", t.access_token))
        .header("User-Agent", "codex-cli");
    if let Some(aid) = t.account_id.as_deref() {
        req = req.header("ChatGPT-Account-Id", aid);
    }
    let resp = req.send().await?;
    let status = resp.status();
    let bytes = resp.bytes().await?.to_vec();

    if !status.is_success() {
        let body: String = String::from_utf8_lossy(&bytes).chars().take(200).collect();
        return Err(AppError::Http {
            status: status.as_u16(),
            body,
        });
    }
    let _: UsageResponse = serde_json::from_slice(&bytes)
        .map_err(|e| AppError::Schema(format!("openai usage response: {e}")))?;
    Ok(bytes)
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
        assert_eq!(out.snapshot.session.utilization_pct, 1);
        assert!(!out.stale);
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
        assert_eq!(out.snapshot.session.utilization_pct, 50);
        assert_eq!(out.last_error.as_ref().map(|(c, _)| *c), Some(500));
    }
}
