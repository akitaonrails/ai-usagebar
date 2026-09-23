//! Wire types for OrcaRouter's `/v1/dashboard/billing/{usage,subscription}`
//! endpoints (one-api/new-api lineage, named verbatim in OrcaRouter's docs).
//!
//! Two shape facts drive everything here:
//!
//! - `total_usage` is **US cents** (`275` = $2.75), so spend is parsed into
//!   exact integer cents rather than a float dollar amount.
//! - The subscription's three limit fields carry the *same* value and mean the
//!   **total credit limit** (remaining + used) — not the remaining balance.
//!   Unlimited-quota keys return `100000000` there, which must not render as a
//!   $100M wallet.

use serde::Deserialize;

use crate::usage::{Cents, OrcaRouterSnapshot};

/// The unlimited sentinel one-api deployments put in every limit field. A key
/// reporting this has no cap, not a hundred-million-dollar one.
pub const UNLIMITED_LIMIT_USD: f64 = 100_000_000.0;

/// `GET /v1/dashboard/billing/usage` — cumulative total usage in US cents.
///
/// Date params exist on this endpoint but are ignored by the deployment; none
/// are sent. `object` is tolerated absent (fixtures from the live adapter
/// omit it).
#[derive(Debug, Clone, Deserialize)]
pub struct UsageResponse {
    #[serde(default)]
    pub object: Option<String>,
    #[serde(deserialize_with = "de_cents")]
    pub total_usage: Cents,
}

/// `GET /v1/dashboard/billing/subscription` — the key's credit limit and
/// expiry. `object`, `has_payment_method`, and `access_until` are tolerated
/// absent (`access_until: 0` also means "no expiry").
#[derive(Debug, Clone, Deserialize)]
pub struct SubscriptionResponse {
    #[serde(default)]
    pub object: Option<String>,
    #[serde(default)]
    pub has_payment_method: Option<bool>,
    #[serde(default, deserialize_with = "de_opt_usd_cents")]
    pub soft_limit_usd: Option<Cents>,
    #[serde(default, deserialize_with = "de_opt_usd_cents")]
    pub hard_limit_usd: Option<Cents>,
    #[serde(default, deserialize_with = "de_opt_usd_cents")]
    pub system_hard_limit_usd: Option<Cents>,
    #[serde(default)]
    pub access_until: i64,
}

impl SubscriptionResponse {
    /// The total credit limit in exact cents, or `None` when the key is
    /// unlimited (sentinel) or reported no limit field at all.
    ///
    /// The three wire fields are documented to carry the same value, so the
    /// binding one (`hard`) is preferred and the others are fallbacks rather
    /// than cross-checks — a deployment that disagrees with itself still gets
    /// its most authoritative field, not a schema error.
    pub fn limit_cents(&self) -> Option<i64> {
        self.hard_limit_usd
            .or(self.soft_limit_usd)
            .or(self.system_hard_limit_usd)
            .filter(|cents| cents.0 > 0)
            .map(|cents| cents.0)
    }

    /// Key expiry as a timestamp; `None` for `0` (and absent) — no expiry.
    pub fn access_until_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        (self.access_until > 0)
            .then(|| chrono::DateTime::from_timestamp(self.access_until, 0))
            .flatten()
    }
}

/// Combine the two endpoint responses into the canonical snapshot.
pub fn combine(usage: &UsageResponse, subscription: &SubscriptionResponse) -> OrcaRouterSnapshot {
    OrcaRouterSnapshot {
        spent_cents: usage.total_usage.0,
        limit_cents: subscription.limit_cents(),
        access_until: subscription.access_until_at(),
    }
}

/// Parse a wire money-in-cents number into exact integer cents. Fractional
/// cents (the deployment computes them from an integer quota) round to the
/// nearest cent — the cent is the smallest unit any renderer can show.
fn de_cents<'de, D>(d: D) -> Result<Cents, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = f64::deserialize(d)?;
    if !raw.is_finite() {
        return Err(serde::de::Error::custom(
            "orcarouter `total_usage` is not a finite number",
        ));
    }
    if raw < 0.0 {
        return Err(serde::de::Error::custom(
            "orcarouter `total_usage` cannot be negative",
        ));
    }
    if raw > i64::MAX as f64 {
        return Err(serde::de::Error::custom(
            "orcarouter `total_usage` is out of range",
        ));
    }
    Ok(Cents(raw.round() as i64))
}

/// Parse an optional wire money-in-USD field into exact integer cents. The
/// unlimited sentinel maps to `Cents(0)`, which `limit_cents` filters out
/// alongside any other non-positive limit — an unlimited key is no limit at
/// all, never a $100M one.
fn de_opt_usd_cents<'de, D>(d: D) -> Result<Option<Cents>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Option<f64> = Option::deserialize(d)?;
    raw.map(|v| {
        if v == UNLIMITED_LIMIT_USD {
            Ok(Cents(0))
        } else if !v.is_finite() || v < 0.0 {
            Err(format!(
                "orcarouter `limit` is not finite and non-negative: {v}"
            ))
        } else {
            // The field is USD; the snapshot is exact cents. 12.5 USD → 1250.
            let cents = v * 100.0;
            if cents > i64::MAX as f64 {
                Err(format!("orcarouter `limit` is out of range: {v}"))
            } else {
                Ok(Cents(cents.round() as i64))
            }
        }
    })
    .transpose()
    .map_err(serde::de::Error::custom)
}

#[cfg(test)]
mod tests {
    use super::*;

    const USAGE: &str = r#"{"object":"list","total_usage":275}"#;
    const SUBSCRIPTION: &str = r#"{
        "object":"billing_subscription",
        "has_payment_method":true,
        "soft_limit_usd":12.5,
        "hard_limit_usd":12.5,
        "system_hard_limit_usd":12.5,
        "access_until":1790000000
    }"#;

    #[test]
    fn parses_usage_verbatim_shape() {
        let usage: UsageResponse = serde_json::from_str(USAGE).unwrap();
        assert_eq!(usage.object.as_deref(), Some("list"));
        assert_eq!(usage.total_usage.0, 275);
    }

    #[test]
    fn parses_usage_without_object() {
        let usage: UsageResponse = serde_json::from_str(r#"{"total_usage":0}"#).unwrap();
        assert!(usage.object.is_none());
        assert_eq!(usage.total_usage.0, 0);
    }

    #[test]
    fn missing_total_usage_is_schema_drift_not_zero() {
        assert!(serde_json::from_str::<UsageResponse>(r#"{"object":"list"}"#).is_err());
        assert!(serde_json::from_str::<UsageResponse>(r#"{}"#).is_err());
    }

    /// The wire unit is cents: 275 must reach the snapshot as 275 cents, which
    /// formats as $2.75 — never as $275.00.
    #[test]
    fn combine_keeps_cents_exact() {
        let usage: UsageResponse = serde_json::from_str(USAGE).unwrap();
        let sub: SubscriptionResponse = serde_json::from_str(SUBSCRIPTION).unwrap();
        let snap = combine(&usage, &sub);
        assert_eq!(snap.spent_cents, 275);
        assert!((snap.spent_usd() - 2.75).abs() < 1e-9);
        assert_eq!(snap.limit_cents, Some(1250));
        assert_eq!(snap.remaining_cents(), Some(975));
        assert!((snap.remaining_usd().unwrap() - 9.75).abs() < 1e-9);
        assert_eq!(snap.consumed_pct(), Some(22));
        assert_eq!(
            snap.access_until,
            chrono::DateTime::from_timestamp(1_790_000_000, 0)
        );
    }

    #[test]
    fn fractional_cents_round_to_the_nearest_cent() {
        let usage: UsageResponse = serde_json::from_str(r#"{"total_usage":275.4}"#).unwrap();
        assert_eq!(usage.total_usage.0, 275);
        let usage: UsageResponse = serde_json::from_str(r#"{"total_usage":275.5}"#).unwrap();
        assert_eq!(usage.total_usage.0, 276);
    }

    #[test]
    fn invalid_usage_money_is_schema_drift() {
        for raw in ["-1", "null", "true", r#""275""#, "1e400"] {
            let body = format!(r#"{{"total_usage":{raw}}}"#);
            assert!(
                serde_json::from_str::<UsageResponse>(&body).is_err(),
                "{raw}"
            );
        }
    }

    #[test]
    fn parses_subscription_without_optional_fields() {
        let sub: SubscriptionResponse = serde_json::from_str(
            r#"{"soft_limit_usd":5.0,"hard_limit_usd":5.0,"system_hard_limit_usd":5.0}"#,
        )
        .unwrap();
        assert!(sub.object.is_none());
        assert!(sub.has_payment_method.is_none());
        assert_eq!(sub.access_until, 0);
        assert!(sub.access_until_at().is_none());
        assert_eq!(sub.limit_cents(), Some(500));
    }

    #[test]
    fn limit_falls_back_when_only_some_fields_arrive() {
        let sub: SubscriptionResponse =
            serde_json::from_str(r#"{"system_hard_limit_usd":7.5}"#).unwrap();
        assert_eq!(sub.limit_cents(), Some(750));
        let none: SubscriptionResponse = serde_json::from_str("{}").unwrap();
        assert_eq!(none.limit_cents(), None);
    }

    /// The sentinel is `100000000` in the limit fields. It must collapse to
    /// "no limit" — a spend-only card — never a $100,000,000.00 wallet.
    #[test]
    fn unlimited_sentinel_is_no_limit_not_one_hundred_million() {
        let sub: SubscriptionResponse = serde_json::from_str(
            r#"{"soft_limit_usd":100000000,"hard_limit_usd":100000000,"system_hard_limit_usd":100000000}"#,
        )
        .unwrap();
        assert_eq!(sub.limit_cents(), None);

        let usage: UsageResponse = serde_json::from_str(r#"{"total_usage":275}"#).unwrap();
        let snap = combine(&usage, &sub);
        assert_eq!(snap.limit_cents, None);
        assert_eq!(snap.remaining_cents(), None);
        assert_eq!(snap.remaining_usd(), None);
        assert_eq!(snap.consumed_pct(), None);
        assert_eq!(snap.spent_cents, 275);
    }

    #[test]
    fn access_until_zero_means_no_expiry() {
        let sub: SubscriptionResponse =
            serde_json::from_str(r#"{"hard_limit_usd":5.0,"access_until":0}"#).unwrap();
        assert!(sub.access_until_at().is_none());
    }

    #[test]
    fn invalid_limit_money_is_schema_drift() {
        let body = r#"{"hard_limit_usd":-5.0}"#;
        assert!(serde_json::from_str::<SubscriptionResponse>(body).is_err());
    }

    /// Spend past the limit keeps a signed remaining, exactly like OpenRouter
    /// debt: the number is real and the user must top up.
    #[test]
    fn overrun_keeps_a_negative_remaining() {
        let snap = OrcaRouterSnapshot {
            spent_cents: 1300,
            limit_cents: Some(1250),
            access_until: None,
        };
        assert_eq!(snap.remaining_cents(), Some(-50));
        assert_eq!(snap.consumed_pct(), Some(100));
    }
}
