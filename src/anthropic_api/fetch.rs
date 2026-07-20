//! Anthropic Admin API fetch — sums the current month's `cost_report` buckets
//! into a month-to-date spend, under the shared cache + flock primitives.
//!
//! Auth is a Console **Admin key** (`sk-ant-admin01-…`, distinct from an
//! inference key) in the `x-api-key` header. The monthly `limit` is NOT part of
//! the API response — it's supplied from config and carried in the snapshot so
//! the renderer can show spend-vs-limit.

use std::time::Duration;

use chrono::{DateTime, Datelike, Utc};

use crate::cache::{Cache, MAX_STALE, acquire_lock_async};
use crate::error::{AppError, Result};
use crate::usage::{AnthropicApiSnapshot, finite_amount};
use crate::vendor::{MAX_BODY_BYTES, read_body_capped};

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

/// Identity of the organization whose spend is cached. The Admin API does not
/// return an organization id in the cost report, so the key itself is the only
/// zero-round-trip identity available. Store only a fingerprint: it is a cache
/// change detector, not an authentication secret. If Rust ever changes the
/// hasher algorithm, the harmless result is one extra refetch after upgrade.
fn target_key(admin_key: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    admin_key.hash(&mut hasher);
    format!("key:{:016x}", hasher.finish())
}

fn validate_limit(limit: Option<f64>) -> Result<Option<f64>> {
    if let Some(value) = limit
        && (!value.is_finite() || value <= 0.0)
    {
        return Err(AppError::Schema(
            "anthropic-api monthly_limit must be finite and greater than zero; \
             remove it to show spend without a limit"
                .into(),
        ));
    }
    Ok(limit)
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
    fetch_snapshot_at(
        client,
        admin_key,
        cache,
        endpoints,
        cache_ttl,
        limit,
        Utc::now(),
    )
    .await
}

/// Same as [`fetch_snapshot`] with an injected clock — the seam month-rollover
/// tests use, so they never depend on the wall clock.
pub async fn fetch_snapshot_at(
    client: &reqwest::Client,
    admin_key: &str,
    cache: &Cache,
    endpoints: &Endpoints,
    cache_ttl: Duration,
    limit: Option<f64>,
    now: DateTime<Utc>,
) -> Result<FetchOutcome> {
    let limit = validate_limit(limit)?;
    cache.ensure_dir()?;
    let _lock = acquire_lock_async(&cache.lock_path(), LOCK_TIMEOUT).await?;

    // The query always starts at the current month, so a payload written last
    // month is a *different* figure — not a stale version of this one.
    let month = month_start_rfc3339(now);
    let target = target_key(admin_key);

    if let Some(bytes) = cache.fresh_payload(cache_ttl)?
        && let Ok(outcome) = reuse_cache(&bytes, cache, false, limit, &month, &target)
    {
        return Ok(outcome);
    }

    match fetch_live(client, endpoints, admin_key, now).await {
        Ok(spent) => {
            let snap = AnthropicApiSnapshot { spent, limit };
            let bytes = serde_json::to_vec(&serde_json::json!({
                "month": month,
                "target": target,
                "snapshot": { "spent": snap.spent, "limit": snap.limit },
            }))?;
            cache.write_payload(&bytes)?;
            Ok(FetchOutcome {
                snapshot: snap,
                stale: false,
                last_error: None,
                cache_age: Some(Duration::ZERO),
            })
        }
        Err(e) if e.is_transient() => fallback_silent(cache, limit, &month, &target, e),
        Err(AppError::Http { status, body }) => {
            cache.mark_stale();
            cache.write_last_error(status, &body);
            let diag = (status, body.clone());
            fallback_with_error(
                cache,
                Some(diag),
                limit,
                &month,
                &target,
                AppError::Http { status, body },
            )
        }
        Err(e) => {
            cache.mark_stale();
            cache.write_last_error(0, &e.to_string());
            let diag = (0, e.to_string());
            fallback_with_error(cache, Some(diag), limit, &month, &target, e)
        }
    }
}

fn fallback_silent(
    cache: &Cache,
    limit: Option<f64>,
    month: &str,
    target: &str,
    original: AppError,
) -> Result<FetchOutcome> {
    let Some(bytes) = cache.fallback_payload(MAX_STALE)? else {
        return Err(original);
    };
    reuse_cache(&bytes, cache, true, limit, month, target)
}

/// On failure we show the last good figure with the error alongside it. With
/// nothing usable cached there is nothing to show, so the **original** error is
/// returned — a first-run 401/403 or schema error must reach the user with its
/// Admin-key guidance intact, not as a generic "no usable cache".
fn fallback_with_error(
    cache: &Cache,
    last_error: Option<(u16, String)>,
    limit: Option<f64>,
    month: &str,
    target: &str,
    original: AppError,
) -> Result<FetchOutcome> {
    let Some(bytes) = cache.fallback_payload(MAX_STALE)? else {
        return Err(original);
    };
    // Last month's spend is not this month's, so during an outage it is better
    // to report the error than to display the wrong month as current.
    let Ok(mut outcome) = reuse_cache(&bytes, cache, true, limit, month, target) else {
        return Err(original);
    };
    outcome.last_error = last_error;
    Ok(outcome)
}

fn reuse_cache(
    bytes: &[u8],
    cache: &Cache,
    stale: bool,
    limit: Option<f64>,
    month: &str,
    target: &str,
) -> Result<FetchOutcome> {
    // The cached spend is authoritative; the limit always comes from the
    // current config so editing it takes effect without a refetch.
    let spent = parse_cached_spent(bytes, month, target)?;
    Ok(FetchOutcome {
        snapshot: AnthropicApiSnapshot { spent, limit },
        stale,
        last_error: cache.read_last_error(),
        cache_age: cache.payload_age(),
    })
}

fn parse_cached_spent(bytes: &[u8], month: &str, target: &str) -> Result<f64> {
    let v: serde_json::Value = serde_json::from_slice(bytes)?;
    // Payloads written before the month was recorded cannot be attributed to
    // one, so they are refetched rather than shown as the current month.
    let cached_month = v.get("month").and_then(serde_json::Value::as_str);
    if cached_month != Some(month) {
        return Err(AppError::Schema(format!(
            "anthropic-api cache is for a different month ({}); refetching",
            cached_month.unwrap_or("unknown")
        )));
    }
    let cached_target = v.get("target").and_then(serde_json::Value::as_str);
    if cached_target != Some(target) {
        return Err(AppError::Schema(
            "anthropic-api cache belongs to a different Admin key; refetching".into(),
        ));
    }
    let s = v
        .get("snapshot")
        .ok_or_else(|| AppError::Schema("anthropic-api cache missing 'snapshot'".into()))?;
    let spent = s["spent"]
        .as_f64()
        .ok_or_else(|| AppError::Schema("anthropic-api cache missing 'spent'".into()))?;
    crate::usage::finite_amount("anthropic-api cache", "spent", spent)
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
    let mut seen_pages: Vec<String> = Vec::new();

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
        let bytes = read_body_capped(resp, MAX_BODY_BYTES).await?;
        if !status.is_success() {
            let body = String::from_utf8_lossy(&bytes).chars().take(200).collect();
            return Err(AppError::Http {
                status: status.as_u16(),
                body,
            });
        }

        let report: CostReport = serde_json::from_slice(&bytes)
            .map_err(|e| AppError::Schema(format!("anthropic-api cost_report: {e}")))?;
        total = finite_amount(
            "anthropic-api",
            "cost_report running total",
            total + page_dollars(&report)?,
        )?;

        // A partial total is indistinguishable from a genuinely smaller spend
        // once cached, so every way pagination can go wrong is an error rather
        // than an early `break` with whatever was summed so far.
        match (report.has_more, report.next_page) {
            (false, _) => return Ok(total),
            (true, None) => {
                return Err(AppError::Schema(
                    "anthropic-api cost_report: has_more is true but next_page is missing; \
                     refusing to report a partial month"
                        .into(),
                ));
            }
            (true, Some(p)) if p.trim().is_empty() => {
                return Err(AppError::Schema(
                    "anthropic-api cost_report: has_more is true but next_page is empty; \
                     refusing to report a partial month"
                        .into(),
                ));
            }
            (true, Some(p)) => {
                if seen_pages.contains(&p) {
                    return Err(AppError::Schema(format!(
                        "anthropic-api cost_report: pagination repeated cursor {p:?}; \
                         refusing to report a partial month"
                    )));
                }
                seen_pages.push(p.clone());
                page = Some(p);
            }
        }
    }
    Err(AppError::Schema(format!(
        "anthropic-api cost_report: more than {MAX_PAGES} pages for one month; \
         refusing to report a partial month"
    )))
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
                r#"{"data":[{"results":[{"amount":"100.0","currency":"USD"},
                    {"amount":"34.0","currency":"USD"}]}],
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
        let now = at(2026, 7, 19);
        cache
            .write_payload(
                serde_json::json!({
                    "month": month_start_rfc3339(now),
                    "target": target_key("k"),
                    "snapshot": { "spent": 2.5, "limit": null },
                })
                .to_string()
                .as_bytes(),
            )
            .unwrap();

        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            cost_report: format!("{}/v1/organizations/cost_report", server.url()),
        };
        let out = fetch_snapshot_at(
            &client,
            "k",
            &cache,
            &endpoints,
            Duration::from_secs(0),
            Some(50.0),
            now,
        )
        .await
        .unwrap();
        assert!(out.stale);
        assert!((out.snapshot.spent - 2.5).abs() < 1e-9);
        // limit comes from the current call, not the (null) cache.
        assert_eq!(out.snapshot.limit, Some(50.0));
        assert_eq!(out.last_error.as_ref().map(|(c, _)| *c), Some(401));
    }

    /// Fixed instant helper — tests never read the wall clock.
    fn at(y: i32, m: u32, d: u32) -> DateTime<Utc> {
        chrono::NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_utc()
    }

    fn ok_body(cents: &str) -> String {
        format!(
            r#"{{"data":[{{"results":[{{"amount":"{cents}","currency":"USD"}}]}}],"has_more":false}}"#
        )
    }

    #[tokio::test]
    async fn month_rollover_refetches_instead_of_showing_last_month() {
        // June's spend must never be displayed as July's, even while the
        // payload is still inside the TTL.
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/v1/organizations/cost_report")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_body(ok_body("250.0"))
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        cache
            .write_payload(
                serde_json::json!({
                    "month": month_start_rfc3339(at(2026, 6, 30)),
                    "target": target_key("k"),
                    "snapshot": { "spent": 987.0, "limit": null },
                })
                .to_string()
                .as_bytes(),
            )
            .unwrap();

        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            cost_report: format!("{}/v1/organizations/cost_report", server.url()),
        };
        // Long TTL: the payload IS fresh, it is just the wrong month.
        let out = fetch_snapshot_at(
            &client,
            "k",
            &cache,
            &endpoints,
            Duration::from_secs(3600),
            None,
            at(2026, 7, 1),
        )
        .await
        .unwrap();
        assert!((out.snapshot.spent - 2.5).abs() < 1e-9);
        assert!(!out.stale);
    }

    #[tokio::test]
    async fn last_months_cache_is_not_served_during_an_outage() {
        // With the API down and only June cached, reporting June as the current
        // month is worse than surfacing the error.
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/v1/organizations/cost_report")
            .match_query(mockito::Matcher::Any)
            .with_status(500)
            .with_body("upstream boom")
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        cache
            .write_payload(
                serde_json::json!({
                    "month": month_start_rfc3339(at(2026, 6, 30)),
                    "target": target_key("k"),
                    "snapshot": { "spent": 987.0, "limit": null },
                })
                .to_string()
                .as_bytes(),
            )
            .unwrap();

        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            cost_report: format!("{}/v1/organizations/cost_report", server.url()),
        };
        let out = fetch_snapshot_at(
            &client,
            "k",
            &cache,
            &endpoints,
            Duration::from_secs(0),
            None,
            at(2026, 7, 1),
        )
        .await;
        assert!(out.is_err(), "expected an error, got {out:?}");
    }

    #[tokio::test]
    async fn first_run_auth_failure_preserves_the_original_error() {
        // No cache: the actionable Admin-key message must reach the user
        // instead of a generic "no usable cache".
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/v1/organizations/cost_report")
            .match_query(mockito::Matcher::Any)
            .with_status(401)
            .with_body(r#"{"error":{"message":"invalid x-api-key"}}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            cost_report: format!("{}/v1/organizations/cost_report", server.url()),
        };
        let err = fetch_snapshot_at(
            &client,
            "k",
            &cache,
            &endpoints,
            Duration::from_secs(0),
            None,
            at(2026, 7, 19),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, AppError::Http { status: 401, .. }),
            "original error must survive, got {err:?}"
        );
        assert!(err.to_string().contains("invalid x-api-key"));
    }

    #[tokio::test]
    async fn has_more_without_next_page_is_an_error_not_a_partial_month() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/v1/organizations/cost_report")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_body(
                r#"{"data":[{"results":[{"amount":"100.0","currency":"USD"}]}],
                    "has_more":true}"#,
            )
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            cost_report: format!("{}/v1/organizations/cost_report", server.url()),
        };
        let out = fetch_snapshot_at(
            &client,
            "k",
            &cache,
            &endpoints,
            Duration::from_secs(0),
            None,
            at(2026, 7, 19),
        )
        .await;
        assert!(out.is_err(), "partial month must not be reported: {out:?}");
    }

    #[tokio::test]
    async fn repeated_pagination_cursor_is_an_error() {
        // A server that always hands back the same cursor would otherwise be
        // summed MAX_PAGES times and cached as a wildly inflated spend.
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/v1/organizations/cost_report")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_body(
                r#"{"data":[{"results":[{"amount":"100.0","currency":"USD"}]}],
                    "has_more":true,"next_page":"same"}"#,
            )
            .expect_at_least(1)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            cost_report: format!("{}/v1/organizations/cost_report", server.url()),
        };
        let out = fetch_snapshot_at(
            &client,
            "k",
            &cache,
            &endpoints,
            Duration::from_secs(0),
            None,
            at(2026, 7, 19),
        )
        .await;
        assert!(out.is_err(), "cursor loop must not be reported: {out:?}");
    }

    #[tokio::test]
    async fn malformed_200_is_not_cached_as_zero_spend() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/v1/organizations/cost_report")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_body(r#"{"error":{"message":"permission_error"}}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            cost_report: format!("{}/v1/organizations/cost_report", server.url()),
        };
        let out = fetch_snapshot_at(
            &client,
            "k",
            &cache,
            &endpoints,
            Duration::from_secs(0),
            None,
            at(2026, 7, 19),
        )
        .await;
        assert!(out.is_err(), "expected a schema error, got {out:?}");
        // And nothing was written to the cache.
        assert!(cache.maybe_payload().unwrap().is_none());
    }

    #[tokio::test]
    async fn a_month_with_no_spend_is_cached_as_a_real_zero() {
        // The legitimate zero must still work end to end.
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/v1/organizations/cost_report")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_body(r#"{"data":[],"has_more":false,"next_page":null}"#)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            cost_report: format!("{}/v1/organizations/cost_report", server.url()),
        };
        let out = fetch_snapshot_at(
            &client,
            "k",
            &cache,
            &endpoints,
            Duration::from_secs(0),
            None,
            at(2026, 7, 19),
        )
        .await
        .unwrap();
        assert_eq!(out.snapshot.spent, 0.0);
        assert!(!out.stale);
        assert!(cache.maybe_payload().unwrap().is_some());
    }

    #[tokio::test]
    async fn switching_admin_key_refetches_instead_of_reusing_another_organization() {
        let mut server = mockito::Server::new_async().await;
        let first = server
            .mock("GET", "/v1/organizations/cost_report")
            .match_header("x-api-key", "org-a-key")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_body(ok_body("100.0"))
            .expect(1)
            .create_async()
            .await;
        let second = server
            .mock("GET", "/v1/organizations/cost_report")
            .match_header("x-api-key", "org-b-key")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_body(ok_body("250.0"))
            .expect(1)
            .create_async()
            .await;

        let (_td, cache) = cache_fixture();
        let client = reqwest::Client::new();
        let endpoints = Endpoints {
            cost_report: format!("{}/v1/organizations/cost_report", server.url()),
        };
        let now = at(2026, 7, 19);
        let a = fetch_snapshot_at(
            &client,
            "org-a-key",
            &cache,
            &endpoints,
            Duration::ZERO,
            None,
            now,
        )
        .await
        .unwrap();
        assert_eq!(a.snapshot.spent, 1.0);

        // The first payload is fresh, but belongs to a different Admin key.
        let b = fetch_snapshot_at(
            &client,
            "org-b-key",
            &cache,
            &endpoints,
            Duration::from_secs(3600),
            None,
            now,
        )
        .await
        .unwrap();
        assert_eq!(b.snapshot.spent, 2.5);
        first.assert_async().await;
        second.assert_async().await;
    }

    #[tokio::test]
    async fn invalid_monthly_limits_fail_before_network_or_cache_access() {
        let client = reqwest::Client::new();
        let cache = Cache::at(std::path::PathBuf::from("unused-invalid-limit-cache"));
        for limit in [0.0, -1.0, f64::INFINITY, f64::NAN] {
            let err = fetch_snapshot_at(
                &client,
                "key",
                &cache,
                &Endpoints::default(),
                Duration::ZERO,
                Some(limit),
                at(2026, 7, 19),
            )
            .await
            .unwrap_err();
            assert!(err.to_string().contains("monthly_limit"), "{err:?}");
        }
        assert!(!cache.dir().exists());
    }
}
