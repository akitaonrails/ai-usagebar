//! Kiro CLI — credit-based quota read from kiro-cli's own cached AWS SSO OIDC
//! or social-login session (`db.rs`) against `AmazonCodeWhispererService.GetUsageLimits`,
//! the same call kiro-cli's own `/usage` slash command makes. See `oauth.rs`
//! for the token-refresh flows and `fetch.rs`/`types.rs` for the wire call and
//! schema.

pub mod db;
pub mod fetch;
pub mod oauth;
pub mod types;
pub mod vendor;

pub use fetch::{FetchOutcome, fetch_snapshot};
