//! SuperGrok (xAI subscription OAuth) usage through the official Grok Build
//! CLI's billing surface.
//!
//! Two transports for the usage figures, plus one for banked resets:
//! - `direct` — one HTTPS call to the documented `cli-chat-proxy.grok.com`
//!   billing endpoint using the login's `key` (needed since grok CLI 1.0.13
//!   dropped the ACP extension; primary path).
//! - `acp` — the CLI's `x.ai/billing` ACP extension (fallback for CLI builds
//!   where the proxy endpoint is unreachable or reshaped).
//! - `resets` — `grok.com` `ConsumerUiSvc/GetRemainingResets`, a gRPC-Web
//!   call authenticated with the same `key`. Failures are swallowed: a
//!   broken extra call must not take the usage figures with it.
//!
//! ai-usagebar never copies, caches, refreshes, or writes Grok tokens. The
//! `key` exists only inside the outgoing Authorization headers of those
//! requests; login files are read solely as bytes (key lookup or one-way
//! cache-scope digest) and account selection, OIDC rotation, and
//! `auth.json.lock` stay with Grok Build.
//!
//! Distinct from the `grok` vendor, which reads **prepaid Management API**
//! balance with a management key — SuperGrok is the subscription quota path.

pub mod acp;
pub mod direct;
pub mod fetch;
mod resets;
pub mod scope;
pub mod types;
pub mod vendor;

pub use fetch::{FetchOutcome, fetch_snapshot};

#[cfg(test)]
mod guard_tests {
    /// Both outgoing calls here carry the login's long-lived key, so neither
    /// may take its host from the environment — an ambient variable in
    /// whatever session Waybar inherited would choose where the key goes.
    ///
    /// One test over the whole module rather than a copy inside each file:
    /// `direct.rs` and `resets.rs` each grew their own, and a third endpoint
    /// would have grown a third. A new file is covered the moment it lands.
    #[test]
    fn no_credential_bearing_request_takes_its_host_from_the_environment() {
        let mut offenders = Vec::new();
        for file in crate::guard::rs_files_in("src/supergrok") {
            let source = std::fs::read_to_string(&file).expect("readable module");
            let production = crate::guard::production_code(&source);
            if production.contains("Authorization") && production.contains("env::var") {
                offenders.push(file.display().to_string());
            }
        }
        assert!(
            offenders.is_empty(),
            "a request carrying the login key must use a fixed host; tests reach \
             the seam through an explicit parameter. Found: {offenders:#?}"
        );
    }
}
