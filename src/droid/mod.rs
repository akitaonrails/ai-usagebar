//! Read-only Factory `droid` CLI quota provider.
//!
//! This provider reuses the login the official `droid` CLI already wrote: on
//! Linux the session lives in `~/.factory/auth.v2.keyring`, sealed with a key
//! from the Secret Service keyring. It never logs in, refreshes, or writes
//! credentials itself — near expiry it can ask the CLI to renew its own
//! login (`renew`) — and its HTTP client rejects all redirects so the token
//! is never forwarded through one.

pub mod creds;
pub mod fetch;
pub mod renew;
pub mod types;
pub mod vendor;

use crate::config::DroidConfig;
use crate::error::Result;

pub use creds::keyring_path;

/// Resolve the CLI's stored login. The config holds no paths: the provider
/// only supports the one place the CLI writes on Linux.
pub fn resolve_credentials(_cfg: &DroidConfig) -> Result<creds::Credentials> {
    creds::resolve()
}
