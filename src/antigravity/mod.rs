//! Google Antigravity vendor — quota tracking via the local language server.

pub mod cloud;
pub mod credential;
pub mod fetch;
pub mod vendor;

pub use fetch::fetch_snapshot;
