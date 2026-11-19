//! Google Antigravity vendor — quota tracking via the local language server.

pub mod agy;
pub mod cloud;
pub mod credential;
pub mod fetch;
pub mod statusline;
pub mod vendor;

pub use fetch::fetch_snapshot;

/// Shared identity for local RPC and status-line readings of one account.
pub(crate) fn account_hash(email: &str) -> String {
    use sha2::{Digest, Sha256};

    Sha256::digest(email.trim().to_lowercase().as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
