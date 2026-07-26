//! Wire types for `GET cursor.com/api/usage-summary`.
//!
//! **Undocumented** — the endpoint the Cursor dashboard's own frontend calls
//! to draw the "Cursor Models" / "Other Models" usage bars. Confirmed against a
//! live Ultra account:
//!
//! ```json
//! {
//!   "billingCycleStart": "2026-07-04T00:35:51.000Z",
//!   "billingCycleEnd":   "2026-08-04T00:35:51.000Z",
//!   "membershipType": "ultra",
//!   "isUnlimited": false,
//!   "individualUsage": {
//!     "plan": { "autoPercentUsed": 98.1, "apiPercentUsed": 100, "totalPercentUsed": 98.5 },
//!     "onDemand": { "enabled": false }
//!   }
//! }
//! ```
//!
//! Team accounts report under `teamUsage` instead of `individualUsage`; that
//! shape isn't modeled yet (a personal plan is the common case), so a payload
//! with no `individualUsage.plan` is treated as schema drift rather than a
//! fabricated zero.

use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::error::{AppError, Result};
use crate::usage::CursorSnapshot;

#[derive(Debug, Clone, Deserialize)]
pub struct UsageSummary {
    #[serde(rename = "membershipType", default)]
    pub membership_type: String,
    #[serde(rename = "isUnlimited", default)]
    pub is_unlimited: bool,
    /// RFC3339 end of the current billing cycle — when the pools reset.
    /// Required: without it there is no reset to show, so its absence is
    /// schema drift, not a "no reset" state.
    #[serde(rename = "billingCycleEnd")]
    pub billing_cycle_end: String,
    #[serde(rename = "individualUsage")]
    pub individual_usage: Option<IndividualUsage>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IndividualUsage {
    pub plan: Option<PlanUsage>,
    #[serde(default)]
    pub on_demand: Option<OnDemand>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PlanUsage {
    /// "Cursor Models" pool (Auto + Composer).
    #[serde(rename = "autoPercentUsed")]
    pub auto_percent_used: f64,
    /// "Other Models" pool (named / third-party).
    #[serde(rename = "apiPercentUsed")]
    pub api_percent_used: f64,
    /// Overall included usage — the dashboard headline percentage.
    #[serde(rename = "totalPercentUsed", default)]
    pub total_percent_used: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OnDemand {
    #[serde(default)]
    pub enabled: bool,
}

/// Round a wire percentage to an integer, matching the dashboard's whole-number
/// display and the integer-percent convention used across every vendor here. A
/// non-finite value (NaN/inf) means the payload wasn't what we think it is —
/// surfaced as schema drift rather than silently rendered.
fn pct(field: &str, v: f64) -> Result<i32> {
    if !v.is_finite() {
        return Err(AppError::Schema(format!(
            "cursor: `{field}` is not a finite number"
        )));
    }
    // Clamp only the low end: a pool can legitimately exceed 100% when it is
    // over its included allowance, and callers clamp for bar width themselves.
    Ok(v.round().max(0.0) as i32)
}

pub fn to_snapshot(resp: UsageSummary) -> Result<CursorSnapshot> {
    let reset_at = DateTime::parse_from_rfc3339(&resp.billing_cycle_end)
        .map_err(|e| {
            AppError::Schema(format!(
                "cursor: `billingCycleEnd` is not RFC3339 ({:?}): {e}",
                resp.billing_cycle_end
            ))
        })?
        .with_timezone(&Utc);

    let plan = title_case(&resp.membership_type);

    // Unlimited plans report no meaningful pool percentages; represent them as
    // zeros with the `unlimited` flag so renderers say "unlimited" rather than
    // painting a bogus bar.
    if resp.is_unlimited {
        return Ok(CursorSnapshot {
            plan,
            auto_pct: 0,
            api_pct: 0,
            total_pct: 0,
            unlimited: true,
            on_demand_enabled: false,
            reset_at: Some(reset_at),
        });
    }

    let plan_usage = resp
        .individual_usage
        .as_ref()
        .and_then(|u| u.plan.as_ref())
        .ok_or_else(|| {
            AppError::Schema(
                "cursor: response has no `individualUsage.plan` (team accounts are not \
                 supported yet)"
                    .into(),
            )
        })?;

    let on_demand_enabled = resp
        .individual_usage
        .as_ref()
        .and_then(|u| u.on_demand.as_ref())
        .map(|o| o.enabled)
        .unwrap_or(false);

    Ok(CursorSnapshot {
        plan,
        auto_pct: pct("autoPercentUsed", plan_usage.auto_percent_used)?,
        api_pct: pct("apiPercentUsed", plan_usage.api_percent_used)?,
        total_pct: pct("totalPercentUsed", plan_usage.total_percent_used)?,
        unlimited: false,
        on_demand_enabled,
        reset_at: Some(reset_at),
    })
}

/// "ultra" -> "Ultra". Cursor's `membershipType` is lowercase; the dashboard
/// shows it title-cased.
fn title_case(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => "Cursor".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    const SAMPLE: &str = r#"{
        "billingCycleStart": "2026-07-04T00:35:51.000Z",
        "billingCycleEnd": "2026-08-04T00:35:51.000Z",
        "membershipType": "ultra",
        "limitType": "user",
        "isUnlimited": false,
        "autoModelSelectedDisplayMessage": "You've used 98% of your included total usage",
        "namedModelSelectedDisplayMessage": "You've used 100% of your included API usage",
        "individualUsage": {
            "plan": {
                "enabled": true, "used": 40000, "limit": 40000, "remaining": 0,
                "autoPercentUsed": 98.109, "apiPercentUsed": 100, "totalPercentUsed": 98.5128
            },
            "onDemand": { "enabled": false, "used": 0, "limit": null, "remaining": null }
        },
        "teamUsage": {}
    }"#;

    #[test]
    fn parses_the_live_ultra_shape() {
        let resp: UsageSummary = serde_json::from_str(SAMPLE).unwrap();
        let snap = to_snapshot(resp).unwrap();
        assert_eq!(snap.plan, "Ultra");
        assert_eq!(snap.auto_pct, 98); // 98.109 rounds to 98 (matches the dashboard bar)
        assert_eq!(snap.api_pct, 100);
        assert_eq!(snap.total_pct, 99); // 98.5128 rounds to 99
        assert!(!snap.unlimited);
        assert!(!snap.on_demand_enabled);
        assert_eq!(
            snap.reset_at,
            Some(Utc.with_ymd_and_hms(2026, 8, 4, 0, 35, 51).unwrap())
        );
        assert_eq!(snap.worst_pct(), 100);
    }

    #[test]
    fn over_allowance_percentage_is_kept_above_100() {
        let raw = r#"{
            "billingCycleEnd": "2026-08-04T00:00:00Z", "membershipType": "pro",
            "individualUsage": { "plan": { "autoPercentUsed": 142.7, "apiPercentUsed": 5, "totalPercentUsed": 80 } }
        }"#;
        let snap = to_snapshot(serde_json::from_str(raw).unwrap()).unwrap();
        assert_eq!(
            snap.auto_pct, 143,
            "an over-quota pool must not clamp to 100"
        );
        assert_eq!(snap.worst_pct(), 143);
    }

    #[test]
    fn unlimited_plan_reports_no_pool_percentages() {
        let raw = r#"{
            "billingCycleEnd": "2026-08-04T00:00:00Z", "membershipType": "enterprise",
            "isUnlimited": true
        }"#;
        let snap = to_snapshot(serde_json::from_str(raw).unwrap()).unwrap();
        assert!(snap.unlimited);
        assert_eq!(snap.worst_pct(), 0);
        assert_eq!(snap.plan, "Enterprise");
    }

    #[test]
    fn missing_individual_plan_is_schema_drift_not_zero() {
        // A team-only or unexpected shape must not read as "0% used".
        let raw = r#"{
            "billingCycleEnd": "2026-08-04T00:00:00Z", "membershipType": "team",
            "teamUsage": {}
        }"#;
        let err = to_snapshot(serde_json::from_str(raw).unwrap()).unwrap_err();
        assert!(matches!(err, AppError::Schema(_)));
    }

    #[test]
    fn missing_billing_cycle_end_is_a_parse_error() {
        let raw = r#"{ "membershipType": "pro",
            "individualUsage": { "plan": { "autoPercentUsed": 1, "apiPercentUsed": 2, "totalPercentUsed": 1 } } }"#;
        // `billingCycleEnd` is required by serde → a missing field fails to parse.
        assert!(serde_json::from_str::<UsageSummary>(raw).is_err());
    }

    #[test]
    fn non_finite_percentage_is_rejected() {
        // serde rejects a non-finite JSON literal at parse time, so exercise the
        // guard directly: a NaN reaching a percentage field must be a schema
        // error, never rendered as a bar.
        let resp = UsageSummary {
            membership_type: "pro".into(),
            is_unlimited: false,
            billing_cycle_end: "2026-08-04T00:00:00Z".into(),
            individual_usage: Some(IndividualUsage {
                plan: Some(PlanUsage {
                    auto_percent_used: f64::NAN,
                    api_percent_used: 2.0,
                    total_percent_used: 1.0,
                }),
                on_demand: None,
            }),
        };
        assert!(matches!(to_snapshot(resp), Err(AppError::Schema(_))));
    }
}
