//! Anthropic Admin API fetch — sums the current month's `cost_report` buckets
//! into a month-to-date spend, under the shared cache + flock primitives.
//!
//! Auth is a Console **Admin key** (`sk-ant-admin01-…`, distinct from an
//! inference key) in the `x-api-key` header. The monthly `limit` is NOT part of
//! the API response — it's supplied from config and carried in the snapshot so
//! the renderer can show spend-vs-limit.

use std::time::Duration;

use chrono::{DateTime, Datelike, Utc};

use crate::cache::{Cache, acquire_lock};
use crate::error::{AppError, Result};
use crate::usage::AnthropicApiSnapshot;

use super::types::{CostReport, page_dollars};

pub const BASE_URL: &str = "https://api.anthropic.com";
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const LOCK_TIMEOUT: Duration = Duration::from_secs(15);
/// Safety cap on pagination — a single month is at most 31 daily buckets, so a
/// handful of pages is plenty; this just bounds a runaway `next_page` loop.
const MAX_PAGES: usize = 12;

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub cost_report: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            cost_report: format!("{BASE_URL}/v1/organizations/cost_report"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FetchOutcome {
    pub snapshot: AnthropicApiSnapshot,
    pub stale: bool,
    pub last_error: Option<(u16, String)>,
    pub cache_age: Option<Duration>,
}

/// First instant of `now`'s calendar month, as an RFC-3339 UTC string.
fn month_start_rfc3339(now: DateTime<Utc>) -> String {
    format!("{:04}-{:02}-01T00:00:00Z", now.year(), now.month())
}

/// `limit` is the user-configured monthly USD limit (from config, not the API);
/// it's threaded through so the cached snapshot reflects the current config.
pub async fn fetch_snapshot(
    client: &reqwest::Client,
    admin_key: &str,
    cache: &Cache,
    endpoints: &Endpoints,
    cache_ttl: Duration,
    limit: Option<f64>,
) -> Result<FetchOutcome> {
    cache.ensure_dir()?;
    let _lock = acquire_lock(&cache.lock_path(), LOCK_TIMEOUT)?;

    if let Some(bytes) = cache.fresh_payload(cache_ttl)? {
        return Ok(reuse_cache(bytes, cache, false, limit));
    }

    match fetch_live(client, endpoints, admin_key, Utc::now()).await {
        Ok(spent) => {
            let snap = AnthropicApiSnapshot { spent, limit };
            let bytes = serde_json::to_vec(&serde_json::json!({
                "snapshot": { "spent": snap.spent, "limit": snap.limit },
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
        Err(e) if e.is_transient() => fallback_silent(cache, limit),
        Err(AppError::Http { status, body }) => {
            cache.mark_stale();
            cache.write_last_error(status, &body);
            fallback_with_error(cache, Some((status, body)), limit)
        }
        Err(e) => {
            cache.mark_stale();
            cache.write_last_error(0, &e.to_string());
            fallback_with_error(cache, Some((0, e.to_string())), limit)
        }
    }
}

fn fallback_silent(cache: &Cache, limit: Option<f64>) -> Result<FetchOutcome> {
    let Some(bytes) = cache.maybe_payload()? else {
        return Err(AppError::Transport(
            "anthropic-api: no cache and network unreachable".into(),
        ));
    };
    Ok(reuse_cache(bytes, cache, true, limit))
}

fn fallback_with_error(
    cache: &Cache,
    last_error: Option<(u16, String)>,
    limit: Option<f64>,
) -> Result<FetchOutcome> {
    let Some(bytes) = cache.maybe_payload()? else {
        return Err(AppError::Other("anthropic-api: no usable cache".into()));
    };
    let mut outcome = reuse_cache(bytes, cache, true, limit);
    outcome.last_error = last_error;
    Ok(outcome)
}

fn reuse_cache(bytes: Vec<u8>, cache: &Cache, stale: bool, limit: Option<f64>) -> FetchOutcome {
    // The cached spend is authoritative; the limit always comes from the
    // current config so editing it takes effect without a refetch.
    let spent = parse_cached_spent(&bytes).unwrap_or(0.0);
    FetchOutcome {
        snapshot: AnthropicApiSnapshot { spent, limit },
        stale,
        last_error: cache.read_last_error(),
        cache_age: cache.payload_age(),
    }
}

fn parse_cached_spent(bytes: &[u8]) -> Result<f64> {
    let v: serde_json::Value = serde_json::from_slice(bytes)?;
    let s = v
        .get("snapshot")
        .ok_or_else(|| AppError::Schema("anthropic-api cache missing 'snapshot'".into()))?;
    Ok(s["spent"].as_f64().unwrap_or(0.0))
}

async fn fetch_live(
    client: &reqwest::Client,
    endpoints: &Endpoints,
    admin_key: &str,
    now: DateTime<Utc>,
) -> Result<f64> {
    let starting_at = month_start_rfc3339(now);
    let mut total = 0.0;
    let mut page: Option<String> = None;

    for _ in 0..MAX_PAGES {
        let mut req = client
            .get(&endpoints.cost_report)
            .header("x-api-key", admin_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .query(&[
                ("starting_at", starting_at.as_str()),
                ("bucket_width", "1d"),
            ]);
        if let Some(p) = &page {
            req = req.query(&[("page", p.as_str())]);
        }

        let resp = tokio::time::timeout(HTTP_TIMEOUT, req.send())
            .await
            .map_err(|_| {
                AppError::Transport(format!("anthropic-api timeout: {}", endpoints.cost_report))
            })??;

        let status = resp.status();
        let bytes = resp.bytes().await?;
        if !status.is_success() {
            let body = String::from_utf8_lossy(&bytes).chars().take(200).collect();
            return Err(AppError::Http {
                status: status.as_u16(),
                body,
            });
        }

        let report: CostReport = serde_json::from_slice(&bytes)
            .map_err(|e| AppError::Schema(format!("anthropic-api cost_report: {e}")))?;
        total += page_dollars(&report)?;

        match (report.has_more, report.next_page) {
            (true, Some(p)) => page = Some(p),
            _ => break,
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use tempfile::TempDir;

    fn cache_fixture() -> (TempDir, Cache) {
        let td = TempDir::new().unwrap();
        let cache = Cache::at(td.path().join("anthropic_api"));
        cache.ensure_dir().unwrap();
        (td, cache)
    }

    #[test]
    fn month_start_is_first_of_month_utc() {
        let now = Utc.with_ymd_and_hms(2026, 7, 19, 15, 8, 0).unwrap();
        assert_eq!(month_start_rfc3339(now), "2026-07-01T00:00:00Z");
    }

    #[tokio::test]
    async fn live_fetch_sums_month_to_date_and_divides_by_100() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/v1/organizations/cost_report")
            .match_header("x-api-key", "sk-ant-admin01-test")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_body(
                r#"{"data":[{"results":[{"amount":"100.0"},{"amount":"34.0"}]}],
                    "has_more":false,"next_page":null}"#,
            )
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            cost_report: format!("{}/v1/organizations/cost_report", server.url()),
        };
        let out = fetch_snapshot(
            &client,
            "sk-ant-admin01-test",
            &cache,
            &endpoints,
            Duration::from_secs(0),
            Some(1000.0),
        )
        .await
        .unwrap();
        // 134 cents = $1.34
        assert!((out.snapshot.spent - 1.34).abs() < 1e-9);
        assert_eq!(out.snapshot.limit, Some(1000.0));
        assert!(!out.stale);
    }

    #[tokio::test]
    async fn http_401_falls_back_to_cache_when_present() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/v1/organizations/cost_report")
            .match_query(mockito::Matcher::Any)
            .with_status(401)
            .with_body(r#"{"error":{"message":"invalid x-api-key"}}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        cache
            .write_payload(
                serde_json::json!({ "snapshot": { "spent": 2.5, "limit": null } })
                    .to_string()
                    .as_bytes(),
            )
            .unwrap();

        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            cost_report: format!("{}/v1/organizations/cost_report", server.url()),
        };
        let out = fetch_snapshot(
            &client,
            "k",
            &cache,
            &endpoints,
            Duration::from_secs(0),
            Some(50.0),
        )
        .await
        .unwrap();
        assert!(out.stale);
        assert!((out.snapshot.spent - 2.5).abs() < 1e-9);
        // limit comes from the current call, not the (null) cache.
        assert_eq!(out.snapshot.limit, Some(50.0));
        assert_eq!(out.last_error.as_ref().map(|(c, _)| *c), Some(401));
    }
}
