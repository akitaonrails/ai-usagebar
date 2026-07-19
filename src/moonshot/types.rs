//! Wire types for Moonshot / Kimi's `/v1/users/me/balance`.
//!
//! Confirmed against the official docs (platform.moonshot.ai/docs/api/balance
//! and the `.cn` equivalent). The response wraps the balance in
//! `{ code, data, scode, status }`; the three `data.*` fields are JSON numbers.
//! `available_balance` = cash + voucher (the spendable figure; `<= 0` blocks the
//! inference API). There is **no currency field** — the unit is USD on
//! `api.moonshot.ai` and CNY on `api.moonshot.cn`, so the caller supplies it.

use serde::Deserialize;

use crate::usage::MoonshotSnapshot;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct BalanceEnvelope {
    pub code: i64,
    pub data: BalanceData,
    pub status: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct BalanceData {
    pub available_balance: f64,
    pub voucher_balance: f64,
    pub cash_balance: f64,
}

pub fn to_snapshot(data: BalanceData, currency: &str) -> MoonshotSnapshot {
    MoonshotSnapshot {
        available: data.available_balance,
        voucher: data.voucher_balance,
        cash: data.cash_balance,
        currency: currency.to_string(),
    }
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
        let snap = to_snapshot(env.data, "USD");
        assert!((snap.available - 49.58894).abs() < 1e-9);
        assert!((snap.voucher - 46.58893).abs() < 1e-9);
        assert!((snap.cash - 3.00001).abs() < 1e-9);
        assert_eq!(snap.currency, "USD");
    }

    #[test]
    fn missing_message_field_is_fine() {
        // The docs example has no `message` field; parsing must not require it.
        let env: BalanceEnvelope =
            serde_json::from_str(r#"{"code":0,"data":{"available_balance":1.0},"status":true}"#)
                .unwrap();
        assert_eq!(env.data.available_balance, 1.0);
    }
}
