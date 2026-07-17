//! Wire types for Kimi's `/coding/v1/usages` endpoint.

use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::error::{AppError, Result};
use crate::usage::KimiSnapshot;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct UsagesResponse {
    user: Option<User>,
    usage: Option<UsageBlock>,
    limits: Vec<Limit>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct User {
    membership: Option<Membership>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct Membership {
    level: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct UsageBlock {
    limit: Option<NumericOrString>,
    used: Option<NumericOrString>,
    remaining: Option<NumericOrString>,
    #[serde(rename = "resetTime")]
    reset_time: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct Limit {
    window: Option<Window>,
    detail: Option<UsageBlock>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct Window {
    duration: u64,
    #[serde(rename = "timeUnit")]
    time_unit: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum NumericOrString {
    Number(u64),
    String(String),
}

impl NumericOrString {
    fn as_u64(&self) -> Option<u64> {
        match self {
            NumericOrString::Number(n) => Some(*n),
            NumericOrString::String(s) => s.trim().parse::<u64>().ok(),
        }
    }
}

impl UsagesResponse {
    pub fn into_snapshot(self) -> Result<KimiSnapshot> {
        let plan = self.user.and_then(|u| u.membership).and_then(|m| m.level);

        let usage = self
            .usage
            .ok_or_else(|| AppError::Schema("kimi: missing top-level usage block".into()))?;
        let (weekly_limit, weekly_used, weekly_remaining, weekly_reset) = extract_block(usage)?;

        let mut window_limit = 0u64;
        let mut window_used = 0u64;
        let mut window_remaining = 0u64;
        let mut window_reset = None;

        if let Some(detail) = self.limits.into_iter().find_map(|l| {
            if l.window
                .as_ref()
                .is_some_and(|w| w.duration == 300 && w.time_unit == "TIME_UNIT_MINUTE")
            {
                l.detail
            } else {
                None
            }
        }) {
            (window_limit, window_used, window_remaining, window_reset) = extract_block(detail)?;
        }

        Ok(KimiSnapshot {
            plan,
            weekly_limit,
            weekly_used,
            weekly_remaining,
            weekly_reset_at: weekly_reset,
            window_limit,
            window_used,
            window_remaining,
            window_reset_at: window_reset,
        })
    }
}

fn extract_block(block: UsageBlock) -> Result<(u64, u64, u64, Option<DateTime<Utc>>)> {
    let limit = parse_count(&block.limit, "limit")?
        .ok_or_else(|| AppError::Schema("kimi: missing limit in usage block".into()))?;
    let used = parse_count(&block.used, "used")?;
    let remaining = parse_count(&block.remaining, "remaining")?;
    let reset = parse_reset(block.reset_time.as_deref())?;

    let (used, remaining) = match (used, remaining) {
        (Some(u), Some(r)) => (u, r),
        (Some(u), None) => (u, limit.saturating_sub(u)),
        (None, Some(r)) => (limit.saturating_sub(r), r),
        (None, None) => (0, limit),
    };

    Ok((limit, used, remaining, reset))
}

fn parse_count(field: &Option<NumericOrString>, name: &str) -> Result<Option<u64>> {
    match field {
        None => Ok(None),
        Some(n) => n
            .as_u64()
            .map(Some)
            .ok_or_else(|| AppError::Schema(format!("kimi: invalid numeric value for {name}"))),
    }
}

fn parse_reset(s: Option<&str>) -> Result<Option<DateTime<Utc>>> {
    match s {
        None | Some("") => Ok(None),
        Some(s) => DateTime::parse_from_rfc3339(s)
            .map(|dt| Some(dt.into()))
            .map_err(|e| AppError::Schema(format!("kimi: unparseable resetTime: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_representative_json_with_string_numbers() {
        let raw = r#"{
            "user": { "membership": { "level": "LEVEL_INTERMEDIATE" } },
            "usage": { "limit": "100", "used": "26", "remaining": "74", "resetTime": "2026-02-11T17:32:50.757941Z" },
            "limits": [
                {
                    "window": { "duration": 300, "timeUnit": "TIME_UNIT_MINUTE" },
                    "detail": { "limit": "100", "used": "15", "remaining": "85", "resetTime": "2026-02-07T12:32:50.757941Z" }
                }
            ]
        }"#;
        let snap: KimiSnapshot = serde_json::from_str::<UsagesResponse>(raw)
            .unwrap()
            .into_snapshot()
            .unwrap();
        assert_eq!(snap.plan, Some("LEVEL_INTERMEDIATE".into()));
        assert_eq!(snap.weekly_limit, 100);
        assert_eq!(snap.weekly_used, 26);
        assert_eq!(snap.weekly_remaining, 74);
        assert!(snap.weekly_reset_at.is_some());
        assert_eq!(snap.window_limit, 100);
        assert_eq!(snap.window_used, 15);
        assert_eq!(snap.window_remaining, 85);
        assert!(snap.window_reset_at.is_some());
        assert_eq!(snap.weekly_pct(), 26);
        assert_eq!(snap.window_pct(), 15);
    }

    #[test]
    fn parses_numeric_json_numbers() {
        let raw = r#"{
            "user": { "membership": { "level": "LEVEL_ADVANCED" } },
            "usage": { "limit": 500, "used": 123, "remaining": 377, "resetTime": "2026-02-11T17:32:50Z" },
            "limits": [
                {
                    "window": { "duration": 300, "timeUnit": "TIME_UNIT_MINUTE" },
                    "detail": { "limit": 200, "used": 50, "remaining": 150, "resetTime": "2026-02-07T12:32:50Z" }
                }
            ]
        }"#;
        let snap: KimiSnapshot = serde_json::from_str::<UsagesResponse>(raw)
            .unwrap()
            .into_snapshot()
            .unwrap();
        assert_eq!(snap.plan, Some("LEVEL_ADVANCED".into()));
        assert_eq!(snap.weekly_limit, 500);
        assert_eq!(snap.weekly_used, 123);
        assert_eq!(snap.weekly_remaining, 377);
        assert_eq!(snap.window_limit, 200);
        assert_eq!(snap.window_used, 50);
        assert_eq!(snap.window_remaining, 150);
    }

    #[test]
    fn parses_missing_user_and_limits() {
        let raw = r#"{
            "usage": { "limit": "100", "used": "26", "remaining": "74" }
        }"#;
        let snap: KimiSnapshot = serde_json::from_str::<UsagesResponse>(raw)
            .unwrap()
            .into_snapshot()
            .unwrap();
        assert_eq!(snap.plan, None);
        assert_eq!(snap.weekly_limit, 100);
        assert_eq!(snap.weekly_used, 26);
        assert_eq!(snap.weekly_remaining, 74);
        assert_eq!(snap.weekly_reset_at, None);
        assert_eq!(snap.window_limit, 0);
        assert_eq!(snap.window_used, 0);
        assert_eq!(snap.window_remaining, 0);
        assert_eq!(snap.window_reset_at, None);
    }

    #[test]
    fn computes_used_when_missing() {
        let raw = r#"{
            "usage": { "limit": "100", "remaining": "74" }
        }"#;
        let snap: KimiSnapshot = serde_json::from_str::<UsagesResponse>(raw)
            .unwrap()
            .into_snapshot()
            .unwrap();
        assert_eq!(snap.weekly_used, 26);
        assert_eq!(snap.weekly_remaining, 74);
    }

    #[test]
    fn computes_remaining_when_missing() {
        let raw = r#"{
            "usage": { "limit": "100", "used": "26" }
        }"#;
        let snap: KimiSnapshot = serde_json::from_str::<UsagesResponse>(raw)
            .unwrap()
            .into_snapshot()
            .unwrap();
        assert_eq!(snap.weekly_used, 26);
        assert_eq!(snap.weekly_remaining, 74);
    }

    #[test]
    fn zero_strings_are_valid() {
        let raw = r#"{
            "usage": { "limit": "100", "used": "0", "remaining": "100" }
        }"#;
        let snap: KimiSnapshot = serde_json::from_str::<UsagesResponse>(raw)
            .unwrap()
            .into_snapshot()
            .unwrap();
        assert_eq!(snap.weekly_used, 0);
        assert_eq!(snap.weekly_remaining, 100);
        assert_eq!(snap.weekly_pct(), 0);
    }

    #[test]
    fn both_counts_missing_yields_unused_quota() {
        let raw = r#"{
            "usage": { "limit": "100" }
        }"#;
        let snap: KimiSnapshot = serde_json::from_str::<UsagesResponse>(raw)
            .unwrap()
            .into_snapshot()
            .unwrap();
        assert_eq!(snap.weekly_used, 0);
        assert_eq!(snap.weekly_remaining, 100);
        assert_eq!(snap.weekly_pct(), 0);
    }

    #[test]
    fn malformed_numeric_string_rejected() {
        let raw = r#"{
            "usage": { "limit": "100", "used": "garbage" }
        }"#;
        let err = serde_json::from_str::<UsagesResponse>(raw)
            .unwrap()
            .into_snapshot()
            .unwrap_err();
        assert!(
            err.to_string().contains("used"),
            "expected used parse error, got {err}"
        );
    }

    #[test]
    fn overflow_string_rejected() {
        let raw = r#"{
            "usage": { "limit": "18446744073709551616", "used": "0" }
        }"#;
        let err = serde_json::from_str::<UsagesResponse>(raw)
            .unwrap()
            .into_snapshot()
            .unwrap_err();
        assert!(
            err.to_string().contains("limit"),
            "expected limit overflow error, got {err}"
        );
    }

    #[test]
    fn negative_json_number_rejected_without_panic() {
        let raw = r#"{
            "usage": { "limit": 100, "used": -1 }
        }"#;
        // Deserialization itself must fail because -1 is not a valid u64.
        let res = serde_json::from_str::<UsagesResponse>(raw);
        assert!(
            res.is_err(),
            "negative u64 should not deserialize without panic"
        );
    }

    #[test]
    fn selects_300_min_window() {
        let raw = r#"{
            "usage": { "limit": "100", "used": "26", "remaining": "74" },
            "limits": [
                {
                    "window": { "duration": 300, "timeUnit": "TIME_UNIT_MINUTE" },
                    "detail": { "limit": "100", "used": "15", "remaining": "85" }
                }
            ]
        }"#;
        let snap: KimiSnapshot = serde_json::from_str::<UsagesResponse>(raw)
            .unwrap()
            .into_snapshot()
            .unwrap();
        assert_eq!(snap.window_limit, 100);
        assert_eq!(snap.window_used, 15);
    }

    #[test]
    fn empty_limits_yield_no_window() {
        let raw = r#"{
            "usage": { "limit": "100", "used": "26", "remaining": "74" },
            "limits": []
        }"#;
        let snap: KimiSnapshot = serde_json::from_str::<UsagesResponse>(raw)
            .unwrap()
            .into_snapshot()
            .unwrap();
        assert_eq!(snap.window_limit, 0);
        assert_eq!(snap.window_used, 0);
        assert_eq!(snap.window_remaining, 0);
    }

    #[test]
    fn no_matching_window_yields_no_window() {
        let raw = r#"{
            "usage": { "limit": "100", "used": "26", "remaining": "74" },
            "limits": [
                {
                    "window": { "duration": 60, "timeUnit": "TIME_UNIT_MINUTE" },
                    "detail": { "limit": "100", "used": "1", "remaining": "99" }
                }
            ]
        }"#;
        let snap: KimiSnapshot = serde_json::from_str::<UsagesResponse>(raw)
            .unwrap()
            .into_snapshot()
            .unwrap();
        assert_eq!(snap.window_limit, 0);
        assert_eq!(snap.window_used, 0);
    }

    #[test]
    fn selects_second_300_min_window_when_first_lacks_detail() {
        let raw = r#"{
            "usage": { "limit": "100", "used": "10", "remaining": "90" },
            "limits": [
                {
                    "window": { "duration": 300, "timeUnit": "TIME_UNIT_MINUTE" }
                },
                {
                    "window": { "duration": 300, "timeUnit": "TIME_UNIT_MINUTE" },
                    "detail": { "limit": "100", "used": "25", "remaining": "75" }
                }
            ]
        }"#;
        let snap: KimiSnapshot = serde_json::from_str::<UsagesResponse>(raw)
            .unwrap()
            .into_snapshot()
            .unwrap();
        assert_eq!(snap.window_limit, 100);
        assert_eq!(snap.window_used, 25);
        assert_eq!(snap.window_remaining, 75);
    }

    #[test]
    fn selects_first_300_min_window_among_multiple() {
        let raw = r#"{
            "usage": { "limit": "100", "used": "10", "remaining": "90" },
            "limits": [
                {
                    "window": { "duration": 60, "timeUnit": "TIME_UNIT_MINUTE" },
                    "detail": { "limit": "100", "used": "1", "remaining": "99" }
                },
                {
                    "window": { "duration": 300, "timeUnit": "TIME_UNIT_MINUTE" },
                    "detail": { "limit": "100", "used": "25", "remaining": "75" }
                },
                {
                    "window": { "duration": 300, "timeUnit": "TIME_UNIT_MINUTE" },
                    "detail": { "limit": "100", "used": "50", "remaining": "50" }
                }
            ]
        }"#;
        let snap: KimiSnapshot = serde_json::from_str::<UsagesResponse>(raw)
            .unwrap()
            .into_snapshot()
            .unwrap();
        assert_eq!(snap.window_used, 25);
    }

    #[test]
    fn used_greater_than_limit_clamps_pct() {
        let snap = KimiSnapshot {
            plan: None,
            weekly_limit: 100,
            weekly_used: 150,
            weekly_remaining: 0,
            weekly_reset_at: None,
            window_limit: 0,
            window_used: 0,
            window_remaining: 0,
            window_reset_at: None,
        };
        assert_eq!(snap.weekly_pct(), 100);
    }

    #[test]
    fn u64_max_round_trip() {
        let raw = r#"{
            "usage": { "limit": "18446744073709551615", "used": "0", "remaining": "18446744073709551615" }
        }"#;
        let snap: KimiSnapshot = serde_json::from_str::<UsagesResponse>(raw)
            .unwrap()
            .into_snapshot()
            .unwrap();
        assert_eq!(snap.weekly_limit, u64::MAX);
        assert_eq!(snap.weekly_remaining, u64::MAX);
    }
}
