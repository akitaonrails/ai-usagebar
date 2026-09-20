//! Grok Bot — the Grok Bot desktop app's weekly included-usage pool, reported
//! by `aiserver.v1.DashboardService/GetSandUsageStatus` over Connect-RPC.
//! Separate from `[grok]` (Management API prepaid dollars) and `[supergrok]`
//! (the Grok Build subscription).
//!
//! The credential is the app's own OAuth session: `creds.rs` reads
//! `sand-secrets.json` (read-only, never written), whose token fields are
//! Chromium OSCrypt `v10` blobs. On Linux the file is
//! `~/.config/Grok Bot/sand-secrets.json` and the key is one PBKDF2 round
//! (`secret-tool lookup application "Grok Bot"`, `"peanuts"` fallback). On
//! macOS it is `~/Library/Application Support/Grok Bot/sand-secrets.json`
//! and the key is the login Keychain item `Grok Bot Safe Storage` /
//! `Grok Bot Key` (1003 rounds, same scheme as Claude Desktop). `fetch.rs`
//! refreshes the session through Cursor's public OAuth client and persists
//! rotations only in ai-usagebar's own vendor cache.
//!
//! Windows fails closed with a `Credentials` error: the app's DPAPI store
//! has not been captured.

pub mod creds;
pub mod fetch;
pub mod types;
pub mod vendor;

use std::path::{Path, PathBuf};

use crate::config::GrokbotConfig;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
use crate::error::AppError;
use crate::error::Result;

/// The app's XDG config subdirectory name — note the space.
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

/// The credential file path against the real home directory.
pub fn secrets_path(cfg: &GrokbotConfig) -> Result<PathBuf> {
    Ok(secrets_path_in(cfg, &crate::cache::home_dir()?))
}

/// Resolve the desktop app's stored OAuth session.
///
/// Linux and macOS: decrypt `sand-secrets.json` with the platform OSCrypt
/// key. Elsewhere this fails closed with a `Credentials` error that says so,
/// rather than pretending the file was missing.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn resolve_credentials(cfg: &GrokbotConfig) -> Result<creds::GrokbotCredentials> {
    let path = secrets_path(cfg)?;
    creds::read_at(&path, &creds::oscrypt_key()?)
}

/// Windows (and anywhere else) has no supported credential store to read.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn resolve_credentials(_cfg: &GrokbotConfig) -> Result<creds::GrokbotCredentials> {
    Err(AppError::Credentials(
        "Grok Bot usage is supported on Linux and macOS — the desktop app's credential \
         store is not read on this platform"
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

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
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
