//! macOS Keychain access for Claude Code OAuth credentials.
//!
//! On Linux the Claude CLI writes its OAuth state to
//! `~/.claude/.credentials.json`. On macOS, recent Claude Code builds instead
//! store the *same* `{ "claudeAiOauth": …, "mcpOAuth": … }` JSON as a generic
//! password item in the login Keychain (service `Claude Code-credentials`), so
//! the file never exists and a naive read fails with an I/O error.
//!
//! Reads use the built-in `security(1)` tool, while normal writes use the
//! macOS-native `security-framework` API so credential JSON is never exposed
//! through process arguments. The dependency is macOS-gated, keeping Linux
//! builds untouched.
//!
//! A `CLAUDE_CONFIG_DIR`-scoped login (`CLAUDE_CONFIG_DIR=<dir> claude`, the
//! mechanism `accounts_dir` documents) also lands in the Keychain rather than
//! `<dir>/.credentials.json` — under a *different* service name, `Claude
//! Code-credentials-<hash>`, where `<hash>` is the first 8 hex chars of the
//! SHA-256 of the config dir's absolute path (verified empirically against a
//! real install). [`read_raw_for`]/[`write_raw_for`] target that per-account
//! item so named accounts can find it without ever reading the *default*
//! item — a hash tied to the account's own directory can't collide with a
//! different account's, which is what issue #15 needed the strict
//! `Explicit`-only rule to avoid in the first place.
//!
//! The `*_service` workers are `pub(crate)` because the sync token
//! ([`crate::sync::github::keychain`]) stores itself through these same three
//! under its own service name. The read/write account-selector agreement below
//! is a rule with one copy in this crate, not two — a second implementation is
//! a second place for it to drift back apart.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::error::{AppError, Result};

/// Generic-password *service* name Claude Code uses for the credentials blob.
const SERVICE: &str = "Claude Code-credentials";

/// The per-account service name for a `CLAUDE_CONFIG_DIR`-scoped login. Shells
/// out to `shasum(1)` rather than pulling in a `sha2` crate — same rationale
/// as the rest of this module.
fn service_name_for(config_dir: &Path) -> Result<String> {
    let mut child = Command::new("/usr/bin/shasum")
        .args(["-a", "256"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| AppError::Other(format!("could not run `shasum`: {e}")))?;
    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(config_dir.display().to_string().as_bytes())
        .map_err(|e| AppError::Other(format!("could not run `shasum`: {e}")))?;
    let out = child
        .wait_with_output()
        .map_err(|e| AppError::Other(format!("could not run `shasum`: {e}")))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let hash = stdout
        .split_whitespace()
        .next()
        .and_then(|h| h.get(..8))
        .ok_or_else(|| AppError::Other("shasum produced unexpected output".into()))?;
    Ok(format!("{SERVICE}-{hash}"))
}

/// The Keychain item's *account* is the macOS short username. We match on it
/// when updating so we touch exactly the item Claude Code created.
///
/// `None` when `$USER` is unset or empty: read and write must then agree to
/// select by service alone. Previously the read omitted `-a` while the write
/// passed `-a ""`, so a refresh could create a *second*, empty-account item
/// that the read would never find again.
fn account() -> Option<String> {
    std::env::var("USER").ok().filter(|u| !u.is_empty())
}

/// `security` exits with the raw OSStatus. 44 is `errSecItemNotFound`.
const ERR_SEC_ITEM_NOT_FOUND: i32 = 44;

/// Read the raw credentials JSON from the login Keychain.
///
/// Returns `Ok(None)` only when the item genuinely does not exist, so callers
/// can fall through to the file path / a "run `claude`" error. Every other
/// `security` failure is an `Err`: a locked Keychain or a denied ACL is not the
/// same as "you are not logged in", and reporting it as such sent users off to
/// re-authenticate when the credentials were there all along.
pub fn read_raw() -> Result<Option<String>> {
    read_raw_service(SERVICE)
}

/// Same as [`read_raw`], but for a named account's `CLAUDE_CONFIG_DIR`-scoped
/// Keychain item instead of the default one.
pub fn read_raw_for(config_dir: &Path) -> Result<Option<String>> {
    read_raw_service(&service_name_for(config_dir)?)
}

pub(crate) fn read_raw_service(service: &str) -> Result<Option<String>> {
    find_generic_password(service, true)
}

/// Is the item there — without asking for what it holds?
///
/// `find-generic-password` **without `-w`** returns the item's attributes and
/// never its data, so macOS does not consult the item's ACL and cannot raise
/// the "ai-usagebar wants to use your confidential information" prompt. That is
/// what makes an existence check payable by `ai-usagebar sync status`, which
/// the macOS menu bar runs on every menu open, when a value read is not.
///
/// ponytail: an item that exists but holds an empty value answers `true`
/// here and is skipped by the planner, which reads values. Establishing that
/// difference costs the value, and therefore the prompt.
pub fn has_raw() -> Result<bool> {
    find_generic_password(SERVICE, false).map(|found| found.is_some())
}

/// `security find-generic-password`, selecting exactly the item the write path
/// updates. `with_value` adds `-w` — the flag that asks for the secret, and
/// therefore the only reason macOS consults the item's ACL.
///
/// `Ok(None)` is `errSecItemNotFound` and nothing else; every other failure is
/// an `Err` so a locked Keychain never reads as "not logged in".
fn find_generic_password(service: &str, with_value: bool) -> Result<Option<String>> {
    let mut cmd = Command::new("/usr/bin/security");
    cmd.args(["find-generic-password", "-s", service]);
    if with_value {
        cmd.arg("-w");
    }
    if let Some(acct) = account() {
        cmd.args(["-a", &acct]);
    }

    let out = cmd
        .output()
        .map_err(|e| AppError::Other(format!("could not run `security`: {e}")))?;

    if !out.status.success() {
        if out.status.code() == Some(ERR_SEC_ITEM_NOT_FOUND) {
            return Ok(None);
        }
        let detail = String::from_utf8_lossy(&out.stderr);
        let detail = detail.trim();
        return Err(AppError::Credentials(format!(
            "could not read the Claude credentials from the macOS Keychain \
             (security exited {}): {}. If the login Keychain is locked, unlock \
             it and retry; if access was denied, allow ai-usagebar when prompted.",
            out.status.code().unwrap_or(-1),
            if detail.is_empty() {
                "no detail"
            } else {
                detail
            }
        )));
    }

    let value = String::from_utf8(out.stdout)
        .map_err(|e| AppError::Other(format!("Keychain value was not UTF-8: {e}")))?;
    let value = value.trim_end_matches('\n').to_string();
    if value.is_empty() {
        Ok(None)
    } else {
        Ok(Some(value))
    }
}

/// Persist updated credentials JSON back to the *same* Keychain item, so the
/// widget and Claude Code keep sharing a single source of truth (mirroring how
/// they share one file on Linux).
///
/// A native write requires the same account selector used by the read path.
/// Fail closed if `$USER` is unavailable rather than falling back to the
/// `security(1)` argv form and exposing the OAuth JSON to process inspection.
pub fn write_raw(json: &str) -> Result<()> {
    write_raw_service(SERVICE, json)
}

/// Same as [`write_raw`], but for a named account's `CLAUDE_CONFIG_DIR`-scoped
/// Keychain item instead of the default one.
pub fn write_raw_for(config_dir: &Path, json: &str) -> Result<()> {
    write_raw_service(&service_name_for(config_dir)?, json)
}

/// Remove the default Claude Code credential. Used only while rolling back an
/// account switch that started from an empty default slot.
pub fn delete_raw() -> Result<()> {
    delete_raw_service(SERVICE)
}

/// Remove a named account's config-dir-scoped credential after moving it into
/// the default slot. Keeping both copies would let two Claude processes rotate
/// the same refresh-token lineage independently.
pub fn delete_raw_for(config_dir: &Path) -> Result<()> {
    delete_raw_service(&service_name_for(config_dir)?)
}

pub(crate) fn write_raw_service(service: &str, json: &str) -> Result<()> {
    // Must mirror `read_raw`'s selection exactly, or an update can create a
    // second item the read will never find. The native API updates by this
    // exact (service, account) pair without exposing the secret in argv.
    if let Some(acct) = account() {
        return security_framework::passwords::set_generic_password(
            service,
            &acct,
            json.as_bytes(),
        )
        .map_err(|e| {
            AppError::Credentials(format!(
                "failed to update the Claude credentials in the macOS Keychain: {e}"
            ))
        });
    }

    Err(AppError::Credentials(
        "cannot safely update the Claude credentials in the macOS Keychain because USER is unset"
            .into(),
    ))
}

pub(crate) fn delete_raw_service(service: &str) -> Result<()> {
    let mut cmd = Command::new("/usr/bin/security");
    cmd.args(["delete-generic-password", "-s", service]);
    if let Some(acct) = account() {
        cmd.args(["-a", &acct]);
    }

    let out = cmd
        .output()
        .map_err(|e| AppError::Other(format!("could not run `security`: {e}")))?;
    if out.status.success() || out.status.code() == Some(ERR_SEC_ITEM_NOT_FOUND) {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&out.stderr);
    Err(AppError::Credentials(format!(
        "failed to remove the Claude credentials from the macOS Keychain \
         (security exited {}): {}",
        out.status.code().unwrap_or(-1),
        detail.trim()
    )))
}
