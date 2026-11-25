//! Wire type for Unlid's `GET /v1/me`.
//!
//! The body has no `data` wrapper and carries the key's identity beside three
//! amounts, all **JSON numbers in USD**: `balance_usd` (spendable), `held_usd`
//! (reserved by in-flight requests) and `month_spend_usd`.

use serde::Deserialize;

use crate::error::{AppError, Result};
use crate::usage::{UnlidSnapshot, finite_amount};

/// Every amount is **required**. A 200 body missing them is an error envelope
/// or a schema change — reporting it as a zero balance would cache a
/// fabricated figure as authoritative.
#[derive(Debug, Clone, Deserialize)]
pub struct MeData {
    pub balance_usd: f64,
    pub held_usd: f64,
    pub month_spend_usd: f64,
}

/// `held_usd` and `month_spend_usd` are totals; a negative one is drift, not
/// a figure to cache. `balance_usd` may legitimately go negative (overdrawn).
fn non_negative(field: &str, v: f64) -> Result<f64> {
    let v = finite_amount("unlid", field, v)?;
    if v < 0.0 {
        return Err(AppError::Schema(format!("unlid: `{field}` is negative")));
    }
    Ok(v)
}

pub fn to_snapshot(m: MeData) -> Result<UnlidSnapshot> {
    Ok(UnlidSnapshot {
        balance: finite_amount("unlid", "balance_usd", m.balance_usd)?,
        held: non_negative("held_usd", m.held_usd)?,
        month_spend: non_negative("month_spend_usd", m.month_spend_usd)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_unwrapped_me_body() {
        let raw = r#"{
            "key": {"id": "k-1", "name": "test", "prefix": "ul-test"},
            "balance_usd": 12.34,
            "held_usd": 0.5,
            "month_spend_usd": 6.78
        }"#;
        let snap = to_snapshot(serde_json::from_str(raw).unwrap()).unwrap();
        assert_eq!(snap.balance, 12.34);
        assert_eq!(snap.held, 0.5);
        assert_eq!(snap.month_spend, 6.78);
    }

    #[test]
    fn missing_or_non_numeric_amounts_are_a_schema_error_not_zero() {
        // An error envelope must not be read as "you have $0.00".
        assert!(serde_json::from_str::<MeData>("{}").is_err());
        assert!(
            serde_json::from_str::<MeData>(
                r#"{"error":{"message":"bad key","type":"authentication_error"}}"#
            )
            .is_err()
        );
        // A partial body is drift, not a balance.
        assert!(serde_json::from_str::<MeData>(r#"{"balance_usd":1.0}"#).is_err());
        // The amounts are numbers; a string is drift, not a balance.
        assert!(
            serde_json::from_str::<MeData>(
                r#"{"balance_usd":"1","held_usd":0,"month_spend_usd":0}"#
            )
            .is_err()
        );
    }

    #[test]
    fn a_negative_held_or_month_spend_is_a_schema_error() {
        let held = MeData {
            balance_usd: 1.0,
            held_usd: -0.01,
            month_spend_usd: 0.0,
        };
        assert!(matches!(to_snapshot(held), Err(AppError::Schema(_))));
        let spend = MeData {
            balance_usd: 1.0,
            held_usd: 0.0,
            month_spend_usd: -5.0,
        };
        assert!(matches!(to_snapshot(spend), Err(AppError::Schema(_))));
    }

    #[test]
    fn a_negative_balance_is_still_allowed() {
        let m = MeData {
            balance_usd: -2.5,
            held_usd: 0.0,
            month_spend_usd: 0.0,
        };
        assert_eq!(to_snapshot(m).unwrap().balance, -2.5);
    }

    #[test]
    fn a_non_finite_amount_is_a_schema_error() {
        let bad = MeData {
            balance_usd: f64::INFINITY,
            held_usd: 0.0,
            month_spend_usd: 0.0,
        };
        assert!(to_snapshot(bad).is_err());
    }
}
