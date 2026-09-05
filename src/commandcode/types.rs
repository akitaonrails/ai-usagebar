//! Command Code usage response types and schema validation.
//!
//! Two documents make up a Command Code snapshot. `/alpha/billing/credits`
//! carries the credit ledger and the rolling spend windows; the subscription
//! names the plan. Both are parsed defensively — Command Code publishes no
//! schema for either, so an additive field must never break a running widget,
//! and a missing section degrades that section alone.

use chrono::{DateTime, Utc};
use serde_json::Value;

/// One rolling spend window: dollars drawn against a dollar cap.
///
/// Unlike most vendors' percentage windows, Command Code meters spend, so the
/// absolute figures are kept and the percentage is derived. That keeps
/// "$5.24 of $35" available to the tooltip without a second round trip.
#[derive(Debug, Clone, PartialEq)]
pub struct SpendWindow {
    pub used: f64,
    pub cap: f64,
    pub resets_at: Option<DateTime<Utc>>,
}

impl Eq for SpendWindow {}

impl SpendWindow {
    /// Percentage of the cap consumed, rounded and clamped to `0..=100`.
    pub fn pct(&self) -> i32 {
        // Guards a zero cap and a non-finite one alike: either way there is
        // no denominator to divide by.
        if !self.cap.is_finite() || self.cap <= 0.0 {
            return 0;
        }
        ((self.used / self.cap) * 100.0).round().clamp(0.0, 100.0) as i32
    }
}

/// The monthly credit ledger. Command Code reports three separate pools and
/// expects a client to sum them, which is what the official CLI does.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Credits {
    pub monthly: f64,
    pub purchased: f64,
    pub free: f64,
}

impl Eq for Credits {}

impl Credits {
    /// Total credit still spendable across every pool.
    pub fn remaining(&self) -> f64 {
        self.monthly + self.purchased + self.free
    }
}

/// Everything the widget shows for Command Code.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Snapshot {
    /// Plan label, e.g. "GOAT". `None` until the subscription is read.
    pub plan: Option<String>,
    pub five_hour: Option<SpendWindow>,
    pub weekly: Option<SpendWindow>,
    pub credits: Option<Credits>,
    /// Monthly credit allowance for the plan, when the plan is recognised.
    pub credit_pool: Option<f64>,
    /// End of the current billing period — when the monthly credit ledger
    /// refills. The windows reset on their own clocks; only the subscription
    /// carries this instant.
    pub period_end: Option<DateTime<Utc>>,
}

impl Eq for Snapshot {}

impl Snapshot {
    /// The window closest to its cap, which is what the bar text leads with.
    pub fn worst_pct(&self) -> i32 {
        self.five_hour
            .iter()
            .chain(self.weekly.iter())
            .map(SpendWindow::pct)
            .max()
            .unwrap_or(0)
    }

    /// Fraction of the monthly allowance already spent, when both are known.
    pub fn credits_spent(&self) -> Option<f64> {
        let pool = self.credit_pool?;
        let remaining = self.credits.as_ref()?.remaining();
        Some((pool - remaining).max(0.0))
    }
}

/// Monthly credit allowance per plan, in USD.
///
/// The API reports remaining credit but never the plan's ceiling, so the
/// "spent of allowance" line needs this table. An unknown plan simply omits
/// that line rather than guessing a denominator.
const PLAN_CREDITS: &[(&str, f64)] = &[
    ("individual-go", 10.0),
    ("individual-goat", 70.0),
    ("individual-pro", 30.0),
    ("individual-pro-v1", 80.0),
    ("individual-provider", 15.0),
    ("individual-max", 150.0),
    ("individual-ultra", 300.0),
    ("teams-pro", 40.0),
];

/// Display label per plan id, matching what Command Code's own `/usage` shows.
const PLAN_LABELS: &[(&str, &str)] = &[
    ("individual-go", "Go"),
    ("individual-goat", "GOAT"),
    ("individual-pro", "Pro"),
    ("individual-pro-v1", "Pro"),
    ("individual-provider", "Provider"),
    ("individual-max", "Max"),
    ("individual-ultra", "Ultra"),
    ("teams-pro", "Teams Pro"),
];

pub fn plan_label(plan_id: &str) -> String {
    PLAN_LABELS
        .iter()
        .find(|(id, _)| *id == plan_id)
        .map(|(_, label)| (*label).to_string())
        .unwrap_or_else(|| plan_id.to_string())
}

pub fn plan_credits(plan_id: &str) -> Option<f64> {
    PLAN_CREDITS
        .iter()
        .find(|(id, _)| *id == plan_id)
        .map(|(_, credits)| *credits)
}

/// Parse `/alpha/billing/credits`.
pub fn parse_credits(value: &Value) -> Result<Snapshot, String> {
    let root = value
        .as_object()
        .ok_or_else(|| "Command Code credits response must be a JSON object".to_string())?;
    if root.contains_key("error") {
        return Err("Command Code credits response is an error envelope".to_string());
    }

    // `windowLimits` has been observed both at the top level and beside the
    // ledger; accept either rather than betting on one.
    let ledger = root.get("credits").and_then(Value::as_object);
    let windows = root
        .get("windowLimits")
        .and_then(Value::as_object)
        .or_else(|| {
            ledger
                .and_then(|l| l.get("windowLimits"))
                .and_then(Value::as_object)
        });

    let credits = ledger.map(|ledger| Credits {
        monthly: finite(ledger.get("monthlyCredits")).unwrap_or(0.0),
        purchased: finite(ledger.get("purchasedCredits")).unwrap_or(0.0),
        free: finite(ledger.get("freeCredits")).unwrap_or(0.0),
    });

    let snapshot = Snapshot {
        plan: None,
        five_hour: windows
            .and_then(|w| parse_window(w.get("fiveHour"), "fiveHour"))
            .transpose()?,
        weekly: windows
            .and_then(|w| parse_window(w.get("weekly"), "weekly"))
            .transpose()?,
        credits,
        credit_pool: None,
        period_end: None,
    };

    if snapshot.five_hour.is_none() && snapshot.weekly.is_none() && snapshot.credits.is_none() {
        return Err("Command Code credits response has no windows or ledger".to_string());
    }
    Ok(snapshot)
}

/// Fold `/alpha/billing/subscriptions` into a snapshot: plan label and the
/// allowance its tier includes.
pub fn apply_subscription(snapshot: &mut Snapshot, value: &Value) {
    let Some(plan_id) = value
        .get("data")
        .and_then(|data| data.get("planId"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
    else {
        return;
    };
    snapshot.plan = Some(plan_label(plan_id));
    snapshot.credit_pool = plan_credits(plan_id);
    // The billing period end is when the monthly ledger refills. Parse
    // defensively: a missing or malformed field costs only this line.
    snapshot.period_end = value
        .get("data")
        .and_then(|data| data.get("currentPeriodEnd"))
        .and_then(Value::as_str)
        .and_then(|text| text.trim().parse::<DateTime<chrono::FixedOffset>>().ok())
        .map(|parsed| parsed.with_timezone(&Utc));
}

fn parse_window(value: Option<&Value>, name: &str) -> Option<Result<SpendWindow, String>> {
    let value = value?;
    if value.is_null() {
        return None;
    }
    let Some(object) = value.as_object() else {
        return Some(Err(format!("windowLimits.{name} must be an object")));
    };
    let (Some(used), Some(cap)) = (finite(object.get("used")), finite(object.get("cap"))) else {
        return Some(Err(format!(
            "windowLimits.{name} must carry finite used and cap"
        )));
    };
    if used < 0.0 || cap < 0.0 {
        return Some(Err(format!("windowLimits.{name} must not be negative")));
    }
    Some(Ok(SpendWindow {
        used,
        cap,
        resets_at: object.get("resetAt").and_then(parse_reset),
    }))
}

/// Resets arrive as millisecond epochs, which is why this cannot lean on the
/// RFC3339 parsing every other vendor uses. A string is accepted too, in case
/// the field ever changes shape.
fn parse_reset(value: &Value) -> Option<DateTime<Utc>> {
    if let Some(millis) = value.as_i64() {
        return DateTime::from_timestamp_millis(millis);
    }
    if let Some(text) = value.as_str() {
        if let Ok(parsed) = text.parse::<DateTime<chrono::FixedOffset>>() {
            return Some(parsed.with_timezone(&Utc));
        }
        if let Ok(millis) = text.parse::<i64>() {
            return DateTime::from_timestamp_millis(millis);
        }
    }
    None
}

fn finite(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64).filter(|n| n.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credits_value() -> Value {
        serde_json::from_str(include_str!(
            "../../tests/fixtures/commandcode/credits.json"
        ))
        .expect("fixture JSON must be valid")
    }

    #[test]
    fn parses_the_live_credits_fixture() {
        let snapshot = parse_credits(&credits_value()).expect("fixture must parse");

        let five_hour = snapshot.five_hour.expect("fiveHour window");
        assert_eq!(five_hour.cap, 14.0);
        assert_eq!(five_hour.pct(), 25);
        assert_eq!(
            five_hour.resets_at.expect("reset").to_rfc3339(),
            "2026-08-27T20:00:00+00:00"
        );

        let weekly = snapshot.weekly.expect("weekly window");
        assert_eq!(weekly.cap, 35.0);
        assert_eq!(weekly.pct(), 30);

        let credits = snapshot.credits.expect("ledger");
        assert_eq!(credits.remaining(), 42.0);
    }

    #[test]
    fn millisecond_epoch_resets_become_utc_timestamps() {
        // The API reports resets as ms epochs, not the RFC3339 every other
        // vendor sends; a naive parser silently drops the countdown.
        let value = serde_json::json!({
            "windowLimits": {"weekly": {"used": 1, "cap": 4, "resetAt": 1788374172830_i64}}
        });

        let weekly = parse_credits(&value).unwrap().weekly.expect("weekly");

        assert_eq!(
            weekly.resets_at.expect("reset").to_rfc3339(),
            "2026-09-02T18:36:12.830+00:00"
        );
    }

    #[test]
    fn accepts_windows_nested_beside_the_ledger() {
        let value = serde_json::json!({
            "credits": {
                "monthlyCredits": 5.0,
                "windowLimits": {"weekly": {"used": 1, "cap": 4, "resetAt": null}}
            }
        });

        let snapshot = parse_credits(&value).expect("nested windows must parse");

        assert_eq!(snapshot.weekly.expect("weekly").pct(), 25);
        assert_eq!(snapshot.credits.expect("ledger").remaining(), 5.0);
    }

    #[test]
    fn sums_every_credit_pool() {
        let value = serde_json::json!({
            "credits": {"monthlyCredits": 10.5, "purchasedCredits": 4.0, "freeCredits": 0.5},
            "windowLimits": {"weekly": {"used": 1, "cap": 4}}
        });

        let credits = parse_credits(&value).unwrap().credits.expect("ledger");

        assert_eq!(credits.remaining(), 15.0);
    }

    #[test]
    fn rejects_error_envelopes_and_empty_documents() {
        for value in [
            serde_json::json!({}),
            serde_json::json!({"error": "unauthorized"}),
            serde_json::json!({"windowLimits": {}}),
        ] {
            assert!(parse_credits(&value).is_err(), "accepted {value}");
        }
        assert!(parse_credits(&serde_json::json!("nope")).is_err());
    }

    #[test]
    fn rejects_malformed_or_negative_windows() {
        for window in [
            serde_json::json!("not-an-object"),
            serde_json::json!({"used": -1, "cap": 4}),
            serde_json::json!({"used": 1}),
            serde_json::json!({"used": "1", "cap": 4}),
        ] {
            let value = serde_json::json!({"windowLimits": {"weekly": window}});
            assert!(parse_credits(&value).is_err(), "accepted {window}");
        }
    }

    #[test]
    fn additive_fields_and_null_windows_are_tolerated() {
        // `exceeded` and `limited` already ship alongside the windows, and a
        // plan without a five-hour cap sends null rather than omitting it.
        let value = serde_json::json!({
            "credits": {"monthlyCredits": 1.0, "unexpected": true},
            "windowLimits": {
                "limited": true,
                "exceeded": null,
                "fiveHour": null,
                "weekly": {"used": 1, "cap": 4, "exceeded": false, "future": 1}
            }
        });

        let snapshot = parse_credits(&value).expect("must tolerate additive fields");

        assert!(snapshot.five_hour.is_none());
        assert_eq!(snapshot.weekly.expect("weekly").pct(), 25);
    }

    #[test]
    fn percentage_is_clamped_and_safe_at_a_zero_cap() {
        assert_eq!(
            SpendWindow {
                used: 9.0,
                cap: 0.0,
                resets_at: None
            }
            .pct(),
            0
        );
        assert_eq!(
            SpendWindow {
                used: 9.0,
                cap: 4.0,
                resets_at: None
            }
            .pct(),
            100
        );
    }

    #[test]
    fn subscription_supplies_the_plan_label_and_its_allowance() {
        let subscription: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/commandcode/subscriptions.json"
        ))
        .expect("fixture JSON must be valid");
        let mut snapshot = parse_credits(&credits_value()).unwrap();

        apply_subscription(&mut snapshot, &subscription);

        assert_eq!(snapshot.plan.as_deref(), Some("GOAT"));
        assert_eq!(snapshot.credit_pool, Some(70.0));
        // The $70 allowance less the $42 still on the ledger.
        assert_eq!(snapshot.credits_spent(), Some(28.0));
        // The billing period end doubles as the monthly credit reset.
        assert_eq!(
            snapshot.period_end.expect("period end").to_rfc3339(),
            "2026-09-19T16:39:05+00:00"
        );
    }

    #[test]
    fn a_malformed_period_end_is_dropped_not_fatal() {
        let mut snapshot = parse_credits(&credits_value()).unwrap();

        apply_subscription(
            &mut snapshot,
            &serde_json::json!({"data": {"planId": "individual-goat", "currentPeriodEnd": "not-a-date"}}),
        );

        assert_eq!(snapshot.plan.as_deref(), Some("GOAT"));
        assert_eq!(snapshot.period_end, None);
    }

    #[test]
    fn unknown_plan_keeps_its_id_and_claims_no_allowance() {
        let mut snapshot = parse_credits(&credits_value()).unwrap();

        apply_subscription(
            &mut snapshot,
            &serde_json::json!({"data": {"planId": "individual-future"}}),
        );

        assert_eq!(snapshot.plan.as_deref(), Some("individual-future"));
        assert_eq!(snapshot.credit_pool, None);
        assert_eq!(snapshot.credits_spent(), None);
    }

    #[test]
    fn missing_subscription_leaves_the_snapshot_untouched() {
        let mut snapshot = parse_credits(&credits_value()).unwrap();

        apply_subscription(&mut snapshot, &serde_json::json!({"success": false}));

        assert!(snapshot.plan.is_none());
        assert!(snapshot.weekly.is_some());
    }

    #[test]
    fn worst_window_leads_the_bar_text() {
        let snapshot = Snapshot {
            five_hour: Some(SpendWindow {
                used: 1.0,
                cap: 10.0,
                resets_at: None,
            }),
            weekly: Some(SpendWindow {
                used: 8.0,
                cap: 10.0,
                resets_at: None,
            }),
            ..Snapshot::default()
        };

        assert_eq!(snapshot.worst_pct(), 80);
        assert_eq!(Snapshot::default().worst_pct(), 0);
    }
}
