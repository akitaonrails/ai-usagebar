//! Z.AI / BigModel vendor — undocumented `/api/monitor/usage/quota/limit`.
//! Auth header is `Authorization: <KEY>` with NO `Bearer` prefix.

pub mod fetch;
pub mod types;
pub mod vendor;

pub use fetch::{FetchOutcome, fetch_snapshot};
pub use vendor::ZaiVendor;
