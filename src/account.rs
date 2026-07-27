//! `ai-usagebar account add <label>` — capture the Claude account currently
//! logged into Claude Code and register it as a named `[[anthropic.accounts]]`
//! entry, so `--account <label>` and the TUI's per-account tabs pick it up with
//! no hand-editing of config.toml.
//!
//! Claude Code keeps only one account live at a time (the login Keychain on
//! macOS, `~/.claude/.credentials.json` on Linux). To watch several
//! subscriptions at once you log into each with `claude` → /login and capture
//! it while it is active; ai-usagebar then refreshes that file's token on its
//! own. This is the native version of the capture people were scripting by hand
//! around #14.

use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item, Table, value};

use crate::anthropic::creds;
use crate::cache::atomic_write;
use crate::config;
use crate::error::{AppError, Result};
use crate::widget::cli::AccountAction;

/// Dispatch `ai-usagebar account …` and map the outcome to a process exit code.
/// This is a real subcommand, not the Waybar widget, so a failure exits non-zero
/// (the widget's always-exit-0 rule does not apply here).
#[must_use]
pub fn run(action: &AccountAction) -> i32 {
    let result = match action {
        AccountAction::Add { label } => add(label),
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("ai-usagebar account: {e}");
            1
        }
    }
}

/// Capture the active Claude account and register it under `label`.
fn add(label: &str) -> Result<()> {
    validate_label(label)?;
    let json = capture_active_credentials()?;
    let creds_path = save_account_file(&accounts_dir()?, label, &json)?;
    let config_path = config::default_path()
        .ok_or_else(|| AppError::Other("cannot resolve config path".into()))?;
    let added = register_in_config(&config_path, label, &creds_path)?;

    println!("Saved \"{label}\" credentials → {}", creds_path.display());
    if added {
        println!("Registered \"{label}\" in {}", config_path.display());
    } else {
        println!(
            "\"{label}\" was already registered in {}",
            config_path.display()
        );
    }
    println!("See it with: ai-usagebar --vendor anthropic --account {label}");
    Ok(())
}

/// Labels name a CLI selector (`--account <label>`), a cache subdir, and a file,
/// so keep them to a filesystem- and shell-safe set.
fn validate_label(label: &str) -> Result<()> {
    let ok = !label.is_empty()
        && label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if ok {
        return Ok(());
    }
    Err(AppError::Other(format!(
        "label {label:?} must be non-empty and match [A-Za-z0-9._-] (e.g. personal, work)"
    )))
}

/// The JSON blob for the account currently logged into Claude Code: the login
/// Keychain item on macOS, else `~/.claude/.credentials.json`.
fn capture_active_credentials() -> Result<String> {
    #[cfg(target_os = "macos")]
    if let Some(json) = crate::anthropic::keychain::read_raw()? {
        return validated(json);
    }
    let path = creds::default_path()?;
    let json = std::fs::read_to_string(&path).map_err(|e| {
        AppError::Other(format!(
            "no active Claude account (Keychain empty and {} unreadable: {e}); \
             run `claude` then /login first",
            path.display()
        ))
    })?;
    validated(json)
}

/// A logged-in account always carries a `refreshToken`; its absence means the
/// blob is a stale/empty shell ai-usagebar could not keep alive, so reject it
/// rather than register a dead account.
fn validated(json: String) -> Result<String> {
    if json.contains("refreshToken") {
        return Ok(json);
    }
    Err(AppError::Other(
        "the active credentials have no refreshToken — is an account logged in?".into(),
    ))
}

/// Captured credential files live in an `accounts/` dir beside config.toml, so
/// everything ai-usagebar manages sits under one config root.
fn accounts_dir() -> Result<PathBuf> {
    let config_path = config::default_path()
        .ok_or_else(|| AppError::Other("cannot resolve config path".into()))?;
    let parent = config_path
        .parent()
        .ok_or_else(|| AppError::Other(format!("config path {config_path:?} has no parent")))?;
    Ok(parent.join("accounts"))
}

/// Write the credential blob to `<dir>/<label>.json` with owner-only perms.
fn save_account_file(dir: &Path, label: &str, json: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).map_err(|e| AppError::io_at(dir, e))?;
    restrict(dir, 0o700);
    let path = dir.join(format!("{label}.json"));
    atomic_write(&path, json.as_bytes())?;
    restrict(&path, 0o600);
    Ok(path)
}

/// Append `[[anthropic.accounts]]` for `label` → `creds_path`, preserving the
/// file's existing comments and keys (toml_edit). Idempotent: returns `false`
/// without writing when the label is already registered. Ensures `[anthropic]`
/// stays enabled so the captured account actually shows up.
fn register_in_config(config_path: &Path, label: &str, creds_path: &Path) -> Result<bool> {
    let existing = std::fs::read_to_string(config_path).unwrap_or_default();
    let mut doc: DocumentMut = existing
        .parse()
        .map_err(|e| AppError::Other(format!("config.toml is not valid TOML: {e}")))?;
    if account_exists(&doc, label) {
        return Ok(false);
    }
    let anthropic = doc
        .entry("anthropic")
        .or_insert_with(toml_edit::table)
        .as_table_mut()
        .ok_or_else(|| AppError::Other("config.toml: [anthropic] is not a table".into()))?;
    anthropic.entry("enabled").or_insert(value(true));
    let accounts = anthropic
        .entry("accounts")
        .or_insert(Item::ArrayOfTables(toml_edit::ArrayOfTables::new()))
        .as_array_of_tables_mut()
        .ok_or_else(|| {
            AppError::Other("config.toml: [[anthropic.accounts]] is not an array of tables".into())
        })?;
    let mut entry = Table::new();
    entry["label"] = value(label);
    entry["credentials_path"] = value(creds_path.to_string_lossy().into_owned());
    accounts.push(entry);
    atomic_write(config_path, doc.to_string().as_bytes())?;
    Ok(true)
}

/// Whether `[[anthropic.accounts]]` already carries an entry with this label.
fn account_exists(doc: &DocumentMut, label: &str) -> bool {
    doc.get("anthropic")
        .and_then(|a| a.get("accounts"))
        .and_then(Item::as_array_of_tables)
        .is_some_and(|arr| {
            arr.iter()
                .any(|t| t.get("label").and_then(|v| v.as_str()) == Some(label))
        })
}

#[cfg(unix)]
fn restrict(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn restrict(_path: &Path, _mode: u32) {}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn validate_label_accepts_safe_names() {
        for label in ["personal", "work", "acct.1", "a_b-c"] {
            assert!(validate_label(label).is_ok(), "{label} should be valid");
        }
    }

    #[test]
    fn validate_label_rejects_empty_and_unsafe() {
        for label in ["", "has space", "slash/x", "quote\"x"] {
            assert!(
                validate_label(label).is_err(),
                "{label:?} should be rejected"
            );
        }
    }

    #[test]
    fn validated_requires_refresh_token() {
        assert!(validated(r#"{"refreshToken":"x"}"#.into()).is_ok());
        assert!(validated(r#"{"accessToken":"x"}"#.into()).is_err());
    }

    #[test]
    fn register_appends_entry_and_keeps_existing() {
        let tmp = TempDir::new().unwrap();
        let cfg = tmp.path().join("config.toml");
        std::fs::write(&cfg, "# my config\n[ui]\nprimary = \"anthropic\"\n").unwrap();

        let added = register_in_config(&cfg, "work", Path::new("/creds/work.json")).unwrap();
        assert!(added);

        let written = std::fs::read_to_string(&cfg).unwrap();
        // Existing comment + section survive.
        assert!(written.contains("# my config"));
        assert!(written.contains("primary = \"anthropic\""));
        // Our entry is added as an array-of-tables.
        assert!(written.contains("[[anthropic.accounts]]"));
        assert!(written.contains("label = \"work\""));
        assert!(written.contains("credentials_path = \"/creds/work.json\""));
    }

    #[test]
    fn register_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let cfg = tmp.path().join("config.toml");

        assert!(register_in_config(&cfg, "work", Path::new("/creds/work.json")).unwrap());
        // Second call with the same label is a no-op.
        assert!(!register_in_config(&cfg, "work", Path::new("/creds/other.json")).unwrap());

        let written = std::fs::read_to_string(&cfg).unwrap();
        assert_eq!(written.matches("label = \"work\"").count(), 1);
        // The no-op must not have rewritten the path.
        assert!(!written.contains("/creds/other.json"));
    }

    #[test]
    fn save_account_file_writes_blob() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("accounts");
        let path = save_account_file(&dir, "work", r#"{"refreshToken":"x"}"#).unwrap();
        assert_eq!(path, dir.join("work.json"));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            r#"{"refreshToken":"x"}"#
        );
    }
}
