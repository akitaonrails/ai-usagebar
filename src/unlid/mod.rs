//! Unlid vendor — account balance from `GET /v1/me` over an API key.

pub mod fetch;
pub mod types;
pub mod vendor;

pub use fetch::{FetchOutcome, fetch_snapshot};
