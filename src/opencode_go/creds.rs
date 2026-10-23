//! Locate the OpenCode Go credential the official OpenCode CLI already holds.
//!
//! The CLI files every provider's credential in its own data directory:
//! `~/.local/share/opencode/auth.json` (`$XDG_DATA_HOME` when set), shaped
//! `{ "<provider id>": { "type": "api", "key": "..." } }`. OpenCode 1 stored
//! the account OpenCode sells as Zen (pay-as-you-go) and Go (monthly quota)
//! under the provider id `opencode`; OpenCode 2 names the Go integration
//! `opencode-go`. Either account key is what the `zen/go/v1/usage` endpoint
//! accepts — the official console calls it with the logged-in account's
//! credential.
//!
//! OpenCode 2 keeps credentials in `opencode.db` in the same directory. Only
//! the active OpenCode row is used, accepting a key or an unexpired OAuth
//! access token. Refresh remains the CLI's responsibility.
//!
//! This module reads both stores read-only and never writes, copies, or caches
//! the key, exactly like anthropic, cursor, kiro, and devin treat the
//! credential store the official CLI already wrote. Legacy JSON entries must
//! declare an `api`/`zen` key; their OAuth and well-known shapes are unchanged.
//! Only SQLite OAuth entries supply an access token through the v2 reader.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::cache::home_dir;
use crate::error::{AppError, Result};

/// The provider ids OpenCode uses for its own Zen/Go account. Version 1 wrote
/// `opencode`; version 2 writes `opencode-go`. Try the more specific Go id
/// first, then the legacy one.
const PROVIDER_IDS: &[&str] = &["opencode-go", "opencode"];

/// Entry types whose `key` field is a usable API key. Anything else — in
/// particular OpenCode's `oauth` and `wellknown` types, which hold the secret
/// in `token`/`refresh` — must not be read as a key.
const KEY_TYPES: &[&str] = &["api", "zen"];

/// Read a legacy `auth.json` key or an OpenCode 2 SQLite credential.
///
/// A missing or malformed file, no usable OpenCode entry, or an entry whose
/// type does not carry a key yields `None` rather than an error: one broken
/// shape must not mask a working path later in the search list.
pub fn read_key_from(path: &Path) -> Option<String> {
    read_key_from_at(path, chrono::Utc::now().timestamp_millis())
}

fn read_key_from_at(path: &Path, now_ms: i64) -> Option<String> {
    // Inspect the format, not the filename: auth_paths can name a custom store.
    let mut header = [0; 16];
    let mut file = std::fs::File::open(path).ok()?;
    if file.read_exact(&mut header).is_ok() && &header == b"SQLite format 3\0" {
        return read_database_key(path, now_ms);
    }
    read_legacy_key(path)
}

fn read_legacy_key(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    PROVIDER_IDS.iter().find_map(|provider| {
        let entry = value.get(*provider)?.as_object()?;
        if !entry
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|t| KEY_TYPES.contains(&t))
        {
            return None;
        }
        let key = entry.get("key")?.as_str()?.trim();
        (!key.is_empty()).then(|| key.to_string())
    })
}

/// The database belongs to OpenCode. Do not create it, update credentials, or
/// copy refresh tokens. The normal read-only connection can see committed WAL
/// rows while the CLI is running (unlike SQLite's immutable mode).
fn read_database_key(path: &Path, now_ms: i64) -> Option<String> {
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .ok()?;
    conn.busy_timeout(std::time::Duration::from_millis(500))
        .ok()?;
    // Prefer the OpenCode 2 Go integration, then its legacy id. Iterate the
    // active rows so an expired or malformed one cannot hide a usable sibling.
    let mut statement = conn
        .prepare(
            "SELECT value FROM credential \
             WHERE integration_id IN ('opencode-go', 'opencode') AND active = 1 \
             ORDER BY CASE integration_id WHEN 'opencode-go' THEN 0 ELSE 1 END, \
             time_updated DESC",
        )
        .ok()?;
    let mut rows = statement.query([]).ok()?;
    while let Some(row) = rows.next().ok()? {
        let Ok(raw) = row.get::<_, String>(0) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        if let Some(token) = database_token(&value, now_ms) {
            return Some(token);
        }
    }
    None
}

fn database_token(value: &Value, now_ms: i64) -> Option<String> {
    let token = match value.get("type")?.as_str()? {
        "key" => value.get("key")?.as_str()?,
        "oauth" if value.get("expires")?.as_i64()? > now_ms => value.get("access")?.as_str()?,
        _ => return None,
    };
    non_empty(Some(token)).map(str::to_owned)
}

/// The OpenCode CLI's documented data directory: `$XDG_DATA_HOME` when it is
/// absolute, else `~/.local/share` — on every platform, the troubleshooting
/// docs put `auth.json` in `.local/share` on macOS, Linux, and Windows alike.
pub fn default_auth_path() -> Result<PathBuf> {
    let dir = data_dir(std::env::var_os("XDG_DATA_HOME"), home_dir()?);
    Ok(dir.join("opencode").join("auth.json"))
}

/// Pure XDG rule: the variable wins only when absolute, as the XDG Base
/// Directory spec requires; otherwise the documented default under home.
fn data_dir(var: Option<OsString>, home: PathBuf) -> PathBuf {
    if let Some(dir) = var.map(PathBuf::from).filter(|dir| dir.is_absolute()) {
        return dir;
    }
    home.join(".local").join("share")
}

/// The effective search list: a configured override when it names at least
/// one path, else legacy JSON followed by the OpenCode 2 database. Explicit
/// overrides are exclusive: never look at a sibling or the real user store.
pub fn effective_paths(configured: Option<&[PathBuf]>) -> Result<Vec<PathBuf>> {
    match configured {
        Some(paths) if !paths.is_empty() => Ok(paths.to_vec()),
        _ => {
            let legacy = default_auth_path()?;
            Ok(vec![legacy.clone(), legacy.with_file_name("opencode.db")])
        }
    }
}

/// Detection probe: any effective path holding a usable key. The parse here
/// is just the one provider entry, never a subprocess — every frontend start
/// runs this, and the fetch reads the same file and reports honestly.
pub fn probe(configured: Option<&[PathBuf]>) -> bool {
    effective_paths(configured)
        .ok()
        .is_some_and(|paths| paths.iter().any(|path| read_key_from(path).is_some()))
}

/// Production resolver: configured env var, then inline `api_key`, then the
/// CLI's JSON/database stores. Tests use [`resolve_from`], which takes the values as
/// inputs so an ambient `OPENCODE_GO_API_KEY` cannot leak into them.
pub fn resolve_key(
    env_var_name: &str,
    inline: Option<&str>,
    configured: Option<&[PathBuf]>,
) -> Result<String> {
    let paths = effective_paths(configured)?;
    resolve_from(
        env_var_name,
        std::env::var(env_var_name).ok().as_deref(),
        inline,
        &paths,
    )
}

/// Env, then inline config, then each path in turn — the shared lookup used
/// by both call-sites.
pub fn resolve_from(
    env_var_name: &str,
    env_key: Option<&str>,
    inline: Option<&str>,
    paths: &[PathBuf],
) -> Result<String> {
    if let Some(key) = non_empty(env_key).or_else(|| non_empty(inline)) {
        return Ok(key.to_string());
    }
    for path in paths {
        if let Some(key) = read_key_from(path) {
            return Ok(key);
        }
    }
    Err(AppError::Credentials(signed_out_message(env_var_name)))
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

fn signed_out_message(env_var_name: &str) -> String {
    format!(
        "OpenCode Go is not signed in. Run `opencode auth login` and choose \
         OpenCode, or set {env_var_name}. If an OpenCode 2 session has expired, \
         renew it in the CLI; ai-usagebar never refreshes its token."
    )
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;

    fn write(dir: &Path, name: &str, value: Value) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        path
    }

    const NOW_MS: i64 = 1_800_000_000_000;

    fn database(dir: &Path) -> (PathBuf, rusqlite::Connection) {
        let path = dir.join("opencode.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE credential (integration_id TEXT, active INTEGER, time_updated INTEGER, value TEXT);").unwrap();
        (path, conn)
    }

    fn insert(
        conn: &rusqlite::Connection,
        provider: &str,
        active: i64,
        updated: i64,
        value: Value,
    ) {
        conn.execute(
            "INSERT INTO credential VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![provider, active, updated, value.to_string()],
        )
        .unwrap();
    }

    fn oauth(access: &str, expires: i64) -> Value {
        serde_json::json!({"type":"oauth", "access":access, "refresh":"never-copied", "expires":expires})
    }

    // Regression uses the pre-existing public API: this assertion fails on main.
    #[test]
    fn reads_opencode_2_active_key_from_database() {
        let dir = tempfile::tempdir().unwrap();
        let (path, conn) = database(dir.path());
        insert(
            &conn,
            "opencode",
            0,
            3,
            serde_json::json!({"type":"key", "key":"inactive"}),
        );
        insert(
            &conn,
            "other",
            1,
            4,
            serde_json::json!({"type":"key", "key":"other-provider"}),
        );
        insert(
            &conn,
            "opencode",
            1,
            2,
            serde_json::json!({"type":"key", "key":"active-test-key"}),
        );
        assert_eq!(read_key_from(&path).as_deref(), Some("active-test-key"));
        assert!(probe(Some(std::slice::from_ref(&path))));
        assert_eq!(
            resolve_from("OPENCODE_GO_API_KEY", None, None, &[path]).unwrap(),
            "active-test-key"
        );
    }

    #[test]
    fn reads_opencode_2_go_integration_credential() {
        let dir = tempfile::tempdir().unwrap();
        let (path, conn) = database(dir.path());
        insert(
            &conn,
            "opencode",
            1,
            3,
            serde_json::json!({"type":"key", "key":"legacy-key"}),
        );
        insert(
            &conn,
            "opencode-go",
            1,
            2,
            serde_json::json!({"type":"key", "key":"go-key"}),
        );
        assert_eq!(read_key_from_at(&path, NOW_MS).as_deref(), Some("go-key"));
    }

    #[test]
    fn falls_back_to_legacy_row_when_go_integration_is_expired() {
        let dir = tempfile::tempdir().unwrap();
        let (path, conn) = database(dir.path());
        insert(&conn, "opencode-go", 1, 3, oauth("expired-go", NOW_MS - 1));
        insert(
            &conn,
            "opencode",
            1,
            2,
            serde_json::json!({"type":"key", "key":"legacy-key"}),
        );
        assert_eq!(
            read_key_from_at(&path, NOW_MS).as_deref(),
            Some("legacy-key")
        );
    }

    #[test]
    fn reads_opencode_go_entry_from_legacy_auth_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "auth.json",
            serde_json::json!({"opencode-go":{"type":"api", "key":"go-json-key"}}),
        );
        assert_eq!(read_key_from(&path).as_deref(), Some("go-json-key"));
    }

    #[test]
    fn reads_active_oauth_from_live_wal_without_modifying_database() {
        let dir = tempfile::tempdir().unwrap();
        let (path, conn) = database(dir.path());
        insert(&conn, "opencode", 0, 3, oauth("inactive", NOW_MS + 1));
        insert(&conn, "other", 1, 4, oauth("other-provider", NOW_MS + 1));
        insert(&conn, "opencode", 1, 2, oauth("test-access", NOW_MS + 1));
        let db_before = std::fs::read(&path).unwrap();
        let wal_path = dir.path().join("opencode.db-wal");
        let wal_before = std::fs::read(&wal_path).unwrap();
        assert_eq!(
            read_key_from_at(&path, NOW_MS).as_deref(),
            Some("test-access")
        );
        assert_eq!(std::fs::read(&path).unwrap(), db_before);
        assert_eq!(std::fs::read(&wal_path).unwrap(), wal_before);
        // Re-read the owning CLI's rotated token rather than caching it locally.
        conn.execute(
            "UPDATE credential SET value = ?1 WHERE integration_id = 'opencode' AND active = 1",
            [oauth("rotated-access", NOW_MS + 2).to_string()],
        )
        .unwrap();
        assert_eq!(
            read_key_from_at(&path, NOW_MS).as_deref(),
            Some("rotated-access")
        );
    }

    #[test]
    fn database_rejects_expired_blank_malformed_and_external_credentials() {
        for value in [
            oauth("expired", NOW_MS - 1),
            oauth("expired-at-boundary", NOW_MS),
            oauth("   ", NOW_MS + 1),
            serde_json::json!({"type":"key", "key":"  "}),
            serde_json::json!({"type":"oauth", "access":"no-expiry"}),
            serde_json::json!({"type":"oauth", "access":"bad-expiry", "expires":"later"}),
            serde_json::json!({"type":"external", "key":"not-a-key"}),
            serde_json::json!({"type":"api", "key":"legacy-type-not-v2"}),
            serde_json::json!({"type":"oauth", "expires":NOW_MS + 1}),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let (path, conn) = database(dir.path());
            insert(&conn, "opencode", 1, 1, value);
            assert!(read_key_from_at(&path, NOW_MS).is_none());
            conn.execute("UPDATE credential SET value = 'invalid json'", [])
                .unwrap();
            assert!(read_key_from_at(&path, NOW_MS).is_none());
        }
    }

    #[test]
    fn explicit_paths_are_exclusive_and_do_not_create_missing_database() {
        let dir = tempfile::tempdir().unwrap();
        let (db, conn) = database(dir.path());
        insert(
            &conn,
            "opencode",
            1,
            1,
            serde_json::json!({"type":"key", "key":"database-key"}),
        );
        let legacy = dir.path().join("auth.json");
        assert!(read_key_from(&legacy).is_none());
        assert!(!probe(Some(std::slice::from_ref(&legacy))));
        assert_eq!(
            effective_paths(Some(std::slice::from_ref(&db))).unwrap(),
            vec![db]
        );
        let missing = dir.path().join("missing.db");
        assert!(read_key_from(&missing).is_none());
        assert!(!missing.exists());
    }

    #[test]
    fn explicit_key_and_configured_path_order_still_win() {
        let dir = tempfile::tempdir().unwrap();
        let (db, conn) = database(dir.path());
        insert(
            &conn,
            "opencode",
            1,
            1,
            serde_json::json!({"type":"key", "key":"database-key"}),
        );
        let legacy = write(
            dir.path(),
            "auth.json",
            serde_json::json!({"opencode":{"type":"api", "key":"legacy-key"}}),
        );
        let paths = [legacy, db];
        for (env, inline, expected) in [
            (Some("env-key"), Some("inline-key"), "env-key"),
            (None, Some("inline-key"), "inline-key"),
            (None, None, "legacy-key"),
        ] {
            assert_eq!(
                resolve_from("OPENCODE_GO_API_KEY", env, inline, &paths).unwrap(),
                expected
            );
        }
        assert_eq!(
            resolve_from("OPENCODE_GO_API_KEY", None, None, &paths[1..]).unwrap(),
            "database-key"
        );
    }

    #[test]
    fn database_missing_table_or_active_row_is_absent_and_custom_names_work() {
        let dir = tempfile::tempdir().unwrap();
        let (path, conn) = database(dir.path());
        assert!(read_key_from_at(&path, NOW_MS).is_none());
        insert(
            &conn,
            "opencode",
            0,
            1,
            serde_json::json!({"type":"key", "key":"inactive"}),
        );
        assert!(read_key_from_at(&path, NOW_MS).is_none());
        conn.execute("UPDATE credential SET active = 1", [])
            .unwrap();
        drop(conn);
        let renamed = dir.path().join("credentials.custom");
        std::fs::rename(&path, &renamed).unwrap();
        assert_eq!(
            read_key_from_at(&renamed, NOW_MS).as_deref(),
            Some("inactive")
        );
        let conn = rusqlite::Connection::open(&renamed).unwrap();
        conn.execute("DROP TABLE credential", []).unwrap();
        assert!(read_key_from_at(&renamed, NOW_MS).is_none());
    }

    #[test]
    fn reads_the_current_api_key_shape() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "auth.json",
            serde_json::json!({
                "opencode": {"type": "api", "key": "sk-live-key"}
            }),
        );
        assert_eq!(read_key_from(&path).unwrap(), "sk-live-key");
    }

    #[test]
    fn accepts_the_legacy_zen_type() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "auth.json",
            serde_json::json!({"opencode": {"type": "zen", "key": "zen-key"}}),
        );
        assert_eq!(read_key_from(&path).unwrap(), "zen-key");
    }

    #[test]
    fn oauth_and_wellknown_entries_are_not_api_keys() {
        let dir = tempfile::tempdir().unwrap();
        let oauth = write(
            dir.path(),
            "oauth.json",
            serde_json::json!({"opencode": {"type": "oauth", "token": "tok"}}),
        );
        assert!(read_key_from(&oauth).is_none());
        // A well-known entry keeps the env-var name in `key` and the actual
        // secret in `token`; reading `key` would send the wrong value.
        let wellknown = write(
            dir.path(),
            "wellknown.json",
            serde_json::json!({"opencode": {"type": "wellknown", "key": "OPENAI_API_KEY", "token": "sk-real"}}),
        );
        assert!(read_key_from(&wellknown).is_none());
    }

    #[test]
    fn missing_provider_entry_is_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "auth.json",
            serde_json::json!({"google": {"type": "api", "key": "sk-google"}}),
        );
        assert_eq!(read_key_from(&path), None);
    }

    #[test]
    fn blank_key_or_malformed_file_is_absent() {
        let dir = tempfile::tempdir().unwrap();
        let blank = write(
            dir.path(),
            "blank.json",
            serde_json::json!({"opencode": {"type": "api", "key": "  "}}),
        );
        assert_eq!(read_key_from(&blank), None);

        let malformed = dir.path().join("malformed.json");
        std::fs::write(&malformed, "not json").unwrap();
        assert_eq!(read_key_from(&malformed), None);
        assert_eq!(read_key_from(&dir.path().join("missing.json")), None);
    }

    #[test]
    fn data_dir_prefers_an_absolute_xdg_data_home() {
        // An absolute path is platform-shaped: `C:\…`/UNC on Windows, `/…`
        // elsewhere — `is_absolute` is false for `/…` on Windows.
        #[cfg(windows)]
        let absolute = PathBuf::from(r"C:\custom\data");
        #[cfg(not(windows))]
        let absolute = PathBuf::from("/custom/data");
        let home = PathBuf::from(if cfg!(windows) {
            r"C:\Users\you"
        } else {
            "/home/you"
        });
        assert_eq!(
            data_dir(Some(absolute.clone().into_os_string()), home.clone()),
            absolute
        );
        // Relative values are discarded, per the XDG spec.
        assert_eq!(
            data_dir(Some(OsString::from("relative/data")), home.clone()),
            home.join(".local").join("share")
        );
        assert_eq!(
            data_dir(None, home.clone()),
            home.join(".local").join("share")
        );
    }

    #[test]
    fn effective_paths_override_names_at_least_one_path() {
        let paths = vec![PathBuf::from("/a/auth.json"), PathBuf::from("/b/auth.json")];
        assert_eq!(effective_paths(Some(&paths)).unwrap(), paths);
    }

    #[test]
    fn resolve_env_beats_inline_and_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "auth.json",
            serde_json::json!({"opencode": {"type": "api", "key": "file-key"}}),
        );
        let paths = [path];
        assert_eq!(
            resolve_from(
                "OPENCODE_GO_API_KEY",
                Some("env-key"),
                Some("inline-key"),
                &paths
            )
            .unwrap(),
            "env-key"
        );
        assert_eq!(
            resolve_from("OPENCODE_GO_API_KEY", None, Some("inline-key"), &paths).unwrap(),
            "inline-key"
        );
        assert_eq!(
            resolve_from("OPENCODE_GO_API_KEY", None, None, &paths).unwrap(),
            "file-key"
        );
    }

    #[test]
    fn probe_sees_a_live_key_and_ignores_an_absent_one() {
        let dir = tempfile::tempdir().unwrap();
        let live = write(
            dir.path(),
            "live.json",
            serde_json::json!({"opencode": {"type": "api", "key": "sk-key"}}),
        );
        let empty = write(
            dir.path(),
            "empty.json",
            serde_json::json!({"google": {"type": "api", "key": "sk-key"}}),
        );
        assert!(probe(Some(std::slice::from_ref(&live))));
        assert!(!probe(Some(&[empty])));
        assert!(!probe(Some(&[dir.path().join("missing.json")])));
    }

    #[test]
    fn resolve_failure_names_both_ways_in() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "auth.json",
            serde_json::json!({"google": {"type": "api", "key": "sk-key"}}),
        );
        let paths = [path];
        let error = resolve_from("OPENCODE_GO_API_KEY", None, None, &paths).unwrap_err();
        let message = format!("{error}");
        assert!(message.contains("opencode auth login"), "{message}");
        assert!(message.contains("OPENCODE_GO_API_KEY"), "{message}");
    }
}
