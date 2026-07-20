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

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct Window {
    #[serde(deserialize_with = "de_int_or_float_lenient")]
    pub used_percent: i64,
    #[serde(deserialize_with = "de_int_or_float_lenient")]
    pub limit_window_seconds: i64,
    /// Unix seconds. May be absent on older Codex CLIs.
    #[serde(default, deserialize_with = "de_opt_int_or_float")]
    pub reset_at: Option<i64>,
    /// Fallback when `reset_at` is absent. Unix seconds offset from "now".
    #[serde(default, deserialize_with = "de_opt_int_or_float")]
    pub reset_after_seconds: Option<i64>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct CreditsBlock {
    #[serde(deserialize_with = "de_money_string")]
    pub balance: String,
    pub has_credits: bool,
    pub unlimited: bool,
    #[serde(default)]
    pub approx_local_messages: Option<Vec<i64>>,
    #[serde(default)]
    pub approx_cloud_messages: Option<Vec<i64>>,
}

/// Accept a JSON int or float (the API returns these counters as either),
/// plus a numeric string — the same value in a third coat of paint. Anything
/// else (null, bool, array, object, unparseable string) is schema drift and
/// must fail the parse: `fetch_usage` validates before writing the cache, so a
/// fabricated `0` here would be persisted and rendered as a real 0% bar.
fn de_int_or_float_lenient<'de, D>(d: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = serde_json::Value::deserialize(d)?;
    match v {
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i)
            } else if let Some(i) = n.as_f64().and_then(exact_i64) {
                Ok(i)
            } else {
                Err(serde::de::Error::custom("number out of i64 range"))
            }
        }
        serde_json::Value::String(s) => s
            .parse::<i64>()
            .ok()
            .or_else(|| s.parse::<f64>().ok().and_then(exact_i64))
            .ok_or_else(|| serde::de::Error::custom(format!("expected numeric string, got {s:?}"))),
        other => Err(serde::de::Error::custom(format!(
            "expected number or numeric string, got {other:?}"
        ))),
    }
}

/// `f as i64` saturates instead of failing, so `NaN` would coin a `0` and
/// `1e300` an `i64::MAX` — both indistinguishable from a counter the API
/// really sent. Only a magnitude the cast reproduces exactly survives.
fn exact_i64(f: f64) -> Option<i64> {
    // `i64::MAX as f64` rounds up to 2^63, hence the strict upper bound;
    // `i64::MIN as f64` is exact, so its bound is inclusive.
    if f.is_finite() && f >= i64::MIN as f64 && f < i64::MAX as f64 {
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
    Ok(match v {
        serde_json::Value::Null => None,
        serde_json::Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        _ => None,
    })
}

/// Accept either a string ("$0.00") or a number (0.0) — codexbar treats both.
fn de_money_string<'de, D>(d: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::String(s) => s,
        serde_json::Value::Number(n) => format!("${:.2}", n.as_f64().unwrap_or(0.0)),
        _ => "$0.00".to_string(),
    })
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
            balance: c.balance,
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
        utilization_pct: (w.used_percent as i32).clamp(0, 100),
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
    fn used_percent_clamps_to_hundred() {
        let body =
            r#"{"rate_limit":{"primary_window":{"used_percent":250,"limit_window_seconds":1}}}"#;
        let r: UsageResponse = serde_json::from_str(body).unwrap();
        let s = r.into_snapshot(None);
        assert_eq!(s.session.utilization_pct, 100);
    }

    #[test]
    fn plan_hint_used_when_response_omits_plan_type() {
        let r: UsageResponse = serde_json::from_str("{}").unwrap();
        let s = r.into_snapshot(Some("team"));
        assert_eq!(s.plan, "ChatGPT Team");
    }

    #[test]
    fn window_counters_accept_int_float_and_numeric_string() {
        let w: Window =
            serde_json::from_str(r#"{"used_percent":7,"limit_window_seconds":18000.9}"#).unwrap();
        assert_eq!(w.used_percent, 7);
        assert_eq!(w.limit_window_seconds, 18000);

        let w: Window =
            serde_json::from_str(r#"{"used_percent":"42","limit_window_seconds":"604800.5"}"#)
                .unwrap();
        assert_eq!(w.used_percent, 42);
        assert_eq!(w.limit_window_seconds, 604800);
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
    fn omitted_counters_still_default_to_zero() {
        // Deliberate boundary: an *absent* counter keeps the struct-level
        // `#[serde(default)]`, matching the optional `code_review_rate_limit`
        // blocks. Only a present-but-wrong value counts as drift.
        let w: Window = serde_json::from_str("{}").unwrap();
        assert_eq!(w.used_percent, 0);
        assert_eq!(w.limit_window_seconds, 0);
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
