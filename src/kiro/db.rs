//! Read kiro-cli's own local SQLite state — `~/.local/share/kiro-cli/data.sqlite3`
//! — for the AWS SSO OIDC or social-login session and CodeWhisperer profile ARN
//! it already obtained via `kiro-cli login`. Opened read-only and never written
//! back to: it's kiro-cli's own live WAL-mode database (the `-wal`/`-shm`
//! sidecar files sit next to it), the same "not ours to lock for writing"
//! treatment `cursor::db` gives Cursor's `state.vscdb`.
//!
//! Two tables matter:
//! - `auth_kv` — `kirocli:odic:token` + `kirocli:odic:device-registration`
//!   (AWS Builder ID / IAM Identity Center OAuth token and registered client
//!   credentials) and/or `kirocli:social:token` (Google / GitHub social-login
//!   token with its inline `profile_arn`; refreshed through Kiro's desktop auth
//!   service — see `oauth.rs`). Because `kiro-cli` stores both under separate
//!   keys without an active-method column and can leave a previous login row
//!   behind when switching sign-in methods (`aws/amazon-q-developer-cli#3695`),
//!   [`read_credentials`] evaluates both when present and picks the valid
//!   session with the strictly newer `expires_at` timestamp, failing closed if
//!   both are valid with identical timestamps.
//! - `state` — `api.codewhisperer.profile`, the resolved CodeWhisperer profile
//!   ARN. Empty for AWS Builder ID accounts with no IdC profile (the API
//!   accepts an empty `profileArn`); populated with the shared
//!   `Social_Default_Profile` ARN for social logins.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;
use sha1::{Digest, Sha1};

use crate::error::{AppError, Result};

const TOKEN_KEY: &str = "kirocli:odic:token";
const SOCIAL_TOKEN_KEY: &str = "kirocli:social:token";
const DEVICE_REGISTRATION_KEY: &str = "kirocli:odic:device-registration";
const PROFILE_STATE_KEY: &str = "api.codewhisperer.profile";
const LOGIN_HINT: &str = "kiro-cli login";

/// Default location of kiro-cli's local database. Verified on Linux
/// (`~/.local/share/kiro-cli/data.sqlite3`, i.e. `directories::BaseDirs::data_dir()`
/// joined with `kiro-cli/data.sqlite3`). macOS/Windows follow the same
/// `directories` convention every other local-file vendor here uses, but are
/// unverified against a real kiro-cli install on those platforms.
pub fn default_db_path() -> Result<PathBuf> {
    let base = directories::BaseDirs::new().ok_or_else(|| {
        AppError::Other("could not resolve the platform data directory (no HOME?)".into())
    })?;
    Ok(base.data_dir().join("kiro-cli").join("data.sqlite3"))
}

#[derive(Debug, Deserialize)]
struct TokenRow {
    access_token: String,
    refresh_token: String,
    expires_at: String,
    region: String,
}

#[derive(Debug, Deserialize)]
struct SocialTokenRow {
    access_token: String,
    refresh_token: String,
    expires_at: String,
    #[serde(default)]
    profile_arn: String,
}

#[derive(Debug, Deserialize)]
struct DeviceRegistrationRow {
    client_id: String,
    client_secret: String,
}

const SOCIAL_DEFAULT_PROFILE_NAME: &str = "Social_Default_Profile";

#[derive(Debug, Default, Deserialize)]
struct ProfileRow {
    #[serde(default)]
    arn: String,
    #[serde(default)]
    profile_name: String,
}

/// Which Kiro CLI sign-in flow produced the cached token, and the material
/// needed to refresh it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KiroAuthKind {
    /// AWS Builder ID or IAM Identity Center (`kirocli:odic:token`), refreshed
    /// against regional AWS SSO OIDC with the registered device credentials.
    Oidc {
        client_id: String,
        client_secret: String,
    },
    /// Google or GitHub sign-in (`kirocli:social:token`), refreshed against
    /// Kiro's desktop auth service with the refresh token alone.
    Social,
}

/// Everything a `GetUsageLimits` call needs, read out of kiro-cli's own local
/// state in one pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KiroCredentials {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
    pub region: String,
    pub auth_kind: KiroAuthKind,
    /// CodeWhisperer profile ARN; empty for accounts with no IdC profile.
    pub profile_arn: String,
    /// Non-plaintext cache identity for the signed-in account. Never
    /// displayed — exists so a cache written for one account is not served
    /// for another after a `kiro-cli login` switch.
    pub account_key: String,
}

pub fn read_credentials(path: &Path) -> Result<KiroCredentials> {
    if !path.exists() {
        return Err(AppError::Credentials(format!(
            "Kiro CLI database not found at {}. Run `{LOGIN_HINT}`, then try again.",
            path.display()
        )));
    }
    let conn =
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| {
            AppError::Credentials(format!(
                "could not open Kiro CLI database at {}: {e}",
                path.display()
            ))
        })?;

    let state_profile = read_optional_state::<ProfileRow>(&conn, PROFILE_STATE_KEY)?;
    let oidc_profile_arn = state_profile
        .as_ref()
        .filter(|p| p.profile_name != SOCIAL_DEFAULT_PROFILE_NAME)
        .map(|p| p.arn.clone())
        .unwrap_or_default();
    let social_state_profile_arn = state_profile.map(|p| p.arn).unwrap_or_default();

    let oidc = read_oidc_credentials(&conn, &oidc_profile_arn)?;
    let social = read_social_credentials(&conn, &social_state_profile_arn)?;

    match (oidc, social) {
        (Some(Ok(oidc)), None) | (Some(Ok(oidc)), Some(Err(_))) => Ok(oidc),
        (None, Some(Ok(social))) | (Some(Err(_)), Some(Ok(social))) => Ok(social),
        (Some(Ok(oidc)), Some(Ok(social))) => {
            if oidc.expires_at > social.expires_at {
                Ok(oidc)
            } else if social.expires_at > oidc.expires_at {
                Ok(social)
            } else {
                Err(AppError::Credentials(format!(
                    "Kiro CLI database has both `{TOKEN_KEY}` and `{SOCIAL_TOKEN_KEY}` with identical expiry; cannot determine active session. Run `kiro-cli logout` and `{LOGIN_HINT}` again."
                )))
            }
        }
        (Some(Err(err)), None) | (Some(Err(err)), Some(Err(_))) => Err(err),
        (None, Some(Err(err))) => Err(err),
        (None, None) => Err(AppError::Credentials(format!(
            "no Kiro CLI `{TOKEN_KEY}` or `{SOCIAL_TOKEN_KEY}` entry found. Run `{LOGIN_HINT}`, then try again."
        ))),
    }
}

fn read_oidc_credentials(
    conn: &Connection,
    state_profile_arn: &str,
) -> Result<Option<Result<KiroCredentials>>> {
    let Some(raw) = read_optional_raw_auth_kv(conn, TOKEN_KEY)? else {
        return Ok(None);
    };
    Ok(Some((|| {
        let token: TokenRow = serde_json::from_str(&raw).map_err(|e| {
            AppError::Credentials(format!("Kiro CLI `{TOKEN_KEY}` entry is malformed: {e}"))
        })?;
        let device: DeviceRegistrationRow = read_auth_kv(conn, DEVICE_REGISTRATION_KEY)?;
        require_nonempty(&[
            ("access token", token.access_token.as_str()),
            ("refresh token", token.refresh_token.as_str()),
            ("client id", device.client_id.as_str()),
            ("client secret", device.client_secret.as_str()),
        ])?;
        super::oauth::validate_region(&token.region)?;
        let expires_at = parse_expires_at(&token.expires_at)?;
        let auth_kind = KiroAuthKind::Oidc {
            client_id: device.client_id,
            client_secret: device.client_secret,
        };
        let account_key = account_key(
            &auth_kind,
            state_profile_arn,
            &token.region,
            &token.refresh_token,
        );

        Ok(KiroCredentials {
            access_token: token.access_token,
            refresh_token: token.refresh_token,
            expires_at,
            region: token.region,
            auth_kind,
            profile_arn: state_profile_arn.to_string(),
            account_key,
        })
    })()))
}

fn read_social_credentials(
    conn: &Connection,
    state_profile_arn: &str,
) -> Result<Option<Result<KiroCredentials>>> {
    let Some(raw) = read_optional_raw_auth_kv(conn, SOCIAL_TOKEN_KEY)? else {
        return Ok(None);
    };
    Ok(Some((|| {
        let social: SocialTokenRow = serde_json::from_str(&raw).map_err(|e| {
            AppError::Credentials(format!(
                "Kiro CLI `{SOCIAL_TOKEN_KEY}` entry is malformed: {e}"
            ))
        })?;
        let profile_arn = if !social.profile_arn.trim().is_empty() {
            social.profile_arn
        } else {
            state_profile_arn.to_string()
        };
        require_nonempty(&[
            ("access token", social.access_token.as_str()),
            ("refresh token", social.refresh_token.as_str()),
            ("profile ARN", profile_arn.as_str()),
        ])?;
        let region = region_from_profile_arn(&profile_arn)?;
        let expires_at = parse_expires_at(&social.expires_at)?;
        let auth_kind = KiroAuthKind::Social;
        let account_key = account_key(&auth_kind, &profile_arn, &region, &social.refresh_token);

        Ok(KiroCredentials {
            access_token: social.access_token,
            refresh_token: social.refresh_token,
            expires_at,
            region,
            auth_kind,
            profile_arn,
            account_key,
        })
    })()))
}

fn require_nonempty(fields: &[(&str, &str)]) -> Result<()> {
    for (label, value) in fields {
        if value.trim().is_empty() {
            return Err(AppError::Credentials(format!(
                "Kiro CLI {label} is empty. Run `{LOGIN_HINT}` again."
            )));
        }
    }
    Ok(())
}

fn parse_expires_at(raw: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| {
            AppError::Credentials(format!(
                "Kiro CLI token has an unreadable expiry ({raw:?}): {e}. Run `{LOGIN_HINT}` again."
            ))
        })
}

pub(crate) fn region_from_profile_arn(arn: &str) -> Result<String> {
    let parts: Vec<&str> = arn.splitn(6, ':').collect();
    if parts.len() < 6 || parts[0] != "arn" || parts[1].is_empty() || parts[2].is_empty() {
        return Err(AppError::Credentials(format!(
            "Kiro CLI social token contains an invalid profile ARN. Run `{LOGIN_HINT}` again."
        )));
    }
    let region = parts[3];
    super::oauth::validate_region(region)?;
    Ok(region.to_string())
}

fn read_auth_kv<T: for<'de> Deserialize<'de>>(conn: &Connection, key: &str) -> Result<T> {
    read_optional_auth_kv(conn, key)?.ok_or_else(|| {
        AppError::Credentials(format!(
            "no Kiro CLI `{key}` entry found. Run `{LOGIN_HINT}`, then try again."
        ))
    })
}

fn read_optional_auth_kv<T: for<'de> Deserialize<'de>>(
    conn: &Connection,
    key: &str,
) -> Result<Option<T>> {
    read_optional_row(conn, "SELECT value FROM auth_kv WHERE key = ?1", key)
}

fn read_optional_raw_auth_kv(conn: &Connection, key: &str) -> Result<Option<String>> {
    match conn.query_row("SELECT value FROM auth_kv WHERE key = ?1", [key], |row| {
        row.get(0)
    }) {
        Ok(raw) => Ok(Some(raw)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(AppError::Credentials(format!(
            "could not read Kiro CLI `{key}` entry: {e}"
        ))),
    }
}

fn read_optional_state<T: for<'de> Deserialize<'de>>(
    conn: &Connection,
    key: &str,
) -> Result<Option<T>> {
    read_optional_row(conn, "SELECT value FROM state WHERE key = ?1", key)
}

fn read_optional_row<T: for<'de> Deserialize<'de>>(
    conn: &Connection,
    sql: &str,
    key: &str,
) -> Result<Option<T>> {
    let raw: String = match conn.query_row(sql, [key], |row| row.get(0)) {
        Ok(raw) => raw,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(e) => {
            return Err(AppError::Credentials(format!(
                "could not read Kiro CLI `{key}` entry: {e}"
            )));
        }
    };
    serde_json::from_str(&raw)
        .map(Some)
        .map_err(|e| AppError::Credentials(format!("Kiro CLI `{key}` entry is malformed: {e}")))
}

fn account_key(
    auth_kind: &KiroAuthKind,
    profile_arn: &str,
    region: &str,
    refresh_token: &str,
) -> String {
    let mut digest = Sha1::new();
    match auth_kind {
        KiroAuthKind::Oidc { .. } if profile_arn.is_empty() => {
            // Builder ID accounts have no profile ARN. The refresh token is
            // the only account-specific value available locally; fingerprint
            // it so two accounts in the same region cannot share cache state.
            digest.update(b"builder-id\0");
            digest.update(region.as_bytes());
            digest.update(b"\0");
            digest.update(refresh_token.as_bytes());
        }
        KiroAuthKind::Oidc { .. } => {
            digest.update(b"profile\0");
            digest.update(profile_arn.as_bytes());
        }
        KiroAuthKind::Social => {
            // Social logins share kiro-cli's multi-tenant `Social_Default_Profile`
            // ARN across all Google/GitHub users, so include the database's
            // refresh token to keep each signed-in account isolated.
            digest.update(b"social\0");
            digest.update(region.as_bytes());
            digest.update(b"\0");
            digest.update(profile_arn.as_bytes());
            digest.update(b"\0");
            digest.update(refresh_token.as_bytes());
        }
    }
    let digest = digest.finalize();
    let mut encoded = String::with_capacity(digest.len() * 2);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in digest {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn seed_db(
        path: &Path,
        token: Option<&str>,
        device: Option<&str>,
        profile: Option<&str>,
    ) -> Connection {
        let conn = Connection::open(path).unwrap();
        conn.execute("CREATE TABLE auth_kv (key TEXT, value TEXT)", [])
            .unwrap();
        conn.execute("CREATE TABLE state (key TEXT, value TEXT)", [])
            .unwrap();
        if let Some(t) = token {
            conn.execute(
                "INSERT INTO auth_kv (key, value) VALUES (?1, ?2)",
                rusqlite::params![TOKEN_KEY, t],
            )
            .unwrap();
        }
        if let Some(d) = device {
            conn.execute(
                "INSERT INTO auth_kv (key, value) VALUES (?1, ?2)",
                rusqlite::params![DEVICE_REGISTRATION_KEY, d],
            )
            .unwrap();
        }
        if let Some(p) = profile {
            conn.execute(
                "INSERT INTO state (key, value) VALUES (?1, ?2)",
                rusqlite::params![PROFILE_STATE_KEY, p],
            )
            .unwrap();
        }
        conn
    }

    fn token_json() -> String {
        serde_json::json!({
            "access_token": "AT",
            "refresh_token": "RT",
            "expires_at": "2030-01-01T00:00:00Z",
            "region": "us-east-1",
        })
        .to_string()
    }

    fn device_json() -> String {
        serde_json::json!({"client_id": "CID", "client_secret": "CSECRET"}).to_string()
    }

    fn profile_json() -> String {
        serde_json::json!({"arn": "arn:aws:codewhisperer:us-east-1:123:profile/ABC"}).to_string()
    }

    #[test]
    fn default_db_path_ends_with_kiro_cli_data_sqlite3() {
        let p = default_db_path().unwrap();
        assert!(
            p.ends_with(std::path::Path::new("kiro-cli").join("data.sqlite3")),
            "{}",
            p.display()
        );
    }

    #[test]
    fn missing_file_is_a_credentials_error_naming_the_path() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.sqlite3");
        let err = read_credentials(&path).unwrap_err();
        match err {
            AppError::Credentials(m) => assert!(m.contains(&path.display().to_string())),
            other => panic!("expected Credentials error, got {other:?}"),
        }
    }

    #[test]
    fn reads_token_device_and_profile_out_of_the_db() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.sqlite3");
        seed_db(
            &path,
            Some(&token_json()),
            Some(&device_json()),
            Some(&profile_json()),
        );
        let creds = read_credentials(&path).unwrap();
        assert_eq!(creds.access_token, "AT");
        assert_eq!(creds.refresh_token, "RT");
        assert_eq!(creds.region, "us-east-1");
        assert_eq!(
            creds.auth_kind,
            KiroAuthKind::Oidc {
                client_id: "CID".into(),
                client_secret: "CSECRET".into(),
            }
        );
        assert_eq!(
            creds.profile_arn,
            "arn:aws:codewhisperer:us-east-1:123:profile/ABC"
        );
        assert_eq!(creds.expires_at.to_rfc3339(), "2030-01-01T00:00:00+00:00");
    }

    fn seed_social_db(path: &Path, social_token: Option<&str>, profile: Option<&str>) {
        let conn = Connection::open(path).unwrap();
        conn.execute("CREATE TABLE auth_kv (key TEXT, value TEXT)", [])
            .unwrap();
        conn.execute("CREATE TABLE state (key TEXT, value TEXT)", [])
            .unwrap();
        if let Some(t) = social_token {
            conn.execute(
                "INSERT INTO auth_kv (key, value) VALUES (?1, ?2)",
                rusqlite::params![SOCIAL_TOKEN_KEY, t],
            )
            .unwrap();
        }
        if let Some(p) = profile {
            conn.execute(
                "INSERT INTO state (key, value) VALUES (?1, ?2)",
                rusqlite::params![PROFILE_STATE_KEY, p],
            )
            .unwrap();
        }
    }

    fn social_token_json(refresh_token: &str, profile_arn: &str) -> String {
        serde_json::json!({
            "access_token": "SOCIAL-AT",
            "refresh_token": refresh_token,
            "expires_at": "2030-01-01T00:00:00.123456789Z",
            "provider": "google",
            "profile_arn": profile_arn,
        })
        .to_string()
    }

    #[test]
    fn reads_social_token_and_derives_region_from_profile_arn() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.sqlite3");
        let arn = "arn:aws:codewhisperer:us-east-1:699475941385:profile/EHGA3GRVQMUK";
        seed_social_db(&path, Some(&social_token_json("SOCIAL-RT", arn)), None);

        let creds = read_credentials(&path).unwrap();
        assert_eq!(creds.access_token, "SOCIAL-AT");
        assert_eq!(creds.refresh_token, "SOCIAL-RT");
        assert_eq!(creds.region, "us-east-1");
        assert_eq!(creds.auth_kind, KiroAuthKind::Social);
        assert_eq!(creds.profile_arn, arn);
    }

    #[test]
    fn social_token_falls_back_to_state_profile_arn_when_token_arn_is_absent() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.sqlite3");
        let token = serde_json::json!({
            "access_token": "SOCIAL-AT",
            "refresh_token": "SOCIAL-RT",
            "expires_at": "2030-01-01T00:00:00Z",
            "provider": "github",
        })
        .to_string();
        let state_profile = serde_json::json!({
            "arn": "arn:aws:codewhisperer:eu-west-1:123:profile/ABC",
            "profile_name": "Social_Default_Profile",
        })
        .to_string();
        seed_social_db(&path, Some(&token), Some(&state_profile));

        let creds = read_credentials(&path).unwrap();
        assert_eq!(creds.auth_kind, KiroAuthKind::Social);
        assert_eq!(creds.region, "eu-west-1");
        assert_eq!(
            creds.profile_arn,
            "arn:aws:codewhisperer:eu-west-1:123:profile/ABC"
        );
    }

    #[test]
    fn social_token_without_any_profile_arn_is_a_credentials_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.sqlite3");
        seed_social_db(&path, Some(&social_token_json("SOCIAL-RT", "")), None);
        assert!(matches!(
            read_credentials(&path),
            Err(AppError::Credentials(_))
        ));
    }

    #[test]
    fn social_token_with_invalid_arn_region_is_rejected() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.sqlite3");
        seed_social_db(
            &path,
            Some(&social_token_json(
                "SOCIAL-RT",
                "arn:aws:codewhisperer:evil.example/#:123:profile/ABC",
            )),
            None,
        );
        let err = read_credentials(&path).unwrap_err();
        assert!(matches!(err, AppError::Credentials(_)));
        assert!(err.to_string().contains("region"));
    }

    #[test]
    fn social_accounts_sharing_the_default_profile_arn_have_distinct_keys() {
        let dir = TempDir::new().unwrap();
        let arn = "arn:aws:codewhisperer:us-east-1:699475941385:profile/EHGA3GRVQMUK";

        let path1 = dir.path().join("a.sqlite3");
        seed_social_db(&path1, Some(&social_token_json("RT-ONE", arn)), None);
        let one = read_credentials(&path1).unwrap();

        let path2 = dir.path().join("b.sqlite3");
        seed_social_db(&path2, Some(&social_token_json("RT-TWO", arn)), None);
        let two = read_credentials(&path2).unwrap();

        assert_ne!(one.account_key, two.account_key);
        assert!(!one.account_key.contains("RT-ONE"));
        assert!(!two.account_key.contains("RT-TWO"));
    }

    #[test]
    fn missing_profile_row_falls_back_to_empty_arn_not_an_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.sqlite3");
        seed_db(&path, Some(&token_json()), Some(&device_json()), None);
        let creds = read_credentials(&path).unwrap();
        assert_eq!(creds.profile_arn, "");
    }

    #[test]
    fn missing_token_row_is_a_credentials_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.sqlite3");
        seed_db(&path, None, Some(&device_json()), Some(&profile_json()));
        assert!(matches!(
            read_credentials(&path),
            Err(AppError::Credentials(_))
        ));
    }

    #[test]
    fn missing_device_registration_is_a_credentials_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.sqlite3");
        seed_db(&path, Some(&token_json()), None, Some(&profile_json()));
        assert!(matches!(
            read_credentials(&path),
            Err(AppError::Credentials(_))
        ));
    }

    #[test]
    fn malformed_token_json_is_a_credentials_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.sqlite3");
        seed_db(&path, Some("not json"), Some(&device_json()), None);
        assert!(matches!(
            read_credentials(&path),
            Err(AppError::Credentials(_))
        ));
    }

    #[test]
    fn malformed_profile_json_is_not_treated_as_a_builder_id_account() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.sqlite3");
        seed_db(
            &path,
            Some(&token_json()),
            Some(&device_json()),
            Some("not json"),
        );
        let err = read_credentials(&path).unwrap_err();
        assert!(matches!(err, AppError::Credentials(_)));
        assert!(err.to_string().contains(PROFILE_STATE_KEY));
    }

    #[test]
    fn unparseable_expiry_is_a_credentials_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.sqlite3");
        let bad_token = serde_json::json!({
            "access_token": "AT", "refresh_token": "RT",
            "expires_at": "not-a-date", "region": "us-east-1",
        })
        .to_string();
        seed_db(&path, Some(&bad_token), Some(&device_json()), None);
        assert!(matches!(
            read_credentials(&path),
            Err(AppError::Credentials(_))
        ));
    }

    #[test]
    fn account_key_is_stable_and_account_specific() {
        let dir = TempDir::new().unwrap();
        let path1 = dir.path().join("a.sqlite3");
        seed_db(
            &path1,
            Some(&token_json()),
            Some(&device_json()),
            Some(&profile_json()),
        );
        let one = read_credentials(&path1).unwrap();

        let path2 = dir.path().join("b.sqlite3");
        seed_db(
            &path2,
            Some(&token_json()),
            Some(&device_json()),
            Some(&profile_json()),
        );
        let one_again = read_credentials(&path2).unwrap();
        assert_eq!(one.account_key, one_again.account_key);
        assert_eq!(one.account_key, "8b4d254cee50ffb1116ee414d5b91b93239e7507");

        let other_profile =
            serde_json::json!({"arn": "arn:aws:codewhisperer:us-east-1:999:profile/XYZ"})
                .to_string();
        let path3 = dir.path().join("c.sqlite3");
        seed_db(
            &path3,
            Some(&token_json()),
            Some(&device_json()),
            Some(&other_profile),
        );
        let two = read_credentials(&path3).unwrap();
        assert_ne!(one.account_key, two.account_key);
        assert!(!one.account_key.contains("ABC"));
    }

    #[test]
    fn builder_id_accounts_in_the_same_region_have_distinct_keys() {
        let dir = TempDir::new().unwrap();
        let path1 = dir.path().join("a.sqlite3");
        seed_db(&path1, Some(&token_json()), Some(&device_json()), None);
        let one = read_credentials(&path1).unwrap();

        let path2 = dir.path().join("b.sqlite3");
        let other_token = serde_json::json!({
            "access_token": "AT2",
            "refresh_token": "OTHER-RT",
            "expires_at": "2030-01-01T00:00:00Z",
            "region": "us-east-1",
        })
        .to_string();
        seed_db(&path2, Some(&other_token), Some(&device_json()), None);
        let two = read_credentials(&path2).unwrap();

        assert_ne!(one.account_key, two.account_key);
        assert!(!one.account_key.contains("RT"));
        assert!(!two.account_key.contains("OTHER-RT"));
    }

    #[test]
    fn unsafe_region_is_rejected_before_endpoint_construction() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.sqlite3");
        let token = serde_json::json!({
            "access_token": "AT",
            "refresh_token": "RT",
            "expires_at": "2030-01-01T00:00:00Z",
            "region": "evil.example/#",
        })
        .to_string();
        seed_db(&path, Some(&token), Some(&device_json()), None);

        let err = read_credentials(&path).unwrap_err();
        assert!(matches!(err, AppError::Credentials(_)));
        assert!(err.to_string().contains("region"));
    }

    #[test]
    fn coexisting_oidc_and_social_selects_newer_social_on_switch() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.sqlite3");
        let stale_oidc = serde_json::json!({
            "access_token": "OIDC-OLD-AT",
            "refresh_token": "OIDC-OLD-RT",
            "expires_at": "2026-01-01T00:00:00Z",
            "region": "us-east-1",
        })
        .to_string();
        let arn = "arn:aws:codewhisperer:us-east-1:699475941385:profile/EHGA3GRVQMUK";
        let state_profile = serde_json::json!({
            "arn": arn,
            "profile_name": "Social_Default_Profile",
        })
        .to_string();
        let conn = seed_db(
            &path,
            Some(&stale_oidc),
            Some(&device_json()),
            Some(&state_profile),
        );
        conn.execute(
            "INSERT INTO auth_kv (key, value) VALUES (?1, ?2)",
            rusqlite::params![SOCIAL_TOKEN_KEY, social_token_json("SOCIAL-RT", arn)],
        )
        .unwrap();

        let creds = read_credentials(&path).unwrap();
        assert_eq!(creds.auth_kind, KiroAuthKind::Social);
        assert_eq!(creds.access_token, "SOCIAL-AT");
        assert_eq!(creds.refresh_token, "SOCIAL-RT");
        assert_eq!(creds.profile_arn, arn);
    }

    #[test]
    fn coexisting_oidc_and_social_selects_newer_oidc_on_switch() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.sqlite3");
        let newer_oidc = serde_json::json!({
            "access_token": "OIDC-NEW-AT",
            "refresh_token": "OIDC-NEW-RT",
            "expires_at": "2035-01-01T00:00:00Z",
            "region": "us-east-1",
        })
        .to_string();
        let arn = "arn:aws:codewhisperer:us-east-1:699475941385:profile/EHGA3GRVQMUK";
        let state_profile = serde_json::json!({
            "arn": arn,
            "profile_name": "Social_Default_Profile",
        })
        .to_string();
        let conn = seed_db(
            &path,
            Some(&newer_oidc),
            Some(&device_json()),
            Some(&state_profile),
        );
        conn.execute(
            "INSERT INTO auth_kv (key, value) VALUES (?1, ?2)",
            rusqlite::params![SOCIAL_TOKEN_KEY, social_token_json("SOCIAL-RT", arn)],
        )
        .unwrap();

        let creds = read_credentials(&path).unwrap();
        assert_eq!(
            creds.auth_kind,
            KiroAuthKind::Oidc {
                client_id: "CID".into(),
                client_secret: "CSECRET".into(),
            }
        );
        assert_eq!(creds.access_token, "OIDC-NEW-AT");
        assert_eq!(creds.refresh_token, "OIDC-NEW-RT");
        // Leftover Social_Default_Profile in state must not be attached to an
        // AWS Builder ID OIDC credential.
        assert_eq!(creds.profile_arn, "");
    }

    #[test]
    fn coexisting_oidc_and_social_with_equal_expiry_is_an_ambiguous_credentials_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.sqlite3");
        let arn = "arn:aws:codewhisperer:us-east-1:699475941385:profile/EHGA3GRVQMUK";
        let social_same_expiry = serde_json::json!({
            "access_token": "SOCIAL-AT",
            "refresh_token": "SOCIAL-RT",
            "expires_at": "2030-01-01T00:00:00Z",
            "provider": "google",
            "profile_arn": arn,
        })
        .to_string();
        let conn = seed_db(&path, Some(&token_json()), Some(&device_json()), None);
        conn.execute(
            "INSERT INTO auth_kv (key, value) VALUES (?1, ?2)",
            rusqlite::params![SOCIAL_TOKEN_KEY, social_same_expiry],
        )
        .unwrap();

        let err = read_credentials(&path).unwrap_err();
        assert!(matches!(err, AppError::Credentials(_)));
        assert!(err.to_string().contains("identical expiry"));
    }

    #[test]
    fn coexisting_incomplete_oidc_without_device_registration_falls_back_to_valid_social() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.sqlite3");
        let arn = "arn:aws:codewhisperer:us-east-1:699475941385:profile/EHGA3GRVQMUK";
        let conn = seed_db(&path, Some(&token_json()), None, None);
        conn.execute(
            "INSERT INTO auth_kv (key, value) VALUES (?1, ?2)",
            rusqlite::params![SOCIAL_TOKEN_KEY, social_token_json("SOCIAL-RT", arn)],
        )
        .unwrap();

        let creds = read_credentials(&path).unwrap();
        assert_eq!(creds.auth_kind, KiroAuthKind::Social);
        assert_eq!(creds.access_token, "SOCIAL-AT");
    }
}
