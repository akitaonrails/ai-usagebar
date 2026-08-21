//! Wire types for `GET api.github.com/copilot_internal/user`.
//!
//! This endpoint is not publicly documented, but it is the same live quota route
//! GitHub's Copilot clients poll for the currently-authenticated user. The
//! fields modeled here are intentionally required where ai-usagebar depends on
//! them, so a drifted or error envelope cannot silently render as a real `0%`.

use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::error::{AppError, Result};
use crate::usage::{CopilotPool, CopilotSnapshot};

#[derive(Debug, Clone, Deserialize)]
pub struct UserQuotaResponse {
    pub login: String,
    pub copilot_plan: String,
    /// Human-facing reset date (`YYYY-MM-DD`). Always present — used as the
    /// fallback source of truth when `quota_reset_date_utc` is absent (some
    /// GitHub Enterprise deployments omit it; see `quota_reset_date_utc`).
    pub quota_reset_date: String,
    /// ISO 8601 reset timestamp. Present on github.com but not guaranteed on
    /// every GitHub Enterprise Cloud/Server deployment — some only return
    /// `quota_reset_date`. When absent, `quota_reset_date` is parsed as UTC
    /// midnight instead of treating the response as schema drift.
    #[serde(default)]
    pub quota_reset_date_utc: Option<String>,
    pub quota_snapshots: QuotaSnapshots,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QuotaSnapshots {
    pub chat: QuotaSnapshot,
    pub completions: QuotaSnapshot,
    pub premium_interactions: QuotaSnapshot,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QuotaSnapshot {
    pub entitlement: u64,
    #[serde(default)]
    pub remaining: Option<u64>,
    #[serde(default)]
    pub quota_remaining: Option<f64>,
    pub percent_remaining: f64,
    pub unlimited: bool,
    pub has_quota: bool,
}

pub fn to_snapshot(resp: UserQuotaResponse) -> Result<CopilotSnapshot> {
    let reset_at = parse_reset_at(resp.quota_reset_date_utc.as_deref(), &resp.quota_reset_date)?;

    Ok(CopilotSnapshot {
        login: non_empty("login", &resp.login)?.to_string(),
        plan: title_case(non_empty("copilot_plan", &resp.copilot_plan)?),
        chat: pool("chat", &resp.quota_snapshots.chat)?,
        completions: pool("completions", &resp.quota_snapshots.completions)?,
        premium_interactions: pool(
            "premium_interactions",
            &resp.quota_snapshots.premium_interactions,
        )?,
        reset_at: Some(reset_at),
    })
}

/// Prefer the ISO 8601 `quota_reset_date_utc` when present (github.com and
/// most GHE deployments); fall back to `quota_reset_date` (`YYYY-MM-DD`,
/// interpreted as UTC midnight) when it's absent — some GitHub Enterprise
/// Cloud/Server responses only include the date, not the timestamp.
fn parse_reset_at(reset_utc: Option<&str>, reset_date: &str) -> Result<DateTime<Utc>> {
    if let Some(utc) = reset_utc {
        return DateTime::parse_from_rfc3339(utc)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|e| {
                AppError::Schema(format!(
                    "copilot: `quota_reset_date_utc` is not RFC3339 ({reset_utc:?}): {e}"
                ))
            });
    }

    chrono::NaiveDate::parse_from_str(reset_date, "%Y-%m-%d")
        .map_err(|e| {
            AppError::Schema(format!(
                "copilot: `quota_reset_date` is not YYYY-MM-DD ({reset_date:?}): {e}"
            ))
        })
        .map(|date| {
            date.and_hms_opt(0, 0, 0)
                .expect("midnight is always a valid time")
                .and_utc()
        })
}

fn non_empty<'a>(field: &str, value: &'a str) -> Result<&'a str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Err(AppError::Schema(format!("copilot: `{field}` is empty")))
    } else {
        Ok(trimmed)
    }
}

fn pool(field: &str, quota: &QuotaSnapshot) -> Result<CopilotPool> {
    if quota.unlimited {
        return Ok(CopilotPool::Unlimited);
    }
    if !quota.has_quota || quota.entitlement == 0 {
        return Ok(CopilotPool::NotApplicable);
    }

    let remaining = match quota.remaining {
        Some(value) => value,
        None => quota_remaining(field, quota.quota_remaining)?,
    };
    let percent_used = pct_used(field, quota.percent_remaining)?;

    Ok(CopilotPool::Metered {
        entitlement: quota.entitlement,
        remaining,
        percent_used,
    })
}

fn quota_remaining(field: &str, value: Option<f64>) -> Result<u64> {
    let raw = value.ok_or_else(|| {
        AppError::Schema(format!(
            "copilot: `{field}` has quota but neither `remaining` nor `quota_remaining`"
        ))
    })?;
    if !raw.is_finite() || raw < 0.0 || raw > u64::MAX as f64 {
        return Err(AppError::Schema(format!(
            "copilot: `{field}.quota_remaining` is not a usable non-negative finite number"
        )));
    }
    Ok(raw.round() as u64)
}

fn pct_used(field: &str, percent_remaining: f64) -> Result<i32> {
    if !percent_remaining.is_finite() {
        return Err(AppError::Schema(format!(
            "copilot: `{field}.percent_remaining` is not a finite number"
        )));
    }
    Ok((100.0 - percent_remaining).round().clamp(0.0, 9999.0) as i32)
}

fn title_case(plan: &str) -> String {
    plan.split(['-', '_', ' '])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    const SAMPLE: &str = r#"{
      "login": "someuser",
      "copilot_plan": "individual",
      "chat_enabled": true,
      "quota_reset_date": "2026-09-01",
      "quota_reset_date_utc": "2026-09-01T00:00:00.000Z",
      "quota_snapshots": {
        "chat": {
          "quota_id": "chat",
          "entitlement": 200,
          "remaining": 121,
          "quota_remaining": 121.0,
          "percent_remaining": 60.5,
          "unlimited": false,
          "has_quota": true
        },
        "completions": {
          "quota_id": "completions",
          "entitlement": 2000,
          "remaining": 2000,
          "quota_remaining": 2000.0,
          "percent_remaining": 100.0,
          "unlimited": false,
          "has_quota": true
        },
        "premium_interactions": {
          "quota_id": "premium_interactions",
          "entitlement": 0,
          "remaining": 0,
          "quota_remaining": 0.0,
          "percent_remaining": 0.0,
          "unlimited": false,
          "has_quota": false
        }
      }
    }"#;

    #[test]
    fn parses_live_shape_and_preserves_not_applicable_pool() {
        let resp: UserQuotaResponse = serde_json::from_str(SAMPLE).unwrap();
        let snap = to_snapshot(resp).unwrap();
        assert_eq!(snap.login, "someuser");
        assert_eq!(snap.plan, "Individual");
        assert_eq!(
            snap.reset_at,
            Some(Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap())
        );
        assert_eq!(
            snap.chat,
            CopilotPool::Metered {
                entitlement: 200,
                remaining: 121,
                percent_used: 40,
            }
        );
        assert_eq!(
            snap.completions,
            CopilotPool::Metered {
                entitlement: 2000,
                remaining: 2000,
                percent_used: 0,
            }
        );
        assert_eq!(snap.premium_interactions, CopilotPool::NotApplicable);
        assert_eq!(snap.primary_pct(), Some(40));
    }

    #[test]
    fn unlimited_pool_is_distinct_from_not_applicable() {
        let raw = r#"{
          "login": "octocat",
          "copilot_plan": "business",
          "quota_reset_date": "2026-09-01",
          "quota_reset_date_utc": "2026-09-01T00:00:00Z",
          "quota_snapshots": {
            "chat": {"entitlement": 0, "percent_remaining": 0, "unlimited": true, "has_quota": true},
            "completions": {"entitlement": 0, "percent_remaining": 0, "unlimited": true, "has_quota": true},
            "premium_interactions": {"entitlement": 0, "percent_remaining": 0, "unlimited": true, "has_quota": true}
          }
        }"#;
        let snap = to_snapshot(serde_json::from_str(raw).unwrap()).unwrap();
        assert_eq!(snap.plan, "Business");
        assert_eq!(snap.chat, CopilotPool::Unlimited);
        assert_eq!(snap.primary_pct(), None);
    }

    #[test]
    fn remaining_falls_back_to_quota_remaining() {
        let raw = r#"{
          "login": "octocat",
          "copilot_plan": "individual",
          "quota_reset_date": "2026-09-01",
          "quota_reset_date_utc": "2026-09-01T00:00:00Z",
          "quota_snapshots": {
            "chat": {"entitlement": 200, "quota_remaining": 121.0, "percent_remaining": 60.5, "unlimited": false, "has_quota": true},
            "completions": {"entitlement": 1, "remaining": 1, "percent_remaining": 100.0, "unlimited": false, "has_quota": true},
            "premium_interactions": {"entitlement": 0, "remaining": 0, "percent_remaining": 0.0, "unlimited": false, "has_quota": false}
          }
        }"#;
        let snap = to_snapshot(serde_json::from_str(raw).unwrap()).unwrap();
        assert!(matches!(
            snap.chat,
            CopilotPool::Metered {
                remaining: 121,
                percent_used: 40,
                ..
            }
        ));
    }

    #[test]
    fn malformed_envelope_is_rejected_rather_than_read_as_zero() {
        assert!(serde_json::from_str::<UserQuotaResponse>("{}").is_err());
        assert!(serde_json::from_str::<UserQuotaResponse>(r#"{"login":"x"}"#).is_err());
        assert!(serde_json::from_str::<UserQuotaResponse>(r#"{"login":"x","copilot_plan":"individual","quota_reset_date":"2026-09-01","quota_reset_date_utc":"2026-09-01T00:00:00Z","quota_snapshots":{}}"#).is_err());
        assert!(serde_json::from_str::<UserQuotaResponse>(r#"{"error":"forbidden"}"#).is_err());
    }

    #[test]
    fn missing_remaining_fields_on_a_real_pool_is_schema_drift() {
        let raw = r#"{
          "login": "octocat",
          "copilot_plan": "individual",
          "quota_reset_date": "2026-09-01",
          "quota_reset_date_utc": "2026-09-01T00:00:00Z",
          "quota_snapshots": {
            "chat": {"entitlement": 200, "percent_remaining": 60.5, "unlimited": false, "has_quota": true},
            "completions": {"entitlement": 1, "remaining": 1, "percent_remaining": 100.0, "unlimited": false, "has_quota": true},
            "premium_interactions": {"entitlement": 0, "remaining": 0, "percent_remaining": 0.0, "unlimited": false, "has_quota": false}
          }
        }"#;
        let resp: UserQuotaResponse = serde_json::from_str(raw).unwrap();
        assert!(to_snapshot(resp).is_err());
    }

    #[test]
    fn non_finite_percentage_is_rejected() {
        let raw = r#"{
          "login": "octocat",
          "copilot_plan": "individual",
          "quota_reset_date": "2026-09-01",
          "quota_reset_date_utc": "2026-09-01T00:00:00Z",
          "quota_snapshots": {
            "chat": {"entitlement": 200, "remaining": 100, "percent_remaining": "NaN", "unlimited": false, "has_quota": true},
            "completions": {"entitlement": 1, "remaining": 1, "percent_remaining": 100.0, "unlimited": false, "has_quota": true},
            "premium_interactions": {"entitlement": 0, "remaining": 0, "percent_remaining": 0.0, "unlimited": false, "has_quota": false}
          }
        }"#;
        assert!(serde_json::from_str::<UserQuotaResponse>(raw).is_err());
    }

    #[test]
    fn empty_login_or_plan_is_rejected() {
        for (field, body) in [
            (
                "login",
                r#"{"login":" ","copilot_plan":"individual","quota_reset_date":"2026-09-01","quota_reset_date_utc":"2026-09-01T00:00:00Z","quota_snapshots":{"chat":{"entitlement":1,"remaining":1,"percent_remaining":100.0,"unlimited":false,"has_quota":true},"completions":{"entitlement":1,"remaining":1,"percent_remaining":100.0,"unlimited":false,"has_quota":true},"premium_interactions":{"entitlement":0,"remaining":0,"percent_remaining":0.0,"unlimited":false,"has_quota":false}}}"#,
            ),
            (
                "plan",
                r#"{"login":"octocat","copilot_plan":" ","quota_reset_date":"2026-09-01","quota_reset_date_utc":"2026-09-01T00:00:00Z","quota_snapshots":{"chat":{"entitlement":1,"remaining":1,"percent_remaining":100.0,"unlimited":false,"has_quota":true},"completions":{"entitlement":1,"remaining":1,"percent_remaining":100.0,"unlimited":false,"has_quota":true},"premium_interactions":{"entitlement":0,"remaining":0,"percent_remaining":0.0,"unlimited":false,"has_quota":false}}}"#,
            ),
        ] {
            let resp: UserQuotaResponse = serde_json::from_str(body).unwrap();
            assert!(to_snapshot(resp).is_err(), "{field}");
        }
    }

    /// Regression test for a real GitHub Enterprise Cloud (data-residency)
    /// response: it omits `quota_reset_date_utc` entirely and only sends the
    /// date. This must resolve to UTC midnight on that date, not a schema
    /// error.
    #[test]
    fn ghe_response_without_reset_utc_falls_back_to_reset_date() {
        let raw = r#"{
          "login": "testuser",
          "copilot_plan": "business",
          "quota_reset_date": "2026-09-01",
          "quota_snapshots": {
            "chat": {"entitlement": 0, "remaining": 0, "percent_remaining": 100.0, "unlimited": true, "has_quota": true},
            "completions": {"entitlement": 0, "remaining": 0, "percent_remaining": 100.0, "unlimited": true, "has_quota": true},
            "premium_interactions": {"entitlement": 0, "remaining": 0, "percent_remaining": 100.0, "unlimited": true, "has_quota": true}
          }
        }"#;
        let resp: UserQuotaResponse = serde_json::from_str(raw).unwrap();
        assert!(resp.quota_reset_date_utc.is_none());
        let snap = to_snapshot(resp).unwrap();
        assert_eq!(
            snap.reset_at,
            Some(Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap())
        );
        assert_eq!(snap.chat, CopilotPool::Unlimited);
    }

    #[test]
    fn malformed_reset_date_without_reset_utc_is_schema_error() {
        let raw = r#"{
          "login": "octocat",
          "copilot_plan": "individual",
          "quota_reset_date": "not-a-date",
          "quota_snapshots": {
            "chat": {"entitlement": 1, "remaining": 1, "percent_remaining": 100.0, "unlimited": false, "has_quota": true},
            "completions": {"entitlement": 1, "remaining": 1, "percent_remaining": 100.0, "unlimited": false, "has_quota": true},
            "premium_interactions": {"entitlement": 0, "remaining": 0, "percent_remaining": 0.0, "unlimited": false, "has_quota": false}
          }
        }"#;
        let resp: UserQuotaResponse = serde_json::from_str(raw).unwrap();
        assert!(to_snapshot(resp).is_err());
    }
}
