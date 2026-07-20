//! xAI (Grok) fetch — reads the prepaid balance from the Management API. Two
//! steps when no team id is configured: resolve the team from the management
//! key (`/auth/management-keys/validation`), then read
//! `/v1/billing/teams/{team}/prepaid/balance`.

use std::time::Duration;

use crate::cache::{Cache, acquire_lock};
use crate::error::{AppError, Result};
use crate::usage::{GrokSnapshot, finite_amount};

use super::types::{BalanceResp, Validation, to_snapshot};

pub const BASE_URL: &str = "https://management-api.x.ai";
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const LOCK_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub base: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            base: BASE_URL.to_string(),
        }
    }
}

impl Endpoints {
    fn validation_url(&self) -> String {
        format!("{}/auth/management-keys/validation", self.base)
    }
    fn balance_url(&self, team: &str) -> String {
        format!("{}/v1/billing/teams/{team}/prepaid/balance", self.base)
    }
}

#[derive(Debug, Clone)]
pub struct FetchOutcome {
    pub snapshot: GrokSnapshot,
    pub stale: bool,
    pub last_error: Option<(u16, String)>,
    pub cache_age: Option<Duration>,
}

/// `management_key` is the xAI Management API key (distinct from the inference
/// key). `team_id`, when set, skips the validation round-trip.
pub async fn fetch_snapshot(
    client: &reqwest::Client,
    management_key: &str,
    cache: &Cache,
    endpoints: &Endpoints,
    cache_ttl: Duration,
    team_id: Option<&str>,
) -> Result<FetchOutcome> {
    cache.ensure_dir()?;
    let _lock = acquire_lock(&cache.lock_path(), LOCK_TIMEOUT)?;

    // Identity is derived from the *inputs*, not the resolved team, so a fresh
    // cache still costs zero network calls.
    let target = target_key(management_key, team_id);

    if let Some(bytes) = cache.fresh_payload(cache_ttl)?
        && let Ok(outcome) = reuse_cache(&bytes, cache, false, &target)
    {
        return Ok(outcome);
    }

    let team = match resolve_team(client, endpoints, management_key, team_id).await {
        Ok(t) => t,
        Err(e) if e.is_transient() => return fallback_silent(cache, &target, e),
        Err(e) => {
            cache.mark_stale();
            cache.write_last_error(0, &e.to_string());
            let diag = (0, e.to_string());
            return fallback_with_error(cache, Some(diag), &target, e);
        }
    };

    match fetch_live(client, endpoints, management_key, &team).await {
        Ok(snap) => {
            let bytes = serde_json::to_vec(&serde_json::json!({
                "target": target,
                "team": team,
                "snapshot": { "balance": snap.balance },
            }))
            .unwrap_or_default();
            cache.write_payload(&bytes)?;
            Ok(FetchOutcome {
                snapshot: snap,
                stale: false,
                last_error: None,
                cache_age: Some(Duration::ZERO),
            })
        }
        Err(e) if e.is_transient() => fallback_silent(cache, &target, e),
        Err(AppError::Http { status, body }) => {
            cache.mark_stale();
            cache.write_last_error(status, &body);
            let diag = (status, body.clone());
            fallback_with_error(cache, Some(diag), &target, AppError::Http { status, body })
        }
        Err(e) => {
            cache.mark_stale();
            cache.write_last_error(0, &e.to_string());
            let diag = (0, e.to_string());
            fallback_with_error(cache, Some(diag), &target, e)
        }
    }
}

/// Identity of the inputs that determine *whose* balance gets fetched. With an
/// explicit `team_id` that is the team; otherwise the team is derived from the
/// management key, so the key itself is the identity — fingerprinted, never
/// stored in clear. (The fingerprint is only a change detector: if its value
/// ever shifts, the effect is one extra refetch.)
fn target_key(management_key: &str, team_id: Option<&str>) -> String {
    match team_id.filter(|t| !t.is_empty()) {
        Some(t) => format!("team:{t}"),
        None => {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            management_key.hash(&mut h);
            format!("key:{:016x}", h.finish())
        }
    }
}

/// An explicit `team_id` wins; otherwise introspect the management key. The
/// scope decides whether `scopeId` is a team at all — see [`Validation`].
async fn resolve_team(
    client: &reqwest::Client,
    endpoints: &Endpoints,
    key: &str,
    team_id: Option<&str>,
) -> Result<String> {
    match team_id {
        Some(t) if !t.is_empty() => Ok(t.to_string()),
        _ => {
            let v: Validation = get_json(client, &endpoints.validation_url(), key).await?;
            v.resolved_team()
        }
    }
}

fn fallback_silent(cache: &Cache, target: &str, original: AppError) -> Result<FetchOutcome> {
    let Some(bytes) = cache.maybe_payload()? else {
        return Err(original);
    };
    reuse_cache(&bytes, cache, true, target)
}

/// On failure we show the last good figure with the error alongside it. With
/// nothing usable cached there is nothing to show, so the **original** error is
/// returned — a first run against an organization-scoped key must reach the
/// user with its guidance intact, not as a generic "no usable cache".
fn fallback_with_error(
    cache: &Cache,
    last_error: Option<(u16, String)>,
    target: &str,
    original: AppError,
) -> Result<FetchOutcome> {
    let Some(bytes) = cache.maybe_payload()? else {
        return Err(original);
    };
    // A cache we cannot attribute to this target is no better than no cache.
    let Ok(mut outcome) = reuse_cache(&bytes, cache, true, target) else {
        return Err(original);
    };
    outcome.last_error = last_error;
    Ok(outcome)
}

fn reuse_cache(bytes: &[u8], cache: &Cache, stale: bool, target: &str) -> Result<FetchOutcome> {
    let snap = parse_cache(bytes, target)?;
    Ok(FetchOutcome {
        snapshot: snap,
        stale,
        last_error: cache.read_last_error(),
        cache_age: cache.payload_age(),
    })
}

fn parse_cache(bytes: &[u8], target: &str) -> Result<GrokSnapshot> {
    let v: serde_json::Value = serde_json::from_slice(bytes)?;
    // A balance cached for another team (or fetched with a different key) is
    // not this team's money. Payloads written before the target was recorded
    // are equally unattributable.
    let cached_target = v.get("target").and_then(serde_json::Value::as_str);
    if cached_target != Some(target) {
        return Err(AppError::Schema(format!(
            "grok cache belongs to a different team ({}); refetching",
            v.get("team")
                .and_then(serde_json::Value::as_str)
                .or(cached_target)
                .unwrap_or("unknown")
        )));
    }
    let s = v
        .get("snapshot")
        .ok_or_else(|| AppError::Schema("grok cache missing 'snapshot' field".into()))?;
    let balance = s["balance"]
        .as_f64()
        .ok_or_else(|| AppError::Schema("grok cache missing 'balance'".into()))?;
    Ok(GrokSnapshot {
        balance: finite_amount("grok cache", "balance", balance)?,
    })
}

async fn fetch_live(
    client: &reqwest::Client,
    endpoints: &Endpoints,
    key: &str,
    team: &str,
) -> Result<GrokSnapshot> {
    let resp: BalanceResp = get_json(client, &endpoints.balance_url(team), key).await?;
    to_snapshot(resp)
}

async fn get_json<T: for<'de> serde::Deserialize<'de>>(
    client: &reqwest::Client,
    url: &str,
    key: &str,
) -> Result<T> {
    let resp = tokio::time::timeout(
        HTTP_TIMEOUT,
        client
            .get(url)
            .header("Authorization", format!("Bearer {key}"))
            .send(),
    )
    .await
    .map_err(|_| AppError::Transport(format!("grok timeout: {url}")))??;

    let status = resp.status();
    let bytes = resp.bytes().await?;
    if !status.is_success() {
        let body = String::from_utf8_lossy(&bytes).chars().take(200).collect();
        return Err(AppError::Http {
            status: status.as_u16(),
            body,
        });
    }
    serde_json::from_slice(&bytes).map_err(|e| AppError::Schema(format!("grok {url}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn cache_fixture() -> (TempDir, Cache) {
        let td = TempDir::new().unwrap();
        let cache = Cache::at(td.path().join("grok"));
        cache.ensure_dir().unwrap();
        (td, cache)
    }

    #[tokio::test]
    async fn resolves_team_then_reads_balance() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/auth/management-keys/validation")
            .with_status(200)
            .with_body(r#"{"scopeId":"team-xyz","teamId":"team-xyz"}"#)
            .create_async()
            .await;
        server
            .mock("GET", "/v1/billing/teams/team-xyz/prepaid/balance")
            .match_header("authorization", "Bearer xai-mgmt")
            .with_status(200)
            .with_body(r#"{"changes":[],"total":{"val":"-2500"}}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let endpoints = Endpoints { base: server.url() };
        let out = fetch_snapshot(
            &client,
            "xai-mgmt",
            &cache,
            &endpoints,
            Duration::from_secs(0),
            None,
        )
        .await
        .unwrap();
        assert!((out.snapshot.balance - 25.0).abs() < 1e-9);
        assert!(!out.stale);
    }

    #[tokio::test]
    async fn configured_team_skips_validation() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/v1/billing/teams/my-team/prepaid/balance")
            .with_status(200)
            .with_body(r#"{"total":{"val":"-500"}}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let endpoints = Endpoints { base: server.url() };
        let out = fetch_snapshot(
            &client,
            "xai-mgmt",
            &cache,
            &endpoints,
            Duration::from_secs(0),
            Some("my-team"),
        )
        .await
        .unwrap();
        assert!((out.snapshot.balance - 5.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn http_404_falls_back_to_cache_when_present() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/v1/billing/teams/t/prepaid/balance")
            .with_status(404)
            .with_body(r#"{"error":"no team"}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        cache
            .write_payload(
                serde_json::json!({
                    "target": "team:t",
                    "team": "t",
                    "snapshot": { "balance": 12.0 },
                })
                .to_string()
                .as_bytes(),
            )
            .unwrap();

        let client = reqwest::Client::new();
        let endpoints = Endpoints { base: server.url() };
        let out = fetch_snapshot(
            &client,
            "k",
            &cache,
            &endpoints,
            Duration::from_secs(0),
            Some("t"),
        )
        .await
        .unwrap();
        assert!(out.stale);
        assert_eq!(out.snapshot.balance, 12.0);
        assert_eq!(out.last_error.as_ref().map(|(c, _)| *c), Some(404));
    }

    #[tokio::test]
    async fn switching_team_refetches_instead_of_reusing_the_cache() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/v1/billing/teams/team-b/prepaid/balance")
            .with_status(200)
            .with_body(r#"{"total":{"val":"-300"}}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        // Fresh, but it is team-a's money.
        cache
            .write_payload(
                serde_json::json!({
                    "target": "team:team-a",
                    "team": "team-a",
                    "snapshot": { "balance": 999.0 },
                })
                .to_string()
                .as_bytes(),
            )
            .unwrap();

        let client = reqwest::Client::new();
        let endpoints = Endpoints { base: server.url() };
        let out = fetch_snapshot(
            &client,
            "k",
            &cache,
            &endpoints,
            Duration::from_secs(3600),
            Some("team-b"),
        )
        .await
        .unwrap();
        assert!((out.snapshot.balance - 3.0).abs() < 1e-9);
        assert!(!out.stale);
    }

    #[tokio::test]
    async fn fresh_cache_makes_no_network_call() {
        // No mocks registered: any request would fail the test.
        let server = mockito::Server::new_async().await;
        let (_td, cache) = cache_fixture();
        cache
            .write_payload(
                serde_json::json!({
                    "target": "team:t",
                    "team": "t",
                    "snapshot": { "balance": 4.5 },
                })
                .to_string()
                .as_bytes(),
            )
            .unwrap();

        let client = reqwest::Client::new();
        let endpoints = Endpoints { base: server.url() };
        let out = fetch_snapshot(
            &client,
            "k",
            &cache,
            &endpoints,
            Duration::from_secs(3600),
            Some("t"),
        )
        .await
        .unwrap();
        assert_eq!(out.snapshot.balance, 4.5);
    }

    #[tokio::test]
    async fn organization_scoped_key_reports_an_actionable_error() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/auth/management-keys/validation")
            .with_status(200)
            .with_body(r#"{"scope":"SCOPE_ORGANIZATION","scopeId":"org-77"}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let endpoints = Endpoints { base: server.url() };
        // No cache to fall back on, so the guidance must surface to the user.
        let out = fetch_snapshot(
            &client,
            "k",
            &cache,
            &endpoints,
            Duration::from_secs(0),
            None,
        )
        .await;
        let err = out.unwrap_err().to_string();
        assert!(err.contains("team_id"), "unhelpful error: {err}");
        assert!(!err.contains("org-77"), "must not adopt the org id: {err}");
    }

    #[tokio::test]
    async fn malformed_200_balance_does_not_become_zero() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/v1/billing/teams/t/prepaid/balance")
            .with_status(200)
            .with_body(r#"{"error":"forbidden"}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let endpoints = Endpoints { base: server.url() };
        let out = fetch_snapshot(
            &client,
            "k",
            &cache,
            &endpoints,
            Duration::from_secs(0),
            Some("t"),
        )
        .await;
        assert!(out.is_err(), "expected a schema error, got {out:?}");
    }
}
