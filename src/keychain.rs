//! macOS login-Keychain lookup for vendor API keys.
//!
//! The `[keychain]` table in config.toml maps a vendor, or a named
//! `"vendor.label"` account, to the service name of a generic-password item
//! the user created:
//!
//! ```toml
//! [keychain]
//! zai = "MyZaiKey"          # service name of `security add-generic-password`
//! "openrouter.work" = "ORWorkKey"
//! ```
//!
//! The service name is stored in config.toml in plaintext. Only the key it
//! names stays in the Keychain.
//!
//! A vendor's key resolves as env var, then inline `api_key`, then this
//! lookup, so a Keychain entry never masks the other two.
//!
//! The read selects an item by service and account. The account is the macOS
//! short username (`$USER`), the same pin [`crate::anthropic::keychain`]
//! uses. The Keychain allows several items under one service, and with `-s`
//! alone `security` returns whichever it finds first, so one account's key
//! could be reported under another account's label.
//!
//! Reads go through `/usr/bin/security`. macOS stamps the writer's code
//! identity onto an item's partition list (issue #148), so an item this
//! process wrote through Security.framework would raise an ACL dialog on
//! every later `security` read, ours and the creating app's. The command
//! comes from [`crate::anthropic::keychain::security_command`], which removes
//! this process's provider-key env vars. Stdin is null so a prompt cannot
//! hang the bar, and the secret arrives on stdout, so it is never in argv.

use crate::error::Result;

/// Whether this platform has a login Keychain. Off macOS a `[keychain]`
/// entry is validated, but it is never read and does not count as a
/// configured credential.
pub const SUPPORTED: bool = cfg!(target_os = "macos");

/// Upper bound on a service name. Keychain Access shows nothing near it.
const MAX_SERVICE_BYTES: usize = 1024;

/// The rules for a `[keychain]` service name. `Config::validate` and the Add
/// Key dialog's `kc:` form both call this, so the dialog cannot save a name
/// the next config load refuses. The `Err` is the reason, worded to follow
/// the caller's subject ("the service name is empty").
pub fn validate_service_name(service: &str) -> std::result::Result<(), &'static str> {
    if service.trim().is_empty() {
        return Err("is empty");
    }
    if service.trim() != service {
        return Err("has leading or trailing whitespace");
    }
    if service.len() > MAX_SERVICE_BYTES {
        return Err("is longer than 1024 bytes");
    }
    if service.chars().any(char::is_control) {
        return Err("contains control characters");
    }
    Ok(())
}

/// Read the generic-password item `service` (account `$USER`) from the login
/// Keychain.
///
/// `Ok(None)` means the item does not exist. A locked Keychain, a denied ACL
/// and any other failure are an `Err` that names the service. Reporting
/// those as "no API key" would tell the user to export an env var while the
/// credential is sitting in the Keychain.
#[cfg(target_os = "macos")]
pub fn read_generic_password(service: &str) -> Result<Option<String>> {
    use crate::display::sanitize_untrusted_line;
    use crate::error::AppError;

    /// `security` exits with the raw OSStatus. 44 is `errSecItemNotFound`.
    const ERR_SEC_ITEM_NOT_FOUND: i32 = 44;

    // Without `$USER` the read could only select by service, which is the
    // ambiguous lookup the account pin prevents, so refuse.
    let account = std::env::var("USER")
        .ok()
        .filter(|user| !user.is_empty())
        .ok_or_else(|| {
            AppError::Credentials(format!(
                "could not read the Keychain item {service:?}: $USER is not set, so the \
                 item's account cannot be selected"
            ))
        })?;
    let out = read_command(service, &account).output().map_err(|e| {
        AppError::Credentials(format!(
            "could not run `security` to read the Keychain item {service:?}: {e}"
        ))
    })?;
    if !out.status.success() {
        if out.status.code() == Some(ERR_SEC_ITEM_NOT_FOUND) {
            return Ok(None);
        }
        // stderr comes from a subprocess and may reach a terminal, so it is
        // sanitized like any other untrusted text.
        let detail = sanitize_untrusted_line(&String::from_utf8_lossy(&out.stderr));
        let detail = detail.trim();
        return Err(AppError::Credentials(format!(
            "could not read the Keychain item {service:?} named in [keychain] \
             (security exited {}): {}. If the login Keychain is locked, unlock it and \
             retry; if access was denied, allow `security` to read the item in \
             Keychain Access.",
            out.status.code().unwrap_or(-1),
            if detail.is_empty() {
                "no detail"
            } else {
                detail
            }
        )));
    }
    let value = String::from_utf8(out.stdout).map_err(|_| {
        AppError::Credentials(format!("the Keychain item {service:?} is not UTF-8 text"))
    })?;
    let value = value.trim_end_matches('\n');
    Ok((!value.is_empty()).then(|| value.to_string()))
}

/// There is no Keychain off macOS. Production callers check [`SUPPORTED`]
/// first and never call this; it exists so the function compiles everywhere.
#[cfg(not(target_os = "macos"))]
pub fn read_generic_password(_service: &str) -> Result<Option<String>> {
    Ok(None)
}

/// `security find-generic-password -s <service> -a <account> -w`, read-only.
/// Tests inspect the command without running it.
#[cfg(target_os = "macos")]
pub(crate) fn read_command(service: &str, account: &str) -> std::process::Command {
    let mut command = crate::anthropic::keychain::security_command();
    command
        .args(["find-generic-password", "-s", service, "-a", account, "-w"])
        .stdin(std::process::Stdio::null());
    command
}

#[cfg(test)]
mod tests {
    use super::validate_service_name;

    #[test]
    fn service_names_reject_what_security_would_misread() {
        assert!(validate_service_name("My Zai Key").is_ok());
        for bad in ["", "   ", " MyKey", "MyKey ", "My\nKey", "My\u{1b}[2JKey"] {
            assert!(validate_service_name(bad).is_err(), "{bad:?}");
        }
        assert!(validate_service_name(&"k".repeat(1025)).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn read_command_pins_service_and_account() {
        let command = super::read_command("MyZaiKey", "alice");
        assert_eq!(command.get_program(), "/usr/bin/security");
        let args: Vec<String> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            vec![
                "find-generic-password",
                "-s",
                "MyZaiKey",
                "-a",
                "alice",
                "-w"
            ]
        );
    }

    /// The module doc says the spawn does not inherit provider keys. This
    /// checks it against the command that is actually built.
    #[cfg(target_os = "macos")]
    #[test]
    fn read_command_scrubs_vendor_secret_env_vars() {
        let command = super::read_command("MyZaiKey", "alice");
        let removed: Vec<String> = command
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(key, _)| key.to_string_lossy().into_owned())
            .collect();
        let expected = crate::vendor::vendor_secret_env_vars_to_remove(&[]);
        assert!(!expected.is_empty());
        for var in expected {
            assert!(
                removed.iter().any(|key| key == var),
                "expected {var} to be scrubbed from the Keychain read"
            );
        }
    }
}
