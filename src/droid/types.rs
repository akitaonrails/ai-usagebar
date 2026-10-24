//! Response projection for the Droid `/api/billing/limits` endpoint.
//!
//! The API reports consumed percentages (`usedPercent`) and an ISO-8601
//! `windowEnd` per window, in two buckets: `standard` and `core` (Droid Core).
//! A `windowEnd: null` marks a window the account has not started — the
//! percentage is still reported, so the window exists with no reset.
use chrono::{DateTime, Duration, Utc};
use serde_json::Value;

use crate::error::{AppError, Result};
use crate::usage::{DroidSnapshot, UsageWindow};

pub const SESSION_WINDOW: Duration = Duration::hours(5);
pub const WEEKLY_WINDOW: Duration = Duration::days(7);
pub const MONTHLY_WINDOW: Duration = Duration::days(30);

pub fn parse_response(bytes: &[u8]) -> Result<DroidSnapshot> {
    let root: Value = serde_json::from_slice(bytes)
        .map_err(|_| AppError::Schema("Droid limits response was not valid JSON".into()))?;
    let limits = root
        .get("limits")
        .and_then(Value::as_object)
        .ok_or_else(|| AppError::Schema("Droid limits response omitted `limits`".into()))?;
    if !limits.contains_key("standard") && !limits.contains_key("core") {
        return Err(AppError::Schema(
            "Droid limits response contained no recognized quota buckets".into(),
        ));
    }
    let bucket = |name: &str, duration: (Duration, Duration, Duration)| -> Result<_> {
        Ok(match limits.get(name) {
            None | Some(Value::Null) => (None, None, None),
            Some(bucket) => (
                window(bucket.get("fiveHour"), duration.0)?,
                window(bucket.get("weekly"), duration.1)?,
                window(bucket.get("monthly"), duration.2)?,
            ),
        })
    };
    let (session, weekly, monthly) =
        bucket("standard", (SESSION_WINDOW, WEEKLY_WINDOW, MONTHLY_WINDOW))?;
    let (core_session, core_weekly, core_monthly) =
        bucket("core", (SESSION_WINDOW, WEEKLY_WINDOW, MONTHLY_WINDOW))?;
    let extra_balance_cents = match root.get("extraUsageBalanceCents") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_i64()
                .ok_or_else(|| drift("extraUsageBalanceCents"))?,
        ),
    };
    Ok(DroidSnapshot {
        session,
        weekly,
        monthly,
        core_session,
        core_weekly,
        core_monthly,
        extra_balance_cents,
    })
}

/// One `{"usedPercent", "windowEnd", "secondsRemaining"}` window. Absent or
/// null stays absent; a present window with `windowEnd: null` has not started
/// and carries its percentage with no reset.
fn window(value: Option<&Value>, duration: Duration) -> Result<Option<UsageWindow>> {
    let Some(value) = value else { return Ok(None) };
    if value.is_null() {
        return Ok(None);
    }
    let used = value
        .get("usedPercent")
        .and_then(Value::as_i64)
        .filter(|pct| (0..=100).contains(pct))
        .ok_or_else(|| drift("usedPercent"))? as i32;
    let resets_at = match value.get("windowEnd") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(
            DateTime::parse_from_rfc3339(text)
                .map(|at| at.with_timezone(&Utc))
                .map_err(|_| drift("windowEnd"))?,
        ),
        Some(_) => return Err(drift("windowEnd")),
    };
    Ok(Some(UsageWindow {
        utilization_pct: used,
        resets_at,
        window_duration: duration,
    }))
}

fn drift(field: &str) -> AppError {
    AppError::Schema(format!(
        "Droid limits field {field} had an unsupported value"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real response shape, numbers from a live account on 2026-10-21.
    const LIVE: &str = r#"{
      "usesTokenRateLimitsBilling": true,
      "limits": {
        "standard": {
          "fiveHour": {"usedPercent": 2, "windowEnd": "2026-10-22T01:38:45.109Z", "secondsRemaining": 16146},
          "weekly":   {"usedPercent": 1, "windowEnd": "2026-10-28T20:38:45.109Z", "secondsRemaining": 602946},
          "monthly":  {"usedPercent": 1, "windowEnd": "2026-11-20T20:38:45.109Z", "secondsRemaining": 2590146}
        },
        "core": {
          "fiveHour": {"usedPercent": 0, "windowEnd": null, "secondsRemaining": null},
          "weekly":   {"usedPercent": 0, "windowEnd": null, "secondsRemaining": null},
          "monthly":  {"usedPercent": 0, "windowEnd": null, "secondsRemaining": null}
        }
      },
      "overagePreference": "droidCore",
      "canManageOverage": true,
      "extraUsageBalanceCents": 0,
      "extraUsageAllowed": true
    }"#;

    #[test]
    fn the_observed_response_projects_both_buckets_and_the_balance() {
        let snap = parse_response(LIVE.as_bytes()).unwrap();
        let session = snap.session.as_ref().unwrap();
        assert_eq!(session.utilization_pct, 2);
        assert_eq!(
            session.resets_at.unwrap().timestamp(),
            DateTime::parse_from_rfc3339("2026-10-22T01:38:45.109Z")
                .unwrap()
                .timestamp()
        );
        assert_eq!(session.window_duration, SESSION_WINDOW);
        let weekly = snap.weekly.as_ref().unwrap();
        assert_eq!(weekly.utilization_pct, 1);
        assert_eq!(weekly.window_duration, WEEKLY_WINDOW);
        assert_eq!(
            snap.monthly.as_ref().unwrap().window_duration,
            MONTHLY_WINDOW
        );
        // A `windowEnd: null` window is present at 0% with no reset.
        let core = snap.core_session.as_ref().unwrap();
        assert_eq!(core.utilization_pct, 0);
        assert_eq!(core.resets_at, None);
        assert_eq!(snap.extra_balance_cents, Some(0));
    }

    #[test]
    fn an_absent_window_or_bucket_stays_absent() {
        let snap = parse_response(
            br#"{"limits": {"standard": {"fiveHour": {"usedPercent": 40, "windowEnd": "2026-10-22T01:38:45.109Z"}}}, "extraUsageBalanceCents": 250}"#,
        )
        .unwrap();
        assert_eq!(snap.session.as_ref().unwrap().utilization_pct, 40);
        assert!(snap.weekly.is_none());
        assert!(snap.monthly.is_none());
        assert!(snap.core_session.is_none());
        assert_eq!(snap.extra_balance_cents, Some(250));
    }

    #[test]
    fn invalid_values_and_envelopes_are_schema_errors() {
        for body in [
            br#"{"limits":{"standard":{"fiveHour":{"usedPercent":-1}}}}"#.as_slice(),
            br#"{"limits":{"standard":{"fiveHour":{"usedPercent":101}}}}"#.as_slice(),
            br#"{"limits":{"standard":{"fiveHour":{"usedPercent":"5"}}}}"#.as_slice(),
            br#"{"limits":{"standard":{"fiveHour":{"usedPercent":5,"windowEnd":12}}}}"#.as_slice(),
            br#"{"limits":{"standard":{"fiveHour":{"usedPercent":5,"windowEnd":"soon"}}}}"#
                .as_slice(),
            br#"{"limits":{"standard":{"fiveHour":{}}}}"#.as_slice(),
            br#"{"limits":{"extraUsageBalanceCents":"5"}}"#.as_slice(),
            br#"{"limits":{}}"#.as_slice(),
            br#"{"other":1}"#.as_slice(),
            b"<html>secret</html>".as_slice(),
        ] {
            assert!(
                matches!(parse_response(body), Err(AppError::Schema(_))),
                "{body:?}"
            );
        }
    }
}
