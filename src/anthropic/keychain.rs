//! macOS Keychain access for Claude Code OAuth credentials.
//!
//! On Linux the Claude CLI writes its OAuth state to
//! `~/.claude/.credentials.json`. On macOS, recent Claude Code builds instead
//! store the *same* `{ "claudeAiOauth": …, "mcpOAuth": … }` JSON as a generic
//! password item in the login Keychain (service `Claude Code-credentials`), so
//! the file never exists and a naive read fails with an I/O error.
//!
//! Reads, writes and deletes all go through the built-in `security(1)` tool,
//! because the *writer's* code identity is what macOS stamps onto the item's
//! XARA partition list. A native `SecItemAdd`/`SecItemUpdate` from this
//! process leaves the item owned by `cdhash:<ai-usagebar>`, and every later
//! read by `/usr/bin/security` — ours *and* Claude Code's — then trips
//! `ACL partition mismatch: client apple-tool:` and raises a Keychain dialog.
//! "Always Allow" (with the Keychain password) does put `apple-tool:` back,
//! but only until the next native write re-stamps the list, so the dialog
//! returns at every token refresh. Going through `security(1)` keeps writer
//! and reader on the same `apple-tool:` partition. See issue #148.
//!
//! Credential JSON normally never enters process arguments: the command is
//! fed to `security -i` on **stdin**, so argv is just `["/usr/bin/security",
//! "-i"]`. That interactive reader truncates an over-long line *and stores the
//! truncated value*, so [`SECURITY_STDIN_SAFE_MAX`] keeps us clear of the cap.
//! A blob past the cap — real since Claude Code started keeping `mcpOAuth`
//! discovery state for every MCP plugin in the same item (3.6 KB measured on
//! 2026-09-23) — is handed to `security(1)` as argv instead, exactly the
//! fallback Claude Code itself uses. It is never written through the native
//! API. The `security-framework` dependency is macOS-gated and only the opt-in
//! Keychain tests use it.
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
/// Both write paths select by account exactly as the read does. Fail closed if
/// `$USER` is unavailable: a write without `-a` could create a second item the
/// read would never find again.
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
    Err(write_failure(&out))
}

/// Argv fallback for a blob too long for the `security -i` line cap. The
/// secret is visible to `ps` for the few milliseconds `security` runs — the
/// same trade Claude Code makes for this case ("exceeds security -i stdin
/// limit; using argv"). The alternative, a native `SecItemAdd`, re-stamps the
/// item with our `cdhash:` partition and brings the Keychain dialog back on
/// every later read (issue #148).
fn write_via_security_argv(service: &str, account: &str, json: &str) -> Result<()> {
    let out = Command::new("/usr/bin/security")
        .args([
            "add-generic-password",
            "-U",
            "-a",
            account,
            "-s",
            service,
            "-w",
            json,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| AppError::Other(format!("could not run `security`: {e}")))?;
    if out.status.success() {
        return Ok(());
    }
    Err(write_failure(&out))
}

/// Map a failed `security add-generic-password` run to the user-facing error,
/// with the tool's stderr sanitized like every other subprocess line.
fn write_failure(out: &std::process::Output) -> AppError {
    let detail = sanitize_untrusted_line(&String::from_utf8_lossy(&out.stderr));
    let detail = detail.trim();
    AppError::Credentials(format!(
        "failed to update the Claude credentials in the macOS Keychain \
         (security exited {}): {}",
        out.status.code().unwrap_or(-1),
        if detail.is_empty() {
            "no detail"
        } else {
            detail
        }
    ))
}

/// How a credential write reaches `security(1)`. Both paths keep the writer on
/// the `apple-tool:` partition; the native Security API is never used for
/// writes, because that is what re-stamps the item with our `cdhash:` (#148).
///
/// Deliberately not `Debug`: `Stdin` carries the composed command, secret
/// included, and nothing should be able to print it by accident.
enum WritePath {
    /// One composed command fed to `security -i` over stdin: the secret never
    /// enters argv.
    Stdin(String),
    /// The composed line would not fit the `security -i` cap (or could not be
    /// quoted for it), so the blob goes to `security(1)` as argv instead.
    Argv,
}

fn write_path_for(service: &str, account: &str, json: &str) -> WritePath {
    match command_for_security_stdin(service, account, json) {
        Some(command) => WritePath::Stdin(command),
        None => WritePath::Argv,
    }
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

    write_raw_service_as(service, &acct, json)
}

/// The dispatch itself, with the account passed in so the opt-in Keychain
/// tests can drive the production wiring under a synthetic account and prove
/// the item's partition list stays `apple-tool:` on both paths.
fn write_raw_service_as(service: &str, account: &str, json: &str) -> Result<()> {
    match write_path_for(service, account, json) {
        WritePath::Stdin(command) => write_via_security_stdin(&command),
        WritePath::Argv => write_via_security_argv(service, account, json),
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
        assert!(
            matches!(
                write_path_for(service, TEST_ACCOUNT, blob),
                WritePath::Stdin(_)
            ),
            "synthetic test command is within the safe stdin limit"
        );
        write_raw_service_as(service, TEST_ACCOUNT, blob).expect("write synthetic Keychain item");
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

    /// A blob shaped like what Claude Code stores once MCP plugins have run
    /// OAuth discovery. The real item that motivated this measured 3640 bytes
    /// with 302 quotes on 2026-09-23, composing to ~4020 bytes for
    /// `security -i` — just past the cap. The fixture is deliberately far past
    /// it (about 5.7 KB composed) so the argv path is exercised even if the
    /// safe limit is ever nudged upward.
    fn oversized_blob() -> String {
        let entries = (0..9)
            .map(|i| {
                format!(
                    r#""plugin:p{i}:p{i}|0123456789abcdef":{{"accessToken":"","clientId":"{}","clientSecret":"{}","discoveryState":{{"authorizationServerUrl":"https://auth.example.test/oauth/{i}","oauthMetadataFound":true,"resourceMetadataUrl":"https://mcp.example.test/.well-known/oauth-protected-resource/mcp/{i}"}},"issuer":"https://auth.example.test/oauth/{i}","redirectUri":"http://localhost:3000/callback","serverName":"plugin:p{i}:p{i}","serverUrl":"https://mcp.example.test/mcp/{i}"}}"#,
                    "c".repeat(36),
                    "s".repeat(44),
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        format!(
            r#"{{"claudeAiOauth":{{"accessToken":"{}","expiresAt":1758600000000,"rateLimitTier":"default_claude_max_5x","refreshToken":"{}","scopes":["user:file_upload","user:inference","user:mcp_servers","user:profile","user:sessions:claude_code","user:sessions:claude_code_desktop"],"subscriptionType":"max"}},"mcpOAuth":{{{entries}}}}}"#,
            "a".repeat(108),
            "r".repeat(108),
        )
    }

    /// The item's XARA partition list as `security dump-keychain -a` prints it
    /// (attributes and ACLs only — it never decrypts, so it cannot prompt).
    /// Dumps the default keychain, which is where the writes land. A
    /// concurrent Keychain change (another test cleaning up, the menu bar
    /// refreshing) can make one enumeration exit non-zero or come back short,
    /// so only the presence of our item counts and the dump is retried.
    #[cfg(target_os = "macos")]
    fn partition_list_through_security(service: &str) -> String {
        let keychain = default_keychain_path();
        let needle = format!("\"svce\"<blob>=\"{service}\"");
        let mut last_dump = String::new();
        let mut last_status = String::new();
        for attempt in 1u64..=5 {
            let out = Command::new("/usr/bin/security")
                .args(["dump-keychain", "-a", &keychain])
                .output()
                .expect("run security dump-keychain");
            last_dump = String::from_utf8_lossy(&out.stdout).into_owned();
            last_status = format!(
                "exit {}, stderr {:?}",
                out.status.code().unwrap_or(-1),
                sanitize_untrusted_line(&String::from_utf8_lossy(&out.stderr)).trim()
            );
            if let Some(item) = last_dump
                .split("keychain: \"")
                .find(|chunk| chunk.contains(&needle))
            {
                let mut lines = item.lines();
                while let Some(line) = lines.next() {
                    if line.contains("partition_id") {
                        for later in lines.by_ref() {
                            if let Some(desc) = later.trim().strip_prefix("description: ") {
                                return desc.to_string();
                            }
                        }
                    }
                }
                panic!("no partition_id ACL entry for {service}");
            }
            std::thread::sleep(std::time::Duration::from_millis(500 * attempt));
        }
        panic!(
            "test item {service} absent from 5 dumps of {keychain} (last: {last_status}; {} items / {} bytes)",
            last_dump.matches("keychain: \"").count(),
            last_dump.len()
        );
    }

    /// Where `security add-generic-password` puts an item when no keychain is
    /// named: the user's default keychain (normally login.keychain-db).
    #[cfg(target_os = "macos")]
    fn default_keychain_path() -> String {
        let out = Command::new("/usr/bin/security")
            .args(["default-keychain", "-d", "user"])
            .output()
            .expect("run security default-keychain");
        let printed = String::from_utf8_lossy(&out.stdout);
        let path = printed.trim().trim_matches('"').to_string();
        if out.status.success() && !path.is_empty() {
            path
        } else {
            let home = std::env::var("HOME").expect("HOME is set");
            format!("{home}/Library/Keychains/login.keychain-db")
        }
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
        // ~2.8 KB of compact JSON is the bare OAuth blob (no `mcpOAuth`); the
        // composed line must clear `security -i`'s truncation point, or the
        // write has to fall back to argv and expose the secret to `ps` for a
        // moment.
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

    #[test]
    fn oversized_blob_is_written_through_security_argv_not_the_native_api() {
        // The only acceptable fallback past the stdin cap is `security(1)`
        // itself via argv: a native `SecItemAdd` re-stamps the partition list
        // with our cdhash and brings the Keychain dialog back (issue #148).
        let json = oversized_blob();
        assert!(
            command_for_security_stdin(SERVICE, "alice", &json).is_none(),
            "fixture must exceed the stdin cap, composed to {} bytes",
            compose_write_command(SERVICE, "alice", &json)
                .unwrap()
                .len()
        );
        assert!(matches!(
            write_path_for(SERVICE, "alice", &json),
            WritePath::Argv
        ));
    }

    #[test]
    fn normal_blob_keeps_the_stdin_path() {
        let json = r#"{"claudeAiOauth":{"accessToken":"a","refreshToken":"r","expiresAt":1}}"#;
        match write_path_for(SERVICE, "alice", json) {
            WritePath::Stdin(cmd) => {
                assert_eq!(
                    Some(cmd),
                    command_for_security_stdin(SERVICE, "alice", json)
                )
            }
            WritePath::Argv => {
                panic!("a small blob must stay on stdin, keeping the secret out of argv")
            }
        }
    }

    /// Opt-in like the round-trip test:
    /// `cargo test --lib -- --ignored keychain_oversized`.
    ///
    /// Exercises the argv fallback end to end on the real login Keychain: the
    /// value round-trips byte for byte through `/usr/bin/security`, `-U` keeps
    /// a single item, and the partition list stays `apple-tool:` — never our
    /// `cdhash:` — so no later read can raise the XARA dialog.
    #[test]
    #[ignore = "writes to the real login Keychain"]
    #[cfg(target_os = "macos")]
    fn keychain_oversized_blob_round_trips_via_argv_and_keeps_apple_tool_partition() {
        let service = unique_test_service("oversized");
        let mut cleanup = KeychainTestCleanup::new(service.clone());
        assert_eq!(matching_item_count(&service, TEST_ACCOUNT), 0);

        let blob = oversized_blob();
        assert!(matches!(
            write_path_for(&service, TEST_ACCOUNT, &blob),
            WritePath::Argv
        ));

        for pass in 0..2 {
            write_raw_service_as(&service, TEST_ACCOUNT, &blob)
                .expect("oversized write through the production dispatch");
            let got = read_test_item_through_security(&service);
            assert_eq!(got, blob.as_bytes(), "pass {pass}");
        }
        assert_eq!(
            matching_item_count(&service, TEST_ACCOUNT),
            1,
            "-U must update in place, not add a second item"
        );

        let partitions = partition_list_through_security(&service);
        assert!(
            partitions.contains("apple-tool:"),
            "partition list was {partitions:?}"
        );
        assert!(
            !partitions.contains("cdhash:"),
            "partition list was {partitions:?}"
        );

        cleanup.delete_now().expect("cleanup");
        assert_eq!(matching_item_count(&service, TEST_ACCOUNT), 0);
        cleanup.disarm();
    }
}
