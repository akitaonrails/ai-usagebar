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
//! Team accounts (and personal token-based enterprise contracts) report no
//! `individualUsage.plan` at all — there's no per-seat request quota for
//! `pct()` to read.
//!
//! **Unverified against a live team account** — we don't have one to test
//! against. The fallback below is inferred from an independent
//! reverse-engineering of this same endpoint
//! (`github.com/WoojinAhn/CursorMeter`'s `UsageModels.swift`, whose doc
//! comments say it was confirmed against live token-based enterprise
//! contracts): on those accounts, `teamUsage` itself carries no percentages
//! (only an `onDemand` flag) and `individualUsage.overall.limit` — the
//! numerator needed to turn `used` cents into a percentage — comes back
//! `null`; Cursor doesn't put the per-seat limit on *this* endpoint for that
//! account type at all. The **only** percentage this payload exposes for such
//! accounts is the human-readable `autoModelSelectedDisplayMessage` /
//! `namedModelSelectedDisplayMessage` strings (e.g. `"You've used 42% of your
//! included total usage"`), and those line up with the auto/named pools
//! respectively — the same two pools `individualUsage.plan.{autoPercentUsed,
//! apiPercentUsed}` reports on personal accounts (confirmed by the `SAMPLE`
//! fixture below, where both messages' percentages match the corresponding
//! `plan` fields exactly). So when `individualUsage.plan` is missing,
//! `to_snapshot` parses both display messages and only accepts them as a
//! team/enterprise snapshot if **both** parse — a single stray message is
//! unrecognized schema drift, not a half-guessed snapshot. A payload with
//! neither `individualUsage.plan` nor two parseable display messages is
//! schema drift, never a fabricated zero.

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
    /// Team-account usage. Present (possibly `{}`) whether or not the caller
    /// is on a team; only its `onDemand` flag is modeled — see the module
    /// doc for why the pool percentages come from the display-message
    /// fallback below instead of from this object.
    #[serde(rename = "teamUsage", default)]
    pub team_usage: Option<TeamUsage>,
    /// e.g. `"You've used 42% of your included total usage"` — the auto
    /// (Cursor Models) pool's percentage in prose form. Redundant with
    /// `individualUsage.plan.autoPercentUsed` on personal accounts; the only
    /// source of that percentage on accounts with no `plan` object.
    #[serde(rename = "autoModelSelectedDisplayMessage", default)]
    pub auto_model_selected_display_message: Option<String>,
    /// Same idea as above for the named/API pool, e.g. `"You've used 100% of
    /// your included API usage"`.
    #[serde(rename = "namedModelSelectedDisplayMessage", default)]
    pub named_model_selected_display_message: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IndividualUsage {
    pub plan: Option<PlanUsage>,
    // ponytail: pre-existing field lacked `rename = "onDemand"`, so this
    // never actually deserialized (always None) — `on_demand_enabled` was
    // silently always false. Fixed in passing since the new `TeamUsage` below
    // needs the same field to actually work.
    #[serde(rename = "onDemand", default)]
    pub on_demand: Option<OnDemand>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TeamUsage {
    #[serde(rename = "onDemand", default)]
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

    // `onDemand` can live under either `individualUsage` (personal accounts)
    // or `teamUsage` (per CursorMeter's `TeamUsage`, which models nothing
    // else there) — check both rather than assuming one.
    let on_demand_enabled = resp
        .individual_usage
        .as_ref()
        .and_then(|u| u.on_demand.as_ref())
        .or_else(|| resp.team_usage.as_ref().and_then(|t| t.on_demand.as_ref()))
        .map(|o| o.enabled)
        .unwrap_or(false);

    if let Some(plan_usage) = resp.individual_usage.as_ref().and_then(|u| u.plan.as_ref()) {
        return Ok(CursorSnapshot {
            plan,
            auto_pct: pct("autoPercentUsed", plan_usage.auto_percent_used)?,
            api_pct: pct("apiPercentUsed", plan_usage.api_percent_used)?,
            total_pct: pct("totalPercentUsed", plan_usage.total_percent_used)?,
            unlimited: false,
            on_demand_enabled,
            reset_at: Some(reset_at),
        });
    }

    // No numeric `plan` object — the team/enterprise fallback described in
    // the module doc. Both display messages must parse or this isn't the
    // shape we think it is; see the module doc for why "both or neither".
    let team_pcts = resp
        .auto_model_selected_display_message
        .as_deref()
        .and_then(parse_percent_from_message)
        .zip(
            resp.named_model_selected_display_message
                .as_deref()
                .and_then(parse_percent_from_message),
        );
    if let Some((auto_raw, api_raw)) = team_pcts {
        let auto_pct = pct("autoModelSelectedDisplayMessage", auto_raw)?;
        let api_pct = pct("namedModelSelectedDisplayMessage", api_raw)?;
        return Ok(CursorSnapshot {
            // Flag this as the best-effort team path — distinct from the
            // membership label alone, since the number came from prose, not
            // the numeric `plan` object.
            plan: format!("{plan} (team)"),
            auto_pct,
            api_pct,
            // No distinct blended-total signal exists on this path (no
            // `totalPercentUsed` equivalent message); the worse of the two
            // pools is the best-effort stand-in.
            total_pct: auto_pct.max(api_pct),
            unlimited: false,
            on_demand_enabled,
            reset_at: Some(reset_at),
        });
    }

    Err(AppError::Schema(
        "cursor: response has no `individualUsage.plan` and no parseable team-usage \
         display message (unrecognized team-account shape)"
            .into(),
    ))
}

/// Pulls the leading `N` or `N.N` out of a `"…N%…"` prose string, e.g.
/// `"You've used 98% of your included total usage"` -> `Some(98.0)`. Returns
/// `None` if there's no `%` or nothing number-shaped precedes it, so callers
/// treat a reworded message as unparseable rather than misreading it.
fn parse_percent_from_message(msg: &str) -> Option<f64> {
    let pct_idx = msg.find('%')?;
    let before = &msg[..pct_idx];
    let start = before
        .rfind(|c: char| !c.is_ascii_digit() && c != '.')
        .map(|i| i + 1)
        .unwrap_or(0);
    before[start..].parse::<f64>().ok()
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
            team_usage: None,
            auto_model_selected_display_message: None,
            named_model_selected_display_message: None,
        };
        assert!(matches!(to_snapshot(resp), Err(AppError::Schema(_))));
    }

    /// The fixture backing the team-account fallback. **Unverified against a
    /// live team account** — see the module doc for the reasoning: no
    /// `individualUsage.plan`, `teamUsage` carries only `onDemand`, and the
    /// two pool percentages come from the display-message strings instead.
    const TEAM_SAMPLE: &str = r#"{
        "billingCycleEnd": "2026-08-04T00:35:51.000Z",
        "membershipType": "team",
        "isUnlimited": false,
        "autoModelSelectedDisplayMessage": "You've used 42% of your included total usage",
        "namedModelSelectedDisplayMessage": "You've used 15% of your included API usage",
        "teamUsage": { "onDemand": { "enabled": true } }
    }"#;

    #[test]
    fn team_account_falls_back_to_display_message_percentages() {
        let resp: UsageSummary = serde_json::from_str(TEAM_SAMPLE).unwrap();
        let snap = to_snapshot(resp).unwrap();
        assert_eq!(snap.plan, "Team (team)");
        assert_eq!(snap.auto_pct, 42);
        assert_eq!(snap.api_pct, 15);
        assert_eq!(
            snap.total_pct, 42,
            "no blended-total signal exists; worst pool stands in"
        );
        assert!(!snap.unlimited);
        assert!(
            snap.on_demand_enabled,
            "onDemand lives under teamUsage for a team account, not individualUsage"
        );
        assert_eq!(snap.worst_pct(), 42);
    }

    #[test]
    fn team_account_with_only_one_parseable_message_is_still_schema_drift() {
        // Guards the "both or neither" rule: a half-recognized shape must not
        // silently render one real pool and one fabricated zero.
        let raw = r#"{
            "billingCycleEnd": "2026-08-04T00:00:00Z", "membershipType": "team",
            "autoModelSelectedDisplayMessage": "You've used 42% of your included total usage",
            "namedModelSelectedDisplayMessage": "unavailable"
        }"#;
        let err = to_snapshot(serde_json::from_str(raw).unwrap()).unwrap_err();
        assert!(matches!(err, AppError::Schema(_)));
    }

    #[test]
    fn parse_percent_from_message_reads_leading_number_before_percent_sign() {
        assert_eq!(
            parse_percent_from_message("You've used 98% of your included total usage"),
            Some(98.0)
        );
        assert_eq!(
            parse_percent_from_message("You've used 100% of your included API usage"),
            Some(100.0)
        );
        assert_eq!(parse_percent_from_message("no percent here"), None);
        assert_eq!(parse_percent_from_message("unavailable"), None);
    }
}
