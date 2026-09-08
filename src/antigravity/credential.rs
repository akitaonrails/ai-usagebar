//! Antigravity's saved Google session, read from the OS keyring.
//!
//! Every Antigravity product (2.0, the IDE, `agy`) stores the OAuth token it
//! obtained at sign-in through Go's `go-keyring`, under service `gemini` and
//! account `antigravity`. On Windows that is a generic credential named
//! `gemini:antigravity` in the Credential Manager, on macOS a generic
//! password in the login Keychain, and on Linux a Secret Service item. The
//! value is JSON — sometimes wrapped as `go-keyring-base64:<base64>` on
//! backends that cannot hold raw text.
//!
//! This module only *reads* the blob and pulls out the tokens; the network
//! side (refresh, quota) lives in `cloud.rs`. The keyring is never written.
//!
//! [`read`] is deliberately forgiving: any keyring hiccup — no backend, a
//! locked store, a denied ACL — is `Ok(None)`, because the caller has a better
//! message for "no session" than anything the backend produces, and the vendor
//! must keep working when the keyring is simply unavailable.

use std::fmt::Write as _;

use base64::Engine as _;
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};

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
    let fingerprint = fingerprint_of(refresh_token.as_deref().unwrap_or(&access_token));

    Ok(StoredToken {
        access_token,
        refresh_token,
        expires_at,
        fingerprint,
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

fn fingerprint_of(secret: &str) -> String {
    let digest = Sha256::digest(secret.as_bytes());
    let mut hex = String::with_capacity(16);
    for byte in digest.iter().take(8) {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// The raw blob from the OS keyring, or `None` when there is no saved session
/// or the keyring could not be consulted. Never errors on a backend failure —
/// see the module docs.
pub fn read() -> Result<Option<String>> {
    Ok(read_platform())
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
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
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
    let out = std::process::Command::new("/usr/bin/security")
        .args([
            "find-generic-password",
            "-s",
            KEYRING_SERVICE,
            "-a",
            KEYRING_ACCOUNT,
            "-w",
        ])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    decode_blob_bytes(&out.stdout)
}

#[cfg(not(any(windows, target_os = "macos")))]
fn read_platform() -> Option<String> {
    // `secret-tool` (libsecret) speaks to whichever Secret Service is running.
    // A missing binary or no running daemon both exit non-zero / fail to
    // spawn, and both mean "no session available here".
    let out = std::process::Command::new("secret-tool")
        .args([
            "lookup",
            "service",
            KEYRING_SERVICE,
            "username",
            KEYRING_ACCOUNT,
        ])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    decode_blob_bytes(&out.stdout)
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
        assert_eq!(a.fingerprint, fingerprint_of("at-1"));
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

    /// Touches the real Credential Manager; asserts only that the read does
    /// not error, and never inspects or prints the value.
    #[cfg(windows)]
    #[test]
    #[ignore = "reads the real Windows credential store"]
    fn reading_the_real_windows_credential_never_errors() {
        assert!(read().is_ok());
    }
}
