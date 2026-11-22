//! Fireworks account billing summary: rated cost for the current UTC month.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::cache::{Cache, acquire_lock_async, fingerprint_of, human_backoff};
use crate::error::{AppError, Result};
use crate::format::{placeholders, substitute, updated_at_hm, usd};
use crate::outcome::{Outcome, fallback};
use crate::pango::{color_span, escape};
use crate::theme::Theme;
use crate::tooltip::{Line, render_bordered};
use crate::usage::{FireworksSnapshot, VendorSnapshot, finite_amount};
use crate::vendor::{MAX_BODY_BYTES, RenderOpts, VendorId, VendorOutcome, read_body_capped};
use crate::waybar::{Class, WaybarOutput};

const BASE_URL: &str = "https://api.fireworks.ai";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const LOCK_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_FORMAT: &str = "MTD {fw_monthly_cost}";
const NANOS_PER_UNIT: i128 = 1_000_000_000;

type FetchOutcome = Outcome<FireworksSnapshot>;

impl From<FetchOutcome> for VendorOutcome {
    fn from(outcome: FetchOutcome) -> Self {
        outcome.map(VendorSnapshot::Fireworks)
    }
}

pub(crate) fn validate_account_id(account: &str) -> Result<&str> {
    if account.is_empty()
        || account.len() > 128
        || !account
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(AppError::Credentials(
            "Set [fireworks] account_id to the Fireworks account ID (letters, numbers, hyphens, or underscores).".into(),
        ));
    }
    Ok(account)
}

#[derive(Debug, Clone, Copy)]
struct BillingRange {
    month_start: NaiveDate,
    end: NaiveDate,
}

impl BillingRange {
    fn at(now: DateTime<Utc>) -> Result<Self> {
        let today = now.date_naive();
        let month_start = today.with_day(1);
        let end = today.succ_opt();
        match (month_start, end) {
            (Some(month_start), Some(end)) => Ok(Self { month_start, end }),
            _ => Err(AppError::Other(
                "could not determine the Fireworks billing period".into(),
            )),
        }
    }

    fn endpoint(self, base: &str, account: &str) -> Result<reqwest::Url> {
        let mut url = reqwest::Url::parse(base)
            .map_err(|_| AppError::Other("invalid Fireworks billing base URL".into()))?;
        url.path_segments_mut()
            .map_err(|_| AppError::Other("invalid Fireworks billing base URL".into()))?
            .extend(["v1", "accounts", account, "billing", "summary"]);
        // The server uses only the UTC dates: the end date is exclusive.
        url.query_pairs_mut()
            .append_pair("startTime", &format!("{}T00:00:00Z", self.month_start))
            .append_pair("endTime", &format!("{}T00:00:00Z", self.end));
        Ok(url)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BillingSummary {
    line_items: Vec<LineItem>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LineItem {
    total_cost: Money,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Money {
    currency_code: String,
    // Protobuf JSON omits a scalar field when its value is zero.
    #[serde(default)]
    units: Option<String>,
    #[serde(default)]
    nanos: i32,
}

impl Money {
    fn usd_nanos(self) -> Result<i128> {
        let units = match self.units {
            Some(units) => units.parse::<i64>().map_err(|_| invalid_billing())?,
            None => 0,
        };
        if self.currency_code != "USD"
            || !(-999_999_999..=999_999_999).contains(&self.nanos)
            || (units > 0 && self.nanos < 0 || units < 0 && self.nanos > 0)
        {
            return Err(invalid_billing());
        }
        Ok(i128::from(units) * NANOS_PER_UNIT + i128::from(self.nanos))
    }
}

fn invalid_billing() -> AppError {
    AppError::Schema("Fireworks billing summary did not contain valid USD line items".into())
}

fn parse_billing(bytes: &[u8]) -> Result<FireworksSnapshot> {
    let summary: BillingSummary = serde_json::from_slice(bytes).map_err(|_| invalid_billing())?;
    // usageBuckets, if present, are another view of these same costs.
    let total = summary
        .line_items
        .into_iter()
        .try_fold(0_i128, |total, line| {
            total
                .checked_add(line.total_cost.usd_nanos()?)
                .ok_or_else(invalid_billing)
        })?;
    Ok(FireworksSnapshot {
        monthly_cost_usd: total as f64 / NANOS_PER_UNIT as f64,
    })
}

#[derive(Serialize, Deserialize)]
struct Cached {
    scope: String,
    snapshot: FireworksSnapshot,
}

impl Cached {
    fn parse(bytes: &[u8], scope: &str) -> Result<FireworksSnapshot> {
        let cached: Self = serde_json::from_slice(bytes)?;
        if cached.scope != scope {
            return Err(AppError::Schema(
                "Fireworks cache is for another account or month".into(),
            ));
        }
        finite_amount(
            "Fireworks",
            "monthly_cost_usd",
            cached.snapshot.monthly_cost_usd,
        )?;
        Ok(cached.snapshot)
    }
}

pub(crate) async fn fetch_snapshot(
    client: &reqwest::Client,
    key: &str,
    account: &str,
    cache: &Cache,
    ttl: Duration,
) -> Result<FetchOutcome> {
    fetch_snapshot_at(client, key, account, cache, ttl, BASE_URL, Utc::now()).await
}

async fn fetch_snapshot_at(
    client: &reqwest::Client,
    key: &str,
    account: &str,
    cache: &Cache,
    ttl: Duration,
    base: &str,
    now: DateTime<Utc>,
) -> Result<FetchOutcome> {
    let account = validate_account_id(account)?;
    let range = BillingRange::at(now)?;
    let endpoint = range.endpoint(base, account)?;
    // Account IDs cannot contain colons; delimit the two non-secret fields
    // before the key, which is last and can contain any character.
    let scope = fingerprint_of(&format!("{}:{account}:{key}", range.month_start));
    let parse = |bytes: &[u8]| Cached::parse(bytes, &scope);
    cache.ensure_dir()?;
    let _lock = acquire_lock_async(&cache.lock_path(), LOCK_TIMEOUT).await?;
    if let Some(bytes) = cache.fresh_payload(ttl)?
        && let Ok(snapshot) = parse(&bytes)
    {
        return Ok(Outcome::cached(snapshot, cache, false));
    }
    if let Some(remaining) = cache.backoff_remaining() {
        return Err(AppError::Http {
            status: 429,
            body: format!("rate limited; next attempt in {}", human_backoff(remaining)),
        });
    }

    let request = client
        .get(endpoint)
        .bearer_auth(key)
        .header(reqwest::header::ACCEPT, "application/json");
    let result = tokio::time::timeout(REQUEST_TIMEOUT, async {
        let response = request.send().await.map_err(|_| {
            AppError::Transport("Fireworks billing summary request could not connect".into())
        })?;
        let status = response.status();
        if !status.is_success() {
            return Err(AppError::Http {
                status: status.as_u16(),
                body: "Fireworks billing summary request failed".into(),
            });
        }
        parse_billing(&read_body_capped(response, MAX_BODY_BYTES).await?)
    })
    .await
    .unwrap_or_else(|_| {
        Err(AppError::Transport(
            "Fireworks billing summary timed out".into(),
        ))
    });

    match result {
        Ok(snapshot) => {
            cache.write_payload(&serde_json::to_vec(&Cached { scope, snapshot })?)?;
            Ok(Outcome::fresh(snapshot))
        }
        Err(error) if error.is_transient() => fallback(cache, None, error, parse),
        Err(error) => {
            cache.mark_stale();
            let (status, message) = match &error {
                AppError::Http { status, body } => (*status, body.clone()),
                error => (0, error.to_string()),
            };
            let diagnostic = cache.write_last_error(status, &message);
            fallback(cache, Some(diagnostic), error, parse)
        }
    }
}

pub(crate) fn build_placeholders(snap: &FireworksSnapshot) -> HashMap<&'static str, String> {
    let vendor = VendorId::Fireworks;
    placeholders(vec![
        ("icon", vendor.bar_icon().into()),
        ("vendor_short", vendor.short_name().into()),
        ("plan", vendor.display_name().into()),
        ("fw_monthly_cost", usd(snap.monthly_cost_usd)),
    ])
}

pub(crate) fn render(
    outcome: &FetchOutcome,
    theme: &Theme,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> WaybarOutput {
    let values = build_placeholders(&outcome.snapshot);
    let mut text = substitute(opts.format.as_deref().unwrap_or(DEFAULT_FORMAT), &values);
    if outcome.stale {
        text.push_str(" ⏸");
    }
    let tooltip = if let Some(format) = opts.tooltip_format.as_deref() {
        substitute(format, &values)
    } else {
        let mut lines = vec![
            Line::Center(VendorId::Fireworks.display_name().into()),
            Line::Sep,
            Line::Body(format!(
                "Month-to-date cost (UTC): {}",
                values["fw_monthly_cost"]
            )),
        ];
        if let Some((status, message)) = &outcome.last_error {
            let diagnostic = if *status == 0 {
                escape(message)
            } else {
                format!("HTTP {status}: {}", escape(message))
            };
            lines.push(Line::Body(diagnostic));
        }
        lines.push(Line::Body(format!(
            "Updated {}",
            updated_at_hm(now, outcome.cache_age)
        )));
        render_bordered(&lines, theme)
    };
    let icon = opts
        .icon
        .as_deref()
        .filter(|icon| !icon.is_empty())
        .map_or_else(String::new, |icon| format!("{icon} "));
    WaybarOutput {
        text: color_span(&theme.fg, &format!("{icon}{text}")),
        tooltip,
        class: Class::Low,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BILLING: &[u8] = br#"{
        "lineItems": [
            {"totalCost":{"currencyCode":"USD","units":"12","nanos":50000000}},
            {"totalCost":{"currencyCode":"USD","units":"-1","nanos":-250000000}},
            {"totalCost":{"currencyCode":"USD","nanos":125000000}}
        ],
        "usageBuckets":[{"lineItems":[{"totalCost":{"currencyCode":"USD","units":"50"}}]}]
    }"#;

    fn date(raw: &str) -> DateTime<Utc> {
        raw.parse().unwrap()
    }

    #[test]
    fn sums_only_top_level_usd_costs() {
        let snapshot = parse_billing(BILLING).unwrap();
        assert_eq!(build_placeholders(&snapshot)["fw_monthly_cost"], "$10.93");
        assert_eq!(snapshot.monthly_cost_usd, 10.925);
        assert_eq!(
            parse_billing(br#"{"lineItems":[]}"#)
                .unwrap()
                .monthly_cost_usd,
            0.0
        );
        for invalid in [
            br#"{}"#.as_slice(),
            br#"{"lineItems":[{}]}"#,
            br#"{"lineItems":[{"totalCost":{"currencyCode":"JPY","units":"1"}}]}"#,
            br#"{"lineItems":[{"totalCost":{"currencyCode":"USD","units":"broken"}}]}"#,
            br#"{"lineItems":[{"totalCost":{"currencyCode":"USD","units":"-1","nanos":1}}]}"#,
            br#"{"lineItems":[{"totalCost":{"currencyCode":"USD","nanos":1000000000}}]}"#,
        ] {
            assert!(matches!(parse_billing(invalid), Err(AppError::Schema(_))));
        }
    }

    #[test]
    fn the_date_range_tracks_the_utc_month_and_includes_today() {
        let account = validate_account_id("acct-name_42").unwrap();
        assert!(validate_account_id("").is_err());
        assert!(validate_account_id("account/other").is_err());
        assert!(validate_account_id("acct?query").is_err());
        for (now, start, end) in [
            ("2026-12-31T23:59:59Z", "2026-12-01", "2027-01-01"),
            ("2027-01-01T00:00:00Z", "2027-01-01", "2027-01-02"),
        ] {
            let url = BillingRange::at(date(now))
                .unwrap()
                .endpoint(BASE_URL, account)
                .unwrap();
            assert_eq!(url.scheme(), "https");
            assert_eq!(url.host_str(), Some("api.fireworks.ai"));
            assert_eq!(url.path(), "/v1/accounts/acct-name_42/billing/summary");
            assert_eq!(
                url.query_pairs().into_owned().collect::<Vec<_>>(),
                [
                    ("startTime".into(), format!("{start}T00:00:00Z")),
                    ("endTime".into(), format!("{end}T00:00:00Z"))
                ],
            );
        }
    }

    #[tokio::test]
    async fn billing_auth_failure_keeps_only_the_same_account_and_month() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::at(dir.path().join("fireworks")).redacting("test-key");
        let client = crate::widget::run::http_client().unwrap();
        let mut server = mockito::Server::new_async().await;
        let base = server.url();
        let nov = date("2026-11-22T01:00:00Z");
        let dec = date("2026-12-01T01:00:00Z");
        let account_path = "/v1/accounts/account-a/billing/summary";
        let success = server
            .mock("GET", account_path)
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("startTime".into(), "2026-11-01T00:00:00Z".into()),
                mockito::Matcher::UrlEncoded("endTime".into(), "2026-11-23T00:00:00Z".into()),
            ]))
            .match_header("authorization", "Bearer test-key")
            .with_body(BILLING)
            .create_async()
            .await;
        let fresh = fetch_snapshot_at(
            &client,
            "test-key",
            "account-a",
            &cache,
            Duration::ZERO,
            &base,
            nov,
        )
        .await
        .unwrap();
        assert_eq!(fresh.snapshot.monthly_cost_usd, 10.925);
        assert!(!fresh.stale);
        success.assert_async().await;
        success.remove_async().await;

        let rejected = server
            .mock("GET", account_path)
            .match_query(mockito::Matcher::Any)
            .with_status(401)
            .with_body("provider echoed test-key")
            .expect(3)
            .create_async()
            .await;
        let other_account = server
            .mock("GET", "/v1/accounts/account-b/billing/summary")
            .match_query(mockito::Matcher::Any)
            .with_status(401)
            .create_async()
            .await;
        let reused = fetch_snapshot_at(
            &client,
            "test-key",
            "account-a",
            &cache,
            Duration::ZERO,
            &base,
            nov,
        )
        .await
        .unwrap();
        assert!(reused.stale);
        assert_eq!(reused.snapshot, fresh.snapshot);
        assert_eq!(
            reused.last_error,
            Some((401, crate::error::AUTH_FAILURE_MESSAGE.into()))
        );
        for (key, account, now) in [
            ("new-key", "account-a", nov),
            ("test-key", "account-b", nov),
            ("test-key", "account-a", dec),
        ] {
            let result = fetch_snapshot_at(
                &client,
                key,
                account,
                &cache,
                Duration::from_secs(300),
                &base,
                now,
            )
            .await;
            assert!(matches!(result, Err(AppError::Http { status: 401, .. })));
        }
        rejected.assert_async().await;
        other_account.assert_async().await;
        for path in [cache.payload_path(), cache.last_error_path()] {
            let stored = std::fs::read_to_string(path).unwrap();
            assert!(!stored.contains("test-key") && !stored.contains("account-a"));
        }
    }
}
