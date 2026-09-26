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
//! [`crate::safe_storage::derive_key_linux`]) either from the Secret Service
//! item `application=Grok Bot` or from Chromium's documented `"peanuts"`
//! default. Which one the app used is a runtime decision it makes from the
//! Secret Service backend Electron selected, and it encrypts with `"peanuts"`
//! whenever that backend is `basic_text` — so a machine can hold the item and
//! still have `"peanuts"`-keyed blobs on disk. [`oscrypt_keys`] returns both
//! and [`parse_any`] picks the one that opens the file. The `secret-tool`
//! lookup is a read-only subprocess; the secret arrives on stdout, never in
//! argv.
//!
//! On macOS the same `v10` blobs are keyed by the login Keychain generic
//! password `Grok Bot Safe Storage` / `Grok Bot Key` (1003 PBKDF2 rounds —
//! Chromium's macOS OSCrypt, the same scheme as Claude Desktop). There is no
//! `"peanuts"` fallback: a missing item is a credentials error. The Mac app
//! also stores `cursor-accounts` as a JSON *string* wrapping the object Linux
//! writes as an object; [`parse`] accepts both.
//!
//! On Windows the file sits in `%APPDATA%\Grok Bot`, the blobs are Chromium's
//! Windows `v10` (AES-256-GCM), and the key is `os_crypt.encrypted_key` in the
//! `Local State` file beside it, unprotected by DPAPI for the signed-in user
//! ([`crate::safe_storage::windows_key`]). `cursor-accounts` is a JSON string
//! there too.
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

/// The key that opens the app's token blobs: Chromium's Linux/macOS OSCrypt
/// (AES-128-CBC) or its Windows one (AES-256-GCM). Its `Debug` names only the
/// scheme.
#[derive(Clone, PartialEq, Eq)]
pub enum OsCryptKey {
    Cbc([u8; 16]),
    Gcm([u8; crate::safe_storage::WINDOWS_KEY_LEN]),
}

impl std::fmt::Debug for OsCryptKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Cbc(_) => "OsCryptKey::Cbc(..)",
            Self::Gcm(_) => "OsCryptKey::Gcm(..)",
        })
    }
}

impl OsCryptKey {
    fn decrypt(&self, blob: &str) -> Result<Vec<u8>> {
        match self {
            Self::Cbc(key) => crate::safe_storage::decrypt(key, blob),
            Self::Gcm(key) => crate::safe_storage::decrypt_windows(key, blob),
        }
    }
}

/// The app's Cursor OAuth session, decrypted. Its `Debug` keeps the
/// fingerprint and hides both tokens.
#[derive(Clone, PartialEq, Eq)]
pub struct GrokbotCredentials {
    pub access_token: String,
    pub refresh_token: String,
    /// First 16 hex chars of the SHA-256 of the refresh token. Scopes the
    /// vendor cache (payload and persisted OAuth) to this sign-in, so a
    /// re-login never gets the previous session's cache — the same treatment
    /// kiro/minimax give their account keys.
    pub fingerprint: String,
}

impl std::fmt::Debug for GrokbotCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GrokbotCredentials")
            .field("access_token", &"<redacted>")
            .field("refresh_token", &"<redacted>")
            .field("fingerprint", &self.fingerprint)
            .finish()
    }
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
/// macOS: the Keychain item; a missing item is an error, not peanuts.
#[cfg(target_os = "macos")]
pub fn oscrypt_key() -> Result<OsCryptKey> {
    macos_oscrypt_key().map(OsCryptKey::Cbc)
}

/// Every OSCrypt key worth trying on this machine, most specific first.
///
/// The app decides its key at runtime from the Secret Service backend Electron
/// selected, and writes with Chromium's `"peanuts"` default whenever that
/// backend is `basic_text` — on which it then pins plaintext for the session
/// (`setUsePlainTextEncryption`). So a machine can hold an
/// `application=Grok Bot` item while the blobs on disk were keyed with
/// `"peanuts"`; the two are not the same thing, and the item's presence does
/// not imply it opened them. Offer both and let [`parse_any`] decide.
#[cfg(target_os = "linux")]
pub fn oscrypt_keys() -> Vec<OsCryptKey> {
    let mut keys: Vec<OsCryptKey> = Vec::new();
    if let Some(secret) = lookup_secret() {
        keys.push(OsCryptKey::Cbc(key_for(Some(&secret))));
    }
    let fallback = OsCryptKey::Cbc(key_for(None));
    if !keys.contains(&fallback) {
        keys.push(fallback);
    }
    keys
}

/// The `Local State` file Chromium keeps beside `sand-secrets.json`.
pub const LOCAL_STATE_FILE_NAME: &str = "Local State";

/// Windows: the DPAPI-protected key in the `Local State` beside the credential
/// file. Any failure is a credentials error with a fixed message.
#[cfg(windows)]
pub fn windows_oscrypt_key(secrets_path: &Path) -> Result<OsCryptKey> {
    let local_state = secrets_path.with_file_name(LOCAL_STATE_FILE_NAME);
    if !local_state.is_file() {
        return Err(AppError::Credentials(format!(
            "Grok Bot: no Local State at {} — install the Grok Bot desktop app and sign in to it",
            crate::display::sanitize_untrusted_path(&local_state)
        )));
    }
    crate::safe_storage::windows_key(&local_state)
        .map(OsCryptKey::Gcm)
        .map_err(|_| {
            AppError::Credentials(
                "Grok Bot: the desktop app's encryption key could not be read for this Windows user; sign in to the Grok Bot desktop app again"
                    .into(),
            )
        })
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
pub fn read_at(path: &Path, key: &OsCryptKey) -> Result<GrokbotCredentials> {
    parse(&read_raw(path)?, key)
}

/// [`read_at`] against a list of candidate keys — see [`oscrypt_keys`].
pub fn read_at_any(path: &Path, keys: &[OsCryptKey]) -> Result<GrokbotCredentials> {
    parse_any(&read_raw(path)?, keys)
}

fn read_raw(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            missing_file_error(path)
        } else {
            AppError::io_at(path, e)
        }
    })
}

/// No credential file at `path`. Names the fix, and the file actually checked
/// (a configured `secrets_path` makes the default advice useless otherwise).
pub fn missing_file_error(path: &Path) -> AppError {
    AppError::Credentials(format!(
        "Grok Bot: no credential file at {} — install the Grok Bot desktop app and sign in to it",
        crate::display::sanitize_untrusted_path(path)
    ))
}

/// `sand-secrets.json` does not hold a usable active account.
fn malformed() -> AppError {
    AppError::Credentials(
        "Grok Bot: sand-secrets.json does not hold an active signed-in account; sign in to the Grok Bot desktop app again"
            .into(),
    )
}

/// The active account's entry — the two token blobs live in here.
fn active_entry(raw: &[u8]) -> Result<serde_json::Map<String, serde_json::Value>> {
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
    entry.as_object().cloned().ok_or_else(malformed)
}

fn credentials_of(access_token: String, refresh_token: String) -> GrokbotCredentials {
    let fingerprint = fingerprint_of(&refresh_token);
    GrokbotCredentials {
        access_token,
        refresh_token,
        fingerprint,
    }
}

fn parse(raw: &[u8], key: &OsCryptKey) -> Result<GrokbotCredentials> {
    let entry = active_entry(raw)?;
    let access_token = decrypt_field(key, entry.get("cursor-access-token"))?;
    let refresh_token = decrypt_field(key, entry.get("cursor-refresh-token"))?;
    Ok(credentials_of(access_token, refresh_token))
}

/// [`parse`], trying each candidate key until one opens the active account.
///
/// A wrong AES key almost always fails PKCS#7 unpadding, and the rare residue
/// that does not must still decode as UTF-8 for *both* tokens, so a wrong
/// candidate is effectively ruled out and the order is only a preference. The
/// whole pair must open under one key: half a session is no session.
fn parse_any(raw: &[u8], keys: &[OsCryptKey]) -> Result<GrokbotCredentials> {
    let entry = active_entry(raw)?;
    let (access, refresh) = (
        entry.get("cursor-access-token"),
        entry.get("cursor-refresh-token"),
    );
    for key in keys {
        if let (Ok(access_token), Ok(refresh_token)) =
            (decrypt_field(key, access), decrypt_field(key, refresh))
        {
            return Ok(credentials_of(access_token, refresh_token));
        }
    }
    // None of them opened the pair. Re-run the first candidate on its own so
    // the error is the same one the single-key path has always reported.
    let Some(key) = keys.first() else {
        return Err(AppError::Credentials(
            "Grok Bot: no OSCrypt key was available to read the stored session; sign in to the Grok Bot desktop app again"
                .into(),
        ));
    };
    let access_token = decrypt_field(key, access)?;
    let refresh_token = decrypt_field(key, refresh)?;
    Ok(credentials_of(access_token, refresh_token))
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
fn decrypt_field(key: &OsCryptKey, field: Option<&serde_json::Value>) -> Result<String> {
    let blob = field
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            AppError::Credentials(
                "Grok Bot: sand-secrets.json is missing a token field; sign in to the Grok Bot desktop app again"
                    .into(),
            )
        })?;
    let bytes = key.decrypt(blob).map_err(|_| {
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

        let creds = read_at(&path, &OsCryptKey::Cbc(test_key())).unwrap();

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

        let creds = read_at(&path, &OsCryptKey::Cbc(key)).unwrap();
        assert_eq!(creds.access_token, "at-two");
        assert_eq!(creds.refresh_token, "rt-two");
    }

    /// The regression this whole candidate-key path exists for: the app wrote
    /// its blobs with Chromium's `"peanuts"` default while the machine holds a
    /// Secret Service item for it. Trying the item's key first must not fail the
    /// read.
    #[test]
    fn a_peanuts_keyed_file_opens_even_when_another_key_is_tried_first() {
        let td = TempDir::new().unwrap();
        let path = seed_secrets(&td, "at-peanuts", "rt-peanuts");
        let other = OsCryptKey::Cbc([0x5a; 16]);

        let creds = read_at_any(&path, &[other, OsCryptKey::Cbc(test_key())]).unwrap();

        assert_eq!(creds.access_token, "at-peanuts");
        assert_eq!(creds.refresh_token, "rt-peanuts");
    }

    /// And the mirror image: a Secret-Service-keyed file still opens when the
    /// `"peanuts"` candidate is tried first.
    #[test]
    fn a_secret_service_keyed_file_opens_behind_the_peanuts_candidate() {
        let td = TempDir::new().unwrap();
        let real = [0x27; 16];
        let path = td.path().join("sand-secrets.json");
        let doc = serde_json::json!({
            "cursor-accounts": {
                "active": "acct-1",
                "accounts": {
                    "acct-1": {
                        "cursor-access-token": crate::safe_storage::encrypt(&real, b"at-real"),
                        "cursor-refresh-token": crate::safe_storage::encrypt(&real, b"rt-real"),
                    }
                }
            }
        });
        std::fs::write(&path, doc.to_string()).unwrap();

        let creds =
            read_at_any(&path, &[OsCryptKey::Cbc(test_key()), OsCryptKey::Cbc(real)]).unwrap();

        assert_eq!(creds.access_token, "at-real");
        assert_eq!(creds.refresh_token, "rt-real");
    }

    /// No candidate opens the pair: the error stays the one the single-key path
    /// has always reported, so the bar still tells the user to sign in again.
    #[test]
    fn no_working_candidate_reports_the_could_not_be_decrypted_error() {
        let td = TempDir::new().unwrap();
        let path = seed_secrets(&td, "at-test", "rt-test");
        let wrong = [0x5a; 16];

        let err = read_at_any(&path, &[OsCryptKey::Cbc(wrong)]).unwrap_err();

        assert!(matches!(err, AppError::Credentials(_)), "{err:?}");
        assert!(err.to_string().contains("could not be decrypted"), "{err}");
    }

    /// `oscrypt_keys` always yields at least the `"peanuts"` default, so no
    /// caller hits this — but `read_at_any` is public and must not panic.
    #[test]
    fn no_candidate_key_reports_a_credentials_error() {
        let td = TempDir::new().unwrap();
        let path = seed_secrets(&td, "at-test", "rt-test");

        let err = read_at_any(&path, &[]).unwrap_err();

        assert!(matches!(err, AppError::Credentials(_)), "{err:?}");
        assert!(
            err.to_string().contains("no OSCrypt key was available"),
            "{err}"
        );
    }

    /// A malformed file is malformed for every candidate, and says so.
    #[test]
    fn a_malformed_file_is_rejected_before_any_key_is_tried() {
        let td = TempDir::new().unwrap();
        let path = td.path().join("sand-secrets.json");
        std::fs::write(&path, "not json").unwrap();

        let err = read_at_any(&path, &[OsCryptKey::Cbc(test_key())]).unwrap_err();

        assert!(
            err.to_string()
                .contains("does not hold an active signed-in account"),
            "{err}"
        );
    }

    #[test]
    fn a_missing_file_names_the_fix_and_the_path_checked() {
        let td = TempDir::new().unwrap();
        let missing = td.path().join("absent").join("sand-secrets.json");
        let err = read_at(&missing, &OsCryptKey::Cbc(test_key())).unwrap_err();
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
            let err = read_at(&path, &OsCryptKey::Cbc(test_key())).unwrap_err();
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
        let err = read_at(&path, &OsCryptKey::Cbc(wrong_key)).unwrap_err();
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

        let creds = read_at(&path, &OsCryptKey::Cbc(key)).unwrap();
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
        let creds = read_at(&path, &OsCryptKey::Cbc(key)).unwrap();
        assert_eq!(creds.access_token, "at-macos");
        assert_eq!(creds.refresh_token, "rt-macos");
    }

    const WINDOWS_KEY: [u8; 32] = [9; 32];

    /// Seed a `sand-secrets.json` the way the Windows app writes it:
    /// `cursor-accounts` as a JSON string, blobs as AES-256-GCM `v10` values.
    fn seed_windows_secrets(dir: &TempDir) -> std::path::PathBuf {
        let seal = |nonce: u8, text: &str| {
            crate::safe_storage::encrypt_windows(&WINDOWS_KEY, [nonce; 12], text.as_bytes())
        };
        let inner = serde_json::json!({
            "active": "acct-1",
            "accounts": {
                "acct-1": {
                    "cursor-access-token": seal(1, "at-windows"),
                    "cursor-account-profile": {},
                    "cursor-refresh-token": seal(2, "rt-windows"),
                }
            }
        });
        let path = dir.path().join("sand-secrets.json");
        let doc = serde_json::json!({
            "cursor-machine-id": "machine",
            "cursor-accounts": inner.to_string(),
        });
        std::fs::write(&path, doc.to_string()).unwrap();
        path
    }

    #[test]
    fn windows_gcm_blobs_decrypt_with_the_windows_key() {
        let td = TempDir::new().unwrap();
        let path = seed_windows_secrets(&td);
        let creds = read_at(&path, &OsCryptKey::Gcm(WINDOWS_KEY)).unwrap();
        assert_eq!(creds.access_token, "at-windows");
        assert_eq!(creds.refresh_token, "rt-windows");
        assert_eq!(creds.fingerprint, fingerprint_of("rt-windows"));
    }

    #[test]
    fn a_windows_store_under_the_wrong_key_is_a_credential_error_without_the_blob() {
        let td = TempDir::new().unwrap();
        let path = seed_windows_secrets(&td);
        for key in [OsCryptKey::Gcm([1; 32]), OsCryptKey::Cbc(test_key())] {
            let err = read_at(&path, &key).unwrap_err();
            assert!(matches!(err, AppError::Credentials(_)), "{err:?}");
            let message = err.to_string();
            assert!(message.contains("could not be decrypted"), "{message}");
            assert!(!message.contains("at-windows"), "{message}");
        }
    }

    #[test]
    fn debug_output_never_shows_a_key_or_a_token() {
        let td = TempDir::new().unwrap();
        let creds = read_at(&seed_windows_secrets(&td), &OsCryptKey::Gcm(WINDOWS_KEY)).unwrap();
        let shown = format!("{creds:?} {:?}", OsCryptKey::Gcm(WINDOWS_KEY));
        assert!(!shown.contains("at-windows"), "{shown}");
        assert!(!shown.contains("rt-windows"), "{shown}");
        assert!(!shown.contains("[9, 9"), "{shown}");
        assert!(shown.contains(&creds.fingerprint), "{shown}");
    }

    #[cfg(windows)]
    #[test]
    fn a_missing_local_state_names_the_fix_and_the_path_checked() {
        let td = TempDir::new().unwrap();
        let path = seed_windows_secrets(&td);
        let err = windows_oscrypt_key(&path).unwrap_err();
        assert!(matches!(err, AppError::Credentials(_)), "{err:?}");
        let message = err.to_string();
        assert!(message.contains("Local State"), "{message}");
        assert!(
            message.contains("install the Grok Bot desktop app"),
            "{message}"
        );
    }
}
