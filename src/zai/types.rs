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
//! The `unit`/`number` codes don't have a documented mapping, so we classify
//! limits by **position + type**: the first TOKENS_LIMIT entry is treated as
//! the session bucket, the second as weekly, and the TIME_LIMIT entry as the
//! monthly MCP tool ceiling. When the shape drifts the smoke test will catch
//! it and we can update this mapping.

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
        let mut tokens_iter = data
            .limits
            .iter()
            .filter(|l| l.kind == "TOKENS_LIMIT");
        let session = tokens_iter.next().map(|l| to_window(l, chrono::Duration::hours(5)));
        let weekly = tokens_iter.next().map(|l| to_window(l, chrono::Duration::days(7)));
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
    let resets_at = l.next_reset_time.and_then(|ms| {
        chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
    });
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
