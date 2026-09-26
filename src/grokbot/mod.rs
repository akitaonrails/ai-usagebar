//! Grok Bot — the Grok Bot desktop app's weekly included-usage pool, reported
//! by `aiserver.v1.DashboardService/GetSandUsageStatus` over Connect-RPC.
//! Separate from `[grok]` (Management API prepaid dollars) and `[supergrok]`
//! (the Grok Build subscription).
//!
//! The credential is the app's own OAuth session: `creds.rs` reads
//! `sand-secrets.json` (read-only, never written), whose token fields are
//! Chromium OSCrypt `v10` blobs. On Linux the file is
//! `~/.config/Grok Bot/sand-secrets.json`; the key is the Secret Service item
//! `application="Grok Bot"`, Chromium's documented `"peanuts"` default, or both.
//! The app encrypts with `"peanuts"` whenever Electron's selected Linux Secret
//! Service backend is `basic_text`, which it can be on a machine that still
//! holds the item, so both are always tried. On macOS it is
//! `~/Library/Application Support/Grok Bot/sand-secrets.json` and the key is
//! the login Keychain item `Grok Bot Safe Storage` / `Grok Bot Key` (1003
//! rounds, same scheme as Claude Desktop). On Windows it is
//! `%APPDATA%\Grok Bot\sand-secrets.json`, the blobs are AES-256-GCM, and the
//! key is the DPAPI-protected `os_crypt.encrypted_key` in the `Local State`
//! file beside it. `fetch.rs` refreshes the session through Cursor's public
//! OAuth client and persists rotations only in ai-usagebar's own vendor cache.
//!
//! Any other platform fails closed with a `Credentials` error.

pub mod creds;
pub mod fetch;
pub mod types;
pub mod vendor;

use std::path::{Path, PathBuf};

use crate::config::GrokbotConfig;
use crate::error::{AppError, Result};

/// The app's config subdirectory name (XDG, Application Support or
/// `%APPDATA%`) — note the space.
pub const APP_CONFIG_DIR: &str = "Grok Bot";
/// The app's credential file inside that directory.
pub const SECRETS_FILE_NAME: &str = "sand-secrets.json";

/// The credential file path with the home directory injected — the test seam,
/// so no test resolves a real `$HOME`.
pub fn secrets_path_in(cfg: &GrokbotConfig, home: &Path) -> PathBuf {
    cfg.secrets_path.clone().unwrap_or_else(|| {
        if cfg!(target_os = "macos") {
            home.join("Library/Application Support")
                .join(APP_CONFIG_DIR)
                .join(SECRETS_FILE_NAME)
        } else {
            home.join(".config")
                .join(APP_CONFIG_DIR)
                .join(SECRETS_FILE_NAME)
        }
    })
}

/// The Windows credential file path with `%APPDATA%` (Roaming) injected — the
/// test seam, so no test resolves the real one.
pub fn windows_secrets_path_in(cfg: &GrokbotConfig, app_data: &Path) -> PathBuf {
    cfg.secrets_path
        .clone()
        .unwrap_or_else(|| app_data.join(APP_CONFIG_DIR).join(SECRETS_FILE_NAME))
}

/// The credential file path against the real home directory (`%APPDATA%` on
/// Windows).
pub fn secrets_path(cfg: &GrokbotConfig) -> Result<PathBuf> {
    if cfg!(windows) {
        let base = directories::BaseDirs::new().ok_or_else(|| {
            AppError::Other("could not resolve the platform config directory".into())
        })?;
        return Ok(windows_secrets_path_in(cfg, base.config_dir()));
    }
    Ok(secrets_path_in(cfg, &crate::cache::home_dir()?))
}

/// Resolve the desktop app's stored OAuth session.
///
/// Linux and macOS: decrypt `sand-secrets.json` with the platform OSCrypt
/// key. Windows: the same file, keyed by the DPAPI-protected key in the
/// `Local State` beside it. Elsewhere this fails closed with a `Credentials`
/// error that says so, rather than pretending the file was missing.
#[cfg(target_os = "linux")]
pub fn resolve_credentials(cfg: &GrokbotConfig) -> Result<creds::GrokbotCredentials> {
    let path = secrets_path(cfg)?;
    // Both the Secret Service secret and Chromium's `"peanuts"` default: the
    // app writes with `"peanuts"` whenever its Linux Secret Service backend is
    // `basic_text`, which it can be on a machine that still holds an
    // `application=Grok Bot` item. See `creds::oscrypt_keys`.
    creds::read_at_any(&path, &creds::oscrypt_keys())
}

#[cfg(target_os = "macos")]
pub fn resolve_credentials(cfg: &GrokbotConfig) -> Result<creds::GrokbotCredentials> {
    let path = secrets_path(cfg)?;
    creds::read_at(&path, &creds::oscrypt_key()?)
}

#[cfg(windows)]
pub fn resolve_credentials(cfg: &GrokbotConfig) -> Result<creds::GrokbotCredentials> {
    let path = secrets_path(cfg)?;
    // A missing credential file is the fix to report, ahead of its key.
    if !path.is_file() {
        return Err(creds::missing_file_error(&path));
    }
    creds::read_at(&path, &creds::windows_oscrypt_key(&path)?)
}

/// Anywhere else there is no supported credential store to read.
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn resolve_credentials(_cfg: &GrokbotConfig) -> Result<creds::GrokbotCredentials> {
    Err(AppError::Credentials(
        "Grok Bot usage is supported on Linux, macOS and Windows — the desktop app's \
         credential store is not read on this platform"
            .into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn the_default_path_lives_under_the_apps_xdg_config_dir() {
        let cfg = GrokbotConfig::default();
        let path = secrets_path_in(&cfg, Path::new("/home/u"));
        assert_eq!(
            path,
            PathBuf::from("/home/u/.config/Grok Bot/sand-secrets.json")
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn the_default_path_lives_under_application_support() {
        let cfg = GrokbotConfig::default();
        let path = secrets_path_in(&cfg, Path::new("/Users/u"));
        assert_eq!(
            path,
            PathBuf::from("/Users/u/Library/Application Support/Grok Bot/sand-secrets.json")
        );
    }

    #[test]
    fn a_configured_secrets_path_wins() {
        let cfg = GrokbotConfig {
            enabled: true,
            secrets_path: Some(PathBuf::from("/elsewhere/secrets.json")),
        };
        assert_eq!(
            secrets_path_in(&cfg, Path::new("/home/u")),
            PathBuf::from("/elsewhere/secrets.json")
        );
    }

    #[test]
    fn the_windows_default_path_lives_under_appdata() {
        let cfg = GrokbotConfig::default();
        assert_eq!(
            windows_secrets_path_in(&cfg, Path::new("C:/Users/u/AppData/Roaming")),
            Path::new("C:/Users/u/AppData/Roaming")
                .join("Grok Bot")
                .join("sand-secrets.json")
        );
        let configured = GrokbotConfig {
            enabled: true,
            secrets_path: Some(PathBuf::from("D:/elsewhere/secrets.json")),
        };
        assert_eq!(
            windows_secrets_path_in(&configured, Path::new("C:/ignored")),
            PathBuf::from("D:/elsewhere/secrets.json")
        );
    }

    /// Windows reports a missing credential file as such, not as a missing key.
    #[cfg(windows)]
    #[test]
    fn a_missing_windows_credential_file_names_the_file() {
        let td = tempfile::TempDir::new().unwrap();
        let cfg = GrokbotConfig {
            enabled: true,
            secrets_path: Some(td.path().join("sand-secrets.json")),
        };
        let err = resolve_credentials(&cfg).unwrap_err();
        assert!(matches!(err, AppError::Credentials(_)), "{err:?}");
        assert!(err.to_string().contains("sand-secrets.json"), "{err}");
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    #[test]
    fn unsupported_platforms_fail_closed_with_a_credentials_error() {
        let err = resolve_credentials(&GrokbotConfig::default()).unwrap_err();
        assert!(matches!(err, AppError::Credentials(_)), "{err:?}");
        assert!(
            err.to_string().contains("not read on this platform"),
            "{err}"
        );
    }
}
