//! Strict wire types for the official Grok Build `x.ai/billing` ACP response.

use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::error::{AppError, Result};
use crate::usage::{ResetCredits, SuperGrokPeriod, SuperGrokSnapshot};

const MAX_PLAN_CHARS: usize = 128;
const MAX_BENIGN_PERCENT: f64 = 100.5;
const MAX_EXACT_F64_INTEGER: i64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct BillingResponse {
    pub config: Option<BillingConfig>,
    /// The ACP extension currently serializes snake_case; accept camelCase for
    /// compatibility with older/direct extension bridges.
    #[serde(alias = "subscriptionTier")]
    pub subscription_tier: Option<String>,
    #[serde(skip)]
    pub reset_credits: ResetCredits,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct BillingConfig {
    pub credit_usage_percent: Option<f64>,
    pub current_period: Option<UsagePeriod>,
    /// Deprecated legacy monthly fields returned by older Grok Build servers.
    pub monthly_limit: Option<Cent>,
    pub used: Option<Cent>,
    pub on_demand_cap: Option<Cent>,
    pub on_demand_used: Option<Cent>,
    pub prepaid_balance: Option<Cent>,
    pub is_unified_billing_user: Option<bool>,
    pub billing_period_start: Option<String>,
    pub billing_period_end: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct UsagePeriod {
    #[serde(rename = "type")]
    pub period_type: Option<String>,
    pub start: Option<String>,
    pub end: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Cent {
    /// Proto JSON omits zero-valued scalars, so `{}` means zero. Present
    /// values must still be exact integers; fractional/saturated casts would
    /// silently corrupt billing amounts.
    #[serde(default, deserialize_with = "de_cent_val")]
    pub val: i64,
}

fn de_cent_val<'de, D>(deserializer: D) -> std::result::Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Number(number) => number
            .as_i64()
            .ok_or_else(|| serde::de::Error::custom("cent val must be an exact i64 integer")),
        serde_json::Value::String(text) => text
            .trim()
            .parse::<i64>()
            .map_err(|_| serde::de::Error::custom("cent val string must be an exact i64 integer")),
        _ => Err(serde::de::Error::custom(
            "cent val must be an integer number or string",
        )),
    }
}

pub fn to_snapshot(resp: BillingResponse, account_scope: &str) -> Result<SuperGrokSnapshot> {
    let plan = checked_plan(resp.subscription_tier.as_deref())?;
    let cfg = resp
        .config
        .ok_or_else(|| AppError::Schema("Grok Build billing response has no config".into()))?;
    let period = resolve_period(&cfg);
    let weekly_pct = resolve_usage_percent(&cfg)?;
    let reset_at = resolve_reset_at(&cfg)?;
    let prepaid_balance = cfg
        .prepaid_balance
        .map(|cents| checked_prepaid(cents.val))
        .transpose()?;

    Ok(SuperGrokSnapshot {
        plan,
        account: account_scope.to_string(),
        weekly_pct,
        period,
        reset_at,
        prepaid_balance,
        reset_credits: resp.reset_credits,
    })
}

fn checked_plan(value: Option<&str>) -> Result<String> {
    let value = value.map(str::trim).filter(|s| !s.is_empty());
    let Some(value) = value else {
        return Ok("SuperGrok".into());
    };
    if value.chars().count() > MAX_PLAN_CHARS || value.chars().any(char::is_control) {
        return Err(AppError::Schema(
            "Grok Build subscription tier is invalid".into(),
        ));
    }
    Ok(value.to_string())
}

fn resolve_usage_percent(cfg: &BillingConfig) -> Result<i32> {
    if let Some(percent) = cfg.credit_usage_percent {
        return checked_percent(percent);
    }

    // Proto JSON omits the zero-valued percentage immediately after rollover.
    // A typed current period makes that omission unambiguous. Never splice in
    // deprecated monthly counters under a weekly current-period reset.
    if cfg.current_period.is_some() {
        return Ok(0);
    }

    match (&cfg.used, &cfg.monthly_limit) {
        (Some(used), Some(limit)) if limit.val > 0 && used.val >= 0 => {
            checked_percent((used.val as f64 / limit.val as f64) * 100.0)
        }
        (Some(_), Some(_)) => Err(AppError::Schema(
            "Grok Build legacy billing counters are negative or have a non-positive limit".into(),
        )),
        _ if cfg.billing_period_end.is_some()
            || cfg.prepaid_balance.is_some()
            || cfg.is_unified_billing_user.is_some() =>
        {
            Ok(0)
        }
        _ => Err(AppError::Schema(
            "Grok Build billing response has no usage percentage or coherent legacy counters"
                .into(),
        )),
    }
}

fn checked_percent(value: f64) -> Result<i32> {
    if !value.is_finite() || !(0.0..=MAX_BENIGN_PERCENT).contains(&value) {
        return Err(AppError::Schema(
            "Grok Build billing percentage is outside the supported range".into(),
        ));
    }
    Ok(value.round().clamp(0.0, 100.0) as i32)
}

fn resolve_period(cfg: &BillingConfig) -> SuperGrokPeriod {
    let raw = cfg
        .current_period
        .as_ref()
        .and_then(|period| period.period_type.as_deref())
        .unwrap_or_default();
    if raw.ends_with("WEEKLY") {
        SuperGrokPeriod::Weekly
    } else if raw.ends_with("MONTHLY")
        || (cfg.current_period.is_none()
            && (cfg.monthly_limit.is_some()
                || cfg.used.is_some()
                || cfg.billing_period_end.is_some()))
    {
        SuperGrokPeriod::Monthly
    } else {
        SuperGrokPeriod::Unknown
    }
}

fn resolve_reset_at(cfg: &BillingConfig) -> Result<Option<DateTime<Utc>>> {
    if let Some(period) = cfg.current_period.as_ref() {
        return parse_optional_datetime(period.end.as_deref(), "currentPeriod.end");
    }
    parse_optional_datetime(cfg.billing_period_end.as_deref(), "billingPeriodEnd")
}

fn parse_optional_datetime(value: Option<&str>, field: &str) -> Result<Option<DateTime<Utc>>> {
    let Some(value) = value else {
        return Ok(None);
    };
    DateTime::parse_from_rfc3339(value)
        .map(|dt| Some(dt.with_timezone(&Utc)))
        .map_err(|_| AppError::Schema(format!("Grok Build {field} is not RFC 3339")))
}

fn checked_prepaid(cents: i64) -> Result<f64> {
    if !(0..=MAX_EXACT_F64_INTEGER).contains(&cents) {
        return Err(AppError::Schema(
            "Grok Build prepaid balance is negative or too large to represent exactly".into(),
        ));
    }
    Ok(cents as f64 / 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weekly_acp_shape_is_coherent() {
        let response: BillingResponse = serde_json::from_str(
            r#"{
              "config": {
                "creditUsagePercent": 42.5,
                "currentPeriod": {
                  "type": "USAGE_PERIOD_TYPE_WEEKLY",
                  "end": "2026-08-10T00:00:00Z"
                },
                "prepaidBalance": {"val": 1250}
              },
              "subscription_tier": "SuperGrok Heavy"
            }"#,
        )
        .unwrap();
        let snapshot = to_snapshot(response, "opaque-scope").unwrap();
        assert_eq!(snapshot.weekly_pct, 43);
        assert_eq!(snapshot.period, SuperGrokPeriod::Weekly);
        assert_eq!(snapshot.plan, "SuperGrok Heavy");
        assert_eq!(snapshot.prepaid_balance, Some(12.5));
    }

    #[test]
    fn legacy_monthly_shape_keeps_its_own_reset() {
        let response: BillingResponse = serde_json::from_str(
            r#"{"config":{"monthlyLimit":{"val":"2000"},"used":{"val":500},"billingPeriodEnd":"2026-09-01T00:00:00Z"}}"#,
        )
        .unwrap();
        let snapshot = to_snapshot(response, "scope").unwrap();
        assert_eq!(snapshot.weekly_pct, 25);
        assert_eq!(snapshot.period, SuperGrokPeriod::Monthly);
        assert_eq!(
            snapshot.reset_at.unwrap().to_rfc3339(),
            "2026-09-01T00:00:00+00:00"
        );
    }

    #[test]
    fn omitted_zero_percent_does_not_import_legacy_monthly_usage() {
        let response: BillingResponse = serde_json::from_str(
            r#"{"config":{"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY","end":"2026-08-13T00:00:00Z"},"monthlyLimit":{"val":1000},"used":{"val":900}}}"#,
        )
        .unwrap();
        let snapshot = to_snapshot(response, "scope").unwrap();
        assert_eq!(snapshot.weekly_pct, 0);
        assert_eq!(snapshot.period, SuperGrokPeriod::Weekly);
    }

    #[test]
    fn cents_must_be_exact_integers() {
        for value in ["1.5", "1e100", "null", "true"] {
            let body = format!(r#"{{"config":{{"prepaidBalance":{{"val":{value}}}}}}}"#);
            assert!(
                serde_json::from_str::<BillingResponse>(&body).is_err(),
                "{body}"
            );
        }
        let omitted: BillingResponse =
            serde_json::from_str(r#"{"config":{"prepaidBalance":{}}}"#).unwrap();
        assert_eq!(omitted.config.unwrap().prepaid_balance.unwrap().val, 0);
    }

    #[test]
    fn malformed_percentages_and_resets_are_rejected() {
        for percent in [-1.0, 101.0, f64::INFINITY] {
            assert!(checked_percent(percent).is_err());
        }
        let response: BillingResponse = serde_json::from_str(
            r#"{"config":{"creditUsagePercent":5,"currentPeriod":{"end":"not-a-date"}}}"#,
        )
        .unwrap();
        assert!(to_snapshot(response, "scope").is_err());
        assert!(checked_prepaid(-1).is_err());
        assert!(checked_prepaid(MAX_EXACT_F64_INTEGER + 1).is_err());
    }

    #[test]
    fn plan_labels_are_bounded_and_control_free() {
        assert!(checked_plan(Some(&"x".repeat(MAX_PLAN_CHARS + 1))).is_err());
        assert!(checked_plan(Some("bad\u{1b}[31m")).is_err());
        assert_eq!(checked_plan(Some("  ")).unwrap(), "SuperGrok");
    }
}
