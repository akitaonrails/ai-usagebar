//! DeepRelay subscription plan: four weighted-token meters, a dollar usage
//! ceiling, and an optional pay-as-you-go wallet.
//!
//! Everything comes from `GET /v1/billing/subscription` (scope
//! `billing:read`); the wallet is a second request made only when it can
//! change what the user sees. The rolling weekly caps carry no reset instant by
//! design, and are never given a synthesized one.

pub mod fetch;
pub mod types;
pub mod vendor;

pub use fetch::{FetchOutcome, fetch_snapshot};
