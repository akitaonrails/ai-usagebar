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
    // Kimi omits this for accounts without a rolling quota and has also
    // returned `null`; both mean no rolling window is available.
    limits: Option<Vec<Limit>>,
    // The newer response shape: quota ratios keyed by name, *replacing* the
    // top-level `usage` block. Only `limit_month_total` — the combined
    // monthly pool — is read. `limit_month_code` is the Code slice inside
    // that pool, never its own allowance, and `limit_5h` disagrees with
    // `limits[]`, which stays the one source for the rolling window.
    usages: Option<UsagesMap>,
}

/// Quota ratios keyed by quota name on the newer response shape.
pub type UsagesMap = std::collections::HashMap<String, UsageRatio>;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct UsageRatio {
    /// Fraction of the pool consumed (0.0..=1.0); ×100 is the percent.
    used_ratio: Option<f64>,
    #[serde(alias = "resetTime", alias = "resetAt", alias = "reset_at")]
    reset_time: Option<String>,
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
    #[serde(
        rename = "resetTime",
        alias = "resetAt",
        alias = "reset_at",
        alias = "reset_time"
    )]
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
    #[serde(rename = "timeUnit", alias = "time_unit")]
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
        // The raw membership enum ("LEVEL_INTERMEDIATE") is a wire value, not
        // something to put on a status bar. `fetch` overwrites this with the
        // vendor's own tier name from `/me` when that call succeeds.
        let plan = self
            .user
            .and_then(|u| u.membership)
            .and_then(|m| m.level)
            .map(|level| humanize_membership_level(&level));

        // `limits` is absent for accounts where Kimi does not expose the
        // rolling quota. Once it is present, a 5h window is required: silently
        // treating an unfamiliar advertised window as zero usage masks drift.
        let limits = self.limits.unwrap_or_default();
        let (window_limit, window_used, window_remaining, window_reset) = if limits.is_empty() {
            (0, 0, 0, None)
        } else {
            let detail = limits
                .into_iter()
                .find_map(|l| {
                    (l.window.as_ref().is_some_and(is_five_hour_window))
                        .then_some(l.detail)
                        .flatten()
                })
                .ok_or_else(|| {
                    AppError::Schema("kimi: missing recognized 5h usage window".into())
                })?;
            extract_block(detail)?
        };

        match self.usage {
            // The legacy shape: weekly counters in the top-level `usage`
            // block. A `usages` map alongside it is ignored.
            Some(usage) => {
                let (weekly_limit, weekly_used, weekly_remaining, weekly_reset) =
                    extract_block(usage)?;
                Ok(KimiSnapshot {
                    plan,
                    weekly_limit,
                    weekly_used,
                    weekly_remaining,
                    weekly_reset_at: weekly_reset,
                    has_weekly: true,
                    monthly_pct: None,
                    monthly_reset_at: None,
                    window_limit,
                    window_used,
                    window_remaining,
                    window_reset_at: window_reset,
                })
            }
            // The newer shape: no weekly bucket at all. The monthly pool is
            // the only long window, and nothing fabricates weekly counts.
            None => {
                let monthly = self
                    .usages
                    .as_ref()
                    .and_then(|usages| usages.get("limit_month_total"))
                    .ok_or_else(|| {
                        AppError::Schema("kimi: missing top-level usage block".into())
                    })?;
                Ok(KimiSnapshot {
                    plan,
                    weekly_limit: 0,
                    weekly_used: 0,
                    weekly_remaining: 0,
                    weekly_reset_at: None,
                    has_weekly: false,
                    monthly_pct: Some(monthly_pct(monthly.used_ratio)?),
                    monthly_reset_at: parse_reset(monthly.reset_time.as_deref())?,
                    window_limit,
                    window_used,
                    window_remaining,
                    window_reset_at: window_reset,
                })
            }
        }
    }
}

/// `used_ratio` is a fraction of the monthly pool (validated against the
/// vendor's own website); the bar speaks percent. Anything outside
/// 0.0..=1.0 — or not a finite number at all — is schema drift, not a quota.
fn monthly_pct(ratio: Option<f64>) -> Result<i32> {
    let ratio = ratio
        .ok_or_else(|| AppError::Schema("kimi: limit_month_total is missing used_ratio".into()))?;
    if !ratio.is_finite() || !(0.0..=1.0).contains(&ratio) {
        return Err(AppError::Schema(format!(
            "kimi: limit_month_total used_ratio out of range: {ratio}"
        )));
    }
    Ok((ratio * 100.0).round() as i32)
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
        (None, None) => {
            return Err(AppError::Schema(
                "kimi: usage block is missing both used and remaining".into(),
            ));
        }
    };

    Ok((limit, used, remaining, reset))
}

/// The profile response from `/coding/v1/me`, read for exactly one field.
///
/// That endpoint also returns the account's email, phone, nickname, avatar and
/// ids. **None of them are deserialized here** — serde drops unknown fields, so
/// the personal data never enters a snapshot, the cache, or an error message.
/// Keep it that way: the plan label is the only thing this vendor needs.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct UserInfoResponse {
    /// The subscription tier's own name — "Andante", "Moderato",
    /// "Allegretto", "Allegro". Kimi names its plans after tempo markings, so
    /// this reads as a product name and not as a gamification badge.
    user_level_name: Option<String>,
}

impl UserInfoResponse {
    pub fn plan_label(&self) -> Option<String> {
        self.user_level_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
    }
}

/// Make a raw membership enum readable when the vendor's own label is
/// unavailable (`/me` unreachable, or an API key whose account has no coding
/// profile). `LEVEL_INTERMEDIATE` → `Intermediate`.
///
/// Deliberately *not* a table mapping levels onto tier names: the enum-to-tier
/// correspondence is not published anywhere, and inventing "Allegretto" for a
/// level that might mean something else would put a wrong plan on screen with
/// full confidence. Prettifying what the vendor said is the honest fallback.
pub fn humanize_membership_level(level: &str) -> String {
    let trimmed = level.trim();
    let body = trimmed.strip_prefix("LEVEL_").unwrap_or(trimmed);
    if body.is_empty() {
        return trimmed.to_string();
    }
    body.split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => {
                    first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Kimi documents the rolling window as 300 minutes. Accept only equivalent
/// spellings used by protobuf/JSON gateways, not arbitrary duration units.
fn is_five_hour_window(window: &Window) -> bool {
    matches!(
        (window.duration, window.time_unit.as_str()),
        (300, "TIME_UNIT_MINUTE" | "MINUTE" | "MINUTES") | (5, "TIME_UNIT_HOUR" | "HOUR" | "HOURS")
    )
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
    fn membership_levels_are_humanized_not_invented() {
        assert_eq!(
            humanize_membership_level("LEVEL_INTERMEDIATE"),
            "Intermediate"
        );
        assert_eq!(
            humanize_membership_level("LEVEL_SUPER_ADVANCED"),
            "Super Advanced"
        );
        // No LEVEL_ prefix, mixed case, and surrounding space still read well.
        assert_eq!(humanize_membership_level("  basic  "), "Basic");
        // Degenerate inputs are returned rather than turned into an empty label.
        assert_eq!(humanize_membership_level("LEVEL_"), "LEVEL_");
        assert_eq!(humanize_membership_level(""), "");
    }

    #[test]
    fn the_profile_response_yields_only_the_tier_name() {
        let raw = r#"{
            "user_id": "u-1", "nickname": "someone", "email": "someone@example.com",
            "phone": {"country_code": "55", "number": "999999999"},
            "user_level": 25, "user_level_name": "Allegretto",
            "domain_name": "DOMAIN_NEXUS"
        }"#;
        let me: UserInfoResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(me.plan_label(), Some("Allegretto".into()));
        // The struct has no field to hold the personal data, so nothing else
        // can leak into a snapshot or a Debug line.
        let rendered = format!("{me:?}");
        assert!(!rendered.contains("example.com"), "{rendered}");
        assert!(!rendered.contains("999999999"), "{rendered}");
    }

    #[test]
    fn a_blank_or_absent_tier_name_is_no_label() {
        for raw in [
            r#"{"user_level_name": ""}"#,
            r#"{"user_level_name": "  "}"#,
            "{}",
        ] {
            let me: UserInfoResponse = serde_json::from_str(raw).unwrap();
            assert_eq!(me.plan_label(), None, "{raw}");
        }
    }

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
        assert_eq!(snap.plan, Some("Intermediate".into()));
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
        assert_eq!(snap.plan, Some("Advanced".into()));
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
    fn both_counts_missing_is_schema_drift() {
        let raw = r#"{
            "usage": { "limit": "100" }
        }"#;
        let err = serde_json::from_str::<UsagesResponse>(raw)
            .unwrap()
            .into_snapshot()
            .unwrap_err();
        assert!(err.to_string().contains("both used and remaining"));
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
    fn null_limits_yield_no_window() {
        let raw = r#"{
            "usage": { "limit": "100", "used": "26", "remaining": "74" },
            "limits": null
        }"#;
        let snap = serde_json::from_str::<UsagesResponse>(raw)
            .unwrap()
            .into_snapshot()
            .unwrap();
        assert_eq!(snap.window_limit, 0);
        assert_eq!(snap.window_used, 0);
    }

    #[test]
    fn unrecognized_window_is_schema_drift() {
        let raw = r#"{
            "usage": { "limit": "100", "used": "26", "remaining": "74" },
            "limits": [
                {
                    "window": { "duration": 60, "timeUnit": "TIME_UNIT_MINUTE" },
                    "detail": { "limit": "100", "used": "1", "remaining": "99" }
                }
            ]
        }"#;
        let err = serde_json::from_str::<UsagesResponse>(raw)
            .unwrap()
            .into_snapshot()
            .unwrap_err();
        assert!(err.to_string().contains("recognized 5h"));
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
            has_weekly: true,
            monthly_pct: None,
            monthly_reset_at: None,
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

    #[test]
    fn accepts_reset_and_duration_aliases() {
        let raw = r#"{
            "usage": { "limit": 100, "used": 20, "resetAt": "2026-02-11T17:32:50Z" },
            "limits": [{
                "window": { "duration": 5, "time_unit": "TIME_UNIT_HOUR" },
                "detail": { "limit": 100, "remaining": 75, "reset_at": "2026-02-07T12:32:50Z" }
            }]
        }"#;
        let snap = serde_json::from_str::<UsagesResponse>(raw)
            .unwrap()
            .into_snapshot()
            .unwrap();
        assert_eq!(snap.weekly_used, 20);
        assert_eq!(snap.window_used, 25);
        assert!(snap.weekly_reset_at.is_some());
        assert!(snap.window_reset_at.is_some());
    }

    /// Verbatim redacted capture from an account on the newer response shape
    /// (issue #199): no top-level `usage` block, a `usages` map instead.
    const NEWER_SHAPE_CAPTURE: &str = r#"{
        "limits": [
            {
                "window": { "duration": 300, "timeUnit": "TIME_UNIT_MINUTE" },
                "detail": { "limit": "0", "used": "0", "remaining": "0", "resetTime": "2026-09-16T20:11:32.979529Z" }
            }
        ],
        "usages": {
            "limit_5h": { "used_ratio": 0, "reset_time": "2026-09-16T20:11:32Z" },
            "limit_month_total": { "used_ratio": 0.0, "reset_time": "2026-10-16T00:00:00Z" },
            "limit_month_code": { "used_ratio": 0, "reset_time": "2026-10-16T00:00:00Z" }
        }
    }"#;

    #[test]
    fn the_newer_usages_map_shape_parses_into_a_monthly_snapshot() {
        let snap = serde_json::from_str::<UsagesResponse>(NEWER_SHAPE_CAPTURE)
            .unwrap()
            .into_snapshot()
            .unwrap();
        // No weekly bucket on this shape, and nothing fabricates counters.
        assert!(!snap.has_weekly);
        assert_eq!(snap.weekly_limit, 0);
        assert_eq!(snap.weekly_used, 0);
        assert_eq!(snap.weekly_remaining, 0);
        assert_eq!(snap.weekly_reset_at, None);
        // The monthly pool comes from limit_month_total, percent + reset.
        assert_eq!(snap.monthly_pct, Some(0));
        assert_eq!(
            snap.monthly_reset_at.map(|dt| dt.to_rfc3339()),
            Some("2026-10-16T00:00:00+00:00".to_string())
        );
        // The 5h window still comes from limits[], not usages.limit_5h.
        assert_eq!(snap.window_limit, 0);
        assert_eq!(snap.window_used, 0);
        assert_eq!(snap.window_remaining, 0);
        assert_eq!(
            snap.window_reset_at.map(|dt| dt.to_rfc3339()),
            Some("2026-09-16T20:11:32.979529+00:00".to_string())
        );
    }

    #[test]
    fn the_legacy_usage_block_shape_still_has_a_weekly_bucket_and_no_monthly() {
        let raw = r#"{
            "usage": { "limit": "100", "used": "26", "remaining": "74", "resetTime": "2026-02-11T17:32:50Z" }
        }"#;
        let snap = serde_json::from_str::<UsagesResponse>(raw)
            .unwrap()
            .into_snapshot()
            .unwrap();
        assert!(snap.has_weekly);
        assert_eq!(snap.weekly_used, 26);
        assert_eq!(snap.monthly_pct, None);
        assert_eq!(snap.monthly_reset_at, None);
    }

    #[test]
    fn a_usages_map_alongside_the_legacy_block_is_ignored() {
        let raw = r#"{
            "usage": { "limit": "100", "used": "26", "remaining": "74" },
            "usages": { "limit_month_total": { "used_ratio": 0.9, "reset_time": "2026-10-16T00:00:00Z" } }
        }"#;
        let snap = serde_json::from_str::<UsagesResponse>(raw)
            .unwrap()
            .into_snapshot()
            .unwrap();
        assert!(snap.has_weekly);
        assert_eq!(snap.weekly_used, 26);
        assert_eq!(snap.monthly_pct, None);
    }

    #[test]
    fn neither_usage_block_nor_month_total_is_schema_drift() {
        for raw in [
            r#"{"limits": []}"#,
            r#"{"usages": {}}"#,
            r#"{"usages": {"limit_5h": {"used_ratio": 0.5}}}"#,
        ] {
            let err = serde_json::from_str::<UsagesResponse>(raw)
                .unwrap()
                .into_snapshot()
                .unwrap_err();
            assert!(err.to_string().contains("usage block"), "{raw}: {err}");
        }
    }

    #[test]
    fn a_month_total_without_a_ratio_is_schema_drift() {
        let raw = r#"{
            "usages": { "limit_month_total": { "reset_time": "2026-10-16T00:00:00Z" } }
        }"#;
        let err = serde_json::from_str::<UsagesResponse>(raw)
            .unwrap()
            .into_snapshot()
            .unwrap_err();
        assert!(err.to_string().contains("used_ratio"), "{err}");
    }

    #[test]
    fn out_of_range_ratios_are_schema_drift() {
        for ratio in [-0.5, 1.5] {
            let raw =
                format!(r#"{{"usages": {{"limit_month_total": {{"used_ratio": {ratio}}}}}}}"#);
            let err = serde_json::from_str::<UsagesResponse>(&raw)
                .unwrap()
                .into_snapshot()
                .unwrap_err();
            assert!(err.to_string().contains("used_ratio"), "{raw}: {err}");
        }
        // JSON cannot spell NaN or infinity, so those go through the
        // validator directly.
        for ratio in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(monthly_pct(Some(ratio)).is_err(), "{ratio}");
        }
    }

    #[test]
    fn the_ratio_rounds_to_a_percent() {
        assert_eq!(monthly_pct(Some(0.0)).unwrap(), 0);
        assert_eq!(monthly_pct(Some(0.424)).unwrap(), 42);
        assert_eq!(monthly_pct(Some(1.0)).unwrap(), 100);
    }

    /// `limit_month_code` is the Code slice *inside* the combined monthly
    /// pool: never its own allowance, and never added to the total.
    #[test]
    fn the_code_slice_is_never_added_to_the_monthly_pool() {
        let raw = r#"{
            "usages": {
                "limit_month_total": { "used_ratio": 0.4, "reset_time": "2026-10-16T00:00:00Z" },
                "limit_month_code": { "used_ratio": 0.9, "reset_time": "2026-10-16T00:00:00Z" }
            }
        }"#;
        let snap = serde_json::from_str::<UsagesResponse>(raw)
            .unwrap()
            .into_snapshot()
            .unwrap();
        assert_eq!(snap.monthly_pct, Some(40));
        // No field on the snapshot can carry the code slice.
        let rendered = format!("{snap:?}");
        assert!(!rendered.contains("90"), "{rendered}");
    }

    #[test]
    fn the_usages_map_limit_5h_is_never_read() {
        // limit_5h disagrees with limits[] on real accounts, and the website
        // matches limits[] — so the rolling window ignores the map entirely.
        let raw = r#"{
            "limits": [
                {
                    "window": { "duration": 300, "timeUnit": "TIME_UNIT_MINUTE" },
                    "detail": { "limit": "100", "used": "15", "remaining": "85", "resetTime": "2026-09-16T20:11:32Z" }
                }
            ],
            "usages": {
                "limit_5h": { "used_ratio": 0.99, "reset_time": "2026-09-16T22:00:00Z" },
                "limit_month_total": { "used_ratio": 0.4 }
            }
        }"#;
        let snap = serde_json::from_str::<UsagesResponse>(raw)
            .unwrap()
            .into_snapshot()
            .unwrap();
        assert_eq!(snap.window_used, 15);
        assert_eq!(
            snap.window_reset_at.map(|dt| dt.to_rfc3339()),
            Some("2026-09-16T20:11:32+00:00".to_string())
        );
    }
}
