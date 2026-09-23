//! Defensive wire parsing for GitHub Copilot's VS Code quota response.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{AppError, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quota {
    pub percent_remaining: i32,
    pub entitlement: Option<u64>,
    pub remaining: Option<u64>,
    pub unlimited: bool,
}

impl Quota {
    pub fn used_pct(&self) -> i32 {
        if self.unlimited {
            0
        } else {
            100 - self.percent_remaining
        }
    }

    pub fn used_and_entitlement(&self) -> Option<(u64, u64)> {
        Some((
            self.entitlement?.saturating_sub(self.remaining?),
            self.entitlement?,
        ))
    }
}

/// Normalized and cacheable fields only. Raw GitHub responses can include
/// account metadata, which is neither needed for display nor retained.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub plan: String,
    pub premium: Option<Quota>,
    pub chat: Option<Quota>,
    pub completions: Option<Quota>,
    pub reset_at: Option<DateTime<Utc>>,
}

impl Snapshot {
    pub fn quotas(&self) -> impl Iterator<Item = (&'static str, &Quota)> {
        [
            ("Premium requests", self.premium.as_ref()),
            ("Chat", self.chat.as_ref()),
            ("Completions", self.completions.as_ref()),
        ]
        .into_iter()
        .filter_map(|(label, quota)| quota.map(|quota| (label, quota)))
    }

    pub fn worst_pct(&self) -> i32 {
        self.quotas()
            .map(|(_, quota)| quota.used_pct())
            .max()
            .unwrap_or(0)
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Response {
    pub copilot_plan: Option<String>,
    pub quota_reset_date: Option<String>,
    pub quota_reset_date_utc: Option<String>,
    pub quota_snapshots: Option<QuotaSnapshots>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct QuotaSnapshots {
    pub premium_interactions: Option<Value>,
    pub chat: Option<Value>,
    pub completions: Option<Value>,
}

pub fn to_snapshot(response: Response) -> Result<Snapshot> {
    let quotas = response.quota_snapshots.unwrap_or_default();
    let premium = parse_quota(quotas.premium_interactions.as_ref());
    let chat = parse_quota(quotas.chat.as_ref());
    let completions = parse_quota(quotas.completions.as_ref());
    if premium.is_none() && chat.is_none() && completions.is_none() {
        return Err(AppError::Schema(
            "GitHub Copilot response contains no usable quota snapshots".into(),
        ));
    }
    let plan = response
        .copilot_plan
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "GitHub Copilot".to_string());
    let reset_at = response
        .quota_reset_date_utc
        .as_deref()
        .and_then(parse_reset)
        .or_else(|| response.quota_reset_date.as_deref().and_then(parse_reset));
    Ok(Snapshot {
        plan,
        premium,
        chat,
        completions,
        reset_at,
    })
}

/// One malformed optional bucket must not hide usable data in another. The
/// caller still rejects a response where every known bucket is unusable.
fn parse_quota(value: Option<&Value>) -> Option<Quota> {
    let object = value?.as_object()?;
    let unlimited = object
        .get("unlimited")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let entitlement = object.get("entitlement").and_then(nonnegative_integer);
    let remaining = object.get("remaining").and_then(nonnegative_integer);
    let percent_remaining = object
        .get("percent_remaining")
        .and_then(percent)
        .or_else(|| {
            entitlement
                .zip(remaining)
                .and_then(|(entitlement, remaining)| {
                    (entitlement > 0).then(|| ((remaining * 100) / entitlement).min(100) as i32)
                })
        });
    (unlimited || percent_remaining.is_some()).then_some(Quota {
        percent_remaining: percent_remaining.unwrap_or(100),
        entitlement,
        remaining,
        unlimited,
    })
}

fn nonnegative_integer(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str()?.trim().parse().ok())
}

fn percent(value: &Value) -> Option<i32> {
    let value = value
        .as_f64()
        .or_else(|| value.as_str()?.trim().parse::<f64>().ok())?;
    value
        .is_finite()
        .then(|| i32::from(crate::format::clamp_pct(value)))
}

/// GitHub has returned both RFC3339 `quota_reset_date_utc` and date-only
/// `quota_reset_date`. A date-only reset means midnight UTC of that date.
fn parse_reset(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
        .or_else(|| {
            NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .ok()?
                .and_hms_opt(0, 0, 0)
                .map(|value| value.and_utc())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_quota_buckets_and_date_only_reset() {
        let response: Response = serde_json::from_str(
            r#"{
                "copilot_plan":"business",
                "quota_reset_date":"2026-09-15",
                "quota_snapshots":{
                    "premium_interactions":{"entitlement":300,"remaining":45,"percent_remaining":15},
                    "chat":{"entitlement":"1000","remaining":"250"},
                    "completions":{"unlimited":true}
                }
            }"#,
        )
        .unwrap();
        let snapshot = to_snapshot(response).unwrap();
        assert_eq!(snapshot.plan, "business");
        assert_eq!(snapshot.premium.unwrap().used_pct(), 85);
        assert_eq!(
            snapshot.chat.unwrap().used_and_entitlement(),
            Some((750, 1000))
        );
        assert!(snapshot.completions.unwrap().unlimited);
        assert_eq!(
            snapshot.reset_at.unwrap().to_rfc3339(),
            "2026-09-15T00:00:00+00:00"
        );
    }

    #[test]
    fn accepts_one_good_bucket_and_rejects_an_empty_schema() {
        let partial: Response = serde_json::from_str(
            r#"{"quota_snapshots":{"chat":{"percent_remaining":"not-a-number"},"completions":{"remaining":4,"entitlement":8}}}"#,
        )
        .unwrap();
        let snapshot = to_snapshot(partial).unwrap();
        assert!(snapshot.chat.is_none());
        assert_eq!(snapshot.completions.unwrap().used_pct(), 50);

        let empty: Response = serde_json::from_str(r#"{"quota_snapshots":{}}"#).unwrap();
        assert!(to_snapshot(empty).is_err());
    }
}
