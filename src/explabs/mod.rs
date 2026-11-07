//! Experiential Labs: the daily token pool, the credit wallet, and the key's
//! own cap, from the management API's usage reads.
//!
//! Everything comes from `GET /api/gateway/usage/daily` (scope `usage:read`)
//! and `GET /api/v1/credits` (balance = `total_credits - total_usage`); the
//! key's cap is `GET /api/v1/key`. The daily meters reset at 00:00 UTC by
//! design, and are never given a synthesized reset. The free-tier rate limits
//! clear on their own and are reported, not enforced: 10 requests/minute per
//! free-core model, 250,000 tokens/minute shared, 60 requests/minute for a new
//! account.

pub mod fetch;
pub mod types;
pub mod vendor;

pub use fetch::{FetchOutcome, fetch_snapshot};
