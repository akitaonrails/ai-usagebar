//! Read the Factory `droid` CLI's own login — read-only, never written.
//!
//! On Linux the CLI stores its session in `~/.factory/auth.v2.keyring`, a
//! file holding `iv_b64:tag_b64:ciphertext_b64` sealed with AES-256-GCM under
//! a 256-bit key the CLI keeps in the Secret Service keyring as the item
//! `service="Factory CLI" account="auth-encryption-key"` (a base64 string
//! decoding to 32 bytes). The plaintext is a JSON document whose
//! `access_token` is a short-lived JWT the CLI itself renews. Only that token
//! and `active_organization_id` (hashed, to scope the cache) are read. The
//! `refresh_token` and `whoami` fields are never inspected, stored, or sent
//! anywhere.
//!
//! The `secret-tool lookup` is a read-only subprocess; the key arrives on
//! stdout, never in argv. Every error here is a fixed string: the key, the
//! file contents and the token are all credentials and must never reach a
//! tooltip or a log line. Other `auth.v2.*` stores (`auth.v2.file`,
//! `auth.v2.key`, `auth.v2.loginkeychain`) and other platforms' storage are
//! not supported: their presence alone produces a credentials error that says
//! only the Linux keyring login is readable.

use std::io::Read;
use std::path::{Path, PathBuf};

use aes_gcm::aead::Aead as _;
use aes_gcm::{AesGcm, KeyInit as _};
use base64::Engine as _;
use chrono::{DateTime, Utc};

use crate::error::{AppError, Result};

/// The `droid` CLI's per-user directory under home.
pub const DATA_DIR_NAME: &str = ".factory";
/// The Secret Service keyring login store — the only storage this reads.
pub const KEYRING_FILE_NAME: &str = "auth.v2.keyring";
/// Other `auth.v2.*` stores the CLI may write. Their presence without the
/// keyring store means a login exists but cannot be read here.
const UNSUPPORTED_AUTH_FILES: [&str; 3] = ["auth.v2.file", "auth.v2.key", "auth.v2.loginkeychain"];
const MAX_KEYRING_FILE_BYTES: u64 = 64 * 1024;
const KEY_LEN: usize = 32;
const IV_LEN: usize = 16;
const TAG_LEN: usize = 16;

/// The CLI's Secret Service item coordinates for `secret-tool lookup`.
const KEYRING_SERVICE: &str = "Factory CLI";
const KEYRING_ACCOUNT: &str = "auth-encryption-key";

/// What an expired login and a rejected token both tell the user: the `droid`
/// CLI renews the JWT when it is used, so running it once is the whole fix.
pub const EXPIRED_LOGIN_MESSAGE: &str = "Droid login expired; run `droid` once to refresh it";

/// AES-256-GCM with the 16-byte IV the CLI's `createCipheriv` uses.
type DroidGcm = AesGcm<aes_gcm::aes::Aes256, aes_gcm::aead::consts::U16>;

/// The decrypted session's access token, plus the cache-scoping fingerprint.
/// `Debug` hides the token; the fingerprint is safe to show.
#[derive(Clone)]
pub struct Credentials {
    pub(crate) access_token: String,
    /// Hash of `active_organization_id`, so the cache survives the CLI
    /// rotating its ~24h access token; the token's own hash when the login
    /// names no organization.
    pub fingerprint: String,
    /// The JWT's `exp`, when it carries one.
    pub expires_at: Option<DateTime<Utc>>,
    /// The JWT's `exp` is already past. The fetch layer answers this from
    /// the cache with the re-login note — the token is never sent for a 401.
    pub expired: bool,
}

impl Credentials {
    /// Expired, or expiring within `margin` of `now`: the window in which
    /// asking the CLI to renew the login pays off.
    pub fn expires_within(&self, margin: chrono::Duration, now: DateTime<Utc>) -> bool {
        self.expired || self.expires_at.is_some_and(|exp| exp - margin <= now)
    }
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("access_token", &"[REDACTED]")
            .field("fingerprint", &self.fingerprint)
            .field("expires_at", &self.expires_at)
            .field("expired", &self.expired)
            .finish()
    }
}

/// `~/.factory/auth.v2.keyring` with the home directory injected.
pub fn keyring_path_in(home: &Path) -> PathBuf {
    home.join(DATA_DIR_NAME).join(KEYRING_FILE_NAME)
}

pub fn keyring_path() -> Result<PathBuf> {
    Ok(keyring_path_in(&crate::cache::home_dir()?))
}

/// Resolve the login on this platform. Only the Linux Secret Service keyring
/// store is supported; elsewhere this is a credentials error, never a guess.
#[cfg(target_os = "linux")]
pub fn resolve() -> Result<Credentials> {
    let home = crate::cache::home_dir()?;
    resolve_with(&home.join(DATA_DIR_NAME), keyring_key, Utc::now())
}

#[cfg(not(target_os = "linux"))]
pub fn resolve() -> Result<Credentials> {
    Err(AppError::Credentials(
        "Droid: reading the `droid` CLI login is only supported on Linux, where the CLI stores it in the Secret Service keyring; sign in with `droid` on Linux".into(),
    ))
}

/// The pure seam: credential directory, key lookup and clock are injected so
/// tests never touch the real keyring or `$HOME`. The file is checked before
/// the keyring is queried, so a missing store reports the missing login
/// rather than the missing key — and never runs `secret-tool` at all.
pub fn resolve_with(
    dir: &Path,
    keyring_key: impl FnOnce() -> Result<[u8; KEY_LEN]>,
    now: DateTime<Utc>,
) -> Result<Credentials> {
    let path = dir.join(KEYRING_FILE_NAME);
    let raw = read_bounded(&path, dir)?;
    let key = keyring_key()?;
    let plaintext = decrypt(&key, &raw)?;
    parse_plaintext(&plaintext, now)
}

fn read_bounded(path: &Path, dir: &Path) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            missing_file_error(dir, path)
        } else {
            AppError::io_at(path, error)
        }
    })?;
    let mut bytes = Vec::new();
    file.take(MAX_KEYRING_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| AppError::io_at(path, error))?;
    if bytes.len() as u64 > MAX_KEYRING_FILE_BYTES {
        return Err(malformed());
    }
    Ok(bytes)
}

/// No `auth.v2.keyring`. When a different `auth.v2.*` store is the only login
/// present, say so plainly — the file exists, it is just not one this reads.
fn missing_file_error(dir: &Path, path: &Path) -> AppError {
    if UNSUPPORTED_AUTH_FILES
        .iter()
        .any(|name| dir.join(name).is_file())
    {
        return AppError::Credentials(
            "Droid: the `droid` CLI login on this machine does not use the Secret Service keyring store, the only one supported; sign in with `droid` on Linux".into(),
        );
    }
    AppError::Credentials(format!(
        "Droid CLI login not found at {}; sign in with `droid` and retry",
        crate::display::sanitize_untrusted_path(path)
    ))
}

/// `secret-tool lookup service "Factory CLI" account "auth-encryption-key"`,
/// read-only; the key arrives on stdout, never in argv. stderr is piped, not
/// dropped: telling an absent item (a silent non-zero exit) apart from a
/// daemon that never answered (a diagnostic on stderr) is what decides
/// whether the fix is "sign in" or "the keyring is unavailable" — the text
/// itself is never echoed.
#[cfg(target_os = "linux")]
pub fn keyring_key() -> Result<[u8; KEY_LEN]> {
    keyring_key_with(keyring_command().output())
}

/// The lookup command with its safety settings, built apart from the spawn
/// so a test can inspect it.
#[cfg(target_os = "linux")]
fn keyring_command() -> std::process::Command {
    let mut command = std::process::Command::new("secret-tool");
    command
        .args([
            "lookup",
            "service",
            KEYRING_SERVICE,
            "account",
            KEYRING_ACCOUNT,
        ])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped());
    // A credential lookup must not inherit this process's provider keys.
    for var in crate::vendor::vendor_secret_env_vars_to_remove(&[]) {
        command.env_remove(var);
    }
    command
}

/// Interpret the `secret-tool lookup` result — the command seam, split from
/// the spawn so tests can feed canned outputs. A missing binary or no
/// running daemon (spawn failure, or a non-zero exit with a diagnostic on
/// stderr) means the keyring cannot be queried here; only the silent
/// failure that means an absent item tells the user to sign in with `droid`.
#[cfg(target_os = "linux")]
fn keyring_key_with(output: std::io::Result<std::process::Output>) -> Result<[u8; KEY_LEN]> {
    let output = output.map_err(|_| keyring_unavailable())?;
    if !output.status.success() {
        return Err(if output.stderr.is_empty() {
            missing_keyring_item()
        } else {
            keyring_unavailable()
        });
    }
    let secret = String::from_utf8(output.stdout)
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .ok_or_else(missing_keyring_item)?;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(secret.as_bytes())
        .map_err(|_| malformed_key())?;
    <[u8; KEY_LEN]>::try_from(raw.as_slice()).map_err(|_| malformed_key())
}

#[cfg(target_os = "linux")]
fn missing_keyring_item() -> AppError {
    AppError::Credentials(
        "Droid: no `Factory CLI/auth-encryption-key` item in the Secret Service keyring; sign in with `droid` and retry".into(),
    )
}

#[cfg(target_os = "linux")]
fn keyring_unavailable() -> AppError {
    AppError::Credentials(
        "Droid: the Secret Service keyring could not be queried — `secret-tool` is missing or no daemon is running".into(),
    )
}

fn malformed_key() -> AppError {
    AppError::Credentials(
        "Droid: the `Factory CLI/auth-encryption-key` keyring item is malformed; sign in with `droid` again".into(),
    )
}

fn malformed() -> AppError {
    AppError::Credentials(
        "Droid: the `droid` CLI login could not be read; sign in with `droid` again".into(),
    )
}

/// Open an `iv_b64:tag_b64:ciphertext_b64` envelope. The tag travels between
/// the IV and the ciphertext — the CLI's `getAuthTag()` output — and GCM's
/// decrypt wants `ciphertext || tag`, so it is rejoined in that order.
fn decrypt(key: &[u8; KEY_LEN], contents: &[u8]) -> Result<Vec<u8>> {
    let text = std::str::from_utf8(contents).map_err(|_| malformed())?;
    let mut parts = text.trim().split(':');
    let (Some(iv_b64), Some(tag_b64), Some(ct_b64), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(malformed());
    };
    let decode = |part: &str| {
        base64::engine::general_purpose::STANDARD
            .decode(part)
            .map_err(|_| malformed())
    };
    let iv = decode(iv_b64)?;
    let tag = decode(tag_b64)?;
    let mut sealed = decode(ct_b64)?;
    if iv.len() != IV_LEN || tag.len() != TAG_LEN {
        return Err(malformed());
    }
    sealed.extend_from_slice(&tag);
    let nonce = aes_gcm::Nonce::try_from(iv.as_slice()).map_err(|_| malformed())?;
    DroidGcm::new(key.into())
        .decrypt(&nonce, sealed.as_ref())
        .map_err(|_| malformed())
}

/// The decrypted `auth.v2.keyring` JSON. Only `access_token` is consumed; a
/// JWT whose `exp` has passed comes back flagged `expired` — the fetch layer
/// answers it from the cache with the re-login note rather than sending it
/// to the API for a 401 to explain.
fn parse_plaintext(bytes: &[u8], now: DateTime<Utc>) -> Result<Credentials> {
    let root: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| malformed())?;
    let access_token = root
        .get("access_token")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty() && token.bytes().all(|b| b.is_ascii_graphic()))
        .map(str::to_string)
        .ok_or_else(malformed)?;
    let expires_at = crate::jwt::claims(&access_token)
        .and_then(|claims| claims.get("exp").and_then(serde_json::Value::as_i64))
        .and_then(|exp| DateTime::from_timestamp(exp, 0));
    let expired = expires_at.is_some_and(|exp| exp <= now);
    let fingerprint = root
        .get("active_organization_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|org| !org.is_empty())
        .map(|org| crate::cache::fingerprint_of(&format!("droid-org:{org}")))
        .unwrap_or_else(|| crate::cache::fingerprint_of(&access_token));
    Ok(Credentials {
        access_token,
        fingerprint,
        expires_at,
        expired,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const TEST_KEY: [u8; KEY_LEN] = [0x2a; KEY_LEN];

    fn jwt(exp: i64) -> String {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(format!(r#"{{"sub":"user-1","exp":{exp}}}"#));
        format!("header.{payload}.signature")
    }

    /// Seal a plaintext the way the CLI writes it: `iv:tag:ciphertext`, each
    /// standard base64. A fixed IV keeps the fixture deterministic.
    fn seal(key: &[u8; KEY_LEN], plaintext: &[u8]) -> String {
        let iv = [7u8; IV_LEN];
        let nonce = aes_gcm::Nonce::try_from(iv.as_slice()).unwrap();
        let sealed = DroidGcm::new(key.into())
            .encrypt(&nonce, plaintext)
            .unwrap();
        let (ct, tag) = sealed.split_at(sealed.len() - TAG_LEN);
        let b64 = base64::engine::general_purpose::STANDARD;
        format!("{}:{}:{}", b64.encode(iv), b64.encode(tag), b64.encode(ct))
    }

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_791_014_400, 0).unwrap()
    }

    fn write(dir: &TempDir, contents: &str) -> PathBuf {
        let path = dir.path().join(KEYRING_FILE_NAME);
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn ok_key() -> Result<[u8; KEY_LEN]> {
        Ok(TEST_KEY)
    }

    #[test]
    fn a_keyring_file_opens_into_the_access_token_and_scopes_the_cache() {
        let td = TempDir::new().unwrap();
        let token = jwt(now().timestamp() + 3600);
        let doc = serde_json::json!({
            "access_token": token,
            "refresh_token": "refresh-canary-must-not-be-read",
            "active_organization_id": "org-1",
            "whoami": {"inferenceRegion": "us", "isEnterprise": false},
        })
        .to_string();
        write(&td, &seal(&TEST_KEY, doc.as_bytes()));

        let credentials = resolve_with(td.path(), ok_key, now()).unwrap();

        assert!(!credentials.expired);
        assert_eq!(credentials.access_token, token);
        assert_eq!(
            credentials.fingerprint,
            crate::cache::fingerprint_of("droid-org:org-1")
        );
        assert_eq!(credentials.fingerprint.len(), 16);
    }

    #[test]
    fn the_cache_scope_follows_the_organization_across_token_rotation() {
        let td = TempDir::new().unwrap();
        let resolve_token = |token: String, org: Option<&str>| {
            let mut doc = serde_json::json!({"access_token": token});
            if let Some(org) = org {
                doc["active_organization_id"] = org.into();
            }
            write(&td, &seal(&TEST_KEY, doc.to_string().as_bytes()));
            resolve_with(td.path(), ok_key, now()).unwrap().fingerprint
        };
        let before = resolve_token(jwt(now().timestamp() + 60), Some("org-1"));
        let rotated = resolve_token(jwt(now().timestamp() + 86_400), Some("org-1"));
        let other_org = resolve_token(jwt(now().timestamp() + 86_400), Some("org-2"));
        assert_eq!(before, rotated);
        assert_ne!(rotated, other_org);

        // No organization: the token itself scopes the cache.
        let token = jwt(now().timestamp() + 60);
        assert_eq!(
            resolve_token(token.clone(), None),
            crate::cache::fingerprint_of(&token)
        );
        assert_eq!(
            resolve_token(token.clone(), Some("  ")),
            crate::cache::fingerprint_of(&token)
        );
    }

    #[test]
    fn expiry_is_read_from_the_jwt_and_drives_the_renewal_window() {
        let td = TempDir::new().unwrap();
        let exp = now().timestamp() + 300;
        let doc = serde_json::json!({"access_token": jwt(exp)}).to_string();
        write(&td, &seal(&TEST_KEY, doc.as_bytes()));
        let credentials = resolve_with(td.path(), ok_key, now()).unwrap();
        assert_eq!(credentials.expires_at.unwrap().timestamp(), exp);
        assert!(!credentials.expired);
        assert!(credentials.expires_within(chrono::Duration::minutes(10), now()));
        assert!(!credentials.expires_within(chrono::Duration::minutes(1), now()));
    }

    #[test]
    fn a_wrong_key_or_a_tampered_tag_fails_to_open() {
        let td = TempDir::new().unwrap();
        let doc = serde_json::json!({"access_token": jwt(now().timestamp() + 3600)}).to_string();
        let sealed = seal(&TEST_KEY, doc.as_bytes());
        write(&td, &sealed);
        assert!(resolve_with(td.path(), || Ok([0x9b; KEY_LEN]), now()).is_err());

        // Flip a byte inside the base64 ciphertext: GCM's tag check fails.
        let mut tampered = sealed.into_bytes();
        let last = tampered.len() - 3;
        tampered[last] = if tampered[last] == b'A' { b'B' } else { b'A' };
        write(&td, std::str::from_utf8(&tampered).unwrap());
        assert!(resolve_with(td.path(), ok_key, now()).is_err());
    }

    #[test]
    fn an_expired_jwt_comes_back_flagged_for_the_fetch_layer() {
        let td = TempDir::new().unwrap();
        let doc = serde_json::json!({"access_token": jwt(now().timestamp() - 60)}).to_string();
        write(&td, &seal(&TEST_KEY, doc.as_bytes()));

        // Still parseable — the fingerprint scopes the cache the fallback
        // serves — but the flag keeps it off the wire.
        let credentials = resolve_with(td.path(), ok_key, now()).unwrap();
        assert!(credentials.expired);
    }

    #[test]
    fn a_jwt_without_exp_is_accepted() {
        let td = TempDir::new().unwrap();
        let doc = serde_json::json!({"access_token": "header.c2lnbmVk.signature"}).to_string();
        write(&td, &seal(&TEST_KEY, doc.as_bytes()));
        assert!(resolve_with(td.path(), ok_key, now()).is_ok());
    }

    #[test]
    fn malformed_envelopes_and_documents_are_credentials_errors() {
        for contents in [
            "not-three-parts",
            "a:b",
            "a:b:c:d",
            "@@@:@@@:@@@",
            // Valid base64, wrong IV length (8 bytes).
            &seal(&TEST_KEY, b"{}").replace("BwcHBwcHBwcHBwcHBwcHBw==", "AAAA"),
        ] {
            let td = TempDir::new().unwrap();
            write(&td, contents);
            assert!(
                resolve_with(td.path(), ok_key, now()).is_err(),
                "{contents:?}"
            );
        }
        // Opens as GCM but is not a session document.
        let td = TempDir::new().unwrap();
        write(&td, &seal(&TEST_KEY, b"not json"));
        assert!(resolve_with(td.path(), ok_key, now()).is_err());
        let td = TempDir::new().unwrap();
        write(
            &td,
            &seal(&TEST_KEY, br#"{"refresh_token":"only-a-refresh"}"#),
        );
        assert!(resolve_with(td.path(), ok_key, now()).is_err());
    }

    #[test]
    fn errors_never_echo_key_or_token_material() {
        let td = TempDir::new().unwrap();
        let token = jwt(now().timestamp() + 3600);
        let doc = serde_json::json!({
            "access_token": token,
            "refresh_token": "refresh-canary-9911",
        })
        .to_string();
        let sealed = seal(&TEST_KEY, doc.as_bytes());
        write(&td, &sealed);
        let error = resolve_with(td.path(), || Ok([0x9b; KEY_LEN]), now())
            .unwrap_err()
            .to_string();
        assert!(!error.contains("refresh-canary"));
        assert!(!error.contains(&sealed[..24]));

        let credentials = resolve_with(td.path(), ok_key, now()).unwrap();
        let debug = format!("{credentials:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains(&token));
    }

    #[test]
    fn a_missing_file_reports_other_stores_as_unsupported() {
        let td = TempDir::new().unwrap();
        let error = resolve_with(td.path(), ok_key, now()).unwrap_err();
        assert!(error.to_string().contains("not found"), "{error}");

        std::fs::write(td.path().join("auth.v2.file"), "{}").unwrap();
        let error = resolve_with(td.path(), ok_key, now()).unwrap_err();
        assert!(
            error.to_string().contains("keyring"),
            "expected the unsupported-store message: {error}"
        );
    }

    #[test]
    fn the_file_check_runs_before_the_keyring_lookup() {
        let td = TempDir::new().unwrap();
        let error = resolve_with(
            td.path(),
            || panic!("the keyring must not be queried when no file exists"),
            now(),
        )
        .unwrap_err();
        assert!(matches!(error, AppError::Credentials(_)));
    }

    /// A `secret-tool` result, faked: exit `code` with the given stdout and
    /// stderr (the wait-status encoding shifts the exit code left 8).
    #[cfg(target_os = "linux")]
    fn output(code: i32, stdout: &[u8], stderr: &[u8]) -> std::io::Result<std::process::Output> {
        use std::os::unix::process::ExitStatusExt;
        Ok(std::process::Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.to_vec(),
            stderr: stderr.to_vec(),
        })
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_missing_secret_tool_or_daemon_does_not_say_to_sign_in() {
        // The binary itself is missing: the spawn fails.
        let error = keyring_key_with(Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "secret-tool",
        )))
        .unwrap_err();
        let text = error.to_string();
        assert!(text.contains("could not be queried"), "{text}");
        assert!(!text.contains("sign in"), "{text}");

        // The binary ran but no daemon answered: non-zero exit carrying a
        // diagnostic on stderr.
        let error = keyring_key_with(output(1, b"", b"secret-tool: cannot connect")).unwrap_err();
        let text = error.to_string();
        assert!(text.contains("could not be queried"), "{text}");
        assert!(!text.contains("sign in"), "{text}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn only_an_absent_keyring_item_says_to_sign_in() {
        // A silent non-zero exit is how `lookup` reports a missing item.
        let error = keyring_key_with(output(1, b"", b"")).unwrap_err();
        assert!(
            error.to_string().contains("sign in with `droid`"),
            "{error}"
        );
        // A successful run still decodes the base64 key from stdout.
        let key = base64::engine::general_purpose::STANDARD.encode(TEST_KEY);
        assert_eq!(
            keyring_key_with(output(0, key.as_bytes(), b"")).unwrap(),
            TEST_KEY
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_keyring_lookup_keeps_the_key_off_argv_and_drops_provider_secrets() {
        let command = keyring_command();
        assert_eq!(command.get_program(), "secret-tool");
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(
            args,
            [
                "lookup",
                "service",
                KEYRING_SERVICE,
                "account",
                KEYRING_ACCOUNT
            ]
        );
        let removed: Vec<_> = command
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(key, _)| key.to_string_lossy().into_owned())
            .collect();
        let expected = crate::vendor::vendor_secret_env_vars_to_remove(&[]);
        assert!(!expected.is_empty());
        for var in expected {
            assert!(removed.iter().any(|key| key == var), "{var} not removed");
        }
    }
}
