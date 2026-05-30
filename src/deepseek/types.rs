//! Wire types for DeepSeek's `/user/balance` endpoint.

use serde::Deserialize;

use crate::usage::DeepseekSnapshot;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct BalanceResponse {
    pub is_available: bool,
    pub balance_infos: Vec<BalanceInfo>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct BalanceInfo {
    pub currency: String,
    pub total_balance: String,
    pub granted_balance: String,
    pub topped_up_balance: String,
}

impl BalanceResponse {
    pub fn into_snapshot(self) -> DeepseekSnapshot {
        // Prefer USD, fall back to CNY, then whatever's first.
        let info = self
            .balance_infos
            .iter()
            .find(|b| b.currency == "USD")
            .or_else(|| self.balance_infos.iter().find(|b| b.currency == "CNY"))
            .or_else(|| self.balance_infos.first())
            .cloned()
            .unwrap_or_default();

        DeepseekSnapshot {
            is_available: self.is_available,
            balance: parse_f64(&info.total_balance),
            granted: parse_f64(&info.granted_balance),
            topped_up: parse_f64(&info.topped_up_balance),
            currency: info.currency,
        }
    }
}

fn parse_f64(s: &str) -> f64 {
    s.trim().parse::<f64>().unwrap_or(0.0)
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
        let snap = r.into_snapshot();
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
        let snap = r.into_snapshot();
        assert_eq!(snap.currency, "CNY");
        assert!((snap.balance - 20.0).abs() < 1e-9);
    }

    #[test]
    fn empty_balance_infos() {
        let raw = r#"{"is_available": false, "balance_infos": []}"#;
        let r: BalanceResponse = serde_json::from_str(raw).unwrap();
        let snap = r.into_snapshot();
        assert!(!snap.is_available);
        assert_eq!(snap.balance, 0.0);
        assert_eq!(snap.currency, "");
    }
}
