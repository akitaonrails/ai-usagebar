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
//!
//! Windows OSCrypt is a different scheme under the same `v10` tag: a random
//! 256-bit key lives in the app's `Local State` JSON (`os_crypt.encrypted_key`,
//! base64 of `"DPAPI"` + a blob only the signed-in Windows user can unprotect),
//! and each value is `"v10"` + a 12-byte nonce + AES-256-GCM ciphertext and
//! 16-byte tag. [`windows_protected_key`] and [`decrypt_windows`] are the pure
//! halves, tested everywhere; only [`windows_key`] calls DPAPI (Windows-gated).
//! Chromium's newer app-bound `v20` values need the browser's own elevation
//! service and are refused with an error that says so.

use aes::cipher::{BlockModeDecrypt, BlockModeEncrypt, KeyIvInit, block_padding::Pkcs7};
use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
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

/// Windows OSCrypt key length: AES-256.
pub const WINDOWS_KEY_LEN: usize = 32;
/// Tag DPAPI-protected keys carry inside `os_crypt.encrypted_key`.
const DPAPI_PREFIX: &[u8] = b"DPAPI";
/// Chromium's app-bound encryption tag (Windows), which this module cannot open.
const APP_BOUND_PREFIX: &[u8] = b"v20";
const GCM_NONCE_LEN: usize = 12;
const GCM_TAG_LEN: usize = 16;

/// The DPAPI-protected key out of a Chromium `Local State` file's contents:
/// `os_crypt.encrypted_key`, base64-decoded, less its `"DPAPI"` tag. Pure — the
/// caller unprotects it. Errors are fixed strings; nothing from the file leaks.
pub fn windows_protected_key(local_state: &[u8]) -> Result<Vec<u8>> {
    let root: serde_json::Value = serde_json::from_slice(local_state)
        .map_err(|_| AppError::Other("Local State is not JSON".into()))?;
    let os_crypt = root
        .get("os_crypt")
        .ok_or_else(|| AppError::Other("Local State has no os_crypt key".into()))?;
    let Some(encoded) = os_crypt
        .get("encrypted_key")
        .and_then(serde_json::Value::as_str)
    else {
        return Err(AppError::Other(
            if os_crypt.get("app_bound_encrypted_key").is_some() {
                "Local State holds only an app-bound (v20) key, which is not supported"
            } else {
                "Local State has no os_crypt.encrypted_key"
            }
            .into(),
        ));
    };
    let raw = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|_| AppError::Other("os_crypt.encrypted_key is not base64".into()))?;
    raw.strip_prefix(DPAPI_PREFIX)
        .filter(|blob| !blob.is_empty())
        .map(<[u8]>::to_vec)
        .ok_or_else(|| AppError::Other("os_crypt.encrypted_key is not DPAPI-protected".into()))
}

/// Decrypt a base64 Windows `v10…` value (`v10` + nonce + AES-256-GCM
/// ciphertext and tag) into its plaintext bytes.
pub fn decrypt_windows(key: &[u8; WINDOWS_KEY_LEN], value_b64: &str) -> Result<Vec<u8>> {
    let raw = base64::engine::general_purpose::STANDARD
        .decode(value_b64.trim())
        .map_err(|e| AppError::Other(format!("safeStorage value is not base64: {e}")))?;
    if raw.starts_with(APP_BOUND_PREFIX) {
        return Err(AppError::Other(
            "safeStorage value uses app-bound (v20) encryption, which is not supported".into(),
        ));
    }
    let Some(body) = raw.strip_prefix(PREFIX) else {
        return Err(AppError::Other(
            "safeStorage value is missing the v10 prefix".into(),
        ));
    };
    if body.len() < GCM_NONCE_LEN + GCM_TAG_LEN {
        return Err(AppError::Other("safeStorage value is truncated".into()));
    }
    let (nonce, sealed) = body.split_at(GCM_NONCE_LEN);
    let nonce = Nonce::try_from(nonce)
        .map_err(|_| AppError::Other("safeStorage value has a malformed nonce".into()))?;
    Aes256Gcm::new(key.into())
        .decrypt(&nonce, sealed)
        .map_err(|_| AppError::Other("safeStorage decrypt failed".into()))
}

/// The Windows OSCrypt key of a Chromium/Electron app, from its `Local State`
/// file: the protected key unwrapped by DPAPI for the signed-in user.
#[cfg(windows)]
pub fn windows_key(local_state: &std::path::Path) -> Result<[u8; WINDOWS_KEY_LEN]> {
    let raw = std::fs::read(local_state).map_err(|e| AppError::io_at(local_state, e))?;
    let key = dpapi_unprotect(&windows_protected_key(&raw)?)?;
    <[u8; WINDOWS_KEY_LEN]>::try_from(key.as_slice())
        .map_err(|_| AppError::Other("the unprotected OSCrypt key is not 32 bytes".into()))
}

/// `CryptUnprotectData` for the current user, with no UI.
#[cfg(windows)]
fn dpapi_unprotect(blob: &[u8]) -> Result<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptUnprotectData,
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(blob.len())
            .map_err(|_| AppError::Other("DPAPI blob is too large".into()))?,
        pbData: blob.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // SAFETY: `input` points at `blob`, which outlives the call and is only read;
    // the optional pointers are null; on success `output` receives a buffer
    // LocalAlloc'ed by the call, copied out and freed below exactly once.
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 || output.pbData.is_null() {
        return Err(AppError::Other(
            "DPAPI could not unprotect the OSCrypt key for this Windows user".into(),
        ));
    }
    // SAFETY: on success `pbData` holds `cbData` initialized bytes owned by us.
    let plain =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) }.to_vec();
    // SAFETY: the buffer came from LocalAlloc inside CryptUnprotectData.
    unsafe { LocalFree(output.pbData.cast()) };
    Ok(plain)
}

/// Seal plaintext as a Windows `v10` value under a fixed nonce — the inverse of
/// [`decrypt_windows`], for tests that seed a store the way the app writes it.
#[cfg(test)]
pub fn encrypt_windows(
    key: &[u8; WINDOWS_KEY_LEN],
    nonce: [u8; GCM_NONCE_LEN],
    plaintext: &[u8],
) -> String {
    let sealed = Aes256Gcm::new(key.into())
        .encrypt(&Nonce::from(nonce), plaintext)
        .expect("AES-GCM sealing a test value");
    let mut out = Vec::with_capacity(PREFIX.len() + GCM_NONCE_LEN + sealed.len());
    out.extend_from_slice(PREFIX);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
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

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    // A fixed, fake 256-bit key — never a real DPAPI unwrap (hermeticity).
    const WINDOWS_KEY: [u8; WINDOWS_KEY_LEN] = [7; WINDOWS_KEY_LEN];

    #[test]
    fn windows_values_decrypt_the_nist_gcm_vector() {
        // NIST GCM spec Test Case 14 (AES-256, zero key, zero IV, one zero
        // block), wrapped the way Chromium stores it: `v10` + nonce + ct + tag.
        // Independent of the crate, so it pins the on-disk format.
        let mut value = b"v10".to_vec();
        value.extend_from_slice(&[0u8; 12]);
        value.extend_from_slice(&hex("cea7403d4d606b6e074ec5d3baf39d18"));
        value.extend_from_slice(&hex("d0d1c8a799996bf0265b98b5d48ab919"));
        assert_eq!(
            decrypt_windows(&[0u8; 32], &b64(&value)).unwrap(),
            [0u8; 16]
        );
    }

    #[test]
    fn windows_values_round_trip() {
        let msg = b"cursor-access-token-value";
        let enc = encrypt_windows(&WINDOWS_KEY, [3; 12], msg);
        assert_eq!(decrypt_windows(&WINDOWS_KEY, &enc).unwrap(), msg);
    }

    #[test]
    fn a_windows_value_fails_under_the_wrong_key_or_when_tampered() {
        let enc = encrypt_windows(&WINDOWS_KEY, [3; 12], b"payload");
        assert!(decrypt_windows(&[8; WINDOWS_KEY_LEN], &enc).is_err());
        let mut raw = base64::engine::general_purpose::STANDARD
            .decode(&enc)
            .unwrap();
        *raw.last_mut().unwrap() ^= 1;
        assert!(decrypt_windows(&WINDOWS_KEY, &b64(&raw)).is_err());
    }

    #[test]
    fn app_bound_and_malformed_windows_values_are_refused() {
        let mut v20 = b"v20".to_vec();
        v20.extend_from_slice(&[0u8; 40]);
        let err = decrypt_windows(&WINDOWS_KEY, &b64(&v20)).unwrap_err();
        assert!(err.to_string().contains("app-bound (v20)"), "{err}");
        assert!(decrypt_windows(&WINDOWS_KEY, &b64(b"x10-no-prefix-at-all")).is_err());
        assert!(decrypt_windows(&WINDOWS_KEY, &b64(b"v10short")).is_err());
        assert!(decrypt_windows(&WINDOWS_KEY, "@@@not base64@@@").is_err());
    }

    #[test]
    fn a_macos_or_linux_value_does_not_open_as_a_windows_one() {
        let cbc = encrypt(&derive_key_linux(b"peanuts"), b"some token value here");
        assert!(decrypt_windows(&WINDOWS_KEY, &cbc).is_err());
    }

    fn local_state(os_crypt: serde_json::Value) -> Vec<u8> {
        serde_json::json!({ "os_crypt": os_crypt, "other": {} })
            .to_string()
            .into_bytes()
    }

    #[test]
    fn the_protected_key_is_the_encrypted_key_less_its_dpapi_tag() {
        let raw = local_state(serde_json::json!({
            "audit_enabled": true,
            "encrypted_key": b64(b"DPAPIprotected-bytes"),
        }));
        assert_eq!(windows_protected_key(&raw).unwrap(), b"protected-bytes");
    }

    #[test]
    fn a_local_state_without_a_usable_dpapi_key_is_an_error() {
        let app_bound = local_state(serde_json::json!({ "app_bound_encrypted_key": "QVBQQg==" }));
        let err = windows_protected_key(&app_bound).unwrap_err();
        assert!(err.to_string().contains("app-bound (v20)"), "{err}");
        for raw in [
            b"not json".to_vec(),
            br#"{"other": {}}"#.to_vec(),
            local_state(serde_json::json!({})),
            local_state(serde_json::json!({ "encrypted_key": "@@@" })),
            local_state(serde_json::json!({ "encrypted_key": b64(b"NOTDPAPIkey") })),
            local_state(serde_json::json!({ "encrypted_key": b64(b"DPAPI") })),
        ] {
            assert!(windows_protected_key(&raw).is_err());
        }
    }

    /// DPAPI itself, on synthetic bytes in a temp dir: protect a fake key for
    /// the current user, write a Local State around it, and read it back.
    #[cfg(windows)]
    #[test]
    fn windows_key_unwraps_a_dpapi_protected_local_state() {
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData,
        };
        let input = CRYPT_INTEGER_BLOB {
            cbData: WINDOWS_KEY_LEN as u32,
            pbData: WINDOWS_KEY.as_ptr().cast_mut(),
        };
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        // SAFETY: `input` points at a live constant; `output` receives a
        // LocalAlloc'ed buffer that is copied and freed below.
        let ok = unsafe {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        assert_ne!(ok, 0, "CryptProtectData failed");
        // SAFETY: on success `pbData` holds `cbData` bytes owned by us.
        let protected =
            unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) }.to_vec();
        // SAFETY: allocated by CryptProtectData with LocalAlloc.
        unsafe { LocalFree(output.pbData.cast()) };

        let mut tagged = DPAPI_PREFIX.to_vec();
        tagged.extend_from_slice(&protected);
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("Local State");
        std::fs::write(
            &path,
            local_state(serde_json::json!({ "encrypted_key": b64(&tagged) })),
        )
        .unwrap();
        assert_eq!(windows_key(&path).unwrap(), WINDOWS_KEY);
    }
}
