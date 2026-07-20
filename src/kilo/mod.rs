//! Kilo Code vendor — credit balance from `/api/profile/balance` over an API
//! key (an undocumented endpoint used by the Kilo Code extension; see
//! `types.rs`).

pub mod fetch;
pub mod types;
pub mod vendor;

pub use fetch::{FetchOutcome, fetch_snapshot};
