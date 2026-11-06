//! Mistral AI — monthly API and Vibe Code budgets from the billing routes the
//! official `mistral-vibe` CLI uses.
//!
//! The documented Admin API is Enterprise-only. The two routes need a key whose
//! account holds the `see_org_inference_usage` permission; a valid key without
//! it is reported as a permission problem, not a rejected credential.
//!
//! - `GET /v1/billing/budget` — the monthly API and Vibe Code allowances.
//!   `vibe_budget` is required; `api_budget` is optional, because some plans
//!   do not report a separate API allowance.
//! - `GET /v1/billing/subscription` — the plan name and pay-as-you-go flags.
//!
//! The subscription read is best-effort: a failure there costs the plan label,
//! never the budgets.

pub mod fetch;
pub mod types;
pub mod vendor;

pub use fetch::fetch_snapshot;
