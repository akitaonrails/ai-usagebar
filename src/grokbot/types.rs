//! Wire types for `aiserver.v1.DashboardService/GetSandUsageStatus` — the
//! Grok Bot desktop app's Connect-RPC usage call, captured live by the
//! reporter of #206.

use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::error::{AppError, Result};
use crate::usage::GrokbotSnapshot;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct SandUsageStatus {
    current_period_start: Option<String>,
    next_reset_timestamp_utc: Option<String>,
    /// Integer percent, or the same as a numeric string.
    usage_percent: Option<PercentOrString>,
    has_available_usage: bool,
    has_non_zero_included_limit: bool,
    on_demand_settings: OnDemandSettings,
    grok_plan_label: Option<String>,
    cursor_plan_name: Option<String>,
}

/// The on-demand (pay-as-you-go) block. `visible`/`eligible`/`enabled` are
/// the only fields read; the response also carries a **`dashboardUrl`, which
/// is account-identifying and is deliberately not deserialized** — serde
/// drops unknown fields, so the URL never enters a snapshot, the cache, or a
/// Debug line. Keep it that way.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct OnDemandSettings {
    visible: bool,
    eligible: bool,
    /// Live captures have sent `null` here; treat that as off.
    enabled: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum PercentOrString {
    Int(i64),
    /// Live captures have sent a fractional JSON number (`19.150778`).
    Float(f64),
    Text(String),
}

impl SandUsageStatus {
    pub fn into_snapshot(self) -> Result<GrokbotSnapshot> {
        // The app's own label first ("Grok Bot Plan"); the underlying Cursor
        // plan name is the fallback, not a peer.
        let plan = [self.grok_plan_label, self.cursor_plan_name]
            .into_iter()
            .flatten()
            .map(|label| label.trim().to_string())
            .find(|label| !label.is_empty())
            .unwrap_or_else(|| "Grok Bot".to_string());

        let period_start = parse_timestamp("currentPeriodStart", self.current_period_start)?;
        let reset_at = parse_timestamp("nextResetTimestampUtc", self.next_reset_timestamp_utc)?;
        // The window's length is the two reported instants apart — honest,
        // not an assumed 7 days. A reset at or before the period start is not
        // a window at all.
        let window = match (period_start, reset_at) {
            (Some(start), Some(reset)) if reset > start => Some(reset - start),
            _ => None,
        };

        // `usagePercent` means something only against a non-zero included
        // limit; an account without one is a distinct "no included allowance"
        // state, not 0%.
        let weekly_pct = if self.has_non_zero_included_limit {
            let percent = self
                .usage_percent
                .ok_or_else(|| AppError::Schema("grokbot: missing usagePercent".into()))?;
            parse_percent(percent)?
        } else {
            0
        };

        Ok(GrokbotSnapshot {
            plan,
            has_included_allowance: self.has_non_zero_included_limit,
            weekly_pct,
            has_available_usage: self.has_available_usage,
            on_demand_enabled: self.on_demand_settings.enabled.unwrap_or(false),
            period_start,
            reset_at,
            window,
        })
    }
}

/// `usagePercent` is an integer percent in 0..=100. A numeric string goes
/// through f64 so `"42"` and `"42.0"` both parse; anything non-finite
/// (`"NaN"`) or out of range is schema drift, not a quota.
fn parse_percent(value: PercentOrString) -> Result<i32> {
    let raw = match value {
        PercentOrString::Int(n) => n as f64,
        PercentOrString::Float(n) => n,
        PercentOrString::Text(s) => s.trim().parse::<f64>().map_err(|_| {
            AppError::Schema(format!("grokbot: usagePercent is not numeric (got {s:?})"))
        })?,
    };
    if !raw.is_finite() || !(0.0..=100.0).contains(&raw) {
        return Err(AppError::Schema(format!(
            "grokbot: usagePercent out of range: {raw}"
        )));
    }
    Ok(raw.round() as i32)
}

fn parse_timestamp(field: &str, value: Option<String>) -> Result<Option<DateTime<Utc>>> {
    match value {
        None => Ok(None),
        Some(s) if s.trim().is_empty() => Ok(None),
        Some(s) => DateTime::parse_from_rfc3339(&s)
            .map(|dt| Some(dt.with_timezone(&Utc)))
            .map_err(|e| AppError::Schema(format!("grokbot: unparseable {field}: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reporter's verbatim typed capture (#206).
    const CAPTURE: &str = r#"{"currentPeriodStart":"2026-09-11T18:43:19.645Z","nextResetTimestampUtc":"2026-09-18T18:43:19.645Z","usagePercent":0,"hasAvailableUsage":true,"hasNonZeroIncludedLimit":true,"onDemandSettings":{"visible":true,"eligible":true,"enabled":false,"dashboardUrl":"https://cursor.com/dashboard?team=acct-123-secret"},"grokPlanLabel":"Grok Bot Plan","cursorPlanName":"Pro"}"#;

    #[test]
    fn the_verbatim_capture_parses() {
        let snap = serde_json::from_str::<SandUsageStatus>(CAPTURE)
            .unwrap()
            .into_snapshot()
            .unwrap();
        assert_eq!(snap.plan, "Grok Bot Plan");
        assert!(snap.has_included_allowance);
        assert_eq!(snap.weekly_pct, 0);
        assert!(snap.has_available_usage);
        assert!(!snap.on_demand_enabled);
        assert_eq!(
            snap.period_start.map(|dt| dt.to_rfc3339()),
            Some("2026-09-11T18:43:19.645+00:00".to_string())
        );
        assert_eq!(
            snap.reset_at.map(|dt| dt.to_rfc3339()),
            Some("2026-09-18T18:43:19.645+00:00".to_string())
        );
        // The captured window is exactly seven days — computed, not assumed.
        assert_eq!(snap.window, Some(chrono::Duration::days(7)));
    }

    #[test]
    fn the_dashboard_url_cannot_be_held_or_rendered() {
        let status = serde_json::from_str::<SandUsageStatus>(CAPTURE).unwrap();
        // The struct has no field to hold the account-identifying URL, so it
        // can leak into neither a snapshot nor a Debug line.
        let rendered = format!("{status:?}");
        assert!(!rendered.contains("dashboardUrl"), "{rendered}");
        assert!(!rendered.contains("acct-123-secret"), "{rendered}");
        let snap = status.into_snapshot().unwrap();
        assert!(!format!("{snap:?}").contains("acct-123-secret"));
    }

    #[test]
    fn a_live_macos_shape_parses_fractional_percent_and_null_on_demand() {
        // Redacted 2026-09-19 capture from GetSandUsageStatus against a macOS
        // Grok Bot 0.57.1 session: fractional usagePercent, enabled: null.
        let json = r#"{"currentPeriodStart":"2026-09-16T15:56:23.315Z","nextResetTimestampUtc":"2026-09-23T15:56:23.315Z","usagePercent":19.150778,"hasAvailableUsage":true,"hasNonZeroIncludedLimit":true,"onDemandSettings":{"visible":true,"eligible":true,"enabled":null},"grokPlanLabel":"Grok Bot Plan","cursorPlanName":"Ultra"}"#;
        let snap = serde_json::from_str::<SandUsageStatus>(json)
            .unwrap()
            .into_snapshot()
            .unwrap();
        assert_eq!(snap.plan, "Grok Bot Plan");
        assert_eq!(snap.weekly_pct, 19);
        assert!(!snap.on_demand_enabled);
        assert_eq!(snap.window, Some(chrono::Duration::days(7)));
    }

    #[test]
    fn usage_percent_accepts_an_int_or_a_numeric_string() {
        for (raw, expected) in [
            (r#""usagePercent": 42"#, 42),
            (r#""usagePercent": "42""#, 42),
            (r#""usagePercent": 19.150778"#, 19),
        ] {
            let json = format!(r#"{{"hasNonZeroIncludedLimit": true, {raw}}}"#);
            let snap = serde_json::from_str::<SandUsageStatus>(&json)
                .unwrap()
                .into_snapshot()
                .unwrap();
            assert_eq!(snap.weekly_pct, expected, "{raw}");
        }
    }

    #[test]
    fn out_of_range_and_non_numeric_percents_are_schema_drift() {
        for raw in [
            r#""usagePercent": 150"#,
            r#""usagePercent": -1"#,
            r#""usagePercent": "NaN""#,
            r#""usagePercent": "garbage""#,
            r#""usagePercent": "101""#,
        ] {
            let json = format!(r#"{{"hasNonZeroIncludedLimit": true, {raw}}}"#);
            let err = serde_json::from_str::<SandUsageStatus>(&json)
                .unwrap()
                .into_snapshot()
                .unwrap_err();
            assert!(matches!(err, AppError::Schema(_)), "{raw}: {err:?}");
        }
    }

    #[test]
    fn a_missing_reset_is_none_not_an_error() {
        let json = r#"{"hasNonZeroIncludedLimit": true, "usagePercent": 10}"#;
        let snap = serde_json::from_str::<SandUsageStatus>(json)
            .unwrap()
            .into_snapshot()
            .unwrap();
        assert_eq!(snap.reset_at, None);
        assert_eq!(snap.period_start, None);
        // No window can be derived from one missing endpoint.
        assert_eq!(snap.window, None);
    }

    #[test]
    fn an_unparseable_reset_is_schema_drift() {
        let json = r#"{"hasNonZeroIncludedLimit": true, "usagePercent": 10,
            "nextResetTimestampUtc": "next tuesday"}"#;
        let err = serde_json::from_str::<SandUsageStatus>(json)
            .unwrap()
            .into_snapshot()
            .unwrap_err();
        assert!(err.to_string().contains("nextResetTimestampUtc"), "{err}");
    }

    /// `hasNonZeroIncludedLimit: false` is a distinct state — the account has
    /// no included allowance at all — never a 0% meter.
    #[test]
    fn no_included_limit_is_the_no_allowance_state_not_zero_percent() {
        let json = r#"{"hasNonZeroIncludedLimit": false, "hasAvailableUsage": false,
            "usagePercent": 0, "grokPlanLabel": "Grok Bot Plan"}"#;
        let snap = serde_json::from_str::<SandUsageStatus>(json)
            .unwrap()
            .into_snapshot()
            .unwrap();
        assert!(!snap.has_included_allowance);
        // The percent is not meaningful here, and the renderers must key off
        // the flag, not the number.
        assert_eq!(snap.weekly_pct, 0);

        // Without the flag, a missing usagePercent is schema drift.
        let json = r#"{"hasNonZeroIncludedLimit": true}"#;
        let err = serde_json::from_str::<SandUsageStatus>(json)
            .unwrap()
            .into_snapshot()
            .unwrap_err();
        assert!(err.to_string().contains("usagePercent"), "{err}");
    }

    #[test]
    fn the_on_demand_note_needs_an_exhausted_pool_and_on_demand_enabled() {
        let base = |pct: i64, available: bool, enabled: bool| {
            let json = format!(
                r#"{{"hasNonZeroIncludedLimit": true, "usagePercent": {pct},
                    "hasAvailableUsage": {available},
                    "onDemandSettings": {{"enabled": {enabled}}}}}"#
            );
            serde_json::from_str::<SandUsageStatus>(&json)
                .unwrap()
                .into_snapshot()
                .unwrap()
        };
        // The footnote fires at exactly the documented combination.
        assert!(base(100, true, true).on_demand_note().is_some());
        // 100% with on-demand off really is exhausted.
        assert!(base(100, true, false).on_demand_note().is_none());
        assert!(base(100, false, true).on_demand_note().is_none());
        assert!(base(99, true, true).on_demand_note().is_none());
    }

    #[test]
    fn the_plan_label_prefers_grok_and_falls_back_to_cursor_plan() {
        let json =
            r#"{"hasNonZeroIncludedLimit": true, "usagePercent": 5, "cursorPlanName": "Pro"}"#;
        let snap = serde_json::from_str::<SandUsageStatus>(json)
            .unwrap()
            .into_snapshot()
            .unwrap();
        assert_eq!(snap.plan, "Pro");

        let json = r#"{"hasNonZeroIncludedLimit": true, "usagePercent": 5}"#;
        let snap = serde_json::from_str::<SandUsageStatus>(json)
            .unwrap()
            .into_snapshot()
            .unwrap();
        assert_eq!(snap.plan, "Grok Bot");
    }
}
