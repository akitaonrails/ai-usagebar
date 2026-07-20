//! Wire types for DeepSeek's `/user/balance` endpoint.

use serde::Deserialize;

use crate::error::{AppError, Result};
use crate::usage::DeepseekSnapshot;

#[derive(Debug, Clone, Deserialize)]
pub struct BalanceResponse {
    pub is_available: bool,
    pub balance_infos: Vec<BalanceInfo>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BalanceInfo {
    pub currency: String,
    pub total_balance: String,
    pub granted_balance: String,
    pub topped_up_balance: String,
}

impl BalanceResponse {
    /// Project the wire response into the canonical snapshot.
    ///
    /// Every monetary field is required: an empty or error body used to
    /// deserialize into a confident $0.00 that then overwrote a good cache.
    /// Drift must surface as a schema error instead.
    pub fn into_snapshot(self) -> Result<DeepseekSnapshot> {
        // Prefer USD, fall back to CNY, then whatever's first.
        let info = self
            .balance_infos
            .iter()
            .find(|b| b.currency == "USD")
            .or_else(|| self.balance_infos.iter().find(|b| b.currency == "CNY"))
            .or_else(|| self.balance_infos.first())
            .ok_or_else(|| {
                AppError::Schema("deepseek: response carried no balance records".into())
            })?;

        Ok(DeepseekSnapshot {
            is_available: self.is_available,
            balance: parse_money(&info.total_balance, "total_balance")?,
            granted: parse_money(&info.granted_balance, "granted_balance")?,
            topped_up: parse_money(&info.topped_up_balance, "topped_up_balance")?,
            currency: info.currency.clone(),
        })
    }
}

/// DeepSeek sends amounts as decimal strings. `"NaN"` and `"inf"` both parse
/// successfully in Rust, so the finiteness check is load-bearing — an
/// infinite balance would render and cache as a plausible-looking number.
fn parse_money(s: &str, field: &str) -> Result<f64> {
    let n: f64 = s.trim().parse().map_err(|_| {
        AppError::Schema(format!("deepseek balance '{field}' is not numeric: {s:?}"))
    })?;
    if n.is_finite() {
        Ok(n)
    } else {
        Err(AppError::Schema(format!(
            "deepseek balance '{field}' is not finite"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_balance_response() {
        let raw = r#"{
            "is_available": true,
            "balance_infos": [
                {"currency": "CNY", "total_balance": "10.00", "granted_balance": "10.00", "topped_up_balance": "0.00"},
                {"currency": "USD", "total_balance": "1.50", "granted_balance": "1.50", "topped_up_balance": "0.00"}
            ]
        }"#;
        let r: BalanceResponse = serde_json::from_str(raw).unwrap();
        let snap = r.into_snapshot().unwrap();
        assert!(snap.is_available);
        assert_eq!(snap.currency, "USD");
        assert!((snap.balance - 1.50).abs() < 1e-9);
    }

    #[test]
    fn fallback_to_cny_when_no_usd() {
        let raw = r#"{
            "is_available": true,
            "balance_infos": [
                {"currency": "CNY", "total_balance": "20.00", "granted_balance": "20.00", "topped_up_balance": "0.00"}
            ]
        }"#;
        let r: BalanceResponse = serde_json::from_str(raw).unwrap();
        let snap = r.into_snapshot().unwrap();
        assert_eq!(snap.currency, "CNY");
        assert!((snap.balance - 20.0).abs() < 1e-9);
    }

    /// The legitimate zero: a drained account still renders, it is not an error.
    #[test]
    fn genuine_zero_balance_is_preserved() {
        let raw = r#"{
            "is_available": false,
            "balance_infos": [
                {"currency": "USD", "total_balance": "0.00", "granted_balance": "0.00", "topped_up_balance": "0.00"}
            ]
        }"#;
        let r: BalanceResponse = serde_json::from_str(raw).unwrap();
        let snap = r.into_snapshot().unwrap();
        assert!(!snap.is_available);
        assert_eq!(snap.balance, 0.0);
        assert_eq!(snap.currency, "USD");
    }

    #[test]
    fn empty_balance_infos_is_schema_error() {
        let raw = r#"{"is_available": false, "balance_infos": []}"#;
        let r: BalanceResponse = serde_json::from_str(raw).unwrap();
        assert!(matches!(r.into_snapshot(), Err(AppError::Schema(_))));
    }

    #[test]
    fn non_numeric_amount_is_schema_error() {
        let raw = r#"{
            "is_available": true,
            "balance_infos": [
                {"currency": "USD", "total_balance": "n/a", "granted_balance": "0.00", "topped_up_balance": "0.00"}
            ]
        }"#;
        let r: BalanceResponse = serde_json::from_str(raw).unwrap();
        assert!(matches!(r.into_snapshot(), Err(AppError::Schema(_))));
    }

    #[test]
    fn empty_amount_is_schema_error() {
        let raw = r#"{
            "is_available": true,
            "balance_infos": [
                {"currency": "USD", "total_balance": "", "granted_balance": "0.00", "topped_up_balance": "0.00"}
            ]
        }"#;
        let r: BalanceResponse = serde_json::from_str(raw).unwrap();
        assert!(matches!(r.into_snapshot(), Err(AppError::Schema(_))));
    }

    /// Rust parses these to real `f64` values, so only the finiteness check
    /// catches them.
    #[test]
    fn non_finite_amounts_are_schema_errors() {
        for amount in ["NaN", "inf", "-inf", "infinity"] {
            let raw = format!(
                r#"{{"is_available": true, "balance_infos": [
                    {{"currency": "USD", "total_balance": "{amount}",
                      "granted_balance": "0.00", "topped_up_balance": "0.00"}}
                ]}}"#
            );
            let r: BalanceResponse = serde_json::from_str(&raw).unwrap();
            assert!(
                matches!(r.into_snapshot(), Err(AppError::Schema(_))),
                "{amount} should be rejected"
            );
        }
    }

    /// A non-`total_balance` component is just as unvouchable as the headline.
    #[test]
    fn non_numeric_component_is_schema_error() {
        let raw = r#"{
            "is_available": true,
            "balance_infos": [
                {"currency": "USD", "total_balance": "5.00", "granted_balance": "5.00", "topped_up_balance": "oops"}
            ]
        }"#;
        let r: BalanceResponse = serde_json::from_str(raw).unwrap();
        assert!(matches!(r.into_snapshot(), Err(AppError::Schema(_))));
    }

    #[test]
    fn empty_body_does_not_deserialize() {
        assert!(serde_json::from_str::<BalanceResponse>("{}").is_err());
    }

    #[test]
    fn error_body_does_not_deserialize() {
        let raw = r#"{"error": {"message": "Authentication Fails"}}"#;
        assert!(serde_json::from_str::<BalanceResponse>(raw).is_err());
    }

    #[test]
    fn missing_money_field_does_not_deserialize() {
        let raw = r#"{
            "is_available": true,
            "balance_infos": [{"currency": "USD", "total_balance": "5.00"}]
        }"#;
        assert!(serde_json::from_str::<BalanceResponse>(raw).is_err());
    }
}
