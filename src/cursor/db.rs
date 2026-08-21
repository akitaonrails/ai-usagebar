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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::{hash::Hash, hash::Hasher};

use base64::Engine;
use rusqlite::{Connection, OpenFlags, types::ValueRef};

use crate::error::{AppError, Result};

const TOKEN_KEY: &str = "cursorAuth/accessToken";

/// The `ItemTable` namespace Cursor's own sign-in writes, and the unit
/// [`crate::sync::keystore`] moves between machines.
///
/// A **prefix**, not a hand-list of key names. Cursor's sign-in populates
/// `cursorAuth/accessToken`, `cursorAuth/refreshToken`, `cursorAuth/cachedEmail`,
/// `cursorAuth/cachedSignUpType`, `cursorAuth/stripeMembershipType` and
/// `cursorAuth/scopePerMembershipType` — measured, but the list is Cursor's to
/// change and a copy of it here goes stale silently, leaving a restored session
/// that authenticates and then cannot say which plan it is on. The namespace is
/// the thing that means "this login"; everything in it travels, and nothing
/// outside it does.
///
/// It is also the **write** rule: [`write_auth_rows`] refuses any key that does
/// not start with this, so a tampered bundle cannot reach another key in the
/// 38 MB of editor state that shares this table.
const AUTH_PREFIX: &str = "cursorAuth/";

/// How long a write waits for a running Cursor to release the database before
/// giving up. SQLite's own busy handler, rather than a lock invented here —
/// `state.vscdb` is Cursor's file, and a second lock protocol only one side
/// observes is not a lock.
const WRITE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

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

/// Every `cursorAuth/*` row in `path`, as an ordered `key -> value` map.
///
/// Ordered because the map is serialised into a sync bundle and hashed to
/// decide "is this the same login?" — a `HashMap`'s iteration order would make
/// one unchanged credential look like a different one on every push.
///
/// A missing database, or one with no such rows, is an **empty map** and not an
/// error: that is "this machine has never signed in to Cursor", which is a fact
/// and not a failure. A database that exists but cannot be opened *is* an
/// error, because reporting a locked or corrupt store as "no login here" is how
/// a push silently ships a bundle without the credential in it.
///
/// Read-only, like [`read_access_token`] and for the same reason: this is
/// Cursor's live file and a reader is safe alongside a running IDE.
pub fn read_auth_rows(path: &Path) -> Result<BTreeMap<String, String>> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let conn =
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| {
            AppError::Credentials(format!(
                "could not open Cursor database at {}: {e}",
                path.display()
            ))
        })?;
    let mut stmt = conn
        .prepare("SELECT key, value FROM ItemTable WHERE key LIKE ?1")
        .map_err(|e| {
            AppError::Credentials(format!(
                "could not read Cursor sign-in state from {}: {e}",
                path.display()
            ))
        })?;
    let mut rows = stmt.query([format!("{AUTH_PREFIX}%")]).map_err(|e| {
        AppError::Credentials(format!(
            "could not read Cursor sign-in state from {}: {e}",
            path.display()
        ))
    })?;
    let mut out = BTreeMap::new();
    while let Some(row) = rows.next().map_err(|e| {
        AppError::Credentials(format!(
            "could not read Cursor sign-in state from {}: {e}",
            path.display()
        ))
    })? {
        let (Ok(key), Ok(value)) = (row.get::<_, String>(0), text_at(row, 1)) else {
            continue;
        };
        // The `LIKE` above is a coarse filter, not the rule. SQLite's `LIKE` is
        // case-insensitive for ASCII by default, so it also matches
        // `CursorAuth/…` — a different key, which must not travel as if it were
        // this one. The byte-exact rule is here.
        if !key.starts_with(AUTH_PREFIX) || value.is_empty() {
            continue;
        }
        out.insert(key, value);
    }
    Ok(out)
}

/// Is there a Cursor sign-in in `path` — **without reading one**?
///
/// The question `sync status` asks, and it may not read a credential to answer
/// it (the macOS menu bar runs that on every menu open). `SELECT 1 … LIMIT 1`
/// returns no value at all.
pub fn has_auth_rows(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let conn =
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| {
            AppError::Credentials(format!(
                "could not open Cursor database at {}: {e}",
                path.display()
            ))
        })?;
    conn.query_row(
        "SELECT 1 FROM ItemTable WHERE key LIKE ?1 AND value IS NOT NULL LIMIT 1",
        [format!("{AUTH_PREFIX}%")],
        |_| Ok(true),
    )
    .or_else(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(false),
        // No `ItemTable` at all is a database that is not Cursor's — "no
        // sign-in here" is the honest reading, not a failure to report.
        other => Err(AppError::Credentials(format!(
            "could not check Cursor sign-in state in {}: {other}",
            path.display()
        ))),
    })
}

/// Write `rows` into the `ItemTable` of an **existing** Cursor database,
/// leaving every other row exactly as it was.
///
/// # Why this writes rows and never the file
///
/// `state.vscdb` is tens of megabytes of live editor state — open tabs,
/// history, workspace layout. Copying the file between machines to move a
/// few hundred bytes of credential would destroy the receiving machine's
/// editor state, so the credential travels as rows and lands as rows.
///
/// # All or nothing
///
/// One transaction. Either every row lands or none does, so a failure leaves
/// the machine's existing Cursor login exactly as it was rather than half
/// replaced by another machine's — a spliced credential is worse than either
/// whole one.
///
/// # If Cursor is running
///
/// SQLite's own busy handler waits [`WRITE_BUSY_TIMEOUT`] for the writer lock
/// and then gives up with an error, which the caller turns into a refusal of
/// this one item. Nothing here forces a lock or removes one.
///
/// The converse is the case this cannot solve and the caller must state: a
/// *running* Cursor holds its own copy of these values in memory and may write
/// them back over ours when it next persists. Quit Cursor before restoring
/// into it.
///
/// # The keys are the bundle's, so they are checked
///
/// Every key must be in the [`AUTH_PREFIX`] namespace. Without that check a
/// tampered manifest could set any key in this table — including the ones that
/// tell the editor which files and extensions to load.
pub fn write_auth_rows(path: &Path, rows: &BTreeMap<String, String>) -> Result<()> {
    if !path.exists() {
        return Err(AppError::Credentials(format!(
            "no Cursor database at {}. Open the Cursor IDE once so it creates its state \
             database, then restore again — ai-usagebar will not fabricate one.",
            path.display()
        )));
    }
    for key in rows.keys() {
        if !key.starts_with(AUTH_PREFIX) || key.contains(|c: char| c.is_control()) {
            return Err(AppError::Credentials(format!(
                "refusing to write the Cursor state key {key:?}: a restore may only write the \
                 `{AUTH_PREFIX}` namespace"
            )));
        }
    }
    let mut conn = Connection::open(path).map_err(|e| {
        AppError::Credentials(format!(
            "could not open Cursor database at {} for writing: {e}",
            path.display()
        ))
    })?;
    conn.busy_timeout(WRITE_BUSY_TIMEOUT).map_err(|e| {
        AppError::Credentials(format!(
            "could not set the Cursor database busy timeout: {e}"
        ))
    })?;
    let tx = conn.transaction().map_err(|e| {
        AppError::Credentials(format!(
            "could not begin a write to {} (is Cursor running?): {e}",
            path.display()
        ))
    })?;
    for (key, value) in rows {
        tx.execute(
            "INSERT OR REPLACE INTO ItemTable (key, value) VALUES (?1, ?2)",
            rusqlite::params![key, value],
        )
        .map_err(|e| {
            // `key` is a bundle string, `{:?}` escapes it; `value` is a live
            // credential and appears nowhere.
            AppError::Credentials(format!(
                "could not write the Cursor state key {key:?} (is Cursor running?): {e}"
            ))
        })?;
    }
    tx.commit().map_err(|e| {
        AppError::Credentials(format!(
            "could not commit the Cursor sign-in to {} (is Cursor running?): {e}",
            path.display()
        ))
    })?;
    Ok(())
}

/// A row's column as text, whichever storage class SQLite chose for it.
///
/// `ItemTable.value` is declared `BLOB` but written as text by the editor, and
/// SQLite stores what it is given — so a `get::<String>` alone fails on any
/// value that happened to land as a blob.
///
/// Never lossy. A replacement character substituted into a token produces a
/// credential that is silently wrong rather than one that is visibly missing.
fn text_at(row: &rusqlite::Row<'_>, idx: usize) -> Result<String> {
    match row.get_ref(idx) {
        Ok(ValueRef::Text(b) | ValueRef::Blob(b)) => std::str::from_utf8(b)
            .map(str::to_string)
            .map_err(|_| AppError::Credentials("a Cursor state value is not UTF-8".into())),
        _ => Err(AppError::Credentials(
            "a Cursor state value is not text".into(),
        )),
    }
}

/// Default location of the `cursor-agent` CLI's own login state — a plain
/// JSON file, not the IDE's `state.vscdb`. Written by the headless
/// `cursor-agent` tool, so it stays
/// populated on machines that never run the desktop IDE at all.
pub fn default_agent_auth_path() -> Result<PathBuf> {
    let base = directories::BaseDirs::new().ok_or_else(|| {
        AppError::Other("could not resolve the platform config directory (no HOME?)".into())
    })?;
    Ok(base.config_dir().join("cursor").join("auth.json"))
}

/// Read `cursor-agent`'s `accessToken` out of its `auth.json`. Same error
/// shape as [`read_access_token`] (missing file / missing field / empty
/// value are all a [`AppError::Credentials`]) so callers can treat both
/// sources interchangeably.
pub fn read_agent_access_token(path: &Path) -> Result<String> {
    if !path.exists() {
        return Err(AppError::Credentials(format!(
            "cursor-agent auth file not found at {}. Run `cursor-agent` and sign in at least \
             once, then try again.",
            path.display()
        )));
    }
    let bytes = std::fs::read(path).map_err(|e| AppError::io_at(path, e))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| AppError::Credentials(format!("could not parse {}: {e}", path.display())))?;
    let token = value
        .get("accessToken")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            AppError::Credentials(format!(
                "no accessToken in {}. Sign in with `cursor-agent` again.",
                path.display()
            ))
        })?;
    Ok(token.to_string())
}

/// Resolve a Cursor session token from either source. The IDE's `state.vscdb`
/// is tried first (it is the live, continuously-refreshed source when the
/// desktop app is actually running); a text-only / headless machine that has
/// never opened the IDE falls back to whatever `cursor-agent` last wrote to
/// its own `auth.json`. If the agent file exists but cannot be used, its error
/// is surfaced so a headless user gets an actionable diagnostic. The IDE's
/// error remains the one surfaced when both sources are absent, since it names
/// the more commonly expected path.
pub fn resolve_access_token(db_path: &Path, agent_auth_path: &Path) -> Result<String> {
    match read_access_token(db_path) {
        Ok(token) => Ok(token),
        Err(_) if !db_path.exists() && agent_auth_path.exists() => {
            read_agent_access_token(agent_auth_path)
        }
        Err(ide_err) => Err(ide_err),
    }
}

/// The two values the `/api/usage` call needs, both derived from the same JWT:
/// the bare user id (a query param) and the `WorkosCursorSessionToken` cookie
/// value (`userId%3A%3Atoken` — literal, pre-encoded `::`, matching what the
/// Cursor dashboard's own JS sends).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionAuth {
    pub user_id: String,
    /// Stable, non-plaintext cache identity for the signed-in Cursor account.
    /// The hash is only a change detector: a different toolchain may produce a
    /// different value and force one harmless refetch.
    pub account_key: String,
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
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    user_id.hash(&mut hasher);
    let account_key = format!("{:016x}", hasher.finish());
    let cookie_value = format!("{user_id}%3A%3A{token}");
    Ok(SessionAuth {
        user_id,
        account_key,
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

    /// Cursor's own `ItemTable` declaration, verbatim — `UNIQUE ON CONFLICT
    /// REPLACE` and a `BLOB` value column. A laxer schema here would let
    /// [`write_auth_rows`] pass a test that duplicates rows against the real
    /// thing.
    fn seed_db(path: &Path, token: Option<&str>) {
        let conn = Connection::open(path).unwrap();
        conn.execute(
            "CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB)",
            [],
        )
        .unwrap();
        if let Some(t) = token {
            conn.execute(
                "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
                rusqlite::params![TOKEN_KEY, t],
            )
            .unwrap();
        }
    }

    /// Every row in the table, as a plain map — what "the rest of the database
    /// is untouched" is asserted against.
    fn all_rows(path: &Path) -> BTreeMap<String, String> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let mut stmt = conn.prepare("SELECT key, value FROM ItemTable").unwrap();
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, text_at(row, 1).unwrap()))
            })
            .unwrap();
        rows.map(|row| row.unwrap()).collect()
    }

    fn put(path: &Path, key: &str, value: &str) {
        let conn = Connection::open(path).unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO ItemTable (key, value) VALUES (?1, ?2)",
            rusqlite::params![key, value],
        )
        .unwrap();
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
        assert_eq!(auth.account_key.len(), 16);
        assert!(!auth.account_key.contains("user_abc123"));
        assert_eq!(auth.cookie_value, format!("user_abc123%3A%3A{token}"));
    }

    #[test]
    fn session_auth_account_key_is_stable_and_account_specific() {
        let one = session_auth(&fake_jwt(serde_json::json!({"sub": "auth0|one"}))).unwrap();
        let one_again = session_auth(&fake_jwt(serde_json::json!({"sub": "auth0|one"}))).unwrap();
        let two = session_auth(&fake_jwt(serde_json::json!({"sub": "auth0|two"}))).unwrap();
        assert_eq!(one.account_key, one_again.account_key);
        assert_ne!(one.account_key, two.account_key);
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

    #[test]
    fn auth_rows_are_every_key_in_the_namespace_and_nothing_else() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("state.vscdb");
        seed_db(&path, Some("the-jwt"));
        put(&path, "cursorAuth/refreshToken", "the-refresh");
        put(&path, "cursorAuth/cachedEmail", "person@example.com");
        put(&path, "cursorAuth/stripeMembershipType", "pro");
        // Editor state that shares the table, and must not travel.
        put(&path, "workbench.explorer.views.state", "{}");
        put(&path, "memento/workbench.parts.editor", "{}");
        // Case is not the namespace: SQLite's LIKE would admit this.
        put(&path, "CursorAuth/imposter", "no");

        let rows = read_auth_rows(&path).unwrap();
        assert_eq!(
            rows.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "cursorAuth/accessToken",
                "cursorAuth/cachedEmail",
                "cursorAuth/refreshToken",
                "cursorAuth/stripeMembershipType",
            ]
        );
        assert_eq!(rows["cursorAuth/accessToken"], "the-jwt");
    }

    #[test]
    fn a_missing_database_has_no_rows_rather_than_failing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("state.vscdb");
        assert!(read_auth_rows(&path).unwrap().is_empty());
        assert!(!has_auth_rows(&path).unwrap());
    }

    #[test]
    fn presence_is_answered_without_reading_a_credential() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("state.vscdb");
        seed_db(&path, None);
        assert!(!has_auth_rows(&path).unwrap());
        put(&path, "cursorAuth/accessToken", "the-jwt");
        assert!(has_auth_rows(&path).unwrap());
    }

    /// **The property the whole row-level design exists for.** A restore moves
    /// a few hundred bytes of credential into a database holding tens of
    /// megabytes of the receiving machine's own editor state, and every byte of
    /// that state has to still be there afterwards.
    #[test]
    fn writing_the_login_leaves_every_other_row_untouched() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("state.vscdb");
        seed_db(&path, Some("this-macs-old-token"));
        put(&path, "cursorAuth/cachedEmail", "old@example.com");
        for i in 0..500 {
            put(
                &path,
                &format!("workbench.state.{i}"),
                &format!("value {i}"),
            );
        }
        let before: BTreeMap<String, String> = all_rows(&path)
            .into_iter()
            .filter(|(k, _)| !k.starts_with(AUTH_PREFIX))
            .collect();

        let incoming = BTreeMap::from([
            (
                "cursorAuth/accessToken".to_string(),
                "other-mac".to_string(),
            ),
            (
                "cursorAuth/refreshToken".to_string(),
                "other-refresh".to_string(),
            ),
        ]);
        write_auth_rows(&path, &incoming).unwrap();

        let after = all_rows(&path);
        assert_eq!(after["cursorAuth/accessToken"], "other-mac");
        assert_eq!(after["cursorAuth/refreshToken"], "other-refresh");
        // Replaced in place, never duplicated — the real schema's UNIQUE.
        assert_eq!(read_access_token(&path).unwrap(), "other-mac");
        let rest: BTreeMap<String, String> = after
            .into_iter()
            .filter(|(k, _)| !k.starts_with(AUTH_PREFIX))
            .collect();
        assert_eq!(rest, before, "the rest of the editor state changed");
        assert_eq!(rest.len(), 500);
    }

    #[test]
    fn a_key_outside_the_auth_namespace_is_refused_and_nothing_is_written() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("state.vscdb");
        seed_db(&path, Some("mine"));
        put(&path, "workbench.colorTheme", "dark");

        let hostile = BTreeMap::from([
            ("cursorAuth/accessToken".to_string(), "theirs".to_string()),
            ("workbench.colorTheme".to_string(), "evil".to_string()),
        ]);
        let err = write_auth_rows(&path, &hostile).unwrap_err();
        assert!(err.to_string().contains("workbench.colorTheme"), "{err}");

        let after = all_rows(&path);
        assert_eq!(after["workbench.colorTheme"], "dark");
        assert_eq!(
            after["cursorAuth/accessToken"], "mine",
            "one refused key must not leave the others half applied"
        );
    }

    #[test]
    fn writing_into_a_machine_with_no_cursor_database_refuses_rather_than_creating_one() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("state.vscdb");
        let rows = BTreeMap::from([("cursorAuth/accessToken".to_string(), "t".to_string())]);
        let err = write_auth_rows(&path, &rows).unwrap_err();
        assert!(
            err.to_string().contains("Open the Cursor IDE once"),
            "{err}"
        );
        assert!(!path.exists(), "no database was fabricated");
    }

    #[test]
    fn default_agent_auth_path_ends_with_cursor_auth_json() {
        let p = default_agent_auth_path().unwrap();
        assert!(p.ends_with(std::path::Path::new("cursor").join("auth.json")));
    }

    #[test]
    fn agent_auth_missing_file_is_a_credentials_error_naming_the_path() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("auth.json");
        let err = read_agent_access_token(&path).unwrap_err();
        match err {
            AppError::Credentials(m) => assert!(m.contains(&path.display().to_string())),
            other => panic!("expected Credentials error, got {other:?}"),
        }
    }

    #[test]
    fn agent_auth_reads_access_token_out_of_the_json_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(
            &path,
            serde_json::json!({"accessToken": "agent-token-value", "refreshToken": "r"})
                .to_string(),
        )
        .unwrap();
        assert_eq!(read_agent_access_token(&path).unwrap(), "agent-token-value");
    }

    #[test]
    fn agent_auth_missing_field_is_a_credentials_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(&path, serde_json::json!({"refreshToken": "r"}).to_string()).unwrap();
        let err = read_agent_access_token(&path).unwrap_err();
        assert!(matches!(err, AppError::Credentials(_)));
    }

    #[test]
    fn agent_auth_empty_token_is_a_credentials_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(&path, serde_json::json!({"accessToken": ""}).to_string()).unwrap();
        let err = read_agent_access_token(&path).unwrap_err();
        assert!(matches!(err, AppError::Credentials(_)));
    }

    #[test]
    fn agent_auth_malformed_json_is_a_credentials_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(&path, "not json").unwrap();
        let err = read_agent_access_token(&path).unwrap_err();
        assert!(matches!(err, AppError::Credentials(_)));
    }

    #[test]
    fn resolve_prefers_the_ide_db_when_both_are_present() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("state.vscdb");
        seed_db(&db_path, Some("ide-token"));
        let agent_path = dir.path().join("auth.json");
        std::fs::write(
            &agent_path,
            serde_json::json!({"accessToken": "agent-token"}).to_string(),
        )
        .unwrap();
        assert_eq!(
            resolve_access_token(&db_path, &agent_path).unwrap(),
            "ide-token"
        );
    }

    #[test]
    fn resolve_falls_back_to_the_agent_file_when_the_ide_db_is_missing() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("state.vscdb");
        let agent_path = dir.path().join("auth.json");
        std::fs::write(
            &agent_path,
            serde_json::json!({"accessToken": "agent-token"}).to_string(),
        )
        .unwrap();
        assert_eq!(
            resolve_access_token(&db_path, &agent_path).unwrap(),
            "agent-token"
        );
    }

    #[test]
    fn resolve_does_not_hide_an_existing_broken_ide_db_with_the_agent_file() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("state.vscdb");
        seed_db(&db_path, None);
        let agent_path = dir.path().join("auth.json");
        std::fs::write(
            &agent_path,
            serde_json::json!({"accessToken": "agent-token"}).to_string(),
        )
        .unwrap();

        let err = resolve_access_token(&db_path, &agent_path).unwrap_err();
        match err {
            AppError::Credentials(m) => {
                assert!(m.contains(&db_path.display().to_string()));
                assert!(!m.contains(&agent_path.display().to_string()));
            }
            other => panic!("expected Credentials error, got {other:?}"),
        }
    }

    #[test]
    fn resolve_surfaces_the_ide_error_when_both_sources_are_missing() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("state.vscdb");
        let agent_path = dir.path().join("auth.json");
        let err = resolve_access_token(&db_path, &agent_path).unwrap_err();
        match err {
            AppError::Credentials(m) => assert!(m.contains(&db_path.display().to_string())),
            other => panic!("expected Credentials error, got {other:?}"),
        }
    }

    #[test]
    fn resolve_surfaces_the_agent_error_when_its_file_exists_but_is_malformed() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("state.vscdb");
        let agent_path = dir.path().join("auth.json");
        std::fs::write(&agent_path, "not json").unwrap();

        let err = resolve_access_token(&db_path, &agent_path).unwrap_err();
        match err {
            AppError::Credentials(m) => {
                assert!(m.contains(&agent_path.display().to_string()));
                assert!(m.contains("could not parse"));
            }
            other => panic!("expected Credentials error, got {other:?}"),
        }
    }
}
