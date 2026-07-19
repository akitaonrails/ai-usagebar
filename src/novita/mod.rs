//! Novita AI vendor — account credit balance from
//! `/openapi/v1/billing/balance/detail` over an API key.

pub mod fetch;
pub mod types;
pub mod vendor;

pub use fetch::{FetchOutcome, fetch_snapshot};
