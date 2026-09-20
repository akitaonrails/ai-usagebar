//! Read the Grok Bot desktop app's own credential file — read-only, never
//! written. `sand-secrets.json` holds the app's Cursor OAuth session as
//! Chromium OSCrypt `v10` blobs under `cursor-accounts`:
//!
//! ```json
//! {"cursor-accounts": {"active": "<account-id>", "accounts": {
//!   "<account-id>": {"cursor-access-token": "djE…", "cursor-refresh-token": "djE…"}}}}
//! ```
//!
//! On Linux the OSCrypt key derives (one PBKDF2 round — see
//! [`crate::safe_storage::derive_key_linux`]) from the Secret Service item
//! `application=Grok Bot`, falling back to Chromium's documented `"peanuts"`
//! default when no secret is stored. The `secret-tool` lookup is a read-only
//! subprocess; the secret arrives on stdout, never in argv.
//!
//! On macOS the same `v10` blobs are keyed by the login Keychain generic
//! password `Grok Bot Safe Storage` / `Grok Bot Key` (1003 PBKDF2 rounds —
//! Chromium's macOS OSCrypt, the same scheme as Claude Desktop). There is no
//! `"peanuts"` fallback: a missing item is a credentials error. The Mac app
//! also stores `cursor-accounts` as a JSON *string* wrapping the object Linux
//! writes as an object; [`parse`] accepts both.
//!
//! Error messages are fixed strings: every input here is a credential and
//! must never end up in a tooltip or a log line.

use std::fmt::Write as _;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::{AppError, Result};

/// Chromium's documented OSCrypt default secret on Linux, used when the app
/// stored no Secret Service item. Not a credential — it ships in every
/// Chromium build's source.
const FALLBACK_SECRET: &str = "peanuts";

/// Login-Keychain generic-password service holding Grok Bot's OSCrypt secret.
#[cfg(target_os = "macos")]
const MACOS_SERVICE: &str = "Grok Bot Safe Storage";
/// Account of that item. Pairing it with the service avoids colliding with a
/// differently-named password under the same service.
#[cfg(target_os = "macos")]
const MACOS_ACCOUNT: &str = "Grok Bot Key";

/// The app's Cursor OAuth session, decrypted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrokbotCredentials {
    pub access_token: String,
    pub refresh_token: String,
    /// First 16 hex chars of the SHA-256 of the refresh token. Scopes the
    /// vendor cache (payload and persisted OAuth) to this sign-in, so a
    /// re-login never gets the previous session's cache — the same treatment
    /// kiro/minimax give their account keys.
    pub fingerprint: String,
}

pub fn fingerprint_of(secret: &str) -> String {
    let digest = Sha256::digest(secret.as_bytes());
    let mut hex = String::with_capacity(16);
    for byte in digest.iter().take(8) {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// The OSCrypt key for a known secret — the pure half of [`oscrypt_key`], so
/// tests derive from a fixture secret without any subprocess.
pub fn key_for(secret: Option<&str>) -> [u8; 16] {
    crate::safe_storage::derive_key_linux(secret.unwrap_or(FALLBACK_SECRET).as_bytes())
}

/// The platform OSCrypt key.
///
/// Linux: Secret Service secret when one is stored, else `"peanuts"`.
/// macOS: the Keychain item; a missing item is an error, not peanuts.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn oscrypt_key() -> Result<[u8; 16]> {
    #[cfg(target_os = "linux")]
    {
        Ok(key_for(lookup_secret().as_deref()))
    }
    #[cfg(target_os = "macos")]
    {
        macos_oscrypt_key()
    }
}

/// `secret-tool lookup application "Grok Bot"`, read-only. A missing binary,
/// no running daemon, or no such item all mean the app stored no secret and
/// the `"peanuts"` default applies — so every failure is `None`, not an error.
#[cfg(target_os = "linux")]
fn lookup_secret() -> Option<String> {
    let mut command = std::process::Command::new("secret-tool");
    command
        .args(["lookup", "application", "Grok Bot"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped());
    // A credential lookup must not inherit this process's provider keys.
    for var in crate::vendor::vendor_secret_env_vars_to_remove(&[]) {
        command.env_remove(var);
    }
    let out = command.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let secret = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if secret.is_empty() {
        None
    } else {
        Some(secret)
    }
}

/// `security find-generic-password -s "Grok Bot Safe Storage" -a "Grok Bot Key" -w`.
/// Read-only; the secret arrives on stdout, never in argv.
#[cfg(target_os = "macos")]
fn macos_oscrypt_key() -> Result<[u8; 16]> {
    let secret = lookup_macos_secret().ok_or_else(|| {
        AppError::Credentials(
            "Grok Bot: no `Grok Bot Safe Storage` item in the login Keychain — install the Grok Bot desktop app and sign in to it"
                .into(),
        )
    })?;
    Ok(crate::safe_storage::derive_key(secret.as_bytes()))
}

#[cfg(target_os = "macos")]
fn lookup_macos_secret() -> Option<String> {
    let mut command = std::process::Command::new("/usr/bin/security");
    command
        .args([
            "find-generic-password",
            "-s",
            MACOS_SERVICE,
            "-a",
            MACOS_ACCOUNT,
            "-w",
        ])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped());
    for var in crate::vendor::vendor_secret_env_vars_to_remove(&[]) {
        command.env_remove(var);
    }
    let out = command.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let secret = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if secret.is_empty() {
        None
    } else {
        Some(secret)
    }
}

/// Read and decrypt the credential file at `path`.
pub fn read_at(path: &Path, key: &[u8; 16]) -> Result<GrokbotCredentials> {
    let raw = std::fs::read(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            // Name the fix, and the file actually checked (a configured
            // `secrets_path` makes the default advice useless otherwise).
            AppError::Credentials(format!(
                "Grok Bot: no credential file at {} — install the Grok Bot desktop app and sign in to it",
                crate::display::sanitize_untrusted_path(path)
            ))
        } else {
            AppError::io_at(path, e)
        }
    })?;
    parse(&raw, key)
}

fn parse(raw: &[u8], key: &[u8; 16]) -> Result<GrokbotCredentials> {
    let malformed = || {
        AppError::Credentials(
            "Grok Bot: sand-secrets.json does not hold an active signed-in account; sign in to the Grok Bot desktop app again"
                .into(),
        )
    };
    let root: serde_json::Value = serde_json::from_slice(raw).map_err(|_| malformed())?;
    let accounts = cursor_accounts_object(&root).ok_or_else(malformed)?;
    let active = accounts
        .get("active")
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(malformed)?;
    let entry = accounts
        .get("accounts")
        .and_then(|accounts| accounts.get(active))
        .ok_or_else(malformed)?;

    let access_token = decrypt_field(key, entry.get("cursor-access-token"))?;
    let refresh_token = decrypt_field(key, entry.get("cursor-refresh-token"))?;
    let fingerprint = fingerprint_of(&refresh_token);
    Ok(GrokbotCredentials {
        access_token,
        refresh_token,
        fingerprint,
    })
}

/// Linux writes `cursor-accounts` as a JSON object. The macOS app stores the
/// same object as a JSON string. Either is accepted; anything else is
/// malformed.
fn cursor_accounts_object(root: &serde_json::Value) -> Option<serde_json::Value> {
    match root.get("cursor-accounts") {
        Some(serde_json::Value::Object(_)) => root.get("cursor-accounts").cloned(),
        Some(serde_json::Value::String(s)) => serde_json::from_str(s).ok(),
        _ => None,
    }
}

/// Decrypt one OSCrypt `v10` blob field into a UTF-8 token. The field name is
/// safe to name in an error; the blob and its plaintext never are.
fn decrypt_field(key: &[u8; 16], field: Option<&serde_json::Value>) -> Result<String> {
    let blob = field
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            AppError::Credentials(
                "Grok Bot: sand-secrets.json is missing a token field; sign in to the Grok Bot desktop app again"
                    .into(),
            )
        })?;
    let bytes = crate::safe_storage::decrypt(key, blob).map_err(|_| {
        AppError::Credentials(
            "Grok Bot: a stored token could not be decrypted; sign in to the Grok Bot desktop app again"
                .into(),
        )
    })?;
    String::from_utf8(bytes).map_err(|_| {
        AppError::Credentials(
            "Grok Bot: a stored token is not UTF-8; sign in to the Grok Bot desktop app again"
                .into(),
        )
    })
}

/// The detection probe: a present, non-empty credential file. Cheap enough to
/// run at every frontend start — no decrypt, no subprocess.
pub fn secrets_present_at(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.len() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_key() -> [u8; 16] {
        // The documented default: exercises exactly the production Linux path
        // for an app that stored no Secret Service secret.
        key_for(None)
    }

    /// Seed a `sand-secrets.json` whose blobs are encrypted with the test key,
    /// exactly as the app would have written them.
    fn seed_secrets(dir: &TempDir, access: &str, refresh: &str) -> std::path::PathBuf {
        let key = test_key();
        let path = dir.path().join("sand-secrets.json");
        let doc = serde_json::json!({
            "cursor-accounts": {
                "active": "acct-1",
                "accounts": {
                    "acct-1": {
                        "cursor-access-token": crate::safe_storage::encrypt(&key, access.as_bytes()),
                        "cursor-refresh-token": crate::safe_storage::encrypt(&key, refresh.as_bytes()),
                    }
                }
            }
        });
        std::fs::write(&path, doc.to_string()).unwrap();
        path
    }

    #[test]
    fn a_seeded_secrets_file_yields_decrypted_tokens() {
        let td = TempDir::new().unwrap();
        let path = seed_secrets(&td, "at-test", "rt-test");

        let creds = read_at(&path, &test_key()).unwrap();

        assert_eq!(creds.access_token, "at-test");
        assert_eq!(creds.refresh_token, "rt-test");
        assert_eq!(creds.fingerprint, fingerprint_of("rt-test"));
        assert_eq!(creds.fingerprint.len(), 16);
        assert!(creds.fingerprint.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn the_active_entry_is_selected_among_several_accounts() {
        let td = TempDir::new().unwrap();
        let key = test_key();
        let path = td.path().join("sand-secrets.json");
        let doc = serde_json::json!({
            "cursor-accounts": {
                "active": "acct-2",
                "accounts": {
                    "acct-1": {
                        "cursor-access-token": crate::safe_storage::encrypt(&key, b"at-one"),
                        "cursor-refresh-token": crate::safe_storage::encrypt(&key, b"rt-one"),
                    },
                    "acct-2": {
                        "cursor-access-token": crate::safe_storage::encrypt(&key, b"at-two"),
                        "cursor-refresh-token": crate::safe_storage::encrypt(&key, b"rt-two"),
                    }
                }
            }
        });
        std::fs::write(&path, doc.to_string()).unwrap();

        let creds = read_at(&path, &key).unwrap();
        assert_eq!(creds.access_token, "at-two");
        assert_eq!(creds.refresh_token, "rt-two");
    }

    #[test]
    fn a_missing_file_names_the_fix_and_the_path_checked() {
        let td = TempDir::new().unwrap();
        let missing = td.path().join("absent").join("sand-secrets.json");
        let err = read_at(&missing, &test_key()).unwrap_err();
        let message = err.to_string();
        assert!(matches!(err, AppError::Credentials(_)), "{err:?}");
        assert!(
            message.contains("install the Grok Bot desktop app"),
            "{message}"
        );
        assert!(message.contains("sand-secrets.json"), "{message}");
    }

    #[test]
    fn malformed_files_and_missing_fields_are_credential_errors() {
        let td = TempDir::new().unwrap();
        for (name, contents) in [
            ("not-json", "not json".to_string()),
            ("no-root", r#"{"other": {}}"#.into()),
            (
                "no-active",
                r#"{"cursor-accounts": {"accounts": {}}}"#.into(),
            ),
            (
                "no-entry",
                r#"{"cursor-accounts": {"active": "a", "accounts": {}}}"#.into(),
            ),
            (
                "no-tokens",
                r#"{"cursor-accounts": {"active": "a", "accounts": {"a": {}}}}"#.into(),
            ),
        ] {
            let path = td.path().join(format!("{name}.json"));
            std::fs::write(&path, contents).unwrap();
            let err = read_at(&path, &test_key()).unwrap_err();
            assert!(matches!(err, AppError::Credentials(_)), "{name}: {err:?}");
            // Fixed strings only: nothing from the file leaks into the error.
            assert!(!err.to_string().contains("\"other\""), "{name}: {err}");
        }
    }

    #[test]
    fn an_undecryptable_blob_is_a_credential_error_without_the_blob() {
        let td = TempDir::new().unwrap();
        let path = seed_secrets(&td, "at-test", "rt-test");
        let wrong_key = crate::safe_storage::derive_key_linux(b"somebody-elses-secret");
        let err = read_at(&path, &wrong_key).unwrap_err();
        assert!(matches!(err, AppError::Credentials(_)), "{err:?}");
        let message = err.to_string();
        assert!(message.contains("could not be decrypted"), "{message}");
        assert!(!message.contains("at-test"), "{message}");
    }

    #[test]
    fn the_fallback_secret_is_chromiums_documented_default() {
        assert_eq!(key_for(None), key_for(Some("peanuts")));
    }

    #[test]
    fn the_probe_wants_a_non_empty_file() {
        let td = TempDir::new().unwrap();
        let path = td.path().join("sand-secrets.json");
        assert!(!secrets_present_at(&path));
        std::fs::write(&path, "").unwrap();
        assert!(!secrets_present_at(&path));
        std::fs::write(&path, "{}").unwrap();
        assert!(secrets_present_at(&path));
        assert!(!secrets_present_at(td.path()));
    }

    #[test]
    fn a_string_wrapped_cursor_accounts_object_parses() {
        // The macOS app stores `cursor-accounts` as a JSON string wrapping the
        // same object Linux writes directly.
        let td = TempDir::new().unwrap();
        let key = test_key();
        let inner = serde_json::json!({
            "active": "acct-1",
            "accounts": {
                "acct-1": {
                    "cursor-access-token": crate::safe_storage::encrypt(&key, b"at-mac"),
                    "cursor-refresh-token": crate::safe_storage::encrypt(&key, b"rt-mac"),
                }
            }
        });
        let path = td.path().join("sand-secrets.json");
        let doc = serde_json::json!({ "cursor-accounts": inner.to_string() });
        std::fs::write(&path, doc.to_string()).unwrap();

        let creds = read_at(&path, &key).unwrap();
        assert_eq!(creds.access_token, "at-mac");
        assert_eq!(creds.refresh_token, "rt-mac");
    }

    #[test]
    fn macos_round_blobs_decrypt_with_the_macos_derivation() {
        let td = TempDir::new().unwrap();
        let key = crate::safe_storage::derive_key(b"not-a-real-secret");
        let path = td.path().join("sand-secrets.json");
        let doc = serde_json::json!({
            "cursor-accounts": {
                "active": "acct-1",
                "accounts": {
                    "acct-1": {
                        "cursor-access-token": crate::safe_storage::encrypt(&key, b"at-macos"),
                        "cursor-refresh-token": crate::safe_storage::encrypt(&key, b"rt-macos"),
                    }
                }
            }
        });
        std::fs::write(&path, doc.to_string()).unwrap();
        let creds = read_at(&path, &key).unwrap();
        assert_eq!(creds.access_token, "at-macos");
        assert_eq!(creds.refresh_token, "rt-macos");
    }
}
