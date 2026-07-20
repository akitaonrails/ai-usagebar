//! xAI (Grok) vendor — prepaid credit balance from the Management API
//! (`management-api.x.ai`) over a **management key** (distinct from the
//! inference key), optionally auto-resolving the team id.

pub mod fetch;
pub mod types;
pub mod vendor;

pub use fetch::{FetchOutcome, fetch_snapshot};
