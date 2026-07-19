//! Wire types for the Anthropic Admin API cost report
//! (`GET /v1/organizations/cost_report`).
//!
//! Confirmed against the official docs
//! (<https://platform.claude.com/docs/en/api/admin-api/usage-cost/get-cost-report>):
//! `amount` is a decimal STRING in the currency's LOWEST unit (cents) —
//! `"123.45"` USD represents `$1.23` — so divide by 100 for dollars. There is
//! no API for the remaining prepaid credit balance (Console dashboard only), so
//! this vendor reports **month-to-date spend** instead.

use serde::Deserialize;

use crate::error::{AppError, Result};

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct CostReport {
    pub data: Vec<Bucket>,
    pub has_more: bool,
    pub next_page: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Bucket {
    pub results: Vec<CostResult>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct CostResult {
    /// Cost in the currency's lowest unit (cents), as a decimal string.
    pub amount: String,
}

/// Sum every result's cents on one page and return dollars. A non-numeric
/// `amount` (e.g. a field-rename on schema drift, which `#[serde(default)]`
/// turns into an empty string) raises `AppError::Schema` rather than silently
/// coercing to 0 — so the fetch layer marks the cache stale and records the
/// drift instead of reporting a bogus $0.00.
pub fn page_dollars(report: &CostReport) -> Result<f64> {
    let mut cents = 0.0_f64;
    for r in report.data.iter().flat_map(|b| b.results.iter()) {
        cents += r.amount.trim().parse::<f64>().map_err(|e| {
            AppError::Schema(format!(
                "anthropic-api cost_report amount {:?}: {e}",
                r.amount
            ))
        })?;
    }
    Ok(cents / 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verbatim shape from the docs (amount in cents).
    const BODY: &str = r#"{
      "data": [
        { "starting_at": "2026-07-01T00:00:00Z", "ending_at": "2026-07-02T00:00:00Z",
          "results": [
            { "amount": "100.0", "currency": "USD", "cost_type": "tokens" },
            { "amount": "34.5", "currency": "USD", "cost_type": "web_search" }
          ] }
      ],
      "has_more": false,
      "next_page": null
    }"#;

    #[test]
    fn sums_cents_and_converts_to_dollars() {
        let report: CostReport = serde_json::from_str(BODY).unwrap();
        // 100.0 + 34.5 = 134.5 cents = $1.345
        assert!((page_dollars(&report).unwrap() - 1.345).abs() < 1e-9);
        assert!(!report.has_more);
    }

    #[test]
    fn empty_report_is_zero() {
        let report: CostReport = serde_json::from_str("{}").unwrap();
        assert_eq!(page_dollars(&report).unwrap(), 0.0);
    }

    #[test]
    fn non_numeric_amount_is_a_schema_error() {
        // Schema drift: `amount` renamed → serde default fills "" → parse fails.
        let body = r#"{"data":[{"results":[{"amount":""}]}]}"#;
        let report: CostReport = serde_json::from_str(body).unwrap();
        assert!(matches!(page_dollars(&report), Err(AppError::Schema(_))));
    }
}
