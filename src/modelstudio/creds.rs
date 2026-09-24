//! Read the official `bl` CLI's own console-login file — read-only, never
//! written. `~/.bailian/config.json` (flat snake_case JSON) holds the console
//! Bearer token the CLI stored after `bl auth login --console`:
//!
//! ```json
//! {"access_token": "ey…", "console_site": "domestic",
//!  "console_region": "cn-beijing"}
//! ```
//!
//! A camelCase `accessToken` alias is accepted — the CLI has spelled it both
//! ways. AK/SK refresh is out of scope: the token is only ever read.
//!
//! Error messages are fixed strings: every input here is a credential and
//! must never end up in a tooltip or a log line.

use std::fmt::Write as _;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::{AppError, Result};

use super::types::{ConsoleRegion, ConsoleSite};

/// The console session, as the CLI stored it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    /// Bearer console token. Sent in one outgoing header, never persisted.
    pub access_token: String,
    pub site: ConsoleSite,
    pub region: ConsoleRegion,
    /// First 16 hex chars of the SHA-256 of the access token. Scopes the
    /// vendor cache to this login, so a re-login never gets the previous
    /// session's figures — the kiro/minimax/grokbot treatment.
    pub fingerprint: String,
}

pub fn fingerprint_of(secret: &str) -> String {
    let digest = Sha256::digest(secret.as_bytes());
    let mut hex = String::with_capacity(16);
    for byte in digest.iter().take(8) {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

#[derive(serde::Deserialize)]
struct ConfigFile {
    #[serde(default, alias = "accessToken")]
    access_token: Option<String>,
    #[serde(default)]
    console_site: Option<String>,
    #[serde(default)]
    console_region: Option<String>,
}

/// Read and parse the CLI's config file at `path`. A missing file, a missing
/// token, and an unparseable file are all Credentials errors naming the CLI's
/// own fix — there is nothing else the user can do.
pub fn read_from(path: &Path) -> Result<Credentials> {
    let raw = std::fs::read(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            AppError::Credentials(format!(
                "Model Studio: no bl CLI config at {}; install the official `bl` CLI and \
                 run `bl auth login --console` to re-auth",
                crate::display::sanitize_untrusted_path(path)
            ))
        } else {
            AppError::io_at(path, e)
        }
    })?;
    parse(&raw)
}

fn parse(raw: &[u8]) -> Result<Credentials> {
    let file: ConfigFile = serde_json::from_slice(raw).map_err(|_| malformed())?;
    let access_token = file
        .access_token
        .filter(|token| !token.trim().is_empty())
        .ok_or_else(malformed)?;
    Ok(Credentials {
        fingerprint: fingerprint_of(&access_token),
        site: ConsoleSite::parse(file.console_site.as_deref().unwrap_or("domestic")),
        region: ConsoleRegion::parse(file.console_region.as_deref().unwrap_or("cn-beijing")),
        access_token,
    })
}

fn malformed() -> AppError {
    AppError::Credentials(
        "Model Studio: the bl CLI's config.json carries no console token; \
         run `bl auth login --console` to re-auth"
            .into(),
    )
}

/// The detection probe: a present, non-empty config file. Cheap enough to run
/// at every frontend start — no parse, no network.
pub fn config_present_at(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.len() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_config(dir: &TempDir, contents: &str) -> std::path::PathBuf {
        let path = dir.path().join("config.json");
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn a_snake_case_config_yields_the_console_session() {
        let td = TempDir::new().unwrap();
        let path = write_config(
            &td,
            r#"{"access_token":"tok-1","console_site":"international",
                "console_region":"ap-southeast-1"}"#,
        );
        let creds = read_from(&path).unwrap();
        assert_eq!(creds.access_token, "tok-1");
        assert_eq!(creds.site, ConsoleSite::International);
        assert_eq!(creds.region, ConsoleRegion::ApSoutheast1);
        assert_eq!(creds.fingerprint, fingerprint_of("tok-1"));
        assert_eq!(creds.fingerprint.len(), 16);
        assert!(creds.fingerprint.chars().all(|c| c.is_ascii_hexdigit()));
    }

    /// The CLI has spelled the token field both ways; either must work.
    #[test]
    fn a_camel_case_access_token_alias_parses() {
        let td = TempDir::new().unwrap();
        let path = write_config(&td, r#"{"accessToken":"tok-2"}"#);
        let creds = read_from(&path).unwrap();
        assert_eq!(creds.access_token, "tok-2");
    }

    #[test]
    fn site_and_region_default_to_the_cli_rows_and_unknown_values_fall_back() {
        let td = TempDir::new().unwrap();
        let path = write_config(&td, r#"{"access_token":"tok-3"}"#);
        let creds = read_from(&path).unwrap();
        assert_eq!(creds.site, ConsoleSite::Domestic);
        assert_eq!(creds.region, ConsoleRegion::CnBeijing);

        let path = write_config(
            &td,
            r#"{"access_token":"tok-3","console_site":"mars","console_region":"eu-west-1"}"#,
        );
        let creds = read_from(&path).unwrap();
        assert_eq!(creds.site, ConsoleSite::Domestic);
        assert_eq!(creds.region, ConsoleRegion::CnBeijing);
    }

    /// A missing token is the re-auth error, naming the CLI's own command.
    #[test]
    fn a_missing_token_names_the_fix() {
        let td = TempDir::new().unwrap();
        for (name, contents) in [
            ("no-token", r#"{"console_site":"domestic"}"#),
            ("empty-token", r#"{"access_token":"  "}"#),
            ("not-json", "nope"),
            ("wrong-shape", r#""just a string""#),
        ] {
            let path = write_config(&td, contents);
            let err = read_from(&path).unwrap_err();
            assert!(matches!(err, AppError::Credentials(_)), "{name}: {err:?}");
            assert!(
                err.to_string().contains("bl auth login --console"),
                "{name}: {err}"
            );
            // Fixed strings only: nothing from the file leaks into the error.
            assert!(!err.to_string().contains("console_site"), "{name}: {err}");
        }
    }

    #[test]
    fn a_missing_file_is_a_credentials_error_naming_the_path_checked() {
        let td = TempDir::new().unwrap();
        let missing = td.path().join("absent").join("config.json");
        let err = read_from(&missing).unwrap_err();
        assert!(matches!(err, AppError::Credentials(_)), "{err:?}");
        let message = err.to_string();
        assert!(message.contains("bl auth login --console"), "{message}");
        assert!(message.contains("config.json"), "{message}");
    }

    #[test]
    fn the_probe_wants_a_non_empty_file() {
        let td = TempDir::new().unwrap();
        let path = td.path().join("config.json");
        assert!(!config_present_at(&path));
        std::fs::write(&path, "").unwrap();
        assert!(!config_present_at(&path));
        std::fs::write(&path, "{}").unwrap();
        assert!(config_present_at(&path));
        assert!(!config_present_at(td.path()));
    }
}
