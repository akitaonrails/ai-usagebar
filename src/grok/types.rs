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

use crate::error::{AppError, Result};
use crate::usage::{GrokSnapshot, parse_amount};

/// The scope a management key is issued against
/// (<https://docs.x.ai/developers/rest-api-reference/management/auth>).
/// `scopeId` means different things per variant, which is exactly why it cannot
/// be used as a team id unconditionally.
pub const SCOPE_TEAM: &str = "SCOPE_TEAM";
pub const SCOPE_ORGANIZATION: &str = "SCOPE_ORGANIZATION";

/// `GET /auth/management-keys/validation` — used to discover the team id from
/// the management key when the user hasn't configured one explicitly.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Validation {
    /// `SCOPE_TEAM` or `SCOPE_ORGANIZATION`. Absent on older responses.
    pub scope: Option<String>,
    // `Option` so an explicit JSON `null` (which a non-team-scoped key may
    // return) deserializes to `None` instead of failing the whole parse.
    #[serde(rename = "scopeId")]
    pub scope_id: Option<String>,
    #[serde(rename = "teamId")]
    pub team_id: Option<String>,
}

impl Validation {
    /// Resolve the team to bill against.
    ///
    /// `scopeId` is only a team id when the key is **team-scoped**. For an
    /// organization-scoped key it is the *organization* id, and using it as a
    /// team would query a URL that does not identify the user's team — so we
    /// ask for an explicit `team_id` instead of guessing.
    pub fn resolved_team(&self) -> Result<String> {
        let non_empty =
            |s: &Option<String>| s.as_deref().filter(|v| !v.is_empty()).map(String::from);
        let scope = self
            .scope
            .as_deref()
            .unwrap_or("")
            .trim()
            .to_ascii_uppercase();

        if scope == SCOPE_ORGANIZATION {
            // The deprecated `teamId` is still authoritative when present.
            return non_empty(&self.team_id).ok_or_else(|| {
                AppError::Other(
                    "grok: this management key is organization-scoped, so its `scopeId` is an \
                     organization id, not a team. Set `team_id` under [grok] in config to the \
                     team you want the prepaid balance for."
                        .into(),
                )
            });
        }

        // Team-scoped (or a legacy response with no `scope` at all): `scopeId`
        // is the team, with the deprecated `teamId` as fallback.
        if scope.is_empty() || scope == SCOPE_TEAM {
            return non_empty(&self.scope_id)
                .or_else(|| non_empty(&self.team_id))
                .ok_or_else(|| {
                    AppError::Other(
                        "grok: could not resolve team_id from the management key; \
                         set `team_id` under [grok] in config"
                            .into(),
                    )
                });
        }

        Err(AppError::Other(format!(
            "grok: unrecognized management-key scope {scope:?}; \
             set `team_id` under [grok] in config"
        )))
    }
}

/// `val` is **required**: a 200 error envelope must not read as a $0.00 balance.
#[derive(Debug, Clone, Deserialize)]
pub struct Amount {
    pub val: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BalanceResp {
    pub total: Amount,
}

pub fn to_snapshot(b: BalanceResp) -> Result<GrokSnapshot> {
    let cents = parse_amount("grok", "total.val", &b.total.val)?;
    // Inverted ledger: negative total => credit remaining.
    Ok(GrokSnapshot {
        balance: -cents / 100.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_total_is_positive_remaining_balance() {
        // Docs example: a $10 top-up shows total.val = "-1000" (cents).
        let b: BalanceResp = serde_json::from_str(r#"{"total":{"val":"-1000"}}"#).unwrap();
        assert!((to_snapshot(b).unwrap().balance - 10.0).abs() < 1e-9);
    }

    #[test]
    fn empty_or_malformed_total_is_a_schema_error_not_zero() {
        // A 200 error envelope must not be read as "you have $0.00".
        assert!(serde_json::from_str::<BalanceResp>("{}").is_err());
        assert!(serde_json::from_str::<BalanceResp>(r#"{"total":{}}"#).is_err());
        let b: BalanceResp = serde_json::from_str(r#"{"total":{"val":""}}"#).unwrap();
        assert!(to_snapshot(b).is_err());
        let b2: BalanceResp = serde_json::from_str(r#"{"total":{"val":"n/a"}}"#).unwrap();
        assert!(to_snapshot(b2).is_err());
    }

    #[test]
    fn team_scoped_key_uses_scope_id() {
        let v = Validation {
            scope: Some(SCOPE_TEAM.into()),
            scope_id: Some("team-1".into()),
            team_id: None,
        };
        assert_eq!(v.resolved_team().unwrap(), "team-1");
    }

    #[test]
    fn organization_scoped_key_does_not_use_scope_id_as_a_team() {
        // The regression this guards: scopeId here is an ORG id. Querying
        // /v1/billing/teams/<org-id>/... is not this user's team.
        let v = Validation {
            scope: Some(SCOPE_ORGANIZATION.into()),
            scope_id: Some("org-123".into()),
            team_id: None,
        };
        let err = v.resolved_team().unwrap_err().to_string();
        assert!(!err.contains("org-123"), "must not adopt the org id: {err}");
        assert!(
            err.contains("team_id"),
            "must tell the user what to do: {err}"
        );

        // An explicit teamId alongside an org-scoped key is still usable.
        let v2 = Validation {
            scope: Some(SCOPE_ORGANIZATION.into()),
            scope_id: Some("org-123".into()),
            team_id: Some("team-9".into()),
        };
        assert_eq!(v2.resolved_team().unwrap(), "team-9");
    }

    #[test]
    fn legacy_response_without_scope_still_resolves() {
        // Explicit null must not fail the parse.
        let v: Validation = serde_json::from_str(r#"{"scopeId":null,"teamId":"team-x"}"#).unwrap();
        assert_eq!(v.resolved_team().unwrap(), "team-x");
        let v2: Validation = serde_json::from_str(r#"{"scopeId":"team-y"}"#).unwrap();
        assert_eq!(v2.resolved_team().unwrap(), "team-y");
        // Nothing to go on → an actionable error, not a silently wrong URL.
        let empty: Validation = serde_json::from_str("{}").unwrap();
        assert!(empty.resolved_team().is_err());
    }

    #[test]
    fn unknown_scope_is_rejected() {
        let v = Validation {
            scope: Some("SCOPE_GALAXY".into()),
            scope_id: Some("x-1".into()),
            team_id: None,
        };
        assert!(v.resolved_team().is_err());
    }
}
