//! Alibaba Cloud Model Studio — Token Plan usage over the console gateway the
//! official `bl` CLI uses (`bl auth login --console`). A local-login vendor:
//! the credential is the CLI's own `~/.bailian/config.json` console token
//! (read-only, never written; AK/SK refresh is out of scope).
//!
//! Reports two ratio windows — a 5-hour and a weekly — with epoch-ms reset
//! instants, dispatched per region×site through the gateway matrix in
//! `types.rs`. Cache is scoped by a fingerprint of the access token, so a
//! re-login never inherits the previous session's figures, and the token
//! itself never persists anywhere but the CLI's file.

pub mod creds;
pub mod fetch;
pub mod types;
pub mod vendor;

use std::path::{Path, PathBuf};

use crate::config::ModelStudioConfig;
use crate::error::Result;

/// The `bl` CLI's own config directory, under `$HOME`.
pub const APP_DIR_NAME: &str = ".bailian";
/// The console-login file inside it.
pub const CONFIG_FILE_NAME: &str = "config.json";
/// Environment override for the whole directory (tilde-expanded).
pub const CONFIG_DIR_ENV: &str = "BAILIAN_CONFIG_DIR";

/// The config file path with every input injected — the test seam, so no test
/// resolves a real `$HOME` or branches on the ambient environment. Precedence
/// matches the CLI: an explicit `[modelstudio] config_dir`, then
/// `BAILIAN_CONFIG_DIR`, then `~/.bailian`.
pub fn config_path_in(cfg: &ModelStudioConfig, env_dir: Option<PathBuf>, home: &Path) -> PathBuf {
    cfg.config_dir
        .clone()
        .or(env_dir)
        .unwrap_or_else(|| home.join(APP_DIR_NAME))
        .join(CONFIG_FILE_NAME)
}

/// The config file path against the real environment.
pub fn config_path(cfg: &ModelStudioConfig) -> Result<PathBuf> {
    let env_dir = std::env::var_os(CONFIG_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|p| expand_tilde(&p));
    Ok(config_path_in(cfg, env_dir, &crate::cache::home_dir()?))
}

/// Expand a leading `~` (or `~/`) against the user's home directory, the same
/// rule `[grokbot] secrets_path` gets at config-load time. Anything else —
/// including `~user` — is left untouched.
fn expand_tilde(p: &Path) -> PathBuf {
    let home = crate::cache::home_dir().unwrap_or_else(|_| p.to_path_buf());
    expand_tilde_in(p, &home)
}

/// The pure half of [`expand_tilde`], with the home directory injected.
fn expand_tilde_in(p: &Path, home: &Path) -> PathBuf {
    let Some(s) = p.to_str() else {
        return p.to_path_buf();
    };
    let rest = if s == "~" {
        ""
    } else if let Some(r) = s.strip_prefix("~/") {
        r
    } else {
        return p.to_path_buf();
    };
    if rest.is_empty() {
        home.to_path_buf()
    } else {
        home.join(rest)
    }
}

/// Resolve the console session: the CLI's file, read-only.
pub fn resolve_credentials(cfg: &ModelStudioConfig) -> Result<creds::Credentials> {
    creds::read_from(&config_path(cfg)?)
}

pub use fetch::{FetchOutcome, fetch_snapshot_with};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_path_lives_in_the_bl_cli_dir() {
        let cfg = ModelStudioConfig::default();
        let path = config_path_in(&cfg, None, Path::new("/home/u"));
        assert_eq!(path, PathBuf::from("/home/u/.bailian/config.json"));
    }

    #[test]
    fn a_configured_dir_wins_over_the_env_and_the_default() {
        let cfg = ModelStudioConfig {
            config_dir: Some(PathBuf::from("/cfg/bl")),
            ..ModelStudioConfig::default()
        };
        let path = config_path_in(&cfg, Some(PathBuf::from("/env/bl")), Path::new("/home/u"));
        assert_eq!(path, PathBuf::from("/cfg/bl/config.json"));
    }

    #[test]
    fn the_env_dir_wins_over_the_default() {
        let cfg = ModelStudioConfig::default();
        let path = config_path_in(&cfg, Some(PathBuf::from("/env/bl")), Path::new("/home/u"));
        assert_eq!(path, PathBuf::from("/env/bl/config.json"));
    }

    #[test]
    fn a_tilde_in_the_env_dir_expands() {
        assert_eq!(
            expand_tilde_in(Path::new("~/bl"), Path::new("/home/u")),
            PathBuf::from("/home/u/bl")
        );
        // Bare `~` is the home itself; `~user` stays literal.
        assert_eq!(
            expand_tilde_in(Path::new("~"), Path::new("/home/u")),
            PathBuf::from("/home/u")
        );
        assert_eq!(
            expand_tilde_in(Path::new("~other/config"), Path::new("/home/u")),
            PathBuf::from("~other/config")
        );
    }
}
