//! Wire types for Novita AI's `/openapi/v1/billing/balance/detail`.
//!
//! Confirmed against the official docs
//! (<https://novita.ai/docs/api-reference/basic-get-user-balance>): every
//! monetary field is a **JSON string** holding an integer in **1/10000 USD**
//! (`"10000"` = $1.00). The account credit balance is `availableBalance`.
//! (The older third-party `/v1/user` + `credit_balance` note is stale/wrong.)

use serde::Deserialize;

use crate::usage::NovitaSnapshot;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct BalanceData {
    #[serde(rename = "availableBalance")]
    pub available_balance: String,
    #[serde(rename = "cashBalance")]
    pub cash_balance: String,
    #[serde(rename = "creditLimit")]
    pub credit_limit: String,
    #[serde(rename = "outstandingInvoices")]
    pub outstanding_invoices: String,
}

/// Parse a "1/10000 USD" integer-string field into dollars. Non-numeric or
/// empty values degrade to `0.0` rather than failing the whole fetch.
fn to_usd(s: &str) -> f64 {
    s.trim().parse::<f64>().unwrap_or(0.0) / 10_000.0
}

pub fn to_snapshot(b: BalanceData) -> NovitaSnapshot {
    NovitaSnapshot {
        available: to_usd(&b.available_balance),
        cash: to_usd(&b.cash_balance),
        credit_limit: to_usd(&b.credit_limit),
        outstanding: to_usd(&b.outstanding_invoices),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_string_amounts_as_10000ths_usd() {
        let raw = r#"{
            "availableBalance":"1000000",
            "cashBalance":"800000",
            "creditLimit":"200000",
            "pendingCharges":"0",
            "outstandingInvoices":"0"
        }"#;
        let d: BalanceData = serde_json::from_str(raw).unwrap();
        let snap = to_snapshot(d);
        assert_eq!(snap.available, 100.0);
        assert_eq!(snap.cash, 80.0);
        assert_eq!(snap.credit_limit, 20.0);
        assert_eq!(snap.outstanding, 0.0);
    }

    #[test]
    fn missing_or_blank_fields_default_to_zero() {
        let d: BalanceData = serde_json::from_str("{}").unwrap();
        let snap = to_snapshot(d);
        assert_eq!(snap.available, 0.0);
    }

    #[test]
    fn fractional_10000ths_round_trip() {
        // 12345 tenthousandths = $1.2345
        let snap = to_snapshot(BalanceData {
            available_balance: "12345".into(),
            ..Default::default()
        });
        assert!((snap.available - 1.2345).abs() < 1e-9);
    }
}
