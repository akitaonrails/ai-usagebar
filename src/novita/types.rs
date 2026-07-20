//! Wire types for Novita AI's `/openapi/v1/billing/balance/detail`.
//!
//! Confirmed against the official docs
//! (<https://novita.ai/docs/api-reference/basic-get-user-balance>): every
//! monetary field is a **JSON string** holding an integer in **1/10000 USD**
//! (`"10000"` = $1.00). The account credit balance is `availableBalance`.
//! (The older third-party `/v1/user` + `credit_balance` note is stale/wrong.)

use serde::Deserialize;

use crate::error::Result;
use crate::usage::{NovitaSnapshot, parse_amount};

/// Every field the endpoint documents is **required**. A 200 body missing them
/// is an error envelope or a schema change — reporting it as a zero balance
/// would cache a fabricated figure as authoritative.
#[derive(Debug, Clone, Deserialize)]
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
/// empty values are schema errors, not zeros.
fn to_usd(field: &str, s: &str) -> Result<f64> {
    Ok(parse_amount("novita", field, s)? / 10_000.0)
}

pub fn to_snapshot(b: BalanceData) -> Result<NovitaSnapshot> {
    Ok(NovitaSnapshot {
        available: to_usd("availableBalance", &b.available_balance)?,
        cash: to_usd("cashBalance", &b.cash_balance)?,
        credit_limit: to_usd("creditLimit", &b.credit_limit)?,
        outstanding: to_usd("outstandingInvoices", &b.outstanding_invoices)?,
    })
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
        let snap = to_snapshot(d).unwrap();
        assert_eq!(snap.available, 100.0);
        assert_eq!(snap.cash, 80.0);
        assert_eq!(snap.credit_limit, 20.0);
        assert_eq!(snap.outstanding, 0.0);
    }

    #[test]
    fn missing_fields_are_a_schema_error_not_zero() {
        // A 200 error envelope must not be read as "you have $0.00".
        assert!(serde_json::from_str::<BalanceData>("{}").is_err());
        assert!(
            serde_json::from_str::<BalanceData>(r#"{"message":"invalid key","code":401}"#).is_err()
        );
        // A partial body is drift, not a balance.
        assert!(serde_json::from_str::<BalanceData>(r#"{"availableBalance":"1"}"#).is_err());
    }

    #[test]
    fn blank_or_non_numeric_amount_is_a_schema_error() {
        let blank = BalanceData {
            available_balance: "".into(),
            cash_balance: "0".into(),
            credit_limit: "0".into(),
            outstanding_invoices: "0".into(),
        };
        assert!(to_snapshot(blank).is_err());

        let junk = BalanceData {
            available_balance: "n/a".into(),
            cash_balance: "0".into(),
            credit_limit: "0".into(),
            outstanding_invoices: "0".into(),
        };
        assert!(to_snapshot(junk).is_err());
    }

    #[test]
    fn fractional_10000ths_round_trip() {
        // 12345 tenthousandths = $1.2345
        let snap = to_snapshot(BalanceData {
            available_balance: "12345".into(),
            cash_balance: "0".into(),
            credit_limit: "0".into(),
            outstanding_invoices: "0".into(),
        })
        .unwrap();
        assert!((snap.available - 1.2345).abs() < 1e-9);
    }
}
