//! Electron/Chromium **safeStorage** on macOS — the encryption Claude Desktop
//! uses for the OAuth token blobs in its `config.json` (`oauth:tokenCacheV2`).
//!
//! The scheme is Chromium's `OSCrypt`: a random 128-bit secret lives in the
//! login Keychain (generic-password service `Claude Safe Storage`), a 16-byte
//! AES key is derived from it with PBKDF2-HMAC-SHA1 (salt `saltysalt`, 1003
//! rounds), and each value is `"v10"` followed by AES-128-CBC ciphertext with a
//! fixed all-spaces IV and PKCS7 padding. Both the salt/rounds/IV and the `v10`
//! tag are Chromium constants, identical across every Electron app on macOS.
//!
//! Only [`macos_key`] touches the Keychain (macOS-gated); the derive/decrypt/
//! encrypt transform is pure and platform-independent, so it is exercised by a
//! round-trip test on Linux CI without any real secret (the hermeticity rule).

use aes::cipher::{BlockModeDecrypt, BlockModeEncrypt, KeyIvInit, block_padding::Pkcs7};
use base64::Engine;

use crate::error::{AppError, Result};

/// Chromium OSCrypt constants (macOS). Not secrets — the same values ship in
/// every Chromium build.
const SALT: &[u8] = b"saltysalt";
const ROUNDS: u32 = 1003;
const KEY_LEN: usize = 16;
const IV: [u8; 16] = [b' '; 16];
const PREFIX: &[u8] = b"v10";
/// [`PREFIX`] as it appears at the front of the base64 value: three bytes
/// encode to exactly four characters, so every safeStorage value starts with
/// these and — the point of [`looks_like_value`] — nothing else does.
const PREFIX_B64: &str = "djEw";

/// Login-Keychain generic-password service holding Claude Desktop's secret.
#[cfg(target_os = "macos")]
pub const SERVICE: &str = "Claude Safe Storage";

/// The derived AES key. Sixteen bytes and nothing else; naming it keeps the
/// callers that only pass one around from spelling the width out.
pub type Key = [u8; KEY_LEN];

type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;
type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;

/// Derive the 16-byte AES key from the Keychain secret (PBKDF2-HMAC-SHA1).
pub fn derive_key(secret: &[u8]) -> [u8; KEY_LEN] {
    let mut key = [0u8; KEY_LEN];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(secret, SALT, ROUNDS, &mut key);
    key
}

/// Is this a safeStorage value at all, whoever's key sealed it?
///
/// Key-free and cheap, and it separates *"not this format"* from *"this
/// format, another machine's key"* — a distinction [`decrypt`]'s single `Err`
/// cannot make, and one the restore side needs before it refuses a file.
pub fn looks_like_value(value: &str) -> bool {
    value.trim_start().starts_with(PREFIX_B64)
}

/// Decrypt a base64 `v10…` safeStorage value into its plaintext bytes.
pub fn decrypt(key: &[u8; KEY_LEN], value_b64: &str) -> Result<Vec<u8>> {
    let raw = base64::engine::general_purpose::STANDARD
        .decode(value_b64.trim())
        .map_err(|e| AppError::Other(format!("safeStorage value is not base64: {e}")))?;
    if raw.len() < PREFIX.len() || &raw[..PREFIX.len()] != PREFIX {
        return Err(AppError::Other(
            "safeStorage value is missing the v10 prefix".into(),
        ));
    }
    let ct = &raw[PREFIX.len()..];
    Aes128CbcDec::new(key.into(), &IV.into())
        .decrypt_padded_vec::<Pkcs7>(ct)
        .map_err(|e| AppError::Other(format!("safeStorage decrypt failed: {e}")))
}

/// Encrypt plaintext back into a base64 `v10…` value, byte-compatible with what
/// the app wrote (deterministic: fixed IV, no random salt). Used for the token
/// write-back after a refresh.
pub fn encrypt(key: &[u8; KEY_LEN], plaintext: &[u8]) -> String {
    let ct = Aes128CbcEnc::new(key.into(), &IV.into()).encrypt_padded_vec::<Pkcs7>(plaintext);
    let mut out = Vec::with_capacity(PREFIX.len() + ct.len());
    out.extend_from_slice(PREFIX);
    out.extend_from_slice(&ct);
    base64::engine::general_purpose::STANDARD.encode(out)
}

/// The derived AES key for Claude Desktop, read from the login Keychain.
/// macOS-only; the caller handles the "no key / not macOS" case by skipping the
/// Desktop usage source entirely.
#[cfg(target_os = "macos")]
pub fn macos_key() -> Result<[u8; KEY_LEN]> {
    use std::process::Command;
    let out = Command::new("/usr/bin/security")
        .args(["find-generic-password", "-s", SERVICE, "-w"])
        .output()
        .map_err(|e| AppError::Other(format!("could not run `security`: {e}")))?;
    if !out.status.success() {
        return Err(AppError::Other(format!(
            "no `{SERVICE}` item in the login Keychain (is Claude Desktop installed?)"
        )));
    }
    let secret = String::from_utf8_lossy(&out.stdout);
    Ok(derive_key(secret.trim().as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // A fixed, fake secret — never a real Keychain read (hermeticity).
    fn key() -> [u8; KEY_LEN] {
        derive_key(b"not-a-real-secret")
    }

    #[test]
    fn round_trips_plaintext() {
        let k = key();
        let msg = br#"{"token":"sk-ant-oat01-abc","refreshToken":"sk-ant-ort01-xyz"}"#;
        let enc = encrypt(&k, msg);
        assert!(
            base64::engine::general_purpose::STANDARD
                .decode(&enc)
                .unwrap()
                .starts_with(PREFIX)
        );
        assert_eq!(decrypt(&k, &enc).unwrap(), msg);
    }

    #[test]
    fn key_derivation_matches_the_chromium_compatibility_vector() {
        // Independently reproduced with OpenSSL's PBKDF2-HMAC-SHA1 implementation.
        assert_eq!(
            key(),
            [
                0x9b, 0xa5, 0xa2, 0x8a, 0x32, 0x39, 0xfe, 0xce, 0x3c, 0x5a, 0xe5, 0x70, 0xd6, 0x52,
                0x3d, 0xcc,
            ]
        );
    }

    #[test]
    fn encryption_matches_the_chromium_compatibility_vector() {
        // Fixed IV + no random salt: this OpenSSL-generated vector protects the
        // on-disk format across cryptography-crate upgrades.
        let k = key();
        assert_eq!(encrypt(&k, b"same"), "djEwykc2I53A+doQo9OF96du2A==");
        assert_eq!(encrypt(&k, b"same"), encrypt(&k, b"same"));
    }

    #[test]
    fn the_base64_marker_is_the_v10_prefix_and_cannot_drift_from_it() {
        assert!(
            base64::engine::general_purpose::STANDARD
                .encode(PREFIX)
                .starts_with(PREFIX_B64)
        );
        assert!(looks_like_value(&encrypt(&key(), b"anything")));
        assert!(!looks_like_value(
            &base64::engine::general_purpose::STANDARD.encode(b"not-v10-data")
        ));
        assert!(!looks_like_value(r#"{"token":"plain json"}"#));
    }

    #[test]
    fn rejects_a_value_without_the_v10_prefix() {
        let k = key();
        let no_prefix = base64::engine::general_purpose::STANDARD.encode(b"not-v10-data");
        assert!(decrypt(&k, &no_prefix).is_err());
    }

    #[test]
    fn rejects_non_base64() {
        assert!(decrypt(&key(), "@@@not base64@@@").is_err());
    }

    #[test]
    fn wrong_key_fails_rather_than_returning_garbage() {
        // PKCS7 validation makes a wrong key overwhelmingly likely to error on
        // the padding check instead of silently yielding wrong bytes.
        let enc = encrypt(&key(), b"secret payload here, long enough to pad");
        let other = derive_key(b"different-secret");
        assert!(decrypt(&other, &enc).is_err());
    }
}
