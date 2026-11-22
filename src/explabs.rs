//! Credits and plan usage from Experiential Labs' organization balance API.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::cache::{Cache, acquire_lock_async, human_backoff};
use crate::countdown;
use crate::error::{AppError, Result};
use crate::format::{clamp_pct, placeholders, substitute, updated_at_hm, usd};
use crate::outcome::{Outcome, fallback};
use crate::pango::{balance_severity, color_span, escape, severity_color, severity_for};
use crate::theme::Theme;
use crate::tooltip::{Line, render_bordered};
use crate::usage::{UsageWindow, VendorSnapshot, finite_amount};
use crate::vendor::{MAX_BODY_BYTES, RenderOpts, VendorId, VendorOutcome, read_body_capped};
use crate::waybar::{Class, WaybarOutput};

const CREDITS_URL: &str = "https://api.experientiallabs.ai/api/v1/credits";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const LOCK_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum Tier {
    Free,
    Pro,
    Max,
    Ultra,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Snapshot {
    pub(crate) balance_usd: f64,
    tier: Option<Tier>,
    pub(crate) daily: Option<Daily>,
}
impl Eq for Snapshot {}

impl Snapshot {
    pub(crate) fn tier_name(&self) -> Option<&'static str> {
        self.tier.map(|tier| match tier {
            Tier::Free => "Free",
            Tier::Pro => "Pro",
            Tier::Max => "Max",
            Tier::Ultra => "Ultra",
        })
    }

    fn current(mut self, now: DateTime<Utc>) -> Self {
        if self.daily.is_some_and(|daily| daily.reset <= now) {
            self.daily = None;
        }
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub(crate) struct Daily {
    pub(crate) percent: u8,
    reset: DateTime<Utc>,
    free_core: bool,
}

impl Daily {
    pub(crate) fn label(self) -> &'static str {
        if self.free_core {
            "Free core tokens (daily)"
        } else {
            "Included credits (daily)"
        }
    }

    pub(crate) fn window(self) -> UsageWindow {
        UsageWindow {
            utilization_pct: i32::from(self.percent),
            resets_at: Some(self.reset),
            window_duration: chrono::Duration::days(1),
        }
    }
}

#[derive(Deserialize)]
struct Credits {
    total_credits: f64,
    total_usage: f64,
    /// The wallet must still work when plan usage is temporarily unavailable.
    #[serde(default)]
    plan: serde_json::Value,
}

#[derive(Deserialize)]
struct CreditsResponse {
    data: Credits,
}

#[derive(Deserialize)]
struct FreeWindow {
    window: String,
    used_tokens: u64,
    limit_tokens: u64,
    resets_at: DateTime<Utc>,
}

#[derive(Deserialize)]
struct PaidWindow {
    used_credits: f64,
    limit_credits: f64,
    resets_at: DateTime<Utc>,
}

fn parse_credits(bytes: &[u8]) -> Result<Snapshot> {
    let CreditsResponse { data } = serde_json::from_slice(bytes).map_err(|_| {
        AppError::Schema("Experiential Labs response did not contain a USD wallet".into())
    })?;
    let balance_usd = finite_amount(
        "Experiential Labs",
        "wallet balance",
        data.total_credits - data.total_usage,
    )?;
    let tier = data.plan.get("tier").and_then(|tier| match tier.as_str()? {
        "free" => Some(Tier::Free),
        "pro" => Some(Tier::Pro),
        "max" => Some(Tier::Max),
        "ultra" => Some(Tier::Ultra),
        _ => None,
    });
    let daily = tier.and_then(|tier| match tier {
        Tier::Free => {
            let window: FreeWindow =
                FreeWindow::deserialize(data.plan.pointer("/free_core/today")?).ok()?;
            if window.window != "day" {
                return None;
            }
            daily_window(
                window.used_tokens as f64,
                window.limit_tokens as f64,
                window.resets_at,
                true,
            )
        }
        Tier::Pro | Tier::Max | Tier::Ultra => {
            let window: PaidWindow =
                PaidWindow::deserialize(data.plan.pointer("/allowance/today")?).ok()?;
            daily_window(
                window.used_credits,
                window.limit_credits,
                window.resets_at,
                false,
            )
        }
    });
    Ok(Snapshot {
        balance_usd,
        tier,
        daily,
    })
}

fn daily_window(used: f64, limit: f64, reset: DateTime<Utc>, free_core: bool) -> Option<Daily> {
    if !used.is_finite() || !limit.is_finite() || used < 0.0 || limit <= 0.0 {
        return None;
    }
    Some(Daily {
        percent: clamp_pct((used.min(limit) / limit) * 100.0) as u8,
        reset,
        free_core,
    })
}

fn parse_cache(bytes: &[u8]) -> Result<Snapshot> {
    let snapshot: Snapshot = serde_json::from_slice(bytes)
        .map_err(|_| AppError::Schema("Experiential Labs cache is invalid".into()))?;
    finite_amount("Experiential Labs", "wallet balance", snapshot.balance_usd)?;
    if snapshot.daily.is_some_and(|daily| {
        daily.percent > 100
            || snapshot.tier.is_none()
            || daily.free_core != (snapshot.tier == Some(Tier::Free))
    }) {
        return Err(AppError::Schema(
            "Experiential Labs cached window is invalid".into(),
        ));
    }
    Ok(snapshot)
}

type FetchOutcome = Outcome<Snapshot>;

impl From<FetchOutcome> for VendorOutcome {
    fn from(outcome: FetchOutcome) -> Self {
        outcome.map(VendorSnapshot::ExperientialLabs)
    }
}

pub(crate) async fn fetch_snapshot(
    client: &reqwest::Client,
    key: &str,
    cache: &Cache,
    ttl: Duration,
) -> Result<FetchOutcome> {
    fetch_snapshot_at(client, key, cache, ttl, CREDITS_URL, Utc::now()).await
}

async fn fetch_snapshot_at(
    client: &reqwest::Client,
    key: &str,
    cache: &Cache,
    ttl: Duration,
    endpoint: &str,
    now: DateTime<Utc>,
) -> Result<FetchOutcome> {
    cache.ensure_dir()?;
    let _lock = acquire_lock_async(&cache.lock_path(), LOCK_TIMEOUT).await?;
    if let Some(bytes) = cache.fresh_payload(ttl)?
        && let Ok(snapshot) = parse_cache(&bytes)
    {
        return Ok(Outcome::cached(snapshot.current(now), cache, false));
    }
    if let Some(remaining) = cache.backoff_remaining() {
        return Err(AppError::Http {
            status: 429,
            body: format!("rate limited; next attempt in {}", human_backoff(remaining)),
        });
    }

    let request = async {
        let response = client
            .get(endpoint)
            .bearer_auth(key)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            return Err(AppError::Http {
                status: status.as_u16(),
                body: "Experiential Labs credits request failed".into(),
            });
        }
        parse_credits(&read_body_capped(response, MAX_BODY_BYTES).await?)
    };
    let result = tokio::time::timeout(REQUEST_TIMEOUT, request)
        .await
        .unwrap_or_else(|_| {
            Err(AppError::Transport(
                "Experiential Labs credits request timed out".into(),
            ))
        });

    let cached = |bytes: &[u8]| parse_cache(bytes).map(|snapshot| snapshot.current(now));
    match result {
        Ok(snapshot) => {
            // The normalized payload omits response strings and unrelated fields.
            cache.write_payload(&serde_json::to_vec(&snapshot)?)?;
            Ok(Outcome::fresh(snapshot.current(now)))
        }
        Err(error) if error.is_transient() => fallback(cache, None, error, cached),
        Err(error) => {
            cache.mark_stale();
            let (status, message) = match &error {
                AppError::Http { status, body } => (*status, body.clone()),
                other => (0, other.to_string()),
            };
            let diagnostic = cache.write_last_error(status, &message);
            fallback(cache, Some(diagnostic), error, cached)
        }
    }
}

fn build_placeholders(snap: &Snapshot, now: DateTime<Utc>) -> HashMap<&'static str, String> {
    let vendor = VendorId::ExperientialLabs;
    let (percent, reset) = snap.daily.map_or_else(
        || (String::new(), String::new()),
        |daily| {
            (
                daily.percent.to_string(),
                countdown::format(Some(daily.reset), now),
            )
        },
    );
    placeholders(vec![
        ("icon", vendor.bar_icon().into()),
        ("vendor_short", vendor.short_name().into()),
        (
            "plan",
            snap.tier_name().unwrap_or(vendor.display_name()).into(),
        ),
        ("explabs_balance", usd(snap.balance_usd)),
        ("explabs_pct", percent),
        ("explabs_reset", reset),
    ])
}

pub(crate) fn render(
    outcome: &FetchOutcome,
    theme: &Theme,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> WaybarOutput {
    let snap = &outcome.snapshot;
    let values = build_placeholders(snap, now);
    let default_format = if snap.daily.is_some() {
        "{explabs_pct}%"
    } else {
        "{explabs_balance}"
    };
    let mut text = substitute(opts.format.as_deref().unwrap_or(default_format), &values);
    if outcome.stale {
        text.push_str(" ⏸");
    }
    let severity = snap.daily.map_or_else(
        || balance_severity(snap.balance_usd, "USD"),
        |daily| severity_for(i32::from(daily.percent)),
    );
    let color = severity_color(severity, theme);
    let icon = opts
        .icon
        .as_deref()
        .filter(|icon| !icon.is_empty())
        .map_or_else(String::new, |icon| format!("{icon} "));
    let tooltip = if let Some(format) = opts.tooltip_format.as_deref() {
        substitute(format, &values)
    } else {
        let mut lines = vec![
            Line::Center(snap.tier_name().map_or_else(
                || VendorId::ExperientialLabs.display_name().to_owned(),
                |tier| format!("{} {tier}", VendorId::ExperientialLabs.display_name()),
            )),
            Line::Sep,
            Line::Body(format!(
                "Wallet balance: {}",
                escape(&usd(snap.balance_usd))
            )),
        ];
        if let Some(daily) = snap.daily {
            lines.push(Line::Body(format!(
                "{}: {}% · Resets in {}",
                daily.label(),
                daily.percent,
                countdown::format(Some(daily.reset), now)
            )));
        }
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
    WaybarOutput {
        text: color_span(color, &format!("{icon}{text}")),
        tooltip,
        class: Class::from(severity),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRO: &[u8] = br#"{"data":{
        "total_credits":45,"total_usage":12.5,
        "plan":{"tier":"pro","allowance":{
            "today":{"used_credits":150,"limit_credits":700,"remaining_credits":550,
                     "resets_at":"2026-11-22T00:00:00Z"},
            "billing_cycle":{"used_credits":1200,"limit_credits":2000}
        },"included_models":[],"free_core":null,"retiring_benefits":[]},
        "noise":"test-key"
    }}"#;
    const FREE: &[u8] = br#"{"data":{
        "total_credits":45,"total_usage":47,
        "plan":{"tier":"free","free_core":{"today":{
            "window":"day","used_tokens":74,"limit_tokens":100,"remaining_tokens":26,
            "resets_at":"2026-11-22T00:00:00Z"
        }}}
    }}"#;

    fn now() -> DateTime<Utc> {
        "2026-11-21T12:00:00Z".parse().unwrap()
    }

    #[test]
    fn one_shared_free_window_or_the_paid_daily_allowance() {
        let free = parse_credits(FREE).unwrap();
        assert_eq!(free.balance_usd, -2.0);
        assert_eq!(free.tier_name(), Some("Free"));
        let daily = free.daily.unwrap();
        assert_eq!(daily.label(), "Free core tokens (daily)");
        assert_eq!(daily.percent, 74);
        assert_eq!(
            daily.window().resets_at,
            "2026-11-22T00:00:00Z".parse().ok()
        );
        assert_eq!(
            build_placeholders(&free, now())["explabs_balance"],
            "-$2.00"
        );

        let pro = parse_credits(PRO).unwrap();
        assert_eq!(pro.balance_usd, 32.5);
        assert_eq!(pro.tier_name(), Some("Pro"));
        assert_eq!(pro.daily.unwrap().percent, 21);
        assert_eq!(pro.daily.unwrap().label(), "Included credits (daily)");
    }

    #[test]
    fn report_projection_has_one_window_and_the_real_reset() {
        use crate::tui::app::{ReadyTab, TabState};
        use crate::tui::panels::{Section, sections_with_metadata_for};

        let snapshot = parse_credits(PRO).unwrap();
        let tab = TabState::Ready(Box::new(ReadyTab {
            snapshot: VendorSnapshot::ExperientialLabs(snapshot),
            stale: false,
            last_error: None,
            fetched_at: None,
            display: Default::default(),
        }));
        let rows = sections_with_metadata_for(&tab, now(), 5);
        let windows: Vec<_> = rows
            .iter()
            .filter_map(|row| match &row.section {
                Section::Metric { label, pct, .. } => {
                    Some((label.as_str(), *pct, row.reset_at, row.window))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            windows,
            [(
                "Included credits (daily)",
                21,
                "2026-11-22T00:00:00Z".parse().ok(),
                Some(chrono::Duration::days(1)),
            )],
        );
        assert!(rows.iter().any(|row| matches!(
            &row.section, Section::Text { label, value }
                if label == "Wallet balance" && value == "$32.50"
        )));
    }

    #[test]
    fn incomplete_wallet_is_rejected_but_invalid_optional_plan_still_shows_balance() {
        assert!(parse_credits(br#"{"data":{"total_credits":5}}"#).is_err());
        assert!(
            parse_credits(br#"{"data":{"total_credits":1e308,"total_usage":-1e308}}"#).is_err()
        );
        for plan in [
            "null",
            r#"{"tier":"other","allowance":{"today":{"used_credits":1,"limit_credits":5,"resets_at":"2026-11-22T00:00:00Z"}}}"#,
            r#"{"tier":"max","allowance":{"today":{"used_credits":1,"limit_credits":0,"resets_at":"2026-11-22T00:00:00Z"}}}"#,
            r#"{"tier":"ultra","allowance":{"today":{"used_credits":1,"limit_credits":5,"resets_at":"not-a-date"}}}"#,
            r#"{"tier":"free","free_core":{"today":{"window":"week","used_tokens":2,"limit_tokens":3,"resets_at":"2026-11-22T00:00:00Z"}}}"#,
        ] {
            let doc = format!(r#"{{"data":{{"total_credits":5,"total_usage":3,"plan":{plan}}}}}"#);
            let snapshot = parse_credits(doc.as_bytes()).unwrap();
            assert_eq!(snapshot.balance_usd, 2.0);
            assert!(snapshot.daily.is_none());
            assert!(build_placeholders(&snapshot, now())["explabs_pct"].is_empty());
        }
        let over = daily_window(f64::MAX, f64::MIN_POSITIVE, now(), false).unwrap();
        assert_eq!(over.percent, 100);
    }

    #[tokio::test]
    async fn credits_are_the_only_request_and_auth_failure_preserves_wallet_without_an_ended_window()
     {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::at(dir.path().join("explabs"));
        let mut server = mockito::Server::new_async().await;
        let endpoint = format!("{}/api/v1/credits", server.url());
        let client = crate::widget::run::http_client().unwrap();
        let ok = server
            .mock("GET", "/api/v1/credits")
            .match_header("authorization", "Bearer test-key")
            .with_body(PRO)
            .create_async()
            .await;
        let fresh = fetch_snapshot_at(
            &client,
            "test-key",
            &cache,
            Duration::ZERO,
            &endpoint,
            now(),
        )
        .await
        .unwrap();
        assert!(!fresh.stale);
        assert_eq!(fresh.snapshot.daily.unwrap().percent, 21);
        ok.assert_async().await;
        ok.remove_async().await;
        let bytes = cache.maybe_payload().unwrap().unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("test-key"));

        let unauthorized = server
            .mock("GET", "/api/v1/credits")
            .with_status(401)
            .with_body("test-key")
            .create_async()
            .await;
        let reset = fresh.snapshot.daily.unwrap().reset;
        let stale = fetch_snapshot_at(
            &client,
            "test-key",
            &cache,
            Duration::ZERO,
            &endpoint,
            reset,
        )
        .await
        .unwrap();
        assert!(stale.stale);
        assert_eq!(stale.snapshot.balance_usd, fresh.snapshot.balance_usd);
        assert!(stale.snapshot.daily.is_none());
        assert_eq!(
            stale.last_error.unwrap(),
            (401, crate::error::AUTH_FAILURE_MESSAGE.into())
        );
        unauthorized.assert_async().await;
    }
}
