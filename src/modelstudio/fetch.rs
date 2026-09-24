//! Fetch Model Studio Token Plan usage from the console gateway, with the
//! `bl` CLI's own console session (`creds.rs`). Cache/stale/error-fallback
//! shape mirrors `grokbot::fetch`: fresh cache short-circuits; on failure,
//! fall back to cache + mark stale. The cache is scoped by a fingerprint of
//! the access token — a re-login selects another console session, so a cache
//! written for one must never be shown for another, and the token itself is
//! never persisted.

use std::time::Duration;

use crate::cache::{Cache, acquire_lock_async};
use crate::error::{AppError, Result};
use crate::usage::{ModelStudioSnapshot, UsageWindow};
use crate::vendor::{MAX_BODY_BYTES, read_body_capped};

use super::creds::Credentials;
use super::types::{
    ConsoleRegion, ConsoleSite, FIVE_HOUR_WINDOW, WEEKLY_WINDOW, form_body, gateway_for,
    parse_response, usage_path,
};

const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const LOCK_TIMEOUT: Duration = Duration::from_secs(15);

/// Where the gateway lives for one region×site cell: `https://{host}` plus
/// the shared `/cli/api.json` path. The `base` is the test seam — production
/// builds it from [`Self::for_gateway`], tests point it at mockito.
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: String,
}

impl Endpoints {
    pub fn for_gateway(region: ConsoleRegion, site: ConsoleSite) -> Self {
        Self {
            base: format!("https://{}", gateway_for(region, site).host),
        }
    }

    fn usage_url(&self, action: &str) -> String {
        format!("{}{}", self.base, usage_path(action))
    }
}

impl Default for Endpoints {
    fn default() -> Self {
        Self::for_gateway(ConsoleRegion::CnBeijing, ConsoleSite::Domestic)
    }
}

/// This vendor's [`Outcome`](crate::outcome::Outcome) — the shared shape,
/// specialised to its snapshot.
pub type FetchOutcome = crate::outcome::Outcome<ModelStudioSnapshot>;

/// Production entry: resolve the CLI's session (region and site included),
/// then fetch through the matching gateway.
pub async fn fetch_snapshot(
    client: &reqwest::Client,
    cfg: &crate::config::ModelStudioConfig,
    cache: &Cache,
    cache_ttl: Duration,
) -> Result<FetchOutcome> {
    let creds = super::resolve_credentials(cfg)?;
    let endpoints = Endpoints::for_gateway(creds.region, creds.site);
    fetch_snapshot_with(client, &creds, cache, &endpoints, cache_ttl).await
}

/// Fetch with the credential and endpoints resolved by the caller.
pub async fn fetch_snapshot_with(
    client: &reqwest::Client,
    creds: &Credentials,
    cache: &Cache,
    endpoints: &Endpoints,
    cache_ttl: Duration,
) -> Result<FetchOutcome> {
    cache.ensure_dir()?;
    let _lock = acquire_lock_async(&cache.lock_path(), LOCK_TIMEOUT).await?;

    // The cache is scoped to the login: a re-login must not keep serving the
    // previous session's figures.
    if let Some(bytes) = cache.fresh_payload(cache_ttl)?
        && let Ok(outcome) = reuse_cache(&bytes, cache, false, &creds.fingerprint)
    {
        return Ok(outcome);
    }

    match usage_call(client, creds, endpoints).await {
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
/// account-identifying back. Mirrors `grokbot::fetch::error_to_pair`.
fn error_to_pair(e: &AppError) -> Option<(u16, String)> {
    match e {
        AppError::Http { status, .. } if matches!(status, 401 | 403) => {
            Some((*status, "Model Studio authentication failed".into()))
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

async fn usage_call(
    client: &reqwest::Client,
    creds: &Credentials,
    endpoints: &Endpoints,
) -> Result<ModelStudioSnapshot> {
    let action = gateway_for(creds.region, creds.site).action;
    let resp = tokio::time::timeout(
        HTTP_TIMEOUT,
        client
            .post(endpoints.usage_url(action))
            .header("Authorization", format!("Bearer {}", creds.access_token))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(form_body(creds.region))
            .send(),
    )
    .await
    .map_err(|_| AppError::Transport("modelstudio timeout: console gateway".into()))?
    .map_err(|e| AppError::Transport(format!("modelstudio transport: {e}")))?;

    let status = resp.status();
    let bytes = read_body_capped(resp, MAX_BODY_BYTES).await?;
    if !status.is_success() {
        // Never surface upstream/proxy bodies: they can contain credentials
        // or arbitrary markup. The cache records the redacted form centrally.
        let body = if matches!(status.as_u16(), 401 | 403) {
            "Model Studio authentication failed".into()
        } else {
            format!("Model Studio gateway returned HTTP {}", status.as_u16())
        };
        return Err(AppError::Http {
            status: status.as_u16(),
            body,
        });
    }
    parse_response(&bytes)?.to_snapshot()
}

fn snap_to_json(snap: &ModelStudioSnapshot, fingerprint: &str) -> serde_json::Value {
    let window = |w: &Option<UsageWindow>| match w {
        Some(w) => serde_json::json!({
            "pct": w.utilization_pct,
            "reset_ms": w.resets_at.map(|t| t.timestamp_millis()),
        }),
        None => serde_json::Value::Null,
    };
    serde_json::json!({
        "account": fingerprint,
        "session": window(&snap.session),
        "weekly": window(&snap.weekly),
    })
}

fn parse_cache_at(bytes: &[u8], fingerprint: &str) -> Result<ModelStudioSnapshot> {
    let v: serde_json::Value = serde_json::from_slice(bytes)?;
    if v.get("account").and_then(serde_json::Value::as_str) != Some(fingerprint) {
        return Err(AppError::Schema(
            "modelstudio cache belongs to a different login; refetching".into(),
        ));
    }
    let invalid = |field: &str| AppError::Schema(format!("modelstudio cache: invalid {field}"));
    let window = |field: &str, duration| -> Result<Option<UsageWindow>> {
        match v.get(field) {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(w) => {
                let pct = w["pct"]
                    .as_i64()
                    .filter(|pct| (0..=100).contains(pct))
                    .ok_or_else(|| invalid(field))? as i32;
                let resets_at = match w.get("reset_ms") {
                    None | Some(serde_json::Value::Null) => None,
                    Some(value) => {
                        let ms = value
                            .as_i64()
                            .filter(|ms| *ms >= 0)
                            .ok_or_else(|| invalid(field))?;
                        Some(
                            chrono::DateTime::from_timestamp_millis(ms)
                                .ok_or_else(|| invalid(field))?,
                        )
                    }
                };
                Ok(Some(UsageWindow {
                    utilization_pct: pct,
                    resets_at,
                    window_duration: duration,
                }))
            }
        }
    };
    Ok(ModelStudioSnapshot {
        session: window("session", FIVE_HOUR_WINDOW)?,
        weekly: window("weekly", WEEKLY_WINDOW)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn cache_fixture() -> (TempDir, Cache) {
        let td = TempDir::new().unwrap();
        let cache = Cache::at(td.path().join("modelstudio"));
        cache.ensure_dir().unwrap();
        (td, cache)
    }

    fn creds(region: ConsoleRegion, site: ConsoleSite) -> Credentials {
        Credentials {
            access_token: "tok-test".into(),
            site,
            region,
            fingerprint: super::super::creds::fingerprint_of("tok-test"),
        }
    }

    /// The mock gateway for one cell, matching every part of the request the
    /// contract specifies: method, the `/cli/api.json` path with this cell's
    /// `action` and the slash-encoded `api` param, the Bearer token, the form
    /// content type, and the exact two-field form body (region + cornerstone
    /// params).
    fn gateway_mock(server: &mut mockito::ServerGuard, creds: &Credentials) -> mockito::Mock {
        let action = gateway_for(creds.region, creds.site).action;
        server
            .mock("POST", usage_path(action).as_str())
            .match_header("authorization", "Bearer tok-test")
            .match_header("content-type", "application/x-www-form-urlencoded")
            .match_body(mockito::Matcher::Exact(form_body(creds.region)))
    }

    fn usage_body() -> String {
        serde_json::json!({
            "success": true,
            "DataV2": { "data": { "data": {
                "per5HourPercentage": 0.4217,
                "per5HourResetTime": 1789200000000_i64,
                "per1WeekPercentage": 0.7356,
                "per1WeekResetTime": 1789600000000_i64,
            }}}
        })
        .to_string()
    }

    /// All four region×site cells, end to end through the real request
    /// builder: each sends its own action, its own `region` form value, and
    /// the api param slash-encoded in every one of them.
    #[tokio::test]
    async fn all_four_gateway_cells_build_the_url_and_body() {
        for (region, site) in [
            (ConsoleRegion::CnBeijing, ConsoleSite::Domestic),
            (ConsoleRegion::CnBeijing, ConsoleSite::International),
            (ConsoleRegion::ApSoutheast1, ConsoleSite::Domestic),
            (ConsoleRegion::ApSoutheast1, ConsoleSite::International),
        ] {
            let mut server = mockito::Server::new_async().await;
            let c = creds(region, site);
            let m = gateway_mock(&mut server, &c)
                .with_status(200)
                .with_body(usage_body())
                .create_async()
                .await;

            let (_td, cache) = cache_fixture();
            let out = fetch_snapshot_with(
                &reqwest::Client::new(),
                &c,
                &cache,
                &Endpoints { base: server.url() },
                Duration::ZERO,
            )
            .await
            .unwrap();

            m.assert_async().await;
            assert_eq!(
                out.snapshot.session.as_ref().unwrap().utilization_pct,
                42,
                "{region:?}/{site:?}"
            );
            assert_eq!(
                out.snapshot.weekly.as_ref().unwrap().utilization_pct,
                74,
                "{region:?}/{site:?}"
            );
            assert!(!out.stale);
        }
    }

    #[tokio::test]
    async fn live_fetch_round_trips_the_epoch_ms_resets() {
        let mut server = mockito::Server::new_async().await;
        let c = creds(ConsoleRegion::CnBeijing, ConsoleSite::Domestic);
        gateway_mock(&mut server, &c)
            .with_status(200)
            .with_body(usage_body())
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let out = fetch_snapshot_with(
            &reqwest::Client::new(),
            &c,
            &cache,
            &Endpoints { base: server.url() },
            Duration::ZERO,
        )
        .await
        .unwrap();
        assert_eq!(
            out.snapshot
                .session
                .as_ref()
                .unwrap()
                .resets_at
                .unwrap()
                .timestamp_millis(),
            1_789_200_000_000
        );
        assert_eq!(
            out.snapshot
                .weekly
                .as_ref()
                .unwrap()
                .resets_at
                .unwrap()
                .timestamp_millis(),
            1_789_600_000_000
        );

        // What was written is what the cache serves back.
        let stored = std::fs::read(cache.payload_path()).unwrap();
        assert!(!String::from_utf8_lossy(&stored).contains("tok-test"));
        assert_eq!(
            parse_cache_at(&stored, &c.fingerprint).unwrap(),
            out.snapshot
        );
    }

    /// `NotLogined` from the gateway is the Credentials re-auth error, and
    /// with a warm cache the last good figures survive beside it.
    #[tokio::test]
    async fn not_logined_is_a_credentials_error_naming_the_fix() {
        let mut server = mockito::Server::new_async().await;
        let c = creds(ConsoleRegion::CnBeijing, ConsoleSite::Domestic);
        gateway_mock(&mut server, &c)
            .with_status(200)
            .with_body(r#"{"success":false,"errorCode":"NotLogined"}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let err = fetch_snapshot_with(
            &reqwest::Client::new(),
            &c,
            &cache,
            &Endpoints { base: server.url() },
            Duration::ZERO,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, AppError::Credentials(_)), "{err:?}");
        assert!(err.to_string().contains("bl auth login --console"), "{err}");
    }

    /// A failed re-auth with a warm cache: stale figures plus the redacted
    /// Credentials diagnostic — never the upstream body.
    #[tokio::test]
    async fn not_logined_with_a_warm_cache_serves_it_stale_with_the_fix() {
        let mut server = mockito::Server::new_async().await;
        let c = creds(ConsoleRegion::CnBeijing, ConsoleSite::Domestic);
        gateway_mock(&mut server, &c)
            .with_status(200)
            .with_body(r#"{"success":false,"errorCode":"NotLogined","token":"tok-test"}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let snap = ModelStudioSnapshot {
            session: Some(UsageWindow {
                utilization_pct: 42,
                resets_at: None,
                window_duration: FIVE_HOUR_WINDOW,
            }),
            weekly: None,
        };
        cache
            .write_payload(&serde_json::to_vec(&snap_to_json(&snap, &c.fingerprint)).unwrap())
            .unwrap();

        let out = fetch_snapshot_with(
            &reqwest::Client::new(),
            &c,
            &cache,
            &Endpoints { base: server.url() },
            Duration::ZERO,
        )
        .await
        .unwrap();
        assert!(out.stale);
        assert_eq!(out.snapshot, snap);
        let (code, body) = out.last_error.unwrap();
        assert_eq!(code, 0);
        assert!(!body.contains("tok-test"), "{body}");
        assert!(body.contains("bl auth login --console"), "{body}");
    }

    /// Any other `success:false` code is a schema failure carrying its own
    /// name — never a confident zero that could overwrite a good cache.
    #[tokio::test]
    async fn other_gateway_failures_are_schema_errors() {
        let mut server = mockito::Server::new_async().await;
        let c = creds(ConsoleRegion::CnBeijing, ConsoleSite::Domestic);
        gateway_mock(&mut server, &c)
            .with_status(200)
            .with_body(r#"{"success":false,"errorCode":"SystemExeptionError"}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let err = fetch_snapshot_with(
            &reqwest::Client::new(),
            &c,
            &cache,
            &Endpoints { base: server.url() },
            Duration::ZERO,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, AppError::Schema(_)), "{err:?}");
        assert!(err.to_string().contains("SystemExeptionError"), "{err}");
    }

    /// HTTP 401 bodies are redacted before they reach the cache.
    #[tokio::test]
    async fn an_http_401_is_redacted_and_falls_back_to_the_cache() {
        let mut server = mockito::Server::new_async().await;
        let c = creds(ConsoleRegion::CnBeijing, ConsoleSite::Domestic);
        server
            .mock("POST", usage_path("BroadScopeAspnGateway").as_str())
            .with_status(401)
            .with_body(r#"{"message":"Bearer tok-test echoed"}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let snap = ModelStudioSnapshot {
            session: None,
            weekly: Some(UsageWindow {
                utilization_pct: 74,
                resets_at: None,
                window_duration: WEEKLY_WINDOW,
            }),
        };
        cache
            .write_payload(&serde_json::to_vec(&snap_to_json(&snap, &c.fingerprint)).unwrap())
            .unwrap();

        let out = fetch_snapshot_with(
            &reqwest::Client::new(),
            &c,
            &cache,
            &Endpoints { base: server.url() },
            Duration::ZERO,
        )
        .await
        .unwrap();
        assert!(out.stale);
        assert_eq!(out.snapshot.weekly.as_ref().unwrap().utilization_pct, 74);
        let (code, body) = out.last_error.unwrap();
        assert_eq!(code, 401);
        assert_eq!(body, "Model Studio authentication failed");
    }

    #[tokio::test]
    async fn a_fresh_cache_is_served_without_a_network_call() {
        let (_td, cache) = cache_fixture();
        let c = creds(ConsoleRegion::CnBeijing, ConsoleSite::Domestic);
        let snap = ModelStudioSnapshot {
            session: None,
            weekly: Some(UsageWindow {
                utilization_pct: 74,
                resets_at: chrono::DateTime::from_timestamp_millis(1_789_600_000_000),
                window_duration: WEEKLY_WINDOW,
            }),
        };
        cache
            .write_payload(&serde_json::to_vec(&snap_to_json(&snap, &c.fingerprint)).unwrap())
            .unwrap();

        // No mock server at all: a cache hit must never reach the network.
        let out = fetch_snapshot_with(
            &reqwest::Client::new(),
            &c,
            &cache,
            &Endpoints {
                base: "http://127.0.0.1:1".into(),
            },
            Duration::from_secs(60),
        )
        .await
        .unwrap();

        assert!(!out.stale);
        assert_eq!(out.snapshot, snap);
    }

    /// A cache from a previous login must not be shown for a new one.
    #[tokio::test]
    async fn a_cache_from_a_previous_login_is_not_reused() {
        let mut server = mockito::Server::new_async().await;
        let c = creds(ConsoleRegion::CnBeijing, ConsoleSite::Domestic);
        let m = gateway_mock(&mut server, &c)
            .with_status(200)
            .with_body(usage_body())
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let stale_login = ModelStudioSnapshot {
            session: Some(UsageWindow {
                utilization_pct: 99,
                resets_at: None,
                window_duration: FIVE_HOUR_WINDOW,
            }),
            weekly: None,
        };
        cache
            .write_payload(
                &serde_json::to_vec(&snap_to_json(&stale_login, "someone-elses-fingerprint"))
                    .unwrap(),
            )
            .unwrap();

        // A long TTL: only the fingerprint mismatch can explain a refetch.
        let out = fetch_snapshot_with(
            &reqwest::Client::new(),
            &c,
            &cache,
            &Endpoints { base: server.url() },
            Duration::from_secs(3600),
        )
        .await
        .unwrap();
        m.assert_async().await;
        assert!(!out.stale);
        assert_eq!(
            out.snapshot.session.as_ref().unwrap().utilization_pct,
            42,
            "refetched, not 99"
        );
    }

    #[tokio::test]
    async fn a_transport_error_with_a_stale_cache_uses_the_cache() {
        let (_td, cache) = cache_fixture();
        let c = creds(ConsoleRegion::CnBeijing, ConsoleSite::Domestic);
        let snap = ModelStudioSnapshot {
            session: Some(UsageWindow {
                utilization_pct: 42,
                resets_at: None,
                window_duration: FIVE_HOUR_WINDOW,
            }),
            weekly: None,
        };
        cache
            .write_payload(&serde_json::to_vec(&snap_to_json(&snap, &c.fingerprint)).unwrap())
            .unwrap();

        let out = fetch_snapshot_with(
            &reqwest::Client::new(),
            &c,
            &cache,
            &Endpoints {
                base: "http://127.0.0.1:1".into(),
            },
            Duration::ZERO,
        )
        .await
        .unwrap();

        assert!(out.stale);
        assert_eq!(out.snapshot, snap);
    }

    #[test]
    fn cache_round_trips_both_windows_and_validates() {
        let snap = ModelStudioSnapshot {
            session: Some(UsageWindow {
                utilization_pct: 42,
                resets_at: chrono::DateTime::from_timestamp_millis(1_789_200_000_000),
                window_duration: FIVE_HOUR_WINDOW,
            }),
            weekly: Some(UsageWindow {
                utilization_pct: 74,
                resets_at: None,
                window_duration: WEEKLY_WINDOW,
            }),
        };
        let bytes = serde_json::to_vec(&snap_to_json(&snap, "fp")).unwrap();
        assert_eq!(parse_cache_at(&bytes, "fp").unwrap(), snap);

        // Absent windows round-trip too.
        let absent = ModelStudioSnapshot {
            session: None,
            weekly: None,
        };
        let bytes = serde_json::to_vec(&snap_to_json(&absent, "fp")).unwrap();
        assert_eq!(parse_cache_at(&bytes, "fp").unwrap(), absent);

        // Out-of-range percent, string percent, negative reset, and a foreign
        // fingerprint are all drift, not figures.
        for bad in [
            serde_json::json!({"account":"fp","session":{"pct":150},"weekly":null}),
            serde_json::json!({"account":"fp","session":{"pct":"42"},"weekly":null}),
            serde_json::json!({"account":"fp","session":{"pct":42,"reset_ms":-5},"weekly":null}),
            serde_json::json!({"account":"other","session":null,"weekly":null}),
            serde_json::json!({"session":null,"weekly":null}),
        ] {
            assert!(
                parse_cache_at(bad.to_string().as_bytes(), "fp").is_err(),
                "{bad} must not parse"
            );
        }
    }

    /// Endpoints default to the CLI's default row and follow the matrix.
    #[test]
    fn endpoints_follow_the_gateway_matrix() {
        assert_eq!(
            Endpoints::default().base,
            "https://bailian-cs.console.aliyun.com"
        );
        assert_eq!(
            Endpoints::for_gateway(ConsoleRegion::ApSoutheast1, ConsoleSite::International).base,
            "https://bailian-singapore-cs.alibabacloud.com"
        );
        assert_eq!(
            Endpoints::default().usage_url("BroadScopeAspnGateway"),
            "https://bailian-cs.console.aliyun.com/cli/api.json?\
             action=BroadScopeAspnGateway&product=sfm_bailian\
             &api=zeldaHttp.apikeyMgr.%2Ftokenplan%2Fpersonal%2Fapi%2Fv2%2Fusage"
        );
    }
}
