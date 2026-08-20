//! macOS Keychain storage for the sync token — service `ai-usagebar-sync-token`.
//!
//! The rule this module exists to follow is already implemented in
//! [`crate::anthropic::keychain`] and is deliberately **not** written a second
//! time here:
//!
//! - **Reads** go through `security(1)`. A read takes no secret, so a command
//!   line is harmless.
//! - **Writes** go through Security.framework. `security add-generic-password -w
//!   <token>` would put the token in `argv`, where every process on the machine
//!   can read it out of `ps`.
//! - The read and the write must select the item by the **same** (service,
//!   account) pair, and must fail closed when `$USER` is unset rather than
//!   falling back to the argv form.
//!
//! That last rule is why this file is six delegating lines: the read once
//! omitted `-a` while the write passed `-a ""`, so a refresh created a second,
//! empty-account item the read could never find again. `anthropic::keychain`'s
//! workers are already parameterized by service name and already carry the fix;
//! re-deriving them here would re-derive the bug.
//!
//! In production this is reached only through
//! [`TokenChain::keychain`](super::token::TokenChain::keychain) — a closure — so
//! no unit test ever touches the real login Keychain. The `*_service` trio is
//! public for the `#[ignore]`d round trip in `tests/live.rs`, which passes a
//! service name of its own so it cannot disturb a real stored token.

use zeroize::Zeroizing;

use crate::anthropic::keychain as worker;
use crate::error::Result;

/// The generic-password *service* name for the sync token, fixed by D-02.
///
/// This is the address of a secret on the user's machine: renaming it after
/// ship orphans every token already stored under the old name, with nothing
/// left that knows where to look.
pub const SERVICE: &str = "ai-usagebar-sync-token";

/// The chain's source 2, wrapped on the first line this module controls.
///
/// `security(1)`'s output arrives from the shared worker as a plain `String`;
/// this is where it becomes a [`Zeroizing<String>`], so `token`'s "the value is
/// zeroized end to end" is true of everything downstream of here (F-9). The
/// `String` is moved, never copied, so there is no second allocation left
/// behind unwiped.
pub fn read_raw() -> Result<Option<Zeroizing<String>>> {
    Ok(read_raw_service(SERVICE)?.map(Zeroizing::new))
}

pub fn write_raw(token: &str) -> Result<()> {
    write_raw_service(SERVICE, token)
}

pub fn delete_raw() -> Result<()> {
    delete_raw_service(SERVICE)
}

pub fn read_raw_service(service: &str) -> Result<Option<String>> {
    worker::read_raw_service(service)
}

pub fn write_raw_service(service: &str, token: &str) -> Result<()> {
    worker::write_raw_service(service, token)
}

pub fn delete_raw_service(service: &str) -> Result<()> {
    worker::delete_raw_service(service)
}
