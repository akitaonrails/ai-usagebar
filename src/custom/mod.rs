//! User-defined HTTP providers: a URL, a static token, and JSON pointers.
//!
//! Configured as `[[custom]]` tables (see `config::CustomProviderConfig`).
//! Everything vendor-specific that a built-in provider hard-codes — endpoint,
//! auth header, response schema — is data here, which is why the module has
//! a `mapping` where the others have a `types` mirroring an upstream API.

pub mod fetch;
pub mod mapping;
pub mod types;
pub mod vendor;

pub use fetch::fetch_snapshot;
