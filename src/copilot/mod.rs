//! GitHub Copilot — quota from `api.github.com/copilot_internal/user`, using
//! either an explicit token or the locally-authenticated `gh` CLI as the
//! credential owner.

pub mod fetch;
pub mod types;
pub mod vendor;

pub use fetch::{FetchOutcome, fetch_snapshot, fetch_snapshot_with_account};
