//! Antigravity's saved Google session, read from the OS keyring or CLI file.
//!
//! Antigravity normally stores the OAuth token it obtained at sign-in through
//! Go's `go-keyring`, under service `gemini` and account `antigravity`. On
//! Windows that is a generic credential named `gemini:antigravity` in the
//! Credential Manager, on macOS a generic password in the login Keychain, and
//! on Linux a Secret Service item. The `agy` CLI can instead use the
//! JSON file `~/.gemini/antigravity-cli/antigravity-oauth-token`. Both shapes
//! are accepted; the keyring is tried first.
//!
//! This module only *reads* the blob and pulls out the tokens; the network
//! side (refresh, quota) lives in `cloud.rs`. The keyring and CLI file are
//! never written.
//!
//! [`read`] is deliberately forgiving: any keyring hiccup — no backend, a
//! locked store, a denied ACL — falls through to the CLI file. If neither
//! source is available, the caller gets `Ok(None)` and can produce its own
//! "no session" message.

use std::path::Path;

use base64::Engine as _;
use chrono::{DateTime, Utc};

use crate::error::{AppError, Result};

/// `go-keyring` service name Antigravity registers its session under.
pub const KEYRING_SERVICE: &str = "gemini";
/// `go-keyring` account (user) name paired with [`KEYRING_SERVICE`].
pub const KEYRING_ACCOUNT: &str = "antigravity";
/// Prefix `go-keyring` adds when it had to base64-wrap the value.
pub const GO_KEYRING_PREFIX: &str = "go-keyring-base64:";

/// Upper bound on the blob a keyring may hand back. A real token blob is
/// well under 4 KiB; anything larger is not something this module parses.
const MAX_BLOB_BYTES: usize = 64 * 1024;

const ACCESS_TOKEN_KEYS: [&str; 8] = [
    "access_token",
    "accessToken",
    "token",
    "id_token",
    "idToken",
    "bearerToken",
    "auth_token",
    "authToken",
];
const REFRESH_TOKEN_KEYS: [&str; 2] = ["refresh_token", "refreshToken"];
const EXPIRY_KEYS: [&str; 3] = ["expiry", "expires_at", "expiresAt"];

/// Any value at or above this is an epoch in milliseconds, not seconds
/// (10^11 s is the year 5138; 10^11 ms is 1973).
const EPOCH_MILLIS_THRESHOLD: f64 = 1e11;

/// The tokens Antigravity saved, plus a non-secret identity for them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    /// First 16 hex chars of the SHA-256 of the refresh token (or of the
    /// access token when there is none). Scopes ai-usagebar's own persisted
    /// refresh to this sign-in, so a re-login never gets the old session's
    /// cached access token.
    pub fingerprint: String,
    /// Hash of the email the official CLI saved with its ID-token identity.
    /// Kept separate from the credential fingerprint used to scope caches.
    pub account_hash: Option<String>,
}

/// Decode the raw keyring value into a [`StoredToken`].
///
/// Error messages are fixed strings: the input is a credential and must never
/// end up in a tooltip or a log line.
pub fn parse_keyring_blob(raw: &str) -> Result<StoredToken> {
    let raw = raw.trim();
    let json = match raw.strip_prefix(GO_KEYRING_PREFIX) {
        Some(encoded) => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(encoded.trim())
                .map_err(|_| {
                    AppError::Credentials(
                        "Antigravity's saved Google session is not valid base64; open Antigravity and sign in again"
                            .into(),
                    )
                })?;
            String::from_utf8(bytes).map_err(|_| {
                AppError::Credentials(
                    "Antigravity's saved Google session is not UTF-8; open Antigravity and sign in again"
                        .into(),
                )
            })?
        }
        None => raw.to_string(),
    };
    let root: serde_json::Value = serde_json::from_str(&json).map_err(|_| {
        AppError::Credentials(
            "Antigravity's saved Google session is not valid JSON; open Antigravity and sign in again"
                .into(),
        )
    })?;
    let token = match root.get("token") {
        Some(nested) if nested.is_object() => nested,
        _ => &root,
    };

    let access_token = first_string(token, &ACCESS_TOKEN_KEYS).ok_or_else(|| {
        AppError::Credentials(
            "Antigravity's saved Google session has no access token; open Antigravity and sign in"
                .into(),
        )
    })?;
    let refresh_token = first_string(token, &REFRESH_TOKEN_KEYS);
    let expires_at = EXPIRY_KEYS
        .iter()
        .filter_map(|key| token.get(key))
        .find_map(parse_expiry);
    let fingerprint =
        crate::cache::fingerprint_of(refresh_token.as_deref().unwrap_or(&access_token));

    Ok(StoredToken {
        access_token,
        refresh_token,
        expires_at,
        fingerprint,
        account_hash: root
            .get("id_token_email")
            .and_then(serde_json::Value::as_str)
            .filter(|email| !email.trim().is_empty())
            .map(super::account_hash),
    })
}

fn first_string(object: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|key| object.get(key).and_then(serde_json::Value::as_str))
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(str::to_string)
}

fn parse_expiry(value: &serde_json::Value) -> Option<DateTime<Utc>> {
    match value {
        serde_json::Value::String(text) => {
            let text = text.trim();
            if let Ok(parsed) = DateTime::parse_from_rfc3339(text) {
                return Some(parsed.with_timezone(&Utc));
            }
            text.parse::<f64>().ok().and_then(epoch_to_datetime)
        }
        serde_json::Value::Number(number) => number.as_f64().and_then(epoch_to_datetime),
        _ => None,
    }
}

fn epoch_to_datetime(epoch: f64) -> Option<DateTime<Utc>> {
    if !epoch.is_finite() || epoch <= 0.0 {
        return None;
    }
    let millis = if epoch >= EPOCH_MILLIS_THRESHOLD {
        epoch
    } else {
        epoch * 1000.0
    };
    if millis > i64::MAX as f64 {
        return None;
    }
    DateTime::from_timestamp_millis(millis as i64)
}

/// The raw blob from the OS keyring or Antigravity CLI token file, or `None`
/// when no saved session is available. Never errors on a backend failure — see
/// the module docs.
pub fn read() -> Result<Option<String>> {
    let cli_path = crate::cache::home_dir().ok().map(|home| {
        home.join(".gemini")
            .join("antigravity-cli")
            .join("antigravity-oauth-token")
    });
    Ok(read_saved_session(read_platform(), cli_path.as_deref()))
}

fn read_saved_session(keyring: Option<String>, cli_path: Option<&Path>) -> Option<String> {
    keyring.or_else(|| cli_path.and_then(read_cli_token_file))
}

/// Read the read-only OAuth file used by the Antigravity CLI when it cannot
/// use the OS keyring. The file has the same JSON credential shape as the
/// keyring blob, including the nested `token` object.
///
/// Held to the keyring path's limits rather than trusted because it is a file:
/// only a regular file is opened — checked before `open`, since opening a FIFO
/// blocks until a writer appears and this runs inside the widget's fetch — and
/// at most `MAX_BLOB_BYTES + 1` bytes are read, so a runaway file or a symlink
/// to a device is rejected by `decode_blob_bytes` instead of filling memory.
fn read_cli_token_file(path: &Path) -> Option<String> {
    use std::io::Read;
    if !std::fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(MAX_BLOB_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    decode_blob_bytes(&bytes)
}

/// A keyring value as bytes → text. `go-keyring` writes UTF-8; a credential
/// written by something else on Windows may be UTF-16LE.
///
/// UTF-16LE of ASCII text is byte-valid UTF-8 (NUL is a legal code point), so
/// "does it parse as UTF-8" cannot tell them apart. A JSON blob never carries
/// a NUL, so an even-length value with a NUL byte (or a UTF-16 BOM) is UTF-16.
fn decode_blob_bytes(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() || bytes.len() > MAX_BLOB_BYTES {
        return None;
    }
    let looks_utf16 =
        bytes.len().is_multiple_of(2) && (bytes.starts_with(&[0xff, 0xfe]) || bytes.contains(&0));
    let text = if looks_utf16 {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        let units = units.strip_prefix(&[0xfeff]).unwrap_or(&units);
        String::from_utf16(units).ok()?
    } else {
        std::str::from_utf8(bytes).ok()?.to_string()
    };
    let text = text.trim_matches(['\0', '\n', '\r', ' ']);
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

#[cfg(windows)]
fn read_platform() -> Option<String> {
    use windows_sys::Win32::Security::Credentials::{
        CRED_TYPE_GENERIC, CREDENTIALW, CredFree, CredReadW,
    };

    let target: Vec<u16> = format!("{KEYRING_SERVICE}:{KEYRING_ACCOUNT}")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut credential: *mut CREDENTIALW = std::ptr::null_mut();
    // SAFETY: `target` is a NUL-terminated UTF-16 string that outlives the
    // call; `credential` is a local out-slot the API fills on success.
    let ok = unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) };
    // `ERROR_NOT_FOUND` is the expected "never signed in" case; any other
    // failure (locked store, denied access) is treated the same way rather
    // than failing the vendor on a keyring hiccup.
    if ok == 0 || credential.is_null() {
        return None;
    }
    // SAFETY: `CredReadW` succeeded, so `credential` points at a CREDENTIALW
    // owned by the API that stays valid until `CredFree`. The blob pointer
    // and size describe one allocation of exactly `CredentialBlobSize` bytes.
    let bytes = unsafe {
        let cred = &*credential;
        let len = cred.CredentialBlobSize as usize;
        if cred.CredentialBlob.is_null() || len == 0 || len > MAX_BLOB_BYTES {
            Vec::new()
        } else {
            std::slice::from_raw_parts(cred.CredentialBlob, len).to_vec()
        }
    };
    // SAFETY: `credential` came from `CredReadW` and is freed exactly once.
    unsafe { CredFree(credential.cast()) };
    decode_blob_bytes(&bytes)
}

#[cfg(target_os = "macos")]
fn read_platform() -> Option<String> {
    let out = keyring_lookup_command().output().ok()?;
    if !out.status.success() {
        return None;
    }
    decode_blob_bytes(&out.stdout)
}

/// `security find-generic-password -s <service> -a <account> -w`, read-only;
/// the secret arrives on stdout, never in argv.
#[cfg(target_os = "macos")]
fn keyring_lookup_command() -> std::process::Command {
    let mut command = std::process::Command::new("/usr/bin/security");
    command
        .args([
            "find-generic-password",
            "-s",
            KEYRING_SERVICE,
            "-a",
            KEYRING_ACCOUNT,
            "-w",
        ])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // A credential lookup must not inherit this process's provider keys.
    for var in crate::vendor::vendor_secret_env_vars_to_remove(&[]) {
        command.env_remove(var);
    }
    command
}

#[cfg(not(any(windows, target_os = "macos")))]
fn read_platform() -> Option<String> {
    secret_tool_value(
        || keyring_lookup_command().output().ok(),
        || keyring_search_command().output().ok(),
    )
}

/// The saved session as `secret-tool` reports it, given the two ways of
/// asking. Split from the spawns so a test can feed it canned output.
#[cfg(not(any(windows, target_os = "macos")))]
fn secret_tool_value(
    lookup: impl FnOnce() -> Option<std::process::Output>,
    search: impl FnOnce() -> Option<std::process::Output>,
) -> Option<String> {
    // `secret-tool` (libsecret) speaks to whichever Secret Service is running.
    // A missing binary or no running daemon both exit non-zero / fail to
    // spawn, and both mean "no session available here".
    let out = lookup()?;
    if out.status.success() {
        return decode_blob_bytes(&out.stdout);
    }
    // `lookup` prints only a value whose content type is exactly `text/plain`.
    // go-keyring stores `text/plain; charset=utf8`, so on a Secret Service
    // that keeps the type as written (KDE's ksecretd) `lookup` exits 1 with
    // "secret does not contain a textual password" although the item is
    // there. `search` prints the value whatever its content type.
    let out = search()?;
    if !out.status.success() {
        return None;
    }
    decode_blob_bytes(secret_from_search_output(&out.stdout)?)
}

/// `secret-tool lookup service <service> username <account>`, read-only; the
/// secret arrives on stdout, never in argv.
#[cfg(not(any(windows, target_os = "macos")))]
fn keyring_lookup_command() -> std::process::Command {
    secret_tool_command("lookup")
}

/// `secret-tool search service <service> username <account>`, read-only. It
/// reports the first matching item as `key = value` lines, the secret among
/// them, on stdout. Without `--unlock` a locked item prints no `secret` line.
#[cfg(not(any(windows, target_os = "macos")))]
fn keyring_search_command() -> std::process::Command {
    secret_tool_command("search")
}

#[cfg(not(any(windows, target_os = "macos")))]
fn secret_tool_command(action: &str) -> std::process::Command {
    let mut command = std::process::Command::new("secret-tool");
    command
        .args([
            action,
            "service",
            KEYRING_SERVICE,
            "username",
            KEYRING_ACCOUNT,
        ])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // A credential lookup must not inherit this process's provider keys.
    for var in crate::vendor::vendor_secret_env_vars_to_remove(&[]) {
        command.env_remove(var);
    }
    command
}

/// The value of the `secret = ` line in `secret-tool search` output.
/// secret-tool writes the value's raw bytes up to the end of that line, so a
/// value holding a newline is cut there; the token blobs this module reads are
/// single-line JSON or base64, and a cut blob fails to parse instead.
#[cfg(not(any(windows, target_os = "macos")))]
fn secret_from_search_output(stdout: &[u8]) -> Option<&[u8]> {
    stdout
        .split(|&byte| byte == b'\n')
        .find_map(|line| line.strip_prefix(b"secret = "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(json: &str) -> String {
        format!(
            "{GO_KEYRING_PREFIX}{}",
            base64::engine::general_purpose::STANDARD.encode(json)
        )
    }

    #[test]
    fn bare_json_blob_parses() {
        let token = parse_keyring_blob(
            r#"{"access_token":"at","refresh_token":"rt","expiry":"2030-01-02T03:04:05Z"}"#,
        )
        .unwrap();
        assert_eq!(token.access_token, "at");
        assert_eq!(token.refresh_token.as_deref(), Some("rt"));
        assert_eq!(
            token.expires_at.unwrap().to_rfc3339(),
            "2030-01-02T03:04:05+00:00"
        );
        assert_eq!(token.fingerprint.len(), 16);
    }

    #[test]
    fn go_keyring_prefixed_blob_parses_to_the_same_token() {
        let json = r#"{"access_token":"at","refresh_token":"rt"}"#;
        let bare = parse_keyring_blob(json).unwrap();
        let wrapped = parse_keyring_blob(&format!("  {}\n", encode(json))).unwrap();
        assert_eq!(bare, wrapped);
    }

    #[test]
    fn nested_token_object_is_unwrapped() {
        let token = parse_keyring_blob(
            r#"{"email":"someone","token":{"accessToken":"nested-at","refreshToken":"nested-rt"}}"#,
        )
        .unwrap();
        assert_eq!(token.access_token, "nested-at");
        assert_eq!(token.refresh_token.as_deref(), Some("nested-rt"));
    }

    #[test]
    fn cli_identity_is_hashed_without_changing_the_credential_fingerprint() {
        let raw = r#"{"id_token_email":" Someone@Example.COM ","token":{"access_token":"at","refresh_token":"rt"}}"#;
        let token = parse_keyring_blob(raw).unwrap();
        assert_eq!(
            token.account_hash,
            Some(super::super::account_hash("someone@example.com"))
        );
        assert_eq!(parse_keyring_blob(&encode(raw)).unwrap(), token);
        let without_identity =
            parse_keyring_blob(r#"{"access_token":"at","refresh_token":"rt"}"#).unwrap();
        assert_eq!(token.fingerprint, without_identity.fingerprint);
        assert_eq!(without_identity.account_hash, None);
        for metadata in [
            serde_json::json!(null),
            serde_json::json!(false),
            serde_json::json!(" "),
        ] {
            let raw = serde_json::json!({"id_token_email":metadata,"access_token":"at"});
            assert_eq!(
                parse_keyring_blob(&raw.to_string()).unwrap().account_hash,
                None
            );
        }
        let arbitrary_email =
            parse_keyring_blob(r#"{"email":"someone@example.com","access_token":"at"}"#).unwrap();
        assert_eq!(arbitrary_email.account_hash, None);
    }

    #[test]
    fn every_access_token_alias_is_accepted() {
        for key in ACCESS_TOKEN_KEYS {
            let token = parse_keyring_blob(&format!(r#"{{"{key}":"value-{key}"}}"#)).unwrap();
            assert_eq!(token.access_token, format!("value-{key}"), "{key}");
            assert_eq!(token.refresh_token, None);
        }
    }

    #[test]
    fn every_refresh_token_alias_is_accepted() {
        for key in REFRESH_TOKEN_KEYS {
            let token =
                parse_keyring_blob(&format!(r#"{{"access_token":"at","{key}":"rt-{key}"}}"#))
                    .unwrap();
            assert_eq!(
                token.refresh_token.as_deref(),
                Some(format!("rt-{key}")).as_deref()
            );
        }
    }

    #[test]
    fn empty_aliases_are_skipped_in_favour_of_later_ones() {
        let token =
            parse_keyring_blob(r#"{"access_token":"  ","accessToken":"","token":"real"}"#).unwrap();
        assert_eq!(token.access_token, "real");
    }

    #[test]
    fn expiry_accepts_rfc3339_epoch_seconds_and_epoch_millis() {
        let expected = "2030-01-02T03:04:05+00:00";
        let cases = [
            r#"{"access_token":"at","expiry":"2030-01-02T04:04:05+01:00"}"#,
            r#"{"access_token":"at","expires_at":1893553445}"#,
            r#"{"access_token":"at","expiresAt":1893553445000}"#,
            r#"{"access_token":"at","expires_at":"1893553445"}"#,
        ];
        for case in cases {
            let token = parse_keyring_blob(case).unwrap();
            assert_eq!(token.expires_at.unwrap().to_rfc3339(), expected, "{case}");
        }
    }

    #[test]
    fn unparseable_or_missing_expiry_is_none() {
        for case in [
            r#"{"access_token":"at"}"#,
            r#"{"access_token":"at","expiry":"soon"}"#,
            r#"{"access_token":"at","expiry":-5}"#,
            r#"{"access_token":"at","expiry":true}"#,
        ] {
            assert_eq!(parse_keyring_blob(case).unwrap().expires_at, None, "{case}");
        }
    }

    #[test]
    fn fingerprint_is_stable_and_tracks_the_refresh_token() {
        let a = parse_keyring_blob(r#"{"access_token":"at-1","refresh_token":"rt"}"#).unwrap();
        let b = parse_keyring_blob(r#"{"access_token":"at-2","refresh_token":"rt"}"#).unwrap();
        let c = parse_keyring_blob(r#"{"access_token":"at-1","refresh_token":"other"}"#).unwrap();
        assert_eq!(a.fingerprint, b.fingerprint);
        assert_ne!(a.fingerprint, c.fingerprint);
        assert!(a.fingerprint.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(!a.fingerprint.contains("rt"));
    }

    #[test]
    fn fingerprint_falls_back_to_the_access_token() {
        let a = parse_keyring_blob(r#"{"access_token":"at-1"}"#).unwrap();
        let b = parse_keyring_blob(r#"{"access_token":"at-2"}"#).unwrap();
        assert_ne!(a.fingerprint, b.fingerprint);
        assert_eq!(a.fingerprint, crate::cache::fingerprint_of("at-1"));
    }

    #[test]
    fn missing_access_token_is_a_credentials_error() {
        let err = parse_keyring_blob(r#"{"refresh_token":"rt"}"#).unwrap_err();
        match err {
            AppError::Credentials(msg) => assert!(msg.contains("no access token"), "{msg}"),
            other => panic!("expected Credentials, got {other:?}"),
        }
    }

    #[test]
    fn garbage_base64_never_leaks_the_input() {
        let raw = format!("{GO_KEYRING_PREFIX}!!not-base64-SECRETMARKER!!");
        let err = parse_keyring_blob(&raw).unwrap_err();
        assert!(matches!(err, AppError::Credentials(_)));
        assert!(!err.to_string().contains("SECRETMARKER"));
    }

    #[test]
    fn garbage_json_never_leaks_the_input() {
        for raw in [
            "{not json SECRETMARKER".to_string(),
            encode("{still not json SECRETMARKER"),
        ] {
            let err = parse_keyring_blob(&raw).unwrap_err();
            assert!(matches!(err, AppError::Credentials(_)));
            assert!(!err.to_string().contains("SECRETMARKER"));
        }
    }

    #[test]
    fn blob_bytes_decode_utf8_and_utf16le() {
        assert_eq!(
            decode_blob_bytes(
                "{\"a\":\"é\"}
"
                .as_bytes()
            )
            .as_deref(),
            Some("{\"a\":\"é\"}")
        );
        let utf16: Vec<u8> = "{\"a\":\"é\"}"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(decode_blob_bytes(&utf16).as_deref(), Some("{\"a\":\"é\"}"));
        let with_bom: Vec<u8> = [0xff, 0xfe].into_iter().chain(utf16).collect();
        assert_eq!(
            decode_blob_bytes(&with_bom).as_deref(),
            Some("{\"a\":\"é\"}")
        );
        assert_eq!(decode_blob_bytes(b""), None);
        assert_eq!(decode_blob_bytes(&[0xff, 0xfe, 0xfd]), None);
        assert_eq!(decode_blob_bytes(&[0xc3, 0x28]), None);
        assert_eq!(decode_blob_bytes(&vec![b'a'; MAX_BLOB_BYTES + 1]), None);
    }

    #[test]
    fn cli_oauth_token_file_rejects_oversized_and_non_regular_paths() {
        let dir = tempfile::tempdir().unwrap();

        let big = dir.path().join("big");
        std::fs::write(&big, vec![b'a'; MAX_BLOB_BYTES + 1]).unwrap();
        assert_eq!(read_cli_token_file(&big), None, "over the blob cap");

        let not_a_file = dir.path().join("a-directory");
        std::fs::create_dir(&not_a_file).unwrap();
        assert_eq!(read_cli_token_file(&not_a_file), None, "not a regular file");

        assert_eq!(read_cli_token_file(&dir.path().join("missing")), None);
    }

    #[test]
    fn cli_oauth_token_file_is_read_from_the_nested_token_shape() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join(".gemini")
            .join("antigravity-cli")
            .join("antigravity-oauth-token");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"token":{"access_token":"file-at","refresh_token":"file-rt","expiry":"2030-01-02T03:04:05Z"}}"#,
        )
        .unwrap();

        let raw = read_saved_session(None, Some(&path)).expect("CLI OAuth file should be readable");
        let token = parse_keyring_blob(&raw).unwrap();
        assert_eq!(token.access_token, "file-at");
        assert_eq!(token.refresh_token.as_deref(), Some("file-rt"));

        assert_eq!(
            read_saved_session(Some("keyring".into()), Some(&path)).as_deref(),
            Some("keyring")
        );
    }

    /// Touches the real Credential Manager; asserts only that the read does
    /// not error, and never inspects or prints the value.
    #[cfg(windows)]
    #[test]
    #[ignore = "reads the real Windows credential store"]
    fn reading_the_real_windows_credential_never_errors() {
        assert!(read().is_ok());
    }

    /// The keyring lookup is a spawn on the user's PATH (Linux) or an Apple
    /// binary (macOS); either way it must not inherit this process's provider
    /// keys, the rule the Grok Bot lookups already follow.
    #[cfg(not(windows))]
    #[test]
    fn keyring_lookup_scrubs_vendor_secret_env_vars() {
        let command = keyring_lookup_command();
        let expected = if cfg!(target_os = "macos") {
            "/usr/bin/security"
        } else {
            "secret-tool"
        };
        assert_eq!(command.get_program(), expected);
        let removed: Vec<String> = command
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(key, _)| key.to_string_lossy().into_owned())
            .collect();
        for var in crate::vendor::vendor_secret_env_vars_to_remove(&[]) {
            assert!(
                removed.iter().any(|key| key == var),
                "expected {var} to be scrubbed from the keyring lookup"
            );
        }
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn keyring_search_queries_the_same_item_and_scrubs_env() {
        let command = keyring_search_command();
        assert_eq!(command.get_program(), "secret-tool");
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(
            args,
            [
                "search",
                "service",
                KEYRING_SERVICE,
                "username",
                KEYRING_ACCOUNT
            ]
        );
        let removed: Vec<String> = command
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(key, _)| key.to_string_lossy().into_owned())
            .collect();
        for var in crate::vendor::vendor_secret_env_vars_to_remove(&[]) {
            assert!(removed.iter().any(|key| key == var));
        }
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    fn secret_tool_output(code: i32, stdout: &str) -> std::process::Output {
        use std::os::unix::process::ExitStatusExt as _;
        std::process::Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        }
    }

    /// The layout secret-tool 0.21 prints for the item go-keyring writes,
    /// after `lookup` refused it over its `text/plain; charset=utf8` type.
    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn a_lookup_refused_over_content_type_reads_the_search_value() {
        let search = concat!(
            "[/4]\n",
            "label = Password for 'antigravity' on 'gemini'\n",
            "secret = {\"auth_method\":\"oauth\",\"token\":{\"access_token\":\"search-at\",",
            "\"refresh_token\":\"search-rt\",\"expiry\":\"2030-01-02T03:04:05Z\"}}\n",
            "created = 2026-10-14 14:11:14\n",
            "attribute.service = gemini\n",
            "attribute.username = antigravity\n",
        );
        let blob = secret_tool_value(
            || Some(secret_tool_output(1, "")),
            || Some(secret_tool_output(0, search)),
        )
        .unwrap();
        let token = parse_keyring_blob(&blob).unwrap();
        assert_eq!(token.access_token, "search-at");
        assert_eq!(token.refresh_token.as_deref(), Some("search-rt"));
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn a_successful_lookup_is_used_without_a_search() {
        let blob = secret_tool_value(
            || Some(secret_tool_output(0, "{\"access_token\":\"lookup-at\"}\n")),
            || panic!("search must not run after a successful lookup"),
        );
        assert_eq!(blob.as_deref(), Some("{\"access_token\":\"lookup-at\"}"));
    }

    /// No item, a locked item, or no secret-tool at all: still "no session".
    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn no_session_when_neither_lookup_nor_search_yields_a_value() {
        let no_item = secret_tool_value(
            || Some(secret_tool_output(1, "")),
            || Some(secret_tool_output(1, "")),
        );
        let locked = secret_tool_value(
            || Some(secret_tool_output(1, "")),
            || Some(secret_tool_output(0, "[/4]\nlabel = x\n")),
        );
        let no_binary = secret_tool_value(|| None, || panic!("search must not run"));
        assert_eq!((no_item, locked, no_binary), (None, None, None));
    }

    /// A locked item, searched without `--unlock`, has no `secret` line; an
    /// attribute whose value merely contains "secret = " is not one either.
    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn search_output_without_a_secret_line_is_none() {
        let stdout = b"[/4]\nlabel = secret = not this\nattribute.service = gemini\n";
        assert_eq!(secret_from_search_output(stdout), None);
        assert_eq!(secret_from_search_output(b""), None);
    }
}
