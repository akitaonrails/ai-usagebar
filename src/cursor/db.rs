//! Read the Cursor IDE's own session token out of its local `state.vscdb`.
//!
//! Cursor has no documented usage API or API key for personal quota — every
//! community tool that shows it (cursor-stats, cursor-usage-tracker, etc.)
//! reads the same place: a SQLite key-value store the Cursor IDE itself
//! maintains at `.../User/globalStorage/state.vscdb` (the same `state.vscdb`
//! every VS Code-family app uses for `ItemTable`-shaped extension/global
//! state), under the key `cursorAuth/accessToken`. That value is a JWT whose
//! `sub` claim (`auth0|<userId>`) is combined with the raw token into the
//! `WorkosCursorSessionToken` cookie the dashboard's own usage call expects —
//! see `fetch.rs`.

use std::path::{Path, PathBuf};

use base64::Engine;
use rusqlite::{Connection, OpenFlags};

use crate::error::{AppError, Result};

const TOKEN_KEY: &str = "cursorAuth/accessToken";

/// Default location of Cursor's local state database. This is Cursor's own
/// per-OS convention (same one every VS Code-family app uses for its user
/// data), not ai-usagebar's XDG cache — conveniently identical to what
/// `directories::BaseDirs::config_dir()` already resolves on every platform:
///   - Linux: `~/.config`
///   - macOS: `~/Library/Application Support`
///   - Windows: `%APPDATA%` (Roaming)
pub fn default_db_path() -> Result<PathBuf> {
    let base = directories::BaseDirs::new().ok_or_else(|| {
        AppError::Other("could not resolve the platform config directory (no HOME?)".into())
    })?;
    Ok(base
        .config_dir()
        .join("Cursor")
        .join("User")
        .join("globalStorage")
        .join("state.vscdb"))
}

/// Read the raw `cursorAuth/accessToken` value from `path`. A missing file or
/// missing row means "never signed in to Cursor" — reported as a credentials
/// error (like a missing `~/.claude/.credentials.json`) rather than a network
/// or schema failure, so the widget's `⚠` tooltip tells the user to sign in
/// rather than implying the API is down.
pub fn read_access_token(path: &Path) -> Result<String> {
    if !path.exists() {
        return Err(AppError::Credentials(format!(
            "Cursor database not found at {}. Open the Cursor IDE and sign in at least once, \
             then try again.",
            path.display()
        )));
    }
    // Read-only: this file is Cursor's own live state, not ours to lock for
    // writing. SQLite allows concurrent readers, so this is safe alongside a
    // running Cursor IDE.
    let conn =
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| {
            AppError::Credentials(format!(
                "could not open Cursor database at {}: {e}",
                path.display()
            ))
        })?;
    let token: String = conn
        .query_row(
            "SELECT value FROM ItemTable WHERE key = ?1",
            [TOKEN_KEY],
            |row| row.get(0),
        )
        .map_err(|_| {
            AppError::Credentials(format!(
                "no Cursor session found in {}. Sign in to the Cursor IDE, then try again.",
                path.display()
            ))
        })?;
    if token.trim().is_empty() {
        return Err(AppError::Credentials(
            "Cursor session token is empty. Sign in to the Cursor IDE again.".into(),
        ));
    }
    Ok(token)
}

/// The two values the `/api/usage` call needs, both derived from the same JWT:
/// the bare user id (a query param) and the `WorkosCursorSessionToken` cookie
/// value (`userId%3A%3Atoken` — literal, pre-encoded `::`, matching what the
/// Cursor dashboard's own JS sends).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionAuth {
    pub user_id: String,
    pub cookie_value: String,
}

/// Derive [`SessionAuth`] from the raw access token. Fails when the token
/// isn't a decodable JWT or its `sub` claim doesn't have the `issuer|userId`
/// shape every Cursor account token carries — either way the token is
/// unusable, so this is a credentials error, not a schema error (the *shape*
/// of the wire endpoint isn't in play yet at this point).
pub fn session_auth(token: &str) -> Result<SessionAuth> {
    let claims = parse_jwt_claims(token).ok_or_else(|| {
        AppError::Credentials(
            "Cursor session token could not be decoded. Sign in to the Cursor IDE again.".into(),
        )
    })?;
    let sub = claims
        .get("sub")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| AppError::Credentials("Cursor session token has no `sub` claim.".into()))?;
    let user_id = sub
        .split('|')
        .nth(1)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            AppError::Credentials(format!(
                "Cursor session token `sub` claim has an unexpected shape: {sub:?}"
            ))
        })?
        .to_string();
    let cookie_value = format!("{user_id}%3A%3A{token}");
    Ok(SessionAuth {
        user_id,
        cookie_value,
    })
}

/// Decode a JWT's payload segment without verifying its signature — we trust
/// it the same way the Cursor dashboard's own browser JS does (it never
/// verifies either; the server is the one that rejects a bad token).
fn parse_jwt_claims(token: &str) -> Option<serde_json::Value> {
    let mut parts = token.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(payload))
        .ok()?;
    serde_json::from_slice(&decoded).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Build a fake JWT with the given claims (no signature verification,
    /// matching `openai::creds`'s test helper).
    fn fake_jwt(claims: serde_json::Value) -> String {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
        let payload =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string().as_bytes());
        format!("{header}.{payload}.sig")
    }

    fn seed_db(path: &Path, token: Option<&str>) {
        let conn = Connection::open(path).unwrap();
        conn.execute("CREATE TABLE ItemTable (key TEXT, value TEXT)", [])
            .unwrap();
        if let Some(t) = token {
            conn.execute(
                "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
                rusqlite::params![TOKEN_KEY, t],
            )
            .unwrap();
        }
    }

    #[test]
    fn default_db_path_ends_with_the_cursor_state_file() {
        let p = default_db_path().unwrap();
        assert!(
            p.ends_with(
                std::path::Path::new("Cursor")
                    .join("User")
                    .join("globalStorage")
                    .join("state.vscdb")
            )
        );
    }

    #[test]
    fn missing_file_is_a_credentials_error_naming_the_path() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("state.vscdb");
        let err = read_access_token(&path).unwrap_err();
        match err {
            AppError::Credentials(m) => assert!(m.contains(&path.display().to_string())),
            other => panic!("expected Credentials error, got {other:?}"),
        }
    }

    #[test]
    fn reads_the_token_back_out_of_the_item_table() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("state.vscdb");
        seed_db(&path, Some("fake-token-value"));
        assert_eq!(read_access_token(&path).unwrap(), "fake-token-value");
    }

    #[test]
    fn missing_row_is_a_credentials_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("state.vscdb");
        seed_db(&path, None);
        let err = read_access_token(&path).unwrap_err();
        assert!(matches!(err, AppError::Credentials(_)));
    }

    #[test]
    fn empty_token_is_a_credentials_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("state.vscdb");
        seed_db(&path, Some(""));
        let err = read_access_token(&path).unwrap_err();
        assert!(matches!(err, AppError::Credentials(_)));
    }

    #[test]
    fn session_auth_extracts_user_id_and_builds_the_cookie_value() {
        let token = fake_jwt(serde_json::json!({"sub": "auth0|user_abc123"}));
        let auth = session_auth(&token).unwrap();
        assert_eq!(auth.user_id, "user_abc123");
        assert_eq!(auth.cookie_value, format!("user_abc123%3A%3A{token}"));
    }

    #[test]
    fn session_auth_rejects_a_non_jwt_token() {
        let err = session_auth("not-a-jwt").unwrap_err();
        assert!(matches!(err, AppError::Credentials(_)));
    }

    #[test]
    fn session_auth_rejects_missing_sub_claim() {
        let token = fake_jwt(serde_json::json!({"other": "value"}));
        let err = session_auth(&token).unwrap_err();
        match err {
            AppError::Credentials(m) => assert!(m.contains("sub")),
            other => panic!("expected Credentials error, got {other:?}"),
        }
    }

    #[test]
    fn session_auth_rejects_sub_without_a_pipe_separated_user_id() {
        let token = fake_jwt(serde_json::json!({"sub": "no-pipe-here"}));
        let err = session_auth(&token).unwrap_err();
        assert!(matches!(err, AppError::Credentials(_)));
    }
}
