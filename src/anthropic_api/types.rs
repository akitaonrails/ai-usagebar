//! Wire types for the Anthropic Admin API cost report
//! (`GET /v1/organizations/cost_report`).
//!
//! Confirmed against the official docs
//! (<https://platform.claude.com/docs/en/api/admin-api/usage-cost/get-cost-report>):
//! `amount` is a decimal STRING in the currency's LOWEST unit (cents) —
//! `"123.45"` USD represents `$1.23` — so divide by 100 for dollars. There is
//! no API for the remaining prepaid credit balance (Console dashboard only), so
//! this vendor reports **month-to-date spend** instead.
//!
//! Anthropic documents that this endpoint **excludes Priority Tier costs**
//! (<https://platform.claude.com/docs/en/manage-claude/usage-cost-api>), so for
//! an organization on Priority Tier the total here is below its real spend. The
//! tooltip, the TUI panel, and the README all say so — the number must never be
//! presented as complete spend.

use serde::Deserialize;

use crate::error::{AppError, Result};
use crate::usage::parse_amount;

/// The documented envelope. `data` and `has_more` are **required**: a 200 error
/// envelope, or a response whose shape drifted, must not deserialize into an
/// empty report that reads as genuine zero spend. Only `next_page` is optional,
/// because the docs return it as `null` on the last page.
///
/// An empty `data: []` remains the legitimate zero-cost case — a real month with
/// no usage — and is preserved as such.
#[derive(Debug, Clone, Deserialize)]
pub struct CostReport {
    pub data: Vec<Bucket>,
    pub has_more: bool,
    #[serde(default)]
    pub next_page: Option<String>,
}

/// A time bucket. `results` is required and may legitimately be empty (a day
/// with no spend).
#[derive(Debug, Clone, Deserialize)]
pub struct Bucket {
    pub results: Vec<CostResult>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CostResult {
    /// Cost in the currency's lowest unit (cents), as a decimal string.
    pub amount: String,
    /// The API currently documents every cost result as USD. Keep this required
    /// and validate it before summing so a future currency change cannot be
    /// silently rendered with a dollar sign.
    pub currency: String,
}

/// Sum every result's cents on one page and return dollars. A missing,
/// non-numeric, or non-finite `amount` raises `AppError::Schema` rather than
/// silently coercing to 0 — reporting a bogus $0.00 spend, and caching it as
/// authoritative, is the failure this guards against.
pub fn page_dollars(report: &CostReport) -> Result<f64> {
    let mut cents = 0.0_f64;
    for r in report.data.iter().flat_map(|b| b.results.iter()) {
        if r.currency != "USD" {
            return Err(AppError::Schema(format!(
                "anthropic-api: unsupported cost_report currency {:?}; expected USD",
                r.currency
            )));
        }
        cents += parse_amount("anthropic-api", "cost_report.amount", &r.amount)?;
    }
    // Guard the running total too: enough large values can overflow to inf.
    crate::usage::finite_amount("anthropic-api", "cost_report total", cents)?;
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
    fn a_month_with_no_usage_is_genuinely_zero() {
        // The legitimate zero case: a well-formed report with no buckets.
        let report: CostReport =
            serde_json::from_str(r#"{"data":[],"has_more":false,"next_page":null}"#).unwrap();
        assert_eq!(page_dollars(&report).unwrap(), 0.0);
        // A bucket with no results (a day with no spend) is equally valid.
        let report2: CostReport =
            serde_json::from_str(r#"{"data":[{"results":[]}],"has_more":false}"#).unwrap();
        assert_eq!(page_dollars(&report2).unwrap(), 0.0);
    }

    #[test]
    fn malformed_envelope_is_rejected_rather_than_read_as_zero_spend() {
        // The regression this guards: `{}` used to deserialize into an empty
        // report and display as a genuine $0.00 month.
        assert!(serde_json::from_str::<CostReport>("{}").is_err());
        // A 200 error envelope.
        assert!(
            serde_json::from_str::<CostReport>(r#"{"error":{"message":"invalid x-api-key"}}"#)
                .is_err()
        );
        // Drifted shape: `data` present but `has_more` gone.
        assert!(serde_json::from_str::<CostReport>(r#"{"data":[]}"#).is_err());
        // Drifted bucket: `results` gone.
        assert!(serde_json::from_str::<CostReport>(r#"{"data":[{}],"has_more":false}"#).is_err());
        // Drifted result: `amount` renamed away.
        assert!(
            serde_json::from_str::<CostReport>(
                r#"{"data":[{"results":[{"cost":"1"}]}],"has_more":false}"#
            )
            .is_err()
        );
    }

    #[test]
    fn non_numeric_amount_is_a_schema_error() {
        for bad in [r#""""#, r#""n/a""#, r#""  ""#] {
            let body = format!(
                r#"{{"data":[{{"results":[{{"amount":{bad},"currency":"USD"}}]}}],"has_more":false}}"#
            );
            let report: CostReport = serde_json::from_str(&body).unwrap();
            assert!(
                page_dollars(&report).is_err(),
                "amount {bad} should not parse as spend"
            );
        }
    }

    #[test]
    fn non_finite_amount_is_rejected() {
        // "inf" parses as f64::INFINITY — it must not become a displayed spend.
        let body = r#"{"data":[{"results":[{"amount":"inf","currency":"USD"}]}],"has_more":false}"#;
        let report: CostReport = serde_json::from_str(body).unwrap();
        assert!(page_dollars(&report).is_err());
    }

    #[test]
    fn non_usd_or_missing_currency_is_rejected() {
        let non_usd =
            r#"{"data":[{"results":[{"amount":"100","currency":"EUR"}]}],"has_more":false}"#;
        let report: CostReport = serde_json::from_str(non_usd).unwrap();
        assert!(page_dollars(&report).is_err());

        let missing = r#"{"data":[{"results":[{"amount":"100"}]}],"has_more":false}"#;
        assert!(serde_json::from_str::<CostReport>(missing).is_err());
    }
}
