//! Generic macOS login-Keychain lookup for vendor API keys.
//!
//! Configured through the `[keychain]` table in config.toml, which maps a
//! vendor (or a named `"vendor.label"` account) to the *service name* of a
//! generic-password item the user created — nothing is hardcoded to a vendor
//! or to an item name:
//!
//! ```toml
//! [keychain]
//! zai = "MyZaiKey"          # service name of `security add-generic-password`
//! "openrouter.work" = "ORWorkKey"
//! ```
//!
//! Resolution order for every vendor on this source is
//! env var → inline `api_key` → this lookup, so it never masks either.
//!
//! Reads go through `/usr/bin/security(1)`, never Security.framework,
//! because the *writer's* code identity is what macOS stamps onto the
//! item's XARA partition list (issue #148) — an item our process wrote
//! natively would trip an ACL dialog for every later `security` read,
//! ours and the creating app's alike. The command scrubs this process's
//! provider-key env vars (the rule every credential lookup here follows),
//! nulls stdin/stderr so a stray prompt can never hang the bar, and the
//! secret arrives on stdout — never in argv.

use std::process::{Command, Stdio};

/// Read the generic-password item `service` from the login Keychain.
///
/// `None` for every failure — item absent, Keychain locked, access denied,
/// not macOS — because this is a last-resort source: the caller's own "no
/// API key" error names the env var and config section, which is more
/// actionable than a `security` exit code.
#[cfg(target_os = "macos")]
pub fn read_generic_password(service: &str) -> Option<String> {
    read_command(service).output().ok().and_then(|out| {
        if !out.status.success() {
            return None;
        }
        let value = String::from_utf8(out.stdout).ok()?;
        let value = value.trim_end_matches('\n');
        (!value.is_empty()).then(|| value.to_string())
    })
}

#[cfg(not(target_os = "macos"))]
pub fn read_generic_password(_service: &str) -> Option<String> {
    None
}

/// `security find-generic-password -s <service> -w`, read-only. Exposed for
/// tests, which assert on the command without executing it.
#[cfg(target_os = "macos")]
pub(crate) fn read_command(service: &str) -> Command {
    let mut command = Command::new("/usr/bin/security");
    command
        .args(["find-generic-password", "-s", service, "-w"])
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    command
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    #[test]
    fn read_command_targets_the_configured_service() {
        let command = super::read_command("MyZaiKey");
        assert_eq!(command.get_program(), "/usr/bin/security");
        let args: Vec<String> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, vec!["find-generic-password", "-s", "MyZaiKey", "-w"]);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn read_generic_password_is_none_off_macos() {
        assert!(super::read_generic_password("any").is_none());
    }
}
