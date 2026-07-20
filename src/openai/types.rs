//! Wire types for `GET https://chatgpt.com/backend-api/wham/usage`.
//!
//! Reverse-engineered from `~/Projects/codexbar/codexbar` and the official
//! `openai/codex` Rust client. Real captured shape (2026-05-23):
//!
//! ```json
//! {
//!   "user_id": "...", "account_id": "...", "email": "...",
//!   "plan_type": "plus",
//!   "rate_limit": {
//!     "allowed": true, "limit_reached": false,
//!     "primary_window":   {"used_percent": 1, "limit_window_seconds": 18000, "reset_at": 1779597324},
//!     "secondary_window": {"used_percent": 0, "limit_window_seconds": 604800, "reset_at": 1780184124}
//!   },
//!   "code_review_rate_limit": {...optional...},
//!   "credits": {...optional...}
//! }
//! ```

use serde::Deserialize;

use crate::usage::{OpenAiCredits, OpenAiSnapshot, OpenAiSource, UsageWindow};

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct UsageResponse {
    pub plan_type: Option<String>,
    pub rate_limit: Option<RateLimit>,
    pub code_review_rate_limit: Option<RateLimit>,
    pub credits: Option<CreditsBlock>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct RateLimit {
    pub primary_window: Option<Window>,
    pub secondary_window: Option<Window>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Window {
    #[serde(deserialize_with = "de_percent_number_or_string")]
    pub used_percent: f64,
    #[serde(deserialize_with = "de_i64_number_or_string")]
    pub limit_window_seconds: i64,
    /// Unix seconds. May be absent on older Codex CLIs.
    #[serde(default, deserialize_with = "de_opt_int_or_float")]
    pub reset_at: Option<i64>,
    /// Fallback when `reset_at` is absent. Unix seconds offset from "now".
    #[serde(default, deserialize_with = "de_opt_int_or_float")]
    pub reset_after_seconds: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreditsBlock {
    #[serde(default, deserialize_with = "de_opt_money_string")]
    pub balance: Option<String>,
    pub has_credits: bool,
    pub unlimited: bool,
    #[serde(default)]
    pub approx_local_messages: Option<Vec<i64>>,
    #[serde(default)]
    pub approx_cloud_messages: Option<Vec<i64>>,
}

/// Accept a JSON number or numeric string without turning malformed, non-finite
/// or out-of-range values into plausible counters. `fetch_usage` validates
/// before writing the cache, so a fabricated value here would be persisted and
/// rendered as genuine usage.
fn numeric_value<E: serde::de::Error>(v: serde_json::Value) -> Result<f64, E> {
    let value = match v {
        serde_json::Value::Number(n) => n
            .as_f64()
            .ok_or_else(|| E::custom("number is not representable as f64"))?,
        serde_json::Value::String(s) => s
            .parse::<f64>()
            .map_err(|_| E::custom(format!("expected numeric string, got {s:?}")))?,
        other => {
            return Err(E::custom(format!(
                "expected number or numeric string, got {other:?}"
            )));
        }
    };
    if value.is_finite() {
        Ok(value)
    } else {
        Err(E::custom("number is not finite"))
    }
}

fn de_percent_number_or_string<'de, D>(d: D) -> Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = serde_json::Value::deserialize(d)?;
    let value = numeric_value::<D::Error>(v)?;
    if (0.0..=101.0).contains(&value) {
        Ok(value)
    } else {
        Err(serde::de::Error::custom(format!(
            "percentage {value} outside 0..=100"
        )))
    }
}

fn de_i64_number_or_string<'de, D>(d: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    i64_value(serde_json::Value::deserialize(d)?)
}

fn i64_value<E: serde::de::Error>(v: serde_json::Value) -> Result<i64, E> {
    match &v {
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                return Ok(i);
            }
        }
        serde_json::Value::String(s) => {
            if let Ok(i) = s.parse::<i64>() {
                return Ok(i);
            }
        }
        _ => {}
    }
    exact_i64(numeric_value::<E>(v)?).ok_or_else(|| E::custom("expected an integer in i64 range"))
}

/// `f as i64` saturates instead of failing, so `NaN` would coin a `0` and
/// `1e300` an `i64::MAX` — both indistinguishable from a counter the API
/// really sent. Only an integral magnitude that an `f64` represents exactly
/// survives; timestamps and window lengths cannot silently lose a fraction or
/// low bit. Plain JSON/string integers take the exact `i64` path above.
fn exact_i64(f: f64) -> Option<i64> {
    const MAX_EXACT_F64_INT: f64 = (1_u64 << 53) as f64;
    if f.is_finite() && f.trunc() == f && f.abs() <= MAX_EXACT_F64_INT {
        Some(f as i64)
    } else {
        None
    }
}

fn de_opt_int_or_float<'de, D>(d: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = serde_json::Value::deserialize(d)?;
    if v.is_null() {
        Ok(None)
    } else {
        i64_value::<D::Error>(v).map(Some)
    }
}

/// Accept either a string ("$0.00") or a finite number (0.0) — codexbar
/// treats both. Null and an omitted field mean that no balance was supplied.
fn de_opt_money_string<'de, D>(d: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = serde_json::Value::deserialize(d)?;
    match v {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(s) => Ok(Some(s)),
        serde_json::Value::Number(n) => match n.as_f64() {
            Some(value) if value.is_finite() => Ok(Some(format!("${value:.2}"))),
            _ => Err(serde::de::Error::custom(
                "credit balance is not a finite number",
            )),
        },
        other => Err(serde::de::Error::custom(format!(
            "expected credit balance string, number, or null; got {other:?}"
        ))),
    }
}

impl UsageResponse {
    pub fn into_snapshot(self, plan_hint: Option<&str>) -> OpenAiSnapshot {
        let plan_type = self.plan_type.as_deref().or(plan_hint).unwrap_or("Unknown");
        let plan = format!("ChatGPT {}", capitalize(plan_type));

        let rl = self.rate_limit.unwrap_or_default();
        let session = window_or_default(rl.primary_window, chrono::Duration::hours(5));
        let weekly = window_or_default(rl.secondary_window, chrono::Duration::days(7));
        let code_review = self
            .code_review_rate_limit
            .and_then(|c| c.primary_window)
            .map(|w| to_window(&w, chrono::Duration::days(7)));

        let credits = self.credits.map(|c| OpenAiCredits {
            balance: c.balance.unwrap_or_default(),
            has_credits: c.has_credits,
            unlimited: c.unlimited,
            approx_local_messages: range_from_vec(c.approx_local_messages),
            approx_cloud_messages: range_from_vec(c.approx_cloud_messages),
        });

        OpenAiSnapshot {
            plan,
            session,
            weekly,
            code_review,
            credits,
            source: OpenAiSource::CodexOauth,
        }
    }
}

fn window_or_default(w: Option<Window>, default_dur: chrono::Duration) -> UsageWindow {
    let Some(w) = w else {
        return UsageWindow {
            utilization_pct: 0,
            resets_at: None,
            window_duration: default_dur,
        };
    };
    to_window(&w, default_dur)
}

fn to_window(w: &Window, default_dur: chrono::Duration) -> UsageWindow {
    // `Duration::seconds` panics past ~1e16, and the widget must always exit 0
    // — so an absurd counter degrades to the caller's default, never a crash.
    let dur = match chrono::Duration::try_seconds(w.limit_window_seconds) {
        Some(d) if w.limit_window_seconds > 0 => d,
        _ => default_dur,
    };
    let resets_at = match w.reset_at {
        Some(secs) => chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0),
        None => w
            .reset_after_seconds
            .and_then(chrono::Duration::try_seconds)
            .and_then(|d| chrono::Utc::now().checked_add_signed(d)),
    };
    UsageWindow {
        utilization_pct: (w.used_percent.round() as i32).clamp(0, 100),
        resets_at,
        window_duration: dur,
    }
}

fn range_from_vec(v: Option<Vec<i64>>) -> Option<(i64, i64)> {
    let v = v?;
    if v.len() >= 2 {
        Some((v[0], v[1]))
    } else if v.len() == 1 {
        Some((v[0], v[0]))
    } else {
        None
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => {
            let mut out = String::with_capacity(s.len());
            for u in c.to_uppercase() {
                out.push(u);
            }
            out.push_str(chars.as_str());
            out
        }
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL: &str = r#"{
        "user_id":"u","account_id":"a","email":"e",
        "plan_type":"plus",
        "rate_limit":{"allowed":true,"limit_reached":false,
            "primary_window":{"used_percent":1,"limit_window_seconds":18000,"reset_after_seconds":18000,"reset_at":1779597324},
            "secondary_window":{"used_percent":0,"limit_window_seconds":604800,"reset_after_seconds":604800,"reset_at":1780184124}
        }
    }"#;

    #[test]
    fn parses_real_shape() {
        let r: UsageResponse = serde_json::from_str(REAL).unwrap();
        let s = r.into_snapshot(None);
        assert_eq!(s.plan, "ChatGPT Plus");
        assert_eq!(s.session.utilization_pct, 1);
        assert_eq!(s.weekly.utilization_pct, 0);
        assert_eq!(s.session.window_duration, chrono::Duration::hours(5));
        assert_eq!(s.weekly.window_duration, chrono::Duration::days(7));
        assert!(s.session.resets_at.is_some());
        assert!(s.code_review.is_none());
        assert!(s.credits.is_none());
        assert!(matches!(s.source, OpenAiSource::CodexOauth));
    }

    #[test]
    fn missing_rate_limit_yields_neutral() {
        let r: UsageResponse = serde_json::from_str(r#"{"plan_type":"pro"}"#).unwrap();
        let s = r.into_snapshot(None);
        assert_eq!(s.plan, "ChatGPT Pro");
        assert_eq!(s.session.utilization_pct, 0);
        assert_eq!(s.weekly.utilization_pct, 0);
    }

    #[test]
    fn credits_block_parses_with_message_ranges() {
        let body = r#"{
            "plan_type":"plus",
            "credits":{"balance":"$2.50","has_credits":true,"unlimited":false,
                "approx_local_messages":[100,200],"approx_cloud_messages":[40,60]}
        }"#;
        let r: UsageResponse = serde_json::from_str(body).unwrap();
        let s = r.into_snapshot(None);
        let c = s.credits.unwrap();
        assert_eq!(c.balance, "$2.50");
        assert!(c.has_credits);
        assert_eq!(c.approx_local_messages, Some((100, 200)));
        assert_eq!(c.approx_cloud_messages, Some((40, 60)));
    }

    #[test]
    fn balance_as_number_formats_to_dollars() {
        let body = r#"{"credits":{"balance":1.5,"has_credits":true,"unlimited":false}}"#;
        let r: UsageResponse = serde_json::from_str(body).unwrap();
        let s = r.into_snapshot(None);
        assert_eq!(s.credits.unwrap().balance, "$1.50");
    }

    #[test]
    fn benign_percent_overshoot_clamps_to_hundred() {
        let body =
            r#"{"rate_limit":{"primary_window":{"used_percent":100.6,"limit_window_seconds":1}}}"#;
        let r: UsageResponse = serde_json::from_str(body).unwrap();
        let s = r.into_snapshot(None);
        assert_eq!(s.session.utilization_pct, 100);
    }

    #[test]
    fn out_of_range_percent_is_schema_drift() {
        for used_percent in ["-1", "101.5", "250"] {
            let body = format!(
                r#"{{"rate_limit":{{"primary_window":{{"used_percent":{used_percent},"limit_window_seconds":1}}}}}}"#
            );
            assert!(
                serde_json::from_str::<UsageResponse>(&body).is_err(),
                "{used_percent} must not become a clamped usage value"
            );
        }
    }

    #[test]
    fn plan_hint_used_when_response_omits_plan_type() {
        let r: UsageResponse = serde_json::from_str("{}").unwrap();
        let s = r.into_snapshot(Some("team"));
        assert_eq!(s.plan, "ChatGPT Team");
    }

    #[test]
    fn window_counters_accept_fractional_percent_and_integral_number_forms() {
        let w: Window =
            serde_json::from_str(r#"{"used_percent":7.4,"limit_window_seconds":18000.0}"#).unwrap();
        assert_eq!(w.used_percent, 7.4);
        assert_eq!(w.limit_window_seconds, 18000);

        let w: Window =
            serde_json::from_str(r#"{"used_percent":"42.7","limit_window_seconds":"604800.0"}"#)
                .unwrap();
        assert_eq!(w.used_percent, 42.7);
        assert_eq!(w.limit_window_seconds, 604800);

        let r: UsageResponse = serde_json::from_str(
            r#"{"rate_limit":{"primary_window":{"used_percent":42.7,"limit_window_seconds":18000}}}"#,
        )
        .unwrap();
        assert_eq!(r.into_snapshot(None).session.utilization_pct, 43);
    }

    #[test]
    fn fractional_integer_counters_are_schema_drift() {
        for value in ["18000.9", r#""604800.5""#] {
            let body = format!(r#"{{"used_percent":7,"limit_window_seconds":{value}}}"#);
            assert!(serde_json::from_str::<Window>(&body).is_err(), "{value}");
        }
    }

    #[test]
    fn null_counter_is_schema_drift() {
        // `reset_at` is the only field the API is documented to omit, and it
        // carries its own Option deserializer. A null counter is drift.
        let body = r#"{"used_percent":null,"limit_window_seconds":1}"#;
        assert!(serde_json::from_str::<Window>(body).is_err());
        let body = r#"{"used_percent":1,"limit_window_seconds":null}"#;
        assert!(serde_json::from_str::<Window>(body).is_err());
    }

    #[test]
    fn non_numeric_counter_shapes_are_schema_drift() {
        for bad in [
            r#""many""#,
            r#"{"value":1}"#,
            "[1]",
            "true",
            // Each parses as an f64, but `as i64` saturates rather than
            // failing, so it would coin a 0 / i64::MAX that reads as real.
            r#""NaN""#,
            r#""inf""#,
            "1e300",
            "-1e300",
            r#""1e300""#,
        ] {
            let body = format!(r#"{{"used_percent":{bad},"limit_window_seconds":1}}"#);
            assert!(
                serde_json::from_str::<Window>(&body).is_err(),
                "used_percent {bad} must not deserialize"
            );
        }
    }

    #[test]
    fn drifted_counter_fails_whole_usage_response() {
        // The error has to reach `parse_payload` so the widget shows `⚠`
        // rather than caching and rendering a 0% bar.
        let body = r#"{"plan_type":"plus","rate_limit":{
            "primary_window":{"used_percent":"n/a","limit_window_seconds":18000}
        }}"#;
        assert!(serde_json::from_str::<UsageResponse>(body).is_err());
    }

    #[test]
    fn a_present_window_requires_both_counters() {
        for body in [
            r#"{"used_percent":1}"#,
            r#"{"limit_window_seconds":18000}"#,
            "{}",
        ] {
            assert!(serde_json::from_str::<Window>(body).is_err(), "{body}");
        }
    }

    #[test]
    fn malformed_optional_counters_are_not_treated_as_absent() {
        for field in ["reset_at", "reset_after_seconds"] {
            for bad in ["true", r#""tomorrow""#, "1.5", "{}"] {
                let body =
                    format!(r#"{{"used_percent":1,"limit_window_seconds":18000,"{field}":{bad}}}"#);
                assert!(serde_json::from_str::<Window>(&body).is_err(), "{body}");
            }
        }
    }

    #[test]
    fn credits_reject_invalid_present_values_without_inventing_zero() {
        for balance in ["true", "{}", "[]"] {
            let body = format!(
                r#"{{"credits":{{"balance":{balance},"has_credits":true,"unlimited":false}}}}"#
            );
            assert!(serde_json::from_str::<UsageResponse>(&body).is_err());
        }
        assert!(
            serde_json::from_str::<UsageResponse>(r#"{"credits":{"balance":"$1.00"}}"#).is_err(),
            "a present credits block must not default its status flags"
        );

        let response: UsageResponse = serde_json::from_str(
            r#"{"credits":{"balance":null,"has_credits":false,"unlimited":true}}"#,
        )
        .unwrap();
        assert_eq!(response.into_snapshot(None).credits.unwrap().balance, "");
    }

    #[test]
    fn oversized_window_seconds_degrades_instead_of_panicking() {
        // i64::MAX is a faithful integer, so it clears the deserializer — but
        // `chrono::Duration::seconds` panics on it, and a panicking widget
        // exits non-zero and gets hidden by Waybar.
        let body = r#"{"rate_limit":{"primary_window":{
            "used_percent":1,"limit_window_seconds":9223372036854775807,
            "reset_after_seconds":9223372036854775807
        }}}"#;
        let r: UsageResponse = serde_json::from_str(body).unwrap();
        let s = r.into_snapshot(None);
        assert_eq!(s.session.window_duration, chrono::Duration::hours(5));
        assert!(s.session.resets_at.is_none());
    }

    #[test]
    fn missing_reset_at_falls_back_to_after_seconds() {
        let body = r#"{"rate_limit":{"primary_window":{
            "used_percent":50,"limit_window_seconds":1000,"reset_after_seconds":500
        }}}"#;
        let r: UsageResponse = serde_json::from_str(body).unwrap();
        let s = r.into_snapshot(None);
        // The reset should be ~500s from now (within tolerance).
        let now = chrono::Utc::now();
        let reset = s.session.resets_at.unwrap();
        let delta = reset.signed_duration_since(now).num_seconds();
        assert!((400..=600).contains(&delta), "got delta={delta}");
    }
}
