//! Anthropic Admin API vendor — month-to-date spend from
//! `/v1/organizations/cost_report` using a Console key with organization report
//! permissions, separate from the Claude Code OAuth login. The remaining
//! prepaid credit balance is Console-only (no API), so this reports spend.

pub mod fetch;
pub mod types;
pub mod vendor;

pub use fetch::{FetchOutcome, fetch_snapshot};
