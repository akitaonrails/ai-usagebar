//! Mistral billing wire types and their projection into [`MistralSnapshot`].
//!
//! The budget response carries up to two independent monthly allowances:
//! `api_budget` (Studio / API usage) and `vibe_budget` (Vibe Code). Both are
//! percentages of an allowance in the response's `currency` with a
//! calendar-month reset. The official `mistral-vibe` client only requires
//! `vibe_budget`; `api_budget` is optional here because some plans do not
//! expose it. The top-level fields mirror `api_budget`; they are ignored so the
//! two budgets cannot disagree about which figure is which.
//!
//! The subscription response is optional. It supplies the plan label and the
//! pay-as-you-go flags; a missing or malformed subscription leaves the plan
//! unnamed and falls back to the budget's own flags.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};
use crate::usage::{MistralSnapshot, UsageWindow};

/// `GET /v1/billing/subscription`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Subscription {
    pub plan_name: String,
    #[serde(default)]
    pub payg_enabled: bool,
    #[serde(default)]
    pub vibe_payg_enabled: bool,
    #[serde(default)]
    pub enterprise: bool,
}

/// `GET /v1/billing/budget`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Budget {
    /// Absent on plans that do not report a separate API allowance.
    #[serde(default)]
    pub api_budget: Option<BudgetDetail>,
    /// Present on every response observed so far; the official client's type
    /// declares it as optional, so it is documented rather than enforced —
    /// see `BudgetDetail::currency`.
    pub vibe_budget: BudgetDetail,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BudgetDetail {
    pub usage_percentage: f64,
    pub initial_budget: f64,
    /// ISO-4217 code the allowance is denominated in. Every observed response
    /// carries it, but the official client's `BudgetDetail` declares no
    /// `currency` field, so a missing one falls back to [`DEFAULT_CURRENCY`]
    /// rather than failing the read.
    #[serde(default = "default_currency")]
    pub currency: String,
    #[serde(default)]
    pub reset_at: Option<String>,
    #[serde(default)]
    pub payg_enabled: bool,
}

/// What the vendor cache stores: the budget plus the subscription when the
/// optional second read answered. Keeping both in one payload means a cached
/// or stale read still carries the plan it was fetched with.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CachedPayload {
    /// SHA-256 fingerprint of the endpoint and key that produced this
    /// payload, never the key itself. The cache must fail closed when the
    /// configured credential changes, or it serves another account's budgets.
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscription: Option<Subscription>,
    pub budget: Budget,
}

impl CachedPayload {
    pub fn snapshot(&self) -> Result<MistralSnapshot> {
        let api_detail = self.budget.api_budget.as_ref();
        let api = api_detail.map(BudgetDetail::window).transpose()?;
        let vibe = self.budget.vibe_budget.window()?;
        let currency = self.budget.vibe_budget.currency.clone();
        validate_currency(&currency)?;
        if let Some(api_detail) = api_detail {
            validate_currency(&api_detail.currency)?;
            if !api_detail.currency.eq_ignore_ascii_case(&currency) {
                return Err(AppError::Schema(
                    "Mistral budgets report different currencies".into(),
                ));
            }
        }
        let api_payg = api_detail.is_some_and(|detail| detail.payg_enabled)
            || (api.is_some()
                && self
                    .subscription
                    .as_ref()
                    .is_some_and(|subscription| subscription.payg_enabled));
        let vibe_payg = self.budget.vibe_budget.payg_enabled
            || self
                .subscription
                .as_ref()
                .is_some_and(|subscription| subscription.vibe_payg_enabled);
        Ok(MistralSnapshot {
            plan: self
                .subscription
                .as_ref()
                .map(|subscription| subscription.plan_name.clone())
                .filter(|plan| !plan.is_empty()),
            api,
            vibe,
            api_budget: api_detail.map(|detail| detail.initial_budget),
            vibe_budget: self.budget.vibe_budget.initial_budget,
            currency,
            api_payg,
            vibe_payg,
        })
    }
}

impl BudgetDetail {
    fn window(&self) -> Result<UsageWindow> {
        if !self.usage_percentage.is_finite() || self.usage_percentage < 0.0 {
            return Err(AppError::Schema(
                "Mistral budget percentage is not a non-negative finite number".into(),
            ));
        }
        if !self.initial_budget.is_finite() || self.initial_budget < 0.0 {
            return Err(AppError::Schema(
                "Mistral budget allowance is not a non-negative finite number".into(),
            ));
        }
        Ok(UsageWindow {
            utilization_pct: self.usage_percentage.clamp(0.0, 100.0).round() as i32,
            resets_at: self.reset_at.as_deref().map(parse_instant).transpose()?,
            // A calendar month has no fixed length; pacing against a guessed
            // 30 days would be worse than no pacing at all.
            window_duration: chrono::Duration::zero(),
        })
    }
}

fn parse_instant(raw: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw.trim())
        .map(|instant| instant.with_timezone(&Utc))
        .map_err(|_| AppError::Schema("Mistral budget reset is not RFC 3339".into()))
}

/// Fallback when a response omits `currency`; `validate_currency` still runs
/// on whatever ends up here.
const DEFAULT_CURRENCY: &str = "USD";

fn default_currency() -> String {
    DEFAULT_CURRENCY.to_string()
}

fn validate_currency(currency: &str) -> Result<()> {
    if currency.len() == 3 && currency.bytes().all(|byte| byte.is_ascii_uppercase()) {
        Ok(())
    } else {
        Err(AppError::Schema(format!(
            "Mistral budget currency {currency:?} is not a 3-letter uppercase code"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUDGET: &str = r#"{
        "vibe_budget": {
            "usage_percentage": 0.0,
            "initial_budget": 150.0,
            "currency": "USD",
            "reset_at": "2026-12-01T00:00:00Z",
            "payg_enabled": false
        },
        "api_budget": {
            "usage_percentage": 15.6,
            "initial_budget": 15.0,
            "currency": "USD",
            "reset_at": "2026-12-01T00:00:00Z",
            "payg_enabled": false
        },
        "usage_percentage": 15.6,
        "initial_budget": 15.0,
        "currency": "USD",
        "reset_at": "2026-12-01T00:00:00Z"
    }"#;

    const SUBSCRIPTION: &str =
        r#"{"plan_name":"EDU","payg_enabled":false,"vibe_payg_enabled":false,"enterprise":false}"#;

    fn payload() -> CachedPayload {
        CachedPayload {
            target: "https://api.mistral.ai/v1/billing/budget|key:test".into(),
            subscription: Some(serde_json::from_str(SUBSCRIPTION).unwrap()),
            budget: serde_json::from_str(BUDGET).unwrap(),
        }
    }

    #[test]
    fn projects_both_budgets_and_the_plan() {
        let snapshot = payload().snapshot().unwrap();
        assert_eq!(snapshot.plan.as_deref(), Some("EDU"));
        assert_eq!(snapshot.api.as_ref().map(|w| w.utilization_pct), Some(16));
        assert_eq!(snapshot.vibe.utilization_pct, 0);
        assert!((snapshot.api_budget.unwrap() - 15.0).abs() < 1e-9);
        assert!((snapshot.vibe_budget - 150.0).abs() < 1e-9);
        assert_eq!(snapshot.currency, "USD");
        assert_eq!(
            snapshot.api.as_ref().and_then(|w| w.resets_at),
            Some("2026-12-01T00:00:00Z".parse().unwrap())
        );
        assert!(!snapshot.api_payg);
        assert!(!snapshot.vibe_payg);
    }

    #[test]
    fn a_missing_api_budget_leaves_only_the_vibe_window() {
        let mut payload = payload();
        payload.budget.api_budget = None;
        let snapshot = payload.snapshot().unwrap();
        assert!(snapshot.api.is_none());
        assert!(snapshot.api_budget.is_none());
        assert_eq!(snapshot.vibe.utilization_pct, 0);
        assert!((snapshot.vibe_budget - 150.0).abs() < 1e-9);
        assert_eq!(snapshot.currency, "USD");
    }

    // The official client's `BudgetDetail` declares no `currency` field, so a
    // response without one must not fail the whole read.
    #[test]
    fn a_response_without_currency_falls_back_to_usd() {
        let body = r#"{"api_budget":{"usage_percentage":16.0,"initial_budget":10.0},
            "vibe_budget":{"usage_percentage":0.0,"initial_budget":150.0}}"#;
        let budget: Budget = serde_json::from_str(body).unwrap();
        assert_eq!(budget.vibe_budget.currency, DEFAULT_CURRENCY);
        let payload = CachedPayload {
            budget,
            subscription: None,
            target: "t".into(),
        };
        let snapshot = payload.snapshot().unwrap();
        assert_eq!(snapshot.currency, DEFAULT_CURRENCY);
    }

    #[test]
    fn a_euro_account_keeps_its_currency() {
        let mut payload = payload();
        payload.budget.api_budget.as_mut().unwrap().currency = "EUR".into();
        payload.budget.vibe_budget.currency = "EUR".into();
        let snapshot = payload.snapshot().unwrap();
        assert_eq!(snapshot.currency, "EUR");
    }

    #[test]
    fn a_missing_subscription_keeps_the_budgets_and_leaves_the_plan_empty() {
        let mut payload = payload();
        payload.subscription = None;
        let snapshot = payload.snapshot().unwrap();
        assert_eq!(snapshot.plan, None);
        assert_eq!(snapshot.api.map(|w| w.utilization_pct), Some(16));
        assert_eq!(snapshot.vibe.utilization_pct, 0);
    }

    #[test]
    fn subscription_payg_flags_are_a_fallback_for_the_budget_flags() {
        let mut payload = payload();
        payload.subscription = Some(
            serde_json::from_str(
                r#"{"plan_name":"Pro","payg_enabled":true,"vibe_payg_enabled":true}"#,
            )
            .unwrap(),
        );
        let snapshot = payload.snapshot().unwrap();
        assert!(snapshot.api_payg);
        assert!(snapshot.vibe_payg);
    }

    #[test]
    fn over_budget_clamps_to_a_full_meter() {
        let mut payload = payload();
        payload.budget.api_budget.as_mut().unwrap().usage_percentage = 140.0;
        let snapshot = payload.snapshot().unwrap();
        assert_eq!(snapshot.api.map(|w| w.utilization_pct), Some(100));
    }

    #[test]
    fn a_negative_percentage_is_a_schema_error() {
        let mut payload = payload();
        payload.budget.api_budget.as_mut().unwrap().usage_percentage = -1.0;
        let error = payload.snapshot().unwrap_err().to_string();
        assert!(error.contains("non-negative"), "{error}");
    }

    #[test]
    fn a_negative_allowance_is_a_schema_error() {
        let mut payload = payload();
        payload.budget.vibe_budget.initial_budget = -1.0;
        let error = payload.snapshot().unwrap_err().to_string();
        assert!(error.contains("allowance"), "{error}");
    }

    #[test]
    fn a_bad_currency_is_a_schema_error() {
        let mut payload = payload();
        payload.budget.api_budget.as_mut().unwrap().currency = "usd".into();
        let error = payload.snapshot().unwrap_err().to_string();
        assert!(error.contains("currency"), "{error}");
    }

    #[test]
    fn mismatched_budget_currencies_are_a_schema_error() {
        let mut payload = payload();
        payload.budget.vibe_budget.currency = "EUR".into();
        let error = payload.snapshot().unwrap_err().to_string();
        assert!(error.contains("different currencies"), "{error}");
    }

    #[test]
    fn a_bad_reset_is_a_schema_error() {
        let mut payload = payload();
        payload.budget.api_budget.as_mut().unwrap().reset_at = Some("not-a-time".into());
        let error = payload.snapshot().unwrap_err().to_string();
        assert!(error.contains("RFC 3339"), "{error}");
    }

    #[test]
    fn a_missing_reset_is_allowed() {
        let mut payload = payload();
        payload.budget.api_budget.as_mut().unwrap().reset_at = None;
        let snapshot = payload.snapshot().unwrap();
        assert_eq!(snapshot.api.and_then(|w| w.resets_at), None);
    }
}
