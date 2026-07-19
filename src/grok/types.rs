//! Wire types for xAI's Management API prepaid-balance endpoint.
//!
//! Confirmed against the official docs
//! (<https://docs.x.ai/developers/rest-api-reference/management/billing>):
//! `GET /v1/billing/teams/{team_id}/prepaid/balance` returns `{ changes[], total }`
//! where `total.val` is a **string of USD cents**. It's an *inverted ledger*:
//! a top-up shows as a negative value, so the remaining balance in dollars is
//! `-cents / 100` (sign per community reverse-engineering — verify against a
//! live account). `total` has no currency field (USD is implied).

use serde::Deserialize;

use crate::usage::GrokSnapshot;

/// `GET /auth/management-keys/validation` — used to discover the team id from
/// the management key when the user hasn't configured one explicitly.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Validation {
    // `Option` so an explicit JSON `null` (which a non-team-scoped key may
    // return) deserializes to `None` instead of failing the whole parse.
    #[serde(rename = "scopeId")]
    pub scope_id: Option<String>,
    #[serde(rename = "teamId")]
    pub team_id: Option<String>,
}

impl Validation {
    /// Prefer `scopeId` (current); fall back to the deprecated `teamId`. Blank
    /// or absent values are treated as "no team".
    pub fn resolved_team(&self) -> Option<String> {
        self.scope_id
            .as_deref()
            .filter(|s| !s.is_empty())
            .or(self.team_id.as_deref().filter(|s| !s.is_empty()))
            .map(str::to_string)
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Amount {
    pub val: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct BalanceResp {
    pub total: Amount,
}

pub fn to_snapshot(b: BalanceResp) -> GrokSnapshot {
    let cents: f64 = b.total.val.trim().parse().unwrap_or(0.0);
    // Inverted ledger: negative total => credit remaining.
    GrokSnapshot {
        balance: -cents / 100.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_total_is_positive_remaining_balance() {
        // Docs example: a $10 top-up shows total.val = "-1000" (cents).
        let b: BalanceResp = serde_json::from_str(r#"{"total":{"val":"-1000"}}"#).unwrap();
        assert!((to_snapshot(b).balance - 10.0).abs() < 1e-9);
    }

    #[test]
    fn empty_total_is_zero() {
        let b: BalanceResp = serde_json::from_str("{}").unwrap();
        assert_eq!(to_snapshot(b).balance, 0.0);
    }

    #[test]
    fn validation_prefers_scope_id() {
        let v = Validation {
            scope_id: Some("scope-1".into()),
            team_id: Some("team-1".into()),
        };
        assert_eq!(v.resolved_team().as_deref(), Some("scope-1"));
        let v2 = Validation {
            scope_id: Some("".into()),
            team_id: Some("team-2".into()),
        };
        assert_eq!(v2.resolved_team().as_deref(), Some("team-2"));
    }

    #[test]
    fn validation_tolerates_null_and_missing() {
        // Explicit null must not fail the parse.
        let v: Validation =
            serde_json::from_str(r#"{"scopeId":null,"teamId":"team-x"}"#).unwrap();
        assert_eq!(v.resolved_team().as_deref(), Some("team-x"));
        // Both absent → no team.
        let empty: Validation = serde_json::from_str("{}").unwrap();
        assert_eq!(empty.resolved_team(), None);
    }
}
