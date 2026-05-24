//! Wire types for the Anthropic OAuth usage endpoint.
//!
//! Every field is `Option<T>` or has `#[serde(default)]` — the endpoint is
//! undocumented and the shape varies across plan tiers and over time. The
//! lossy `serde(default)` approach matches claudebar's jq pattern of
//! `.field // empty`.

use serde::{Deserialize, Serialize};

use crate::usage::{AnthropicSnapshot, Cents, ExtraUsage, UsageWindow};

/// Top-level response from `GET /api/oauth/usage`.
#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq)]
pub struct UsageResponse {
    #[serde(default)]
    pub five_hour: Option<Window>,
    #[serde(default)]
    pub seven_day: Option<Window>,
    #[serde(default)]
    pub seven_day_sonnet: Option<Window>,
    #[serde(default)]
    pub extra_usage: Option<ExtraUsageBlock>,
}

/// A single usage window — `utilization` is `0..=100` (integer percent).
#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq)]
pub struct Window {
    #[serde(default)]
    pub utilization: f64,
    #[serde(default)]
    pub resets_at: Option<String>,
}

/// Pay-as-you-go extra usage. Both money values are integer cents, but the
/// API sometimes returns them as floats (e.g. `0.0`) so we accept either.
#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq)]
pub struct ExtraUsageBlock {
    #[serde(default)]
    pub is_enabled: bool,
    #[serde(default, deserialize_with = "de_int_or_float")]
    pub monthly_limit: i64,
    #[serde(default, deserialize_with = "de_int_or_float")]
    pub used_credits: i64,
}

/// Accept JSON int or float, truncating floats. Mirrors claudebar's
/// `(.field // 0) | floor` jq pattern.
fn de_int_or_float<'de, D>(d: D) -> std::result::Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = serde_json::Value::deserialize(d)?;
    match v {
        serde_json::Value::Null => Ok(0),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i)
            } else if let Some(f) = n.as_f64() {
                Ok(f as i64)
            } else {
                Err(serde::de::Error::custom("number out of i64 range"))
            }
        }
        other => Err(serde::de::Error::custom(format!(
            "expected number or null, got {other:?}"
        ))),
    }
}

impl UsageResponse {
    /// Lift the wire response into our canonical [`AnthropicSnapshot`].
    ///
    /// `plan_label` is the rendered plan name ("Max 5x" etc.), derived from
    /// the credentials file (since the usage endpoint doesn't include it).
    pub fn into_snapshot(self, plan_label: String) -> AnthropicSnapshot {
        // Window durations are constants per claudebar:172-173.
        const SESSION: chrono::Duration = chrono::Duration::hours(5);
        const WEEKLY: chrono::Duration = chrono::Duration::days(7);

        fn to_window(w: Option<Window>, dur: chrono::Duration) -> UsageWindow {
            let Some(w) = w else {
                return UsageWindow {
                    utilization_pct: 0,
                    resets_at: None,
                    window_duration: dur,
                };
            };
            UsageWindow {
                // Round to nearest, matching claudebar's `| round` jq filter.
                utilization_pct: w.utilization.round() as i32,
                resets_at: w
                    .resets_at
                    .as_deref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.with_timezone(&chrono::Utc)),
                window_duration: dur,
            }
        }

        let session = to_window(self.five_hour, SESSION);
        let weekly = to_window(self.seven_day, WEEKLY);
        let sonnet = self.seven_day_sonnet.map(|w| to_window(Some(w), WEEKLY));
        let extra = self
            .extra_usage
            .filter(|e| e.is_enabled)
            .map(|e| ExtraUsage {
                limit: Cents(e.monthly_limit),
                spent: Cents(e.used_credits),
            });

        AnthropicSnapshot {
            plan: plan_label,
            session,
            weekly,
            sonnet,
            extra,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_response() {
        let raw = r#"{
            "five_hour":         {"utilization": 42.7, "resets_at": "2026-05-23T17:30:00Z"},
            "seven_day":         {"utilization": 27.0, "resets_at": "2026-05-30T12:00:00Z"},
            "seven_day_sonnet":  {"utilization":  4.2, "resets_at": "2026-05-30T12:00:00Z"},
            "extra_usage":       {"is_enabled": true, "monthly_limit": 5000, "used_credits": 250}
        }"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        let snap = resp.into_snapshot("Max 5x".into());
        assert_eq!(snap.session.utilization_pct, 43); // rounded
        assert_eq!(snap.weekly.utilization_pct, 27);
        assert_eq!(snap.sonnet.as_ref().unwrap().utilization_pct, 4);
        assert_eq!(snap.extra.unwrap().limit.0, 5000);
        assert_eq!(snap.extra.unwrap().spent.0, 250);
        assert!(snap.session.resets_at.is_some());
    }

    #[test]
    fn missing_sonnet_and_extra_are_none() {
        let raw = r#"{
            "five_hour": {"utilization": 0, "resets_at": "2026-05-23T17:30:00Z"},
            "seven_day": {"utilization": 0, "resets_at": "2026-05-30T12:00:00Z"}
        }"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        let snap = resp.into_snapshot("Pro".into());
        assert!(snap.sonnet.is_none());
        assert!(snap.extra.is_none());
    }

    #[test]
    fn disabled_extra_usage_becomes_none() {
        let raw = r#"{
            "five_hour": {"utilization": 0},
            "seven_day": {"utilization": 0},
            "extra_usage": {"is_enabled": false, "monthly_limit": 5000, "used_credits": 0}
        }"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        let snap = resp.into_snapshot("Pro".into());
        assert!(snap.extra.is_none());
    }

    #[test]
    fn empty_object_yields_neutral_snapshot() {
        let resp: UsageResponse = serde_json::from_str("{}").unwrap();
        let snap = resp.into_snapshot("Unknown".into());
        assert_eq!(snap.session.utilization_pct, 0);
        assert_eq!(snap.weekly.utilization_pct, 0);
        assert!(snap.session.resets_at.is_none());
    }

    #[test]
    fn unparseable_reset_becomes_none() {
        let raw = r#"{
            "five_hour": {"utilization": 50, "resets_at": "not a date"},
            "seven_day": {"utilization": 0}
        }"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        let snap = resp.into_snapshot("Pro".into());
        assert!(snap.session.resets_at.is_none());
        assert_eq!(snap.session.utilization_pct, 50);
    }
}
