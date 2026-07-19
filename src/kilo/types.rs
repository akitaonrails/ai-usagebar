//! Wire types for Kilo Code's `/api/profile/balance`.
//!
//! The balance endpoint is **undocumented** (used internally by the Kilo Code
//! extension, not in the public gateway API reference). Confirmed against
//! `Kilo-Org/kilocode` `packages/kilo-gateway/src/api/profile.ts`: the response
//! is `{ "balance": <number> }` where the number is a **plain USD decimal**
//! (the extension renders it as `${balance.toFixed(2)}` with no conversion —
//! it is NOT microdollars, despite older third-party notes).

use serde::Deserialize;

use crate::usage::KiloSnapshot;

/// `GET /api/profile/balance` → `{ "balance": 12.34 }` (USD).
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct BalanceData {
    pub balance: f64,
}

/// Project the wire balance into the canonical snapshot. Kilo doesn't expose a
/// purchased-total via this endpoint, so there's no consumed-% — just the
/// remaining USD balance.
pub fn to_snapshot(balance: BalanceData) -> KiloSnapshot {
    KiloSnapshot {
        label: "Kilo".to_string(),
        balance: balance.balance,
    }
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
    fn missing_balance_defaults_to_zero() {
        let d: BalanceData = serde_json::from_str("{}").unwrap();
        assert_eq!(d.balance, 0.0);
    }

    #[test]
    fn to_snapshot_carries_balance_and_labels_kilo() {
        let snap = to_snapshot(BalanceData { balance: 8.42 });
        assert_eq!(snap.label, "Kilo");
        assert_eq!(snap.balance, 8.42);
    }
}
