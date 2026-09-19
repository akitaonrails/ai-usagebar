//! macOS Keychain access for Claude Code OAuth credentials.
//!
//! On Linux the Claude CLI writes its OAuth state to
//! `~/.claude/.credentials.json`. On macOS, recent Claude Code builds instead
//! store the *same* `{ "claudeAiOauth": …, "mcpOAuth": … }` JSON as a generic
//! password item in the login Keychain (service `Claude Code-credentials`), so
//! the file never exists and a naive read fails with an I/O error.
//!
//! Reads, deletes and normal-sized writes go through the built-in `security(1)`
//! tool, because the *writer's* code identity is what macOS stamps onto the item's
//! XARA partition list. A native `SecItemAdd`/`SecItemUpdate` from this
//! process leaves the item owned by `cdhash:<ai-usagebar>`, and every later
//! read by `/usr/bin/security` — ours *and* Claude Code's — then trips
//! `ACL partition mismatch: client apple-tool:` and raises a Keychain dialog
//! that "Always Allow" cannot durably fix (that button edits the trusted-app
//! list, not the partition list). Going through `security(1)` keeps writer and
//! reader on the same `apple-tool:` partition. See issue #148.
//!
//! Credential JSON still never enters process arguments: the command is fed to
//! `security -i` on **stdin**, so argv is just `["/usr/bin/security", "-i"]`.
//! That interactive reader truncates an over-long line *and stores the
//! truncated value*, so [`SECURITY_STDIN_SAFE_MAX`] keeps us clear of the cap
//! and an oversized blob falls back to the native API — the one case where the
//! cdhash partition can still appear, and one no realistic credential reaches.
//! The `security-framework` dependency is macOS-gated, keeping Linux builds
//! untouched.
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

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::display::sanitize_untrusted_line;
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

fn read_raw_service(service: &str) -> Result<Option<String>> {
    let mut cmd = Command::new("/usr/bin/security");
    cmd.args(["find-generic-password", "-s", service, "-w"]);
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
        // `security` is a subprocess whose stderr is not this program's text.
        // It reaches a terminal verbatim, so an escape sequence in it repaints
        // the line and an embedded newline forges one.
        let detail = sanitize_untrusted_line(&String::from_utf8_lossy(&out.stderr));
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

/// Undocumented truncation cap measured for `security -i` on macOS 26.0.
const SECURITY_STDIN_MEASURED_CAP: usize = 4032;

/// Operational maximum for a fully composed stdin command. The 32-byte margin
/// below the measured, undocumented cap avoids relying on its exact boundary.
const SECURITY_STDIN_SAFE_MAX: usize = 4000;

const _: () = assert!(SECURITY_STDIN_SAFE_MAX < SECURITY_STDIN_MEASURED_CAP);

/// Quote one value for `security -i`'s line tokenizer, which honours backslash
/// escapes inside a double-quoted token (single quotes do not protect
/// backslashes).
///
/// `None` when the value contains a newline: that would end the line early and
/// let the remainder be read as a *further* `security` command. Serialized JSON
/// escapes its newlines, so this rejects only inputs that were never valid here.
fn quote_for_security_stdin(value: &str) -> Option<String> {
    if value.contains('\n') || value.contains('\r') {
        return None;
    }
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        if ch == '\\' || ch == '"' {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('"');
    Some(out)
}

/// Compose the `add-generic-password` line for [`write_via_security_stdin`].
///
/// `None` when any component cannot be quoted, so the caller falls back rather
/// than shipping a half-escaped command.
fn compose_write_command(service: &str, account: &str, json: &str) -> Option<String> {
    Some(format!(
        "add-generic-password -U -a {} -s {} -w {}\n",
        quote_for_security_stdin(account)?,
        quote_for_security_stdin(service)?,
        quote_for_security_stdin(json)?,
    ))
}

fn command_fits_security_stdin(command: &str) -> bool {
    command.len() <= SECURITY_STDIN_SAFE_MAX
}

/// Quote every component first, then apply the operational limit to the final
/// command bytes that the interactive reader will actually consume.
fn command_for_security_stdin(service: &str, account: &str, json: &str) -> Option<String> {
    let command = compose_write_command(service, account, json)?;
    command_fits_security_stdin(&command).then_some(command)
}

/// Feed one composed command to `security -i` over stdin, keeping the secret
/// out of argv.
fn write_via_security_stdin(command: &str) -> Result<()> {
    let mut child = Command::new("/usr/bin/security")
        .arg("-i")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| AppError::Other(format!("could not run `security`: {e}")))?;
    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(command.as_bytes())
        .map_err(|e| AppError::Other(format!("could not run `security`: {e}")))?;
    let out = child
        .wait_with_output()
        .map_err(|e| AppError::Other(format!("could not run `security`: {e}")))?;
    if out.status.success() {
        return Ok(());
    }
    let detail = sanitize_untrusted_line(&String::from_utf8_lossy(&out.stderr));
    Err(AppError::Credentials(format!(
        "failed to update the Claude credentials in the macOS Keychain \
         (security exited {}): {}",
        out.status.code().unwrap_or(-1),
        {
            let detail = detail.trim();
            if detail.is_empty() {
                "no detail".to_string()
            } else {
                detail.to_string()
            }
        }
    )))
}

/// Last resort for a blob too large for the `security -i` line cap. This is the
/// write that stamps the item with our `cdhash:` partition (issue #148), so it
/// runs only when the alternative is putting the credential in argv.
fn write_via_native_api(service: &str, account: &str, json: &str) -> Result<()> {
    security_framework::passwords::set_generic_password(service, account, json.as_bytes()).map_err(
        |e| {
            AppError::Credentials(format!(
                "failed to update the Claude credentials in the macOS Keychain: {e}"
            ))
        },
    )
}

fn write_raw_service(service: &str, json: &str) -> Result<()> {
    // Must mirror `read_raw`'s selection exactly, or an update can create a
    // second item the read will never find.
    let Some(acct) = account() else {
        return Err(AppError::Credentials(
            "cannot safely update the Claude credentials in the macOS Keychain because USER is unset"
                .into(),
        ));
    };

    match command_for_security_stdin(service, &acct, json) {
        Some(command) => write_via_security_stdin(&command),
        _ => write_via_native_api(service, &acct, json),
    }
}

fn delete_raw_service(service: &str) -> Result<()> {
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
    let detail = sanitize_untrusted_line(&String::from_utf8_lossy(&out.stderr));
    Err(AppError::Credentials(format!(
        "failed to remove the Claude credentials from the macOS Keychain \
         (security exited {}): {}",
        out.status.code().unwrap_or(-1),
        detail.trim()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "macos")]
    use security_framework::item::{ItemClass, ItemSearchOptions, Limit};
    #[cfg(target_os = "macos")]
    use security_framework::os::macos::keychain::SecKeychain;
    #[cfg(target_os = "macos")]
    use std::panic::{AssertUnwindSafe, catch_unwind};
    #[cfg(target_os = "macos")]
    use std::sync::atomic::{AtomicU64, Ordering};
    #[cfg(target_os = "macos")]
    use std::time::{SystemTime, UNIX_EPOCH};

    #[cfg(target_os = "macos")]
    const TEST_ACCOUNT: &str = "alice";
    #[cfg(target_os = "macos")]
    const ERR_SEC_ITEM_NOT_FOUND_OSSTATUS: i32 = -25300;

    #[cfg(target_os = "macos")]
    fn unique_test_service(test_name: &str) -> String {
        static NONCE: AtomicU64 = AtomicU64::new(0);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time is after the Unix epoch")
            .as_nanos();
        let nonce = NONCE.fetch_add(1, Ordering::Relaxed);
        format!(
            "ai-usagebar-keychain-selftest-{test_name}-{}-{timestamp}-{nonce}",
            std::process::id()
        )
    }

    #[cfg(target_os = "macos")]
    fn matching_item_count(service: &str, account: &str) -> usize {
        let keychain = SecKeychain::default().expect("default Keychain");
        let result = ItemSearchOptions::new()
            .keychains(std::slice::from_ref(&keychain))
            .class(ItemClass::generic_password())
            .service(service)
            .account(account)
            .limit(Limit::All)
            .load_attributes(true)
            .search();

        match result {
            Ok(items) => items.len(),
            Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND_OSSTATUS => 0,
            Err(error) => panic!("restricted Keychain count failed: {error}"),
        }
    }

    #[cfg(target_os = "macos")]
    fn delete_test_item(service: &str, account: &str) -> std::io::Result<()> {
        let out = Command::new("/usr/bin/security")
            .args(["delete-generic-password", "-a", account, "-s", service])
            .output()?;
        if out.status.success() || out.status.code() == Some(ERR_SEC_ITEM_NOT_FOUND) {
            Ok(())
        } else {
            Err(std::io::Error::other(format!(
                "security delete failed with exit code {}",
                out.status.code().unwrap_or(-1)
            )))
        }
    }

    #[cfg(target_os = "macos")]
    struct KeychainTestCleanup {
        service: String,
        account: &'static str,
        armed: bool,
    }

    #[cfg(target_os = "macos")]
    impl KeychainTestCleanup {
        fn new(service: String) -> Self {
            Self {
                service,
                account: TEST_ACCOUNT,
                armed: true,
            }
        }

        fn delete_now(&self) -> std::io::Result<()> {
            delete_test_item(&self.service, self.account)
        }

        fn disarm(&mut self) {
            self.armed = false;
        }
    }

    #[cfg(target_os = "macos")]
    impl Drop for KeychainTestCleanup {
        fn drop(&mut self) {
            if self.armed {
                let _ = delete_test_item(&self.service, self.account);
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn write_test_item(service: &str, blob: &str) {
        let command = command_for_security_stdin(service, TEST_ACCOUNT, blob)
            .expect("synthetic test command is within the safe stdin limit");
        write_via_security_stdin(&command).expect("write synthetic Keychain item");
    }

    #[cfg(target_os = "macos")]
    fn read_test_item_through_security(service: &str) -> Vec<u8> {
        let out = Command::new("/usr/bin/security")
            .args([
                "find-generic-password",
                "-a",
                TEST_ACCOUNT,
                "-s",
                service,
                "-w",
            ])
            .output()
            .expect("run security find-generic-password");
        assert!(
            out.status.success(),
            "security read failed with exit code {}",
            out.status.code().unwrap_or(-1)
        );
        out.stdout
            .strip_suffix(b"\n")
            .unwrap_or(&out.stdout)
            .to_vec()
    }

    #[test]
    fn quoting_wraps_and_escapes_backslash_and_quote() {
        assert_eq!(quote_for_security_stdin("plain").unwrap(), "\"plain\"");
        assert_eq!(
            quote_for_security_stdin(r#"a"b"#).unwrap(),
            r#""a\"b""#,
            "a double quote must be backslash-escaped, not dropped"
        );
        assert_eq!(quote_for_security_stdin(r"a\b").unwrap(), r#""a\\b""#);
        // A trailing backslash must not escape the closing quote.
        assert_eq!(quote_for_security_stdin(r"a\").unwrap(), r#""a\\""#);
    }

    #[test]
    fn quoting_preserves_spaces_and_non_ascii() {
        // The default service name contains a space; the account may not be ASCII.
        assert_eq!(
            quote_for_security_stdin(SERVICE).unwrap(),
            "\"Claude Code-credentials\""
        );
        assert_eq!(quote_for_security_stdin("rené").unwrap(), "\"rené\"");
    }

    #[test]
    fn composition_fails_closed_on_cr_or_lf_in_every_component() {
        for line_break in ['\r', '\n'] {
            let service = format!("service{line_break}injected");
            let account = format!("alice{line_break}injected");
            let json = format!("{{\"value\":\"before{line_break}after\"}}");

            assert!(compose_write_command(&service, "alice", "{}").is_none());
            assert!(compose_write_command(SERVICE, &account, "{}").is_none());
            assert!(compose_write_command(SERVICE, "alice", &json).is_none());
        }

        // A newline would end the line early and let the rest be read as a
        // further `security` command.
        assert!(quote_for_security_stdin("a\nb").is_none());
        assert!(quote_for_security_stdin("a\rb").is_none());
    }

    #[test]
    fn composed_command_is_one_line_and_hides_nothing_from_security() {
        let cmd = compose_write_command(SERVICE, "alice", r#"{"a":"b\"c"}"#).unwrap();
        assert_eq!(
            cmd,
            "add-generic-password -U -a \"alice\" -s \"Claude Code-credentials\" -w \"{\\\"a\\\":\\\"b\\\\\\\"c\\\"}\"\n"
        );
        assert_eq!(cmd.matches('\n').count(), 1, "exactly one command per line");
    }

    #[test]
    fn stdin_limits_preserve_the_measured_cap_and_operational_margin() {
        let safe_boundary = "x".repeat(4000);
        let over_safe_boundary = "x".repeat(4001);
        let measured_cap = "x".repeat(4032);

        assert_eq!(safe_boundary.len(), SECURITY_STDIN_SAFE_MAX);
        assert_eq!(measured_cap.len(), SECURITY_STDIN_MEASURED_CAP);
        assert!(command_fits_security_stdin(&safe_boundary));
        assert!(!command_fits_security_stdin(&over_safe_boundary));
        assert!(!command_fits_security_stdin(&measured_cap));
    }

    #[test]
    fn stdin_limit_is_applied_after_quoting_and_escaping() {
        let raw_json = "\\".repeat(2100);
        assert!(raw_json.len() < SECURITY_STDIN_SAFE_MAX);

        let composed = compose_write_command("service", "alice", &raw_json).unwrap();
        assert!(composed.len() > SECURITY_STDIN_SAFE_MAX);
        assert!(command_for_security_stdin("service", "alice", &raw_json).is_none());
    }

    #[test]
    fn realistic_credential_blob_stays_under_the_stdin_cap() {
        // ~2.8 KB of compact JSON is what Claude Code stores today; the
        // composed line must clear `security -i`'s truncation point, or the
        // write silently falls back to the native API and re-stamps the
        // partition list (issue #148).
        let json = format!(
            r#"{{"claudeAiOauth":{{"accessToken":"{}","refreshToken":"{}","expiresAt":1757430000000,"subscriptionType":"max","scopes":["user:inference","user:profile"]}}}}"#,
            "a".repeat(1300),
            "r".repeat(1300),
        );
        assert!(json.len() > 2600, "guard is only meaningful on a real blob");
        let cmd = command_for_security_stdin(SERVICE, "alice", &json).unwrap();
        assert!(
            cmd.len() <= SECURITY_STDIN_SAFE_MAX,
            "a realistic blob composed to {} bytes, over the {} cap",
            cmd.len(),
            SECURITY_STDIN_SAFE_MAX
        );
    }

    /// Touches the real login Keychain, so it is opt-in:
    /// `cargo test --lib -- --ignored keychain_round_trip`.
    ///
    /// Asserts the three things issue #148 is actually about: the value
    /// round-trips byte for byte, `-U` keeps a single item across repeated
    /// writes, and the item stays readable by `/usr/bin/security` (i.e. the
    /// partition list was never re-stamped with our cdhash).
    #[test]
    #[ignore = "writes to the real login Keychain"]
    #[cfg(target_os = "macos")]
    fn keychain_round_trip_keeps_one_item_readable_by_security() {
        let service = unique_test_service("round-trip");
        let mut cleanup = KeychainTestCleanup::new(service.clone());
        assert_eq!(matching_item_count(&service, TEST_ACCOUNT), 0);

        let blob = format!(
            r#"{{"claudeAiOauth":{{"accessToken":"synthetic-{}","refreshToken":"synthetic-with-\"quotes\"-and-\\slashes","expiresAt":1}}}}"#,
            "a".repeat(1200),
        );

        for pass in 0..2 {
            write_test_item(&service, &blob);
            let got = read_test_item_through_security(&service);
            assert_eq!(got, blob.as_bytes(), "pass {pass}");
        }

        assert_eq!(
            matching_item_count(&service, TEST_ACCOUNT),
            1,
            "-U must update in place, not add a second item"
        );

        cleanup.delete_now().expect("cleanup");
        assert_eq!(matching_item_count(&service, TEST_ACCOUNT), 0);
        cleanup.disarm();
    }

    #[test]
    #[ignore = "writes to the real login Keychain"]
    #[cfg(target_os = "macos")]
    fn keychain_round_trip_cleanup_guard_runs_during_panic() {
        let service = unique_test_service("panic-cleanup");
        let unwind = catch_unwind(AssertUnwindSafe({
            let service = service.clone();
            move || {
                let _cleanup = KeychainTestCleanup::new(service.clone());
                write_test_item(&service, r#"{"synthetic":"panic-cleanup"}"#);
                assert_eq!(matching_item_count(&service, TEST_ACCOUNT), 1);
                panic!("deliberate panic to exercise RAII cleanup");
            }
        }));

        assert!(unwind.is_err(), "the deliberate panic must be caught");
        assert_eq!(matching_item_count(&service, TEST_ACCOUNT), 0);
    }
}
