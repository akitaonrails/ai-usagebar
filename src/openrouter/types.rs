//! Wire types for OpenRouter's `/api/v1/credits` and `/api/v1/key`.
//!
//! Both endpoints wrap their payload in `{ "data": { ... } }`, hence the
//! generic [`OrEnvelope`] wrapper.

use serde::Deserialize;

use crate::usage::OpenRouterSnapshot;

/// Wrapper used by all OpenRouter v1 endpoints.
#[derive(Debug, Clone, Deserialize)]
pub struct OrEnvelope<T> {
    pub data: T,
}

/// `GET /api/v1/credits` — total_credits and total_usage, both USD doubles.
#[derive(Debug, Clone, Deserialize)]
pub struct CreditsData {
    #[serde(deserialize_with = "de_nonnegative_finite")]
    pub total_credits: f64,
    #[serde(deserialize_with = "de_nonnegative_finite")]
    pub total_usage: f64,
}

/// `GET /api/v1/key` — per-key usage and free-tier flag.
#[derive(Debug, Clone, Deserialize)]
pub struct KeyData {
    #[serde(default)]
    pub label: String,
    #[serde(default, deserialize_with = "de_opt_nonnegative_finite")]
    pub limit: Option<f64>,
    #[serde(default, deserialize_with = "de_opt_finite")]
    pub limit_remaining: Option<f64>,
    #[serde(deserialize_with = "de_nonnegative_finite")]
    pub usage_daily: f64,
    #[serde(deserialize_with = "de_nonnegative_finite")]
    pub usage_weekly: f64,
    #[serde(deserialize_with = "de_nonnegative_finite")]
    pub usage_monthly: f64,
    pub is_free_tier: bool,
}

fn checked_finite<E: serde::de::Error>(value: f64) -> Result<f64, E> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(E::custom("money value is not finite"))
    }
}

fn checked_nonnegative<E: serde::de::Error>(value: f64) -> Result<f64, E> {
    let value = checked_finite(value)?;
    if value >= 0.0 {
        Ok(value)
    } else {
        Err(E::custom("money value cannot be negative"))
    }
}

fn de_nonnegative_finite<'de, D>(d: D) -> Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    checked_nonnegative(f64::deserialize(d)?)
}

fn de_opt_nonnegative_finite<'de, D>(d: D) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<f64>::deserialize(d)?
        .map(checked_nonnegative)
        .transpose()
}

fn de_opt_finite<'de, D>(d: D) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<f64>::deserialize(d)?
        .map(checked_finite)
        .transpose()
}

/// Combine the two endpoint responses into the canonical snapshot.
pub fn combine(credits: CreditsData, key: KeyData) -> OpenRouterSnapshot {
    let label = if key.label.is_empty() {
        "OpenRouter".to_string()
    } else {
        format!("OpenRouter — {}", key.label)
    };
    OpenRouterSnapshot {
        label,
        total_credits: credits.total_credits,
        total_usage: credits.total_usage,
        usage_daily: key.usage_daily,
        usage_weekly: key.usage_weekly,
        usage_monthly: key.usage_monthly,
        is_free_tier: key.is_free_tier,
        limit: key.limit,
        limit_remaining: key.limit_remaining,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_credits_envelope() {
        let raw = r#"{"data":{"total_credits":100.0,"total_usage":25.5}}"#;
        let env: OrEnvelope<CreditsData> = serde_json::from_str(raw).unwrap();
        assert_eq!(env.data.total_credits, 100.0);
        assert_eq!(env.data.total_usage, 25.5);
    }

    #[test]
    fn parses_key_envelope_with_nulls() {
        let raw = r#"{"data":{
            "label":"my-key",
            "limit":null,"limit_remaining":null,
            "usage":12.34,"usage_daily":1.0,"usage_weekly":3.0,"usage_monthly":12.0,
            "is_free_tier":false
        }}"#;
        let env: OrEnvelope<KeyData> = serde_json::from_str(raw).unwrap();
        assert_eq!(env.data.label, "my-key");
        assert!(env.data.limit.is_none());
        assert_eq!(env.data.usage_monthly, 12.0);
        assert!(!env.data.is_free_tier);
    }

    #[test]
    fn combine_builds_snapshot() {
        let c = CreditsData {
            total_credits: 100.0,
            total_usage: 30.0,
        };
        let k = KeyData {
            label: "key-A".into(),
            limit: Some(50.0),
            limit_remaining: Some(20.0),
            usage_daily: 1.0,
            usage_weekly: 5.0,
            usage_monthly: 30.0,
            is_free_tier: false,
        };
        let snap = combine(c, k);
        assert_eq!(snap.label, "OpenRouter — key-A");
        assert!((snap.balance() - 70.0).abs() < 1e-9);
        assert_eq!(snap.consumed_pct(), 30);
        assert_eq!(snap.usage_monthly, 30.0);
    }

    #[test]
    fn combine_with_empty_label() {
        let snap = combine(
            CreditsData {
                total_credits: 0.0,
                total_usage: 0.0,
            },
            KeyData {
                label: String::new(),
                limit: None,
                limit_remaining: None,
                usage_daily: 0.0,
                usage_weekly: 0.0,
                usage_monthly: 0.0,
                is_free_tier: false,
            },
        );
        assert_eq!(snap.label, "OpenRouter");
    }

    #[test]
    fn missing_required_money_does_not_deserialize_as_zero() {
        assert!(serde_json::from_str::<OrEnvelope<CreditsData>>(r#"{"data":{}}"#).is_err());
        assert!(
            serde_json::from_str::<OrEnvelope<KeyData>>(
                r#"{"data":{"label":"key","is_free_tier":false}}"#
            )
            .is_err()
        );
    }

    #[test]
    fn invalid_money_values_are_schema_drift() {
        for total in ["-1", "1e400", "true", r#""zero""#] {
            let raw = format!(r#"{{"data":{{"total_credits":{total},"total_usage":0}}}}"#);
            assert!(
                serde_json::from_str::<OrEnvelope<CreditsData>>(&raw).is_err(),
                "{raw}"
            );
        }
    }

    #[test]
    fn consumed_pct_handles_zero_credits() {
        let s = OpenRouterSnapshot {
            label: "x".into(),
            total_credits: 0.0,
            total_usage: 5.0,
            usage_daily: 0.0,
            usage_weekly: 0.0,
            usage_monthly: 0.0,
            is_free_tier: true,
            limit: None,
            limit_remaining: None,
        };
        assert_eq!(s.consumed_pct(), 0);
    }
}
