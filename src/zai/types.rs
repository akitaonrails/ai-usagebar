//! Wire types for the undocumented Z.AI / BigModel monitor endpoint
//! `https://api.z.ai/api/monitor/usage/quota/limit`.
//!
//! Real response shape (captured 2026-05-23):
//!
//! ```json
//! {
//!   "code": 200,
//!   "msg": "Operation successful",
//!   "data": {
//!     "limits": [
//!       {"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":0},
//!       {"type":"TOKENS_LIMIT","unit":6,"number":1,"percentage":0,
//!        "nextResetTime":1779792169974},
//!       {"type":"TIME_LIMIT","unit":5,"number":1,"usage":1000,
//!        "currentValue":0,"remaining":1000,"percentage":0,
//!        "nextResetTime":1779964969979,
//!        "usageDetails":[{"modelCode":"search-prime","usage":0},...]}
//!     ],
//!     "level":"pro"
//!   },
//!   "success": true
//! }
//! ```
//!
//! The `unit`/`number` codes have no documented mapping, but `unit` is the one
//! field that tells the two TOKENS_LIMIT buckets apart independently of where
//! they sit in the array: the 5h window carries `unit:3`, the 7d one `unit:6`.
//! So the session/weekly split keys off `unit`, not off position — Z.AI is free
//! to reorder `limits` or insert a bucket, and a positional split would silently
//! swap the two windows or promote a stranger to "session". A layout we cannot
//! name is an error via [`Envelope::check_ok`], never a guess. The TIME_LIMIT
//! entry is the monthly MCP tool ceiling.

use serde::Deserialize;

use crate::usage::{UsageWindow, ZaiSnapshot};

#[derive(Debug, Clone, Deserialize)]
pub struct Envelope {
    #[serde(default)]
    pub code: i64,
    #[serde(default)]
    pub data: Option<MonitorData>,
    #[serde(default)]
    pub success: bool,
    #[serde(default)]
    pub msg: String,
}

impl Envelope {
    /// Z.AI signals failure *inside* a 200 response: `success: false` with a
    /// non-200 `code` and the reason in `msg`, and `data: null`. Without this
    /// check such a body deserializes cleanly, overwrites a good cache, clears
    /// the previous error, and renders as an unknown plan with empty windows —
    /// indistinguishable from a real account with no usage.
    ///
    /// `code` is accepted when absent (0) or 200; anything else is a failure.
    pub fn check_ok(&self) -> crate::error::Result<()> {
        if !self.success || (self.code != 0 && self.code != 200) {
            let msg = if self.msg.is_empty() {
                "no message".to_string()
            } else {
                self.msg.clone()
            };
            return Err(crate::error::AppError::Schema(format!(
                "zai: API reported failure (code {}, success {}): {msg}",
                self.code, self.success
            )));
        }
        let Some(data) = &self.data else {
            return Err(crate::error::AppError::Schema(
                "zai: success response carried no `data`".into(),
            ));
        };
        // A body whose token buckets we cannot name is drift, not usage: let it
        // through and the widget would render one window's figure under the
        // other's label, and cache it as if it were vouched for.
        if let Err(why) = classify_token_buckets(&data.limits) {
            return Err(crate::error::AppError::Schema(format!(
                "zai: unrecognised limits layout: {why}"
            )));
        }
        Ok(())
    }
}

/// `unit` codes of the two TOKENS_LIMIT buckets in the captured response. The
/// enum behind them is undocumented — `number` (5 and 1) is consistent with
/// "5 hours" / "1 week", but we don't lean on that, so the window durations
/// stay hardcoded and only the *identity* of each bucket comes from `unit`.
const UNIT_SESSION: i64 = 3;
const UNIT_WEEKLY: i64 = 6;

type TokenBuckets<'a> = (Option<&'a LimitEntry>, Option<&'a LimitEntry>);

/// Match the TOKENS_LIMIT entries to the (session, weekly) windows by `unit`.
///
/// Buckets carrying an unknown `unit` are dropped rather than shown under a
/// label we can't justify, so Z.AI adding a third window is inert here. `Err`
/// is reserved for layouts where guessing would be the only alternative:
/// duplicate codes, or unit-bearing buckets none of which we recognise.
fn classify_token_buckets(limits: &[LimitEntry]) -> Result<TokenBuckets<'_>, String> {
    let tokens: Vec<&LimitEntry> = limits.iter().filter(|l| l.kind == "TOKENS_LIMIT").collect();

    // Bodies predating the `unit` field (and caches written from them) have no
    // discriminator at all; position is all there ever was for those.
    if tokens.iter().all(|l| l.unit.is_none()) {
        let mut it = tokens.into_iter();
        let session = it.next();
        let weekly = it.next();
        return Ok((session, weekly));
    }

    let session = unique_by_unit(&tokens, UNIT_SESSION)?;
    let weekly = unique_by_unit(&tokens, UNIT_WEEKLY)?;
    if session.is_none() && weekly.is_none() {
        let seen: Vec<String> = tokens
            .iter()
            .filter_map(|l| l.unit)
            .map(|u| u.to_string())
            .collect();
        return Err(format!(
            "no TOKENS_LIMIT bucket carries a known unit code (saw {})",
            seen.join(", ")
        ));
    }
    Ok((session, weekly))
}

fn unique_by_unit<'a>(
    tokens: &[&'a LimitEntry],
    code: i64,
) -> Result<Option<&'a LimitEntry>, String> {
    let mut matching = tokens.iter().filter(|l| l.unit == Some(code));
    let first = matching.next().copied();
    if matching.next().is_some() {
        return Err(format!("two TOKENS_LIMIT buckets carry unit {code}"));
    }
    Ok(first)
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct MonitorData {
    pub limits: Vec<LimitEntry>,
    pub level: String,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct LimitEntry {
    #[serde(rename = "type")]
    pub kind: String,
    pub percentage: f64,
    /// Unix milliseconds — `null` / `0` / missing → None.
    #[serde(rename = "nextResetTime", default, deserialize_with = "de_opt_ms")]
    pub next_reset_time: Option<i64>,
    pub unit: Option<i64>,
    pub number: Option<i64>,
}

fn de_opt_ms<'de, D>(d: D) -> Result<Option<i64>, D::Error>
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

impl Envelope {
    /// Project the envelope into the canonical [`ZaiSnapshot`]. Returns a
    /// snapshot with all windows `None` when `data` is missing.
    pub fn into_snapshot(self, config_plan_tier: Option<&str>) -> ZaiSnapshot {
        let data = self.data.unwrap_or_default();
        // On the fetch path `check_ok` has already turned an unnameable layout
        // into an error; direct callers get empty windows for the same reason.
        let (session, weekly) = classify_token_buckets(&data.limits).unwrap_or((None, None));
        let session = session.map(|l| to_window(l, chrono::Duration::hours(5)));
        let weekly = weekly.map(|l| to_window(l, chrono::Duration::days(7)));
        let mcp = data
            .limits
            .iter()
            .find(|l| l.kind == "TIME_LIMIT")
            .map(|l| to_window(l, chrono::Duration::days(30)));

        // Prefer the response's `level` field, then any config-provided tier.
        let level = if !data.level.is_empty() {
            data.level
        } else {
            config_plan_tier.unwrap_or("unknown").to_string()
        };
        let plan = format!("GLM Coding {}", capitalize(&level));

        ZaiSnapshot {
            plan,
            session,
            weekly,
            mcp,
        }
    }
}

fn to_window(l: &LimitEntry, dur: chrono::Duration) -> UsageWindow {
    let utilization_pct = l.percentage.round().clamp(0.0, 100.0) as i32;
    let resets_at = l
        .next_reset_time
        .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis);
    UsageWindow {
        utilization_pct,
        resets_at,
        window_duration: dur,
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

    const REAL_BODY: &str = r#"{"code":200,"msg":"Operation successful","data":{
        "limits":[
            {"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":0},
            {"type":"TOKENS_LIMIT","unit":6,"number":1,"percentage":0,"nextResetTime":1779792169974},
            {"type":"TIME_LIMIT","unit":5,"number":1,"usage":1000,"currentValue":0,"remaining":1000,"percentage":0,"nextResetTime":1779964969979,
             "usageDetails":[{"modelCode":"search-prime","usage":0}]}
        ],
        "level":"pro"
    },"success":true}"#;

    #[test]
    fn parses_real_response_shape() {
        let env: Envelope = serde_json::from_str(REAL_BODY).unwrap();
        let snap = env.into_snapshot(None);
        assert_eq!(snap.plan, "GLM Coding Pro");
        assert!(snap.session.is_some());
        assert!(snap.weekly.is_some());
        assert!(snap.mcp.is_some());
        assert_eq!(snap.session.as_ref().unwrap().utilization_pct, 0);
        assert!(snap.weekly.as_ref().unwrap().resets_at.is_some());
    }

    #[test]
    fn missing_data_yields_neutral_snapshot() {
        let env: Envelope = serde_json::from_str(r#"{"code":500,"success":false}"#).unwrap();
        let snap = env.into_snapshot(Some("lite"));
        assert_eq!(snap.plan, "GLM Coding Lite");
        assert!(snap.session.is_none());
    }

    #[test]
    fn percentage_with_float_rounds() {
        let body = r#"{"data":{"limits":[
            {"type":"TOKENS_LIMIT","percentage":42.7}
        ],"level":"max"},"success":true}"#;
        let env: Envelope = serde_json::from_str(body).unwrap();
        let snap = env.into_snapshot(None);
        assert_eq!(snap.session.as_ref().unwrap().utilization_pct, 43);
    }

    #[test]
    fn percentage_clamps_to_hundred() {
        let body = r#"{"data":{"limits":[
            {"type":"TOKENS_LIMIT","percentage":150}
        ]},"success":true}"#;
        let env: Envelope = serde_json::from_str(body).unwrap();
        let snap = env.into_snapshot(None);
        assert_eq!(snap.session.as_ref().unwrap().utilization_pct, 100);
    }

    #[test]
    fn only_time_limit_means_no_session_or_weekly() {
        let body = r#"{"data":{"limits":[
            {"type":"TIME_LIMIT","percentage":12}
        ]},"success":true}"#;
        let env: Envelope = serde_json::from_str(body).unwrap();
        let snap = env.into_snapshot(None);
        assert!(snap.session.is_none());
        assert!(snap.weekly.is_none());
        assert!(snap.mcp.is_some());
    }

    #[test]
    fn config_plan_tier_used_when_level_empty() {
        let body = r#"{"data":{"limits":[],"level":""},"success":true}"#;
        let env: Envelope = serde_json::from_str(body).unwrap();
        let snap = env.into_snapshot(Some("max"));
        assert_eq!(snap.plan, "GLM Coding Max");
    }

    /// The regression: session/weekly used to be "first TOKENS_LIMIT, second
    /// TOKENS_LIMIT", so a reordered array swapped the two windows.
    #[test]
    fn buckets_are_identified_by_unit_not_by_position() {
        let body = r#"{"data":{"limits":[
            {"type":"TOKENS_LIMIT","unit":6,"number":1,"percentage":15,"nextResetTime":1779792169974},
            {"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":42}
        ],"level":"pro"},"success":true}"#;
        let env: Envelope = serde_json::from_str(body).unwrap();
        env.check_ok().unwrap();
        let snap = env.into_snapshot(None);
        assert_eq!(snap.session.as_ref().unwrap().utilization_pct, 42);
        assert_eq!(snap.weekly.as_ref().unwrap().utilization_pct, 15);
        assert!(snap.weekly.as_ref().unwrap().resets_at.is_some());
        assert!(snap.session.as_ref().unwrap().resets_at.is_none());
    }

    /// A third bucket must not be promoted to "session" just by leading the array.
    #[test]
    fn unknown_extra_bucket_is_dropped_not_shown_as_session() {
        let body = r#"{"data":{"limits":[
            {"type":"TOKENS_LIMIT","unit":4,"number":1,"percentage":99},
            {"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":42},
            {"type":"TOKENS_LIMIT","unit":6,"number":1,"percentage":15}
        ],"level":"pro"},"success":true}"#;
        let env: Envelope = serde_json::from_str(body).unwrap();
        env.check_ok().unwrap();
        let snap = env.into_snapshot(None);
        assert_eq!(snap.session.as_ref().unwrap().utilization_pct, 42);
        assert_eq!(snap.weekly.as_ref().unwrap().utilization_pct, 15);
    }

    #[test]
    fn duplicate_unit_is_an_error_not_a_coin_flip() {
        let body = r#"{"data":{"limits":[
            {"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":42},
            {"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":7}
        ],"level":"pro"},"success":true}"#;
        let env: Envelope = serde_json::from_str(body).unwrap();
        let err = env.check_ok().unwrap_err().to_string();
        assert!(err.contains("unit 3"), "unhelpful error: {err}");
        // And the projection refuses to pick one rather than showing either.
        let snap = env.into_snapshot(None);
        assert!(snap.session.is_none());
        assert!(snap.weekly.is_none());
    }

    #[test]
    fn all_unknown_units_is_an_error() {
        let body = r#"{"data":{"limits":[
            {"type":"TOKENS_LIMIT","unit":4,"number":1,"percentage":42},
            {"type":"TOKENS_LIMIT","unit":7,"number":1,"percentage":15}
        ],"level":"pro"},"success":true}"#;
        let env: Envelope = serde_json::from_str(body).unwrap();
        let err = env.check_ok().unwrap_err().to_string();
        assert!(err.contains("4, 7"), "unhelpful error: {err}");
        assert!(env.into_snapshot(None).session.is_none());
    }

    /// A bucket whose `unit` went missing can't be named, so it is dropped —
    /// never quietly slotted into whichever window is still free.
    #[test]
    fn unit_less_bucket_alongside_a_known_one_is_dropped() {
        let body = r#"{"data":{"limits":[
            {"type":"TOKENS_LIMIT","percentage":99},
            {"type":"TOKENS_LIMIT","unit":6,"number":1,"percentage":15}
        ],"level":"pro"},"success":true}"#;
        let env: Envelope = serde_json::from_str(body).unwrap();
        env.check_ok().unwrap();
        let snap = env.into_snapshot(None);
        assert!(snap.session.is_none());
        assert_eq!(snap.weekly.as_ref().unwrap().utilization_pct, 15);
    }

    /// Carve-out: bodies (and caches) predating `unit` carry no discriminator,
    /// so position is all there ever was for them.
    #[test]
    fn bodies_without_any_unit_still_fall_back_to_position() {
        let body = r#"{"data":{"limits":[
            {"type":"TOKENS_LIMIT","percentage":10},
            {"type":"TOKENS_LIMIT","percentage":20}
        ],"level":"lite"},"success":true}"#;
        let env: Envelope = serde_json::from_str(body).unwrap();
        env.check_ok().unwrap();
        let snap = env.into_snapshot(None);
        assert_eq!(snap.session.as_ref().unwrap().utilization_pct, 10);
        assert_eq!(snap.weekly.as_ref().unwrap().utilization_pct, 20);
    }

    #[test]
    fn check_ok_accepts_the_real_response_shape() {
        let env: Envelope = serde_json::from_str(REAL_BODY).unwrap();
        env.check_ok().unwrap();
    }

    #[test]
    fn reset_time_zero_or_null_becomes_none() {
        let body = r#"{"data":{"limits":[
            {"type":"TOKENS_LIMIT","percentage":0,"nextResetTime":null}
        ]},"success":true}"#;
        let env: Envelope = serde_json::from_str(body).unwrap();
        let snap = env.into_snapshot(None);
        assert!(snap.session.as_ref().unwrap().resets_at.is_none());
    }
}
