//! Anthropic Admin API vendor — month-to-date spend from
//! `/v1/organizations/cost_report` over a Console **Admin key** (distinct from
//! the inference key and from the Claude Code OAuth login). The remaining
//! prepaid credit balance is Console-only (no API), so this reports spend.

pub mod fetch;
pub mod types;
pub mod vendor;

pub use fetch::{FetchOutcome, fetch_snapshot};
