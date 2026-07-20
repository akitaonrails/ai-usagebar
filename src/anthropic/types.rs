//! Wire types for the Anthropic OAuth usage endpoint.
//!
//! Every field is `Option<T>` or has `#[serde(default)]` — the endpoint is
//! undocumented and the shape varies across plan tiers and over time. The
//! lossy `serde(default)` approach matches claudebar's jq pattern of
//! `.field // empty`.

use serde::{Deserialize, Serialize};

use crate::usage::{AnthropicSnapshot, Cents, ExtraUsage, ScopedWindow, UsageWindow};

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
    /// Newer per-limit array. Carries model-scoped weekly windows
    /// (`kind == "weekly_scoped"`, e.g. the Fable weekly cap) that have no
    /// dedicated `seven_day_*` field.
    #[serde(default)]
    pub limits: Vec<LimitEntry>,
}

/// One entry of the `limits[]` array. Only `weekly_scoped` entries with a
/// model display name are lifted into the snapshot; everything else in the
/// array duplicates `five_hour`/`seven_day`.
#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq)]
pub struct LimitEntry {
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default, deserialize_with = "de_percent_opt")]
    pub percent: Option<f64>,
    #[serde(default)]
    pub resets_at: Option<String>,
    #[serde(default)]
    pub scope: Option<LimitScope>,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq)]
pub struct LimitScope {
    #[serde(default)]
    pub model: Option<LimitModel>,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq)]
pub struct LimitModel {
    #[serde(default)]
    pub display_name: Option<String>,
}

/// A single usage window — `utilization` is `0..=100` (integer percent).
#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq)]
pub struct Window {
    #[serde(default, deserialize_with = "de_percent")]
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

/// Slack tolerated above 100. The endpoint occasionally reports a hair over its
/// own cap — `used/limit` rounding, or usage that landed just before the block
/// did — and `to_window` saturates that back to 100.
const PCT_SLACK: f64 = 1.0;

/// Gate a wire percentage into `0..=100` (+ [`PCT_SLACK`]).
///
/// Rejecting here rather than in `to_window` is deliberate: `to_window` is
/// infallible and `into_snapshot` has no error channel, so the parse boundary
/// is the only place a bad value can still become a loud failure. A rejection
/// surfaces as `AppError::Json` and reaches the user as `⚠` — never as a
/// number we invented. Past the slack the field simply isn't a percentage on
/// this scale (rescaled to per-mille, a raw counter, a sentinel), and clamping
/// it would paint a "100%" bar we cannot vouch for. Non-finite values matter
/// most: `f64::NAN as i32` is silently `0`.
fn checked_percent<E: serde::de::Error>(v: f64) -> std::result::Result<f64, E> {
    if !v.is_finite() {
        return Err(E::custom(format!("percentage {v} is not finite")));
    }
    if !(0.0..=100.0 + PCT_SLACK).contains(&v) {
        return Err(E::custom(format!("percentage {v} outside 0..=100")));
    }
    Ok(v)
}

fn de_percent<'de, D>(d: D) -> std::result::Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    checked_percent(f64::deserialize(d)?)
}

fn de_percent_opt<'de, D>(d: D) -> std::result::Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<f64>::deserialize(d)?
        .map(checked_percent)
        .transpose()
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
                // Round to nearest, matching claudebar's `| round` jq filter,
                // then absorb the overshoot `de_percent` deliberately lets
                // through (100.4 → 100) so the bar never renders past full.
                utilization_pct: (w.utilization.round() as i32).clamp(0, 100),
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
        let scoped = self
            .limits
            .into_iter()
            .filter(|l| l.kind.as_deref() == Some("weekly_scoped"))
            .filter_map(|l| {
                let label = l.scope?.model?.display_name?;
                // Same `?` discipline as the label above: an entry without a
                // percentage is dropped, not defaulted. `unwrap_or(0.0)` drew a
                // confident "Fable 0%" bar under a real model name — a number
                // the API never sent, which is worse than no bar at all.
                let utilization = l.percent?;
                let window = to_window(
                    Some(Window {
                        utilization,
                        resets_at: l.resets_at,
                    }),
                    WEEKLY,
                );
                Some(ScopedWindow { label, window })
            })
            .collect();

        AnthropicSnapshot {
            plan: plan_label,
            session,
            weekly,
            sonnet,
            scoped,
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
    fn parses_weekly_scoped_limits() {
        // Real shape observed 2026-07-08: the Fable weekly cap only exists
        // inside `limits[]`; there is no `seven_day_fable` field.
        let raw = r#"{
            "five_hour": {"utilization": 10.0, "resets_at": "2026-07-08T22:59:59Z"},
            "seven_day": {"utilization": 55.0, "resets_at": "2026-07-10T10:59:59Z"},
            "limits": [
                {"kind": "session", "group": "session", "percent": 10,
                 "severity": "normal", "resets_at": "2026-07-08T22:59:59Z",
                 "scope": null, "is_active": false},
                {"kind": "weekly_all", "group": "weekly", "percent": 55,
                 "severity": "normal", "resets_at": "2026-07-10T10:59:59Z",
                 "scope": null, "is_active": false},
                {"kind": "weekly_scoped", "group": "weekly", "percent": 84,
                 "severity": "warning", "resets_at": "2026-07-10T10:59:59Z",
                 "scope": {"model": {"id": null, "display_name": "Fable"}, "surface": null},
                 "is_active": true}
            ]
        }"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        let snap = resp.into_snapshot("Pro".into());
        assert_eq!(snap.scoped.len(), 1);
        assert_eq!(snap.scoped[0].label, "Fable");
        assert_eq!(snap.scoped[0].window.utilization_pct, 84);
        assert!(snap.scoped[0].window.resets_at.is_some());
        // Unscoped entries never duplicate into `scoped`.
        assert_eq!(snap.weekly.utilization_pct, 55);
    }

    #[test]
    fn missing_limits_array_yields_empty_scoped() {
        let raw = r#"{
            "five_hour": {"utilization": 0, "resets_at": "2026-05-23T17:30:00Z"},
            "seven_day": {"utilization": 0, "resets_at": "2026-05-30T12:00:00Z"}
        }"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        let snap = resp.into_snapshot("Pro".into());
        assert!(snap.scoped.is_empty());
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
    fn benign_overshoot_saturates_to_hundred() {
        // A hair over the cap is rounding noise, not drift — it must render as
        // a full bar, not break the widget.
        let raw = r#"{
            "five_hour": {"utilization": 100.4},
            "seven_day": {"utilization": 100.6}
        }"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        let snap = resp.into_snapshot("Pro".into());
        assert_eq!(snap.session.utilization_pct, 100);
        assert_eq!(snap.weekly.utilization_pct, 100); // rounds to 101, saturated
    }

    #[test]
    fn out_of_range_utilization_is_rejected_not_clamped() {
        for raw in [
            r#"{"five_hour": {"utilization": 500}}"#,
            r#"{"five_hour": {"utilization": -1}}"#,
            r#"{"seven_day": {"utilization": 101.5}}"#,
        ] {
            let err = serde_json::from_str::<UsageResponse>(raw).unwrap_err();
            assert!(err.to_string().contains("outside 0..=100"), "{raw}: {err}");
        }
    }

    #[test]
    fn out_of_range_scoped_percent_is_rejected() {
        // `limits[].percent` reaches the same bar via the synthesized Window.
        let raw = r#"{
            "limits": [{"kind": "weekly_scoped", "percent": 420,
                        "scope": {"model": {"display_name": "Fable"}}}]
        }"#;
        let err = serde_json::from_str::<UsageResponse>(raw).unwrap_err();
        assert!(err.to_string().contains("outside 0..=100"), "{err}");
    }

    #[test]
    fn non_finite_percentage_is_rejected() {
        // `f64::NAN as i32` is silently 0 — the fabricated number this gate
        // exists to prevent. JSON has no NaN literal, so drive it directly.
        for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(checked_percent::<serde_json::Error>(v).is_err(), "{v}");
        }
    }

    #[test]
    fn scoped_limit_without_a_percentage_is_dropped_not_zeroed() {
        // The gate rejects impossible numbers; an absent one is not a parse
        // failure. But it must not become a bar either: `unwrap_or(0.0)` drew a
        // confident "Fable 0%" under a real model name that the API never sent.
        let raw = r#"{
            "limits": [{"kind": "weekly_scoped", "percent": null,
                        "scope": {"model": {"display_name": "Fable"}}}]
        }"#;
        let resp: UsageResponse = serde_json::from_str(raw).unwrap();
        let snap = resp.into_snapshot("Pro".into());
        assert!(
            snap.scoped.is_empty(),
            "a scoped limit with no percentage must not render a fabricated bar"
        );

        // A real percentage still produces the window, so the drop is targeted.
        let ok = r#"{
            "limits": [{"kind": "weekly_scoped", "percent": 84,
                        "scope": {"model": {"display_name": "Fable"}}}]
        }"#;
        let resp: UsageResponse = serde_json::from_str(ok).unwrap();
        let snap = resp.into_snapshot("Pro".into());
        assert_eq!(snap.scoped[0].label, "Fable");
        assert_eq!(snap.scoped[0].window.utilization_pct, 84);
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
