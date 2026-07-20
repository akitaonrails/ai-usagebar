//! Wire types for Moonshot / Kimi's `/v1/users/me/balance`.
//!
//! Confirmed against the official docs (platform.moonshot.ai/docs/api/balance
//! and the `.cn` equivalent). The response wraps the balance in
//! `{ code, data, scode, status }`; the three `data.*` fields are JSON numbers.
//! `available_balance` = cash + voucher (the spendable figure; `<= 0` blocks the
//! inference API). There is **no currency field** — the unit is USD on
//! `api.moonshot.ai` and CNY on `api.moonshot.cn`, so the caller supplies it.

use serde::Deserialize;

use crate::error::{AppError, Result};
use crate::usage::{MoonshotSnapshot, finite_amount};

/// The documented envelope. `code`/`status` are the API's **in-band failure
/// indicators**: a 200 response can still carry `status: false` with a non-zero
/// `code`, in which case `data` is not a balance and must not be shown as one.
#[derive(Debug, Clone, Deserialize)]
pub struct BalanceEnvelope {
    pub code: i64,
    pub data: BalanceData,
    pub status: bool,
}

impl BalanceEnvelope {
    /// Reject the documented failure shape before any field is read as money.
    pub fn check_ok(&self) -> Result<()> {
        if self.status && self.code == 0 {
            return Ok(());
        }
        Err(AppError::Schema(format!(
            "moonshot: API reported failure (code {}, status {})",
            self.code, self.status
        )))
    }
}

/// All three balances are documented as always present. Defaulting a missing
/// one to zero would report a fabricated balance as authoritative.
#[derive(Debug, Clone, Deserialize)]
pub struct BalanceData {
    pub available_balance: f64,
    pub voucher_balance: f64,
    pub cash_balance: f64,
}

pub fn to_snapshot(data: BalanceData, currency: &str) -> Result<MoonshotSnapshot> {
    Ok(MoonshotSnapshot {
        available: finite_amount("moonshot", "available_balance", data.available_balance)?,
        voucher: finite_amount("moonshot", "voucher_balance", data.voucher_balance)?,
        cash: finite_amount("moonshot", "cash_balance", data.cash_balance)?,
        currency: currency.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_documented_envelope() {
        let raw = r#"{
            "code": 0,
            "data": {
                "available_balance": 49.58894,
                "voucher_balance": 46.58893,
                "cash_balance": 3.00001
            },
            "scode": "0x0",
            "status": true
        }"#;
        let env: BalanceEnvelope = serde_json::from_str(raw).unwrap();
        assert_eq!(env.code, 0);
        assert!(env.status);
        env.check_ok().unwrap();
        let snap = to_snapshot(env.data, "USD").unwrap();
        assert!((snap.available - 49.58894).abs() < 1e-9);
        assert!((snap.voucher - 46.58893).abs() < 1e-9);
        assert!((snap.cash - 3.00001).abs() < 1e-9);
        assert_eq!(snap.currency, "USD");
    }

    #[test]
    fn missing_message_field_is_fine() {
        // The docs example has no `message` field; parsing must not require it.
        let raw = r#"{"code":0,"data":{"available_balance":1.0,"voucher_balance":0.0,
            "cash_balance":1.0},"status":true}"#;
        let env: BalanceEnvelope = serde_json::from_str(raw).unwrap();
        assert_eq!(env.data.available_balance, 1.0);
    }

    #[test]
    fn in_band_failure_is_rejected() {
        // A 200 carrying the documented failure indicators is not a balance.
        let raw = r#"{"code":40100,"data":{"available_balance":0.0,
            "voucher_balance":0.0,"cash_balance":0.0},"status":false}"#;
        let env: BalanceEnvelope = serde_json::from_str(raw).unwrap();
        assert!(env.check_ok().is_err());

        // status true but a non-zero code is equally untrustworthy.
        let raw2 = r#"{"code":500,"data":{"available_balance":0.0,
            "voucher_balance":0.0,"cash_balance":0.0},"status":true}"#;
        let env2: BalanceEnvelope = serde_json::from_str(raw2).unwrap();
        assert!(env2.check_ok().is_err());
    }

    #[test]
    fn missing_fields_are_a_schema_error_not_zero() {
        assert!(serde_json::from_str::<BalanceEnvelope>("{}").is_err());
        // Envelope present but `data` incomplete is drift, not a zero balance.
        assert!(
            serde_json::from_str::<BalanceEnvelope>(
                r#"{"code":0,"data":{"available_balance":1.0},"status":true}"#
            )
            .is_err()
        );
    }

    #[test]
    fn non_finite_balance_is_rejected() {
        let d = BalanceData {
            available_balance: f64::INFINITY,
            voucher_balance: 0.0,
            cash_balance: 0.0,
        };
        assert!(to_snapshot(d, "USD").is_err());
    }
}
