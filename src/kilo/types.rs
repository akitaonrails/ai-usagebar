//! Wire types for Kilo Code's `/api/profile/balance`.
//!
//! The balance endpoint is **undocumented** (used internally by the Kilo Code
//! extension, not in the public gateway API reference). Confirmed against
//! `Kilo-Org/kilocode` `packages/kilo-gateway/src/api/profile.ts`: the response
//! is `{ "balance": <number> }` where the number is a **plain USD decimal**
//! (the extension renders it as `${balance.toFixed(2)}` with no conversion —
//! it is NOT microdollars, despite older third-party notes).

use serde::Deserialize;

use crate::error::Result;
use crate::usage::{KiloSnapshot, finite_amount};

/// `GET /api/profile/balance` → `{ "balance": 12.34 }` (USD).
///
/// `balance` is **required**: a 200 response without it is an error envelope or
/// a schema change, not an account with no money. Defaulting it to zero would
/// cache a fabricated balance as authoritative.
#[derive(Debug, Clone, Deserialize)]
pub struct BalanceData {
    pub balance: f64,
}

/// Project the wire balance into the canonical snapshot. Kilo doesn't expose a
/// purchased-total via this endpoint, so there's no consumed-% — just the
/// remaining USD balance.
pub fn to_snapshot(balance: BalanceData) -> Result<KiloSnapshot> {
    Ok(KiloSnapshot {
        label: "Kilo".to_string(),
        balance: finite_amount("kilo", "balance", balance.balance)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_balance() {
        let raw = r#"{"balance":12.34}"#;
        let d: BalanceData = serde_json::from_str(raw).unwrap();
        assert_eq!(d.balance, 12.34);
    }

    #[test]
    fn missing_balance_is_a_schema_error_not_zero() {
        // A 200 error envelope must not be read as "you have $0.00".
        assert!(serde_json::from_str::<BalanceData>("{}").is_err());
        assert!(serde_json::from_str::<BalanceData>(r#"{"error":"forbidden"}"#).is_err());
    }

    #[test]
    fn non_finite_balance_is_rejected() {
        let d = BalanceData { balance: f64::NAN };
        assert!(to_snapshot(d).is_err());
    }

    #[test]
    fn to_snapshot_carries_balance_and_labels_kilo() {
        let snap = to_snapshot(BalanceData { balance: 8.42 }).unwrap();
        assert_eq!(snap.label, "Kilo");
        assert_eq!(snap.balance, 8.42);
    }
}
