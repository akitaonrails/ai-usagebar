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
//! The same transform also serves Chromium OSCrypt stores on Linux (the Grok
//! Bot desktop app's `sand-secrets.json`), where the only difference is the
//! PBKDF2 round count — hence [`derive_key_with_rounds`] / [`derive_key_linux`].

use aes::cipher::{BlockModeDecrypt, BlockModeEncrypt, KeyIvInit, block_padding::Pkcs7};
use base64::Engine;

use crate::error::{AppError, Result};

/// Chromium OSCrypt constants (macOS). Not secrets — the same values ship in
/// every Chromium build.
const SALT: &[u8] = b"saltysalt";
const ROUNDS: u32 = 1003;
/// Chromium's Linux OSCrypt derivation uses a single PBKDF2 round (macOS uses
/// [`ROUNDS`]). Same salt, same key length, same `v10` envelope.
pub const ROUNDS_LINUX: u32 = 1;
const KEY_LEN: usize = 16;
const IV: [u8; 16] = [b' '; 16];
const PREFIX: &[u8] = b"v10";

/// Login-Keychain generic-password service holding Claude Desktop's secret.
#[cfg(target_os = "macos")]
pub const SERVICE: &str = "Claude Safe Storage";

type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;
type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;

/// Derive the 16-byte AES key from the Keychain secret (PBKDF2-HMAC-SHA1,
/// macOS's 1003 rounds). Byte-identical to what it has always been; the
/// compatibility-vector test below pins that.
pub fn derive_key(secret: &[u8]) -> [u8; KEY_LEN] {
    derive_key_with_rounds(secret, ROUNDS)
}

/// Derive the 16-byte AES key with an explicit PBKDF2 round count. The round
/// count is the only platform difference in Chromium's OSCrypt: Linux uses
/// [`ROUNDS_LINUX`].
pub fn derive_key_with_rounds(secret: &[u8], rounds: u32) -> [u8; KEY_LEN] {
    let mut key = [0u8; KEY_LEN];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(secret, SALT, rounds, &mut key);
    key
}

/// The Linux OSCrypt key — Chromium's documented one-round derivation. The
/// `secret` comes from the app's Secret Service item (or the documented
/// `"peanuts"` fallback when no secret is stored).
pub fn derive_key_linux(secret: &[u8]) -> [u8; KEY_LEN] {
    derive_key_with_rounds(secret, ROUNDS_LINUX)
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

    #[test]
    fn linux_derivation_matches_the_chromium_compatibility_vector() {
        // PBKDF2-HMAC-SHA1("peanuts", "saltysalt", 1 round, 16 bytes),
        // reproduced independently with OpenSSL's PBKDF2 implementation —
        // this is the key every Chromium-derivative app on Linux uses when no
        // Secret Service item overrides it.
        assert_eq!(
            derive_key_linux(b"peanuts"),
            [
                0xfd, 0x62, 0x1f, 0xe5, 0xa2, 0xb4, 0x02, 0x53, 0x9d, 0xfa, 0x14, 0x7c, 0xa9, 0x27,
                0x27, 0x78,
            ]
        );
        // The explicit-rounds form is the same call.
        assert_eq!(
            derive_key_with_rounds(b"peanuts", ROUNDS_LINUX),
            derive_key_linux(b"peanuts")
        );
    }

    #[test]
    fn linux_key_round_trips_through_the_same_envelope() {
        let k = derive_key_linux(b"peanuts");
        let msg = b"cursor-access-token-value";
        let enc = encrypt(&k, msg);
        assert_eq!(decrypt(&k, &enc).unwrap(), msg);
        // A macOS-derived key must not open a Linux blob, and vice versa.
        assert!(decrypt(&derive_key(b"peanuts"), &enc).is_err());
    }
}
