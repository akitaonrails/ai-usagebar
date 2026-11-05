//! Capturing a **new** Claude Desktop account, so this machine can switch to
//! it later.
//!
//! Unlike the `claude` CLI — where `CLAUDE_CONFIG_DIR=<dir> claude` isolates a
//! login into its own directory — the Desktop app has exactly one login slot
//! and no way to ask for a second. The only way to obtain a second account's
//! credential is to sign the app out, have the user sign in as that account,
//! and save what the app then writes. That is what this does, and why it is
//! interactive and destructive-looking in the middle.
//!
//! It is safe to cancel. The live login is copied out **before** anything is
//! cleared, and put straight back if the sign-in times out or the user walks
//! away. The account that was active is also saved into its own profile first,
//! so switching back to it afterwards restores exactly what it looked like.
//!
//! Ported from claude-acc's `add` (<https://github.com/ohmaseclaro/claude-acc>),
//! along with the two empirical rules that make the polling reliable: a login
//! only counts once *both* `lastKnownAccountUuid` and the token cache are
//! written, and the org id has to be recovered from either the new session
//! folder or the dxt allowlist keys.

use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::app::AppControl;
use super::{Paths, merge};
use crate::display::sanitize_untrusted_path;
use crate::error::{AppError, Result};

/// Everything the app remembers an account by. Backed up before a capture
/// clears it, restored verbatim if the capture does not complete.
const LOGIN_STATE_FILES: [&str; 4] = [
    "config.json",
    "Cookies",
    "Cookies-journal",
    "bridge-state.json",
];
const LOGIN_STATE_DIRS: [&str; 3] = ["Local Storage", "Session Storage", "IndexedDB"];

static CAPTURE_CANCELLED: AtomicBool = AtomicBool::new(false);
static CANCEL_HANDLER: OnceLock<std::result::Result<(), String>> = OnceLock::new();

/// How long to wait for each interactive step.
#[derive(Debug, Clone, Copy)]
pub struct WaitOpts {
    pub login: Duration,
    pub org: Duration,
    pub poll: Duration,
}

impl Default for WaitOpts {
    fn default() -> Self {
        Self {
            login: Duration::from_secs(300),
            org: Duration::from_secs(120),
            poll: Duration::from_secs(2),
        }
    }
}

#[derive(Debug)]
pub struct Captured {
    pub label: String,
    pub account_uuid: String,
    /// Absent when the app had not created a session folder yet; history is
    /// then seeded on the first switch instead.
    pub org_uuid: Option<String>,
    pub seeded_sessions: usize,
    pub seeded_routines: usize,
}

#[derive(Debug)]
pub enum CaptureOutcome {
    Captured(Box<Captured>),
    /// The account signed in is one this machine already knows. Nothing was
    /// saved and the app is left signed into it.
    AlreadySaved(String),
    /// Nobody signed in within the timeout; the previous login was restored.
    TimedOut,
    /// Ctrl-C was pressed while waiting; the previous login was restored.
    Cancelled,
}

/// Sign the Desktop app out, wait for the user to sign in as a new account,
/// and save it under `label`.
pub fn capture_profile(
    paths: &Paths,
    label: &str,
    email: Option<&str>,
    app: &dyn AppControl,
    wait: WaitOpts,
    notes: &mut Vec<String>,
) -> Result<CaptureOutcome> {
    crate::config::validate_account_label(label)?;
    install_cancel_handler()?;
    CAPTURE_CANCELLED.store(false, Ordering::SeqCst);
    let _lock =
        crate::cache::acquire_lock(&paths.account_switch_lock(), super::ACCOUNT_LOCK_TIMEOUT)?;
    let profiles = super::load_profiles(&paths.profiles_dir);
    if profiles.iter().any(|profile| profile.label == label) {
        return Err(AppError::Credentials(format!(
            "a Claude Desktop account {label:?} already exists in {}; pick another name",
            sanitize_untrusted_path(&paths.profiles_dir)
        )));
    }
    let known_accounts: Vec<String> = profiles.iter().map(|p| p.account_uuid.clone()).collect();
    let mut known_orgs: Vec<String> = profiles.iter().filter_map(|p| p.org_uuid.clone()).collect();

    let config_json = paths.config_json();
    if let Ok(bytes) = std::fs::read(&config_json) {
        known_orgs.extend(merge::orgs_in_config(&bytes));
        known_orgs.sort();
        known_orgs.dedup();
    }
    let outgoing = super::active_account_uuid(&config_json)
        .and_then(|uuid| super::label_for_uuid(&profiles, &uuid).map(str::to_string));

    app.quit()?;
    // Save the account we are about to sign out, so switching back to it later
    // restores its browser state rather than only its credential.
    if let Some(previous) = &outgoing
        && let Err(error) = super::snapshot_profile(paths, previous, notes)
    {
        return Err(relaunch_after_error(app, error));
    }
    // The safety net runs regardless: the live login may belong to no profile
    // at all, and it still has to survive a cancelled capture.
    if let Err(error) = backup_login_state(paths, notes) {
        return Err(relaunch_after_error(app, error));
    }
    if let Err(error) = clear_login_state(paths) {
        return Err(restore_after_error(paths, app, error));
    }
    if let Err(error) = app.relaunch() {
        return Err(restore_after_error(paths, app, error));
    }

    let Some(account_uuid) = poll_for_login(&config_json, wait) else {
        app.quit()?;
        restore_login_state(paths)?;
        app.relaunch()?;
        return Ok(if CAPTURE_CANCELLED.load(Ordering::SeqCst) {
            CaptureOutcome::Cancelled
        } else {
            CaptureOutcome::TimedOut
        });
    };
    if let Some(existing) = super::label_for_uuid(&profiles, &account_uuid) {
        return Ok(CaptureOutcome::AlreadySaved(existing.to_string()));
    }
    if known_accounts.iter().any(|known| known == &account_uuid) {
        return Ok(CaptureOutcome::AlreadySaved(account_uuid));
    }

    let org_uuid = poll_for_org(paths, &account_uuid, &known_orgs, wait);
    if CAPTURE_CANCELLED.load(Ordering::SeqCst) {
        app.quit()?;
        restore_login_state(paths)?;
        app.relaunch()?;
        return Ok(CaptureOutcome::Cancelled);
    }
    app.quit()?;
    let capture_result = (|| {
        super::snapshot_profile(paths, label, notes)?;
        write_meta(paths, label, email, &account_uuid, org_uuid.as_deref())?;

        // Seed the new account with everything this machine already has, so its
        // first login is not an empty sidebar.
        let seeded = match &org_uuid {
            Some(org) => super::merge_history_into(paths, &account_uuid, org, notes),
            None => {
                notes.push(
                    "no organisation recorded yet, so history was not seeded — open one chat in \
                     the app, then run `ai-usagebar account switch <label> --desktop` to pull it in"
                        .into(),
                );
                (0, 0)
            }
        };
        Ok(seeded)
    })();

    let (seeded_sessions, seeded_routines) = match capture_result {
        Ok(seeded) => seeded,
        Err(error) => {
            let partial = paths.profile_dir(label);
            let cleanup = super::remove_if_present(&partial);
            let mut error = restore_after_error(paths, app, error);
            if let Err(cleanup) = cleanup {
                error = AppError::Other(format!(
                    "{error}; could not remove the partial profile {}: {cleanup}",
                    sanitize_untrusted_path(&partial)
                ));
            }
            return Err(error);
        }
    };
    if let Err(error) = app.relaunch() {
        notes.push(format!(
            "account captured, but Claude Desktop could not be relaunched: {error}"
        ));
    }

    Ok(CaptureOutcome::Captured(Box::new(Captured {
        label: label.to_string(),
        account_uuid,
        org_uuid,
        seeded_sessions,
        seeded_routines,
    })))
}

fn install_cancel_handler() -> Result<()> {
    let installed = CANCEL_HANDLER.get_or_init(|| {
        ctrlc::set_handler(|| CAPTURE_CANCELLED.store(true, Ordering::SeqCst))
            .map_err(|error| error.to_string())
    });
    match installed {
        Ok(()) => Ok(()),
        Err(error) => Err(AppError::Other(format!(
            "could not install the capture cancel handler: {error}"
        ))),
    }
}

fn relaunch_after_error(app: &dyn AppControl, error: AppError) -> AppError {
    match app.relaunch() {
        Ok(()) => error,
        Err(relaunch) => AppError::Other(format!(
            "{error}; Claude Desktop also could not be relaunched: {relaunch}"
        )),
    }
}

/// The app is stopped and the live login may have been cleared or replaced.
/// Put the pre-login state back before attempting to reopen it.
fn restore_after_error(paths: &Paths, app: &dyn AppControl, error: AppError) -> AppError {
    let restore = restore_login_state(paths);
    let relaunch = app.relaunch();
    match (restore, relaunch) {
        (Ok(()), Ok(())) => error,
        (Err(restore), Ok(())) => AppError::Other(format!(
            "{error}; automatic pre-login restore was incomplete: {restore}"
        )),
        (Ok(()), Err(relaunch)) => AppError::Other(format!(
            "{error}; the previous login was restored, but Claude Desktop could not be relaunched: {relaunch}"
        )),
        (Err(restore), Err(relaunch)) => AppError::Other(format!(
            "{error}; automatic pre-login restore was incomplete: {restore}; Claude Desktop could not be relaunched: {relaunch}"
        )),
    }
}

fn poll_for_login(config_json: &Path, wait: WaitOpts) -> Option<String> {
    let deadline = Instant::now() + wait.login;
    while Instant::now() < deadline {
        if CAPTURE_CANCELLED.load(Ordering::SeqCst) {
            return None;
        }
        if let Ok(bytes) = std::fs::read(config_json)
            && let Some(uuid) = merge::logged_in_account(&bytes)
        {
            return Some(uuid);
        }
        std::thread::sleep(wait.poll);
    }
    None
}

/// The account's own session folder is the reliable signal; the config's dxt
/// allowlist keys are the fallback for an account that has not opened a chat.
fn poll_for_org(
    paths: &Paths,
    account_uuid: &str,
    known_orgs: &[String],
    wait: WaitOpts,
) -> Option<String> {
    let deadline = Instant::now() + wait.org;
    let account_dir = paths.sessions_root().join(account_uuid);
    while Instant::now() < deadline {
        if CAPTURE_CANCELLED.load(Ordering::SeqCst) {
            return None;
        }
        if let Ok(entries) = std::fs::read_dir(&account_dir)
            && let Some(org) = entries
                .flatten()
                .filter(|entry| entry.path().is_dir())
                .find_map(|entry| entry.file_name().to_str().map(str::to_string))
        {
            return Some(org);
        }
        if let Ok(bytes) = std::fs::read(paths.config_json())
            && let Some(org) = merge::new_org_in_config(&bytes, known_orgs)
        {
            return Some(org);
        }
        std::thread::sleep(wait.poll);
    }
    None
}

/// The one place that writes `meta.json`. A switch must never touch it — this
/// is the file that says which account a profile *is*.
fn write_meta(
    paths: &Paths,
    label: &str,
    email: Option<&str>,
    account_uuid: &str,
    org_uuid: Option<&str>,
) -> Result<()> {
    let meta = serde_json::json!({
        "label": label,
        "email": email,
        "accountUuid": account_uuid,
        "orgUuid": org_uuid,
        "savedAt": chrono::Local::now().timestamp(),
    });
    crate::cache::atomic_write(
        &paths.profile_dir(label).join(super::META_JSON),
        &serde_json::to_vec_pretty(&meta)?,
    )
}

fn backup_login_state(paths: &Paths, notes: &mut Vec<String>) -> Result<()> {
    let backup = paths.prelogin_dir();
    let parent = backup.parent().ok_or_else(|| {
        AppError::Other(format!(
            "pre-login backup has no parent: {}",
            sanitize_untrusted_path(&backup)
        ))
    })?;
    std::fs::create_dir_all(parent).map_err(|e| AppError::io_at(parent, e))?;
    let staged = tempfile::Builder::new()
        .prefix(".prelogin-backup.pending-")
        .tempdir_in(parent)
        .map_err(|e| AppError::io_at(parent, e))?;
    super::restrict(staged.path(), 0o700, notes);
    for name in LOGIN_STATE_FILES {
        let source = paths.data_dir.join(name);
        if source.is_file() {
            super::copy_file(&source, &staged.path().join(name))?;
        }
    }
    for name in LOGIN_STATE_DIRS {
        let source = paths.data_dir.join(name);
        if source.is_dir() {
            super::copy_dir(&source, &staged.path().join(name))?;
        }
    }

    let previous = backup.with_file_name("prelogin-backup.previous");
    if previous.exists() && !backup.exists() {
        std::fs::rename(&previous, &backup).map_err(|e| AppError::io_at(&previous, e))?;
    } else {
        super::remove_if_present(&previous)?;
    }
    let staged = staged.keep();
    if backup.exists() {
        std::fs::rename(&backup, &previous).map_err(|e| AppError::io_at(&backup, e))?;
    }
    if let Err(error) = std::fs::rename(&staged, &backup) {
        let restore = if previous.exists() {
            std::fs::rename(&previous, &backup)
        } else {
            Ok(())
        };
        let _ = super::remove_if_present(&staged);
        return match restore {
            Ok(()) => Err(AppError::io_at(&staged, error)),
            Err(rollback) => Err(AppError::Other(format!(
                "could not install the pre-login backup: {error}; could not restore the previous backup: {rollback}"
            ))),
        };
    }
    if let Err(error) = super::remove_if_present(&previous) {
        notes.push(format!(
            "could not remove the previous pre-login backup: {error}"
        ));
    }
    Ok(())
}

fn restore_login_state(paths: &Paths) -> Result<()> {
    let backup = paths.prelogin_dir();
    if !backup.is_dir() {
        return Err(AppError::Other(format!(
            "no pre-login backup at {} to restore",
            sanitize_untrusted_path(&backup)
        )));
    }
    for name in LOGIN_STATE_FILES {
        let source = backup.join(name);
        let live = paths.data_dir.join(name);
        if source.is_file() {
            let bytes = std::fs::read(&source).map_err(|e| AppError::io_at(&source, e))?;
            crate::cache::atomic_write(&live, &bytes)?;
        } else {
            super::remove_if_present(&live)?;
        }
    }
    for name in LOGIN_STATE_DIRS {
        let source = backup.join(name);
        let live = paths.data_dir.join(name);
        if source.is_dir() {
            super::replace_dir(&source, &live)?;
        } else {
            super::remove_if_present(&live)?;
        }
    }
    Ok(())
}

fn clear_login_state(paths: &Paths) -> Result<()> {
    let config_json = paths.config_json();
    let bytes = std::fs::read(&config_json).map_err(|e| AppError::io_at(&config_json, e))?;
    let cleared = merge::clear_config_tokens(&bytes)?;
    crate::cache::atomic_write(&config_json, &cleared)?;
    for name in LOGIN_STATE_FILES
        .iter()
        .filter(|name| **name != "config.json")
    {
        super::remove_if_present(&paths.data_dir.join(name))?;
    }
    for name in LOGIN_STATE_DIRS {
        super::remove_if_present(&paths.data_dir.join(name))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude_desktop::app::Recorder;
    use std::cell::{Cell, RefCell};

    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    fn fixture() -> (tempfile::TempDir, Paths) {
        let root = tempfile::TempDir::new().unwrap();
        let data = root.path().join("data");
        write(
            &data.join("config.json"),
            r#"{"lastKnownAccountUuid":"uuid-here","oauth:tokenCache":"live-a",
                "oauth:tokenCacheV2":"live-b","autoUpdates":true}"#,
        );
        write(&data.join("Cookies"), "live-cookies");
        write(
            &data.join("bridge-state.json"),
            r#"{"remoteSessionId":"cse"}"#,
        );
        write(&data.join("Local Storage/leveldb/CURRENT"), "live-ldb");
        let paths = Paths::at(
            data,
            root.path().join("profiles"),
            root.path().join("backups"),
        );
        (root, paths)
    }

    /// The safety net: whatever a capture clears has to come back byte for byte.
    #[test]
    fn a_cleared_login_is_restored_exactly() {
        let (_root, paths) = fixture();
        let mut notes = Vec::new();

        backup_login_state(&paths, &mut notes).unwrap();
        clear_login_state(&paths).unwrap();

        let cleared: serde_json::Value =
            serde_json::from_slice(&std::fs::read(paths.config_json()).unwrap()).unwrap();
        assert!(cleared.get("oauth:tokenCacheV2").is_none());
        assert!(cleared.get("lastKnownAccountUuid").is_none());
        assert_eq!(cleared["autoUpdates"], true, "settings must survive");
        assert!(!paths.data_dir.join("Cookies").exists());
        assert!(!paths.data_dir.join("Local Storage").exists());

        restore_login_state(&paths).unwrap();

        assert!(notes.is_empty(), "{notes:?}");
        let restored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(paths.config_json()).unwrap()).unwrap();
        assert_eq!(restored["oauth:tokenCacheV2"], "live-b");
        assert_eq!(restored["lastKnownAccountUuid"], "uuid-here");
        assert_eq!(
            std::fs::read_to_string(paths.data_dir.join("Cookies")).unwrap(),
            "live-cookies"
        );
        assert_eq!(
            std::fs::read_to_string(paths.data_dir.join("Local Storage/leveldb/CURRENT")).unwrap(),
            "live-ldb"
        );
    }

    /// A file the app created *after* the backup must not survive the restore,
    /// or the new account's cookies would leak into the old one's session.
    #[test]
    fn restoring_clears_state_the_backup_does_not_have() {
        let (_root, paths) = fixture();
        let mut notes = Vec::new();
        std::fs::remove_file(paths.data_dir.join("Cookies")).unwrap();

        backup_login_state(&paths, &mut notes).unwrap();
        write(&paths.data_dir.join("Cookies"), "someone else's");
        restore_login_state(&paths).unwrap();

        assert!(!paths.data_dir.join("Cookies").exists(), "{notes:?}");
    }

    #[test]
    fn restoring_clears_directories_the_backup_does_not_have() {
        let (_root, paths) = fixture();
        let mut notes = Vec::new();
        std::fs::remove_dir_all(paths.data_dir.join("Local Storage")).unwrap();

        backup_login_state(&paths, &mut notes).unwrap();
        write(
            &paths.data_dir.join("Local Storage/leveldb/CURRENT"),
            "someone else's",
        );
        restore_login_state(&paths).unwrap();

        assert!(!paths.data_dir.join("Local Storage").exists(), "{notes:?}");
    }

    #[test]
    fn a_timed_out_capture_puts_the_previous_login_back() {
        let (_root, paths) = fixture();
        let recorder = Recorder::default();
        let mut notes = Vec::new();
        let before = std::fs::read(paths.config_json()).unwrap();

        let outcome = capture_profile(
            &paths,
            "work",
            None,
            &recorder,
            WaitOpts {
                login: Duration::from_millis(1),
                org: Duration::from_millis(1),
                poll: Duration::from_millis(1),
            },
            &mut notes,
        )
        .unwrap();

        assert!(matches!(outcome, CaptureOutcome::TimedOut), "{outcome:?}");
        assert_eq!(std::fs::read(paths.config_json()).unwrap(), before);
        assert!(!paths.profile_dir("work").exists(), "nothing was saved");
        // Quit, reopen at the login screen, quit again, reopen restored.
        assert_eq!(recorder.steps(), ["quit", "relaunch", "quit", "relaunch"]);
    }

    #[test]
    fn a_clear_failure_after_quit_restores_and_relaunches() {
        let (_root, paths) = fixture();
        write(&paths.config_json(), "{ not json");
        let recorder = Recorder::default();

        let error = capture_profile(
            &paths,
            "work",
            None,
            &recorder,
            WaitOpts::default(),
            &mut Vec::new(),
        )
        .unwrap_err();

        assert!(error.to_string().contains("json"), "{error}");
        assert_eq!(recorder.steps(), ["quit", "relaunch"]);
        assert_eq!(
            std::fs::read_to_string(paths.config_json()).unwrap(),
            "{ not json"
        );
    }

    struct IncompleteLogin {
        config_json: std::path::PathBuf,
        relaunches: Cell<usize>,
        steps: RefCell<Vec<String>>,
    }

    impl IncompleteLogin {
        fn new(config_json: std::path::PathBuf) -> Self {
            Self {
                config_json,
                relaunches: Cell::new(0),
                steps: RefCell::new(Vec::new()),
            }
        }
    }

    impl AppControl for IncompleteLogin {
        fn quit(&self) -> Result<()> {
            self.steps.borrow_mut().push("quit".into());
            if self.relaunches.get() == 1 {
                write(
                    &self.config_json,
                    // A config the app rewrote *without* a usable credential:
                    // the V2 blob is present but empty, and 4.16+ writes no
                    // legacy blob at all (#573). Snapshotting this must fail
                    // and roll the partial profile back.
                    r#"{"lastKnownAccountUuid":"uuid-new","oauth:tokenCacheV2":""}"#,
                );
            }
            Ok(())
        }

        fn relaunch(&self) -> Result<()> {
            self.steps.borrow_mut().push("relaunch".into());
            let count = self.relaunches.get();
            self.relaunches.set(count + 1);
            if count == 0 {
                write(
                    &self.config_json,
                    r#"{"lastKnownAccountUuid":"uuid-new","oauth:tokenCache":"new-a","oauth:tokenCacheV2":"new-b"}"#,
                );
            }
            Ok(())
        }

        fn archive(&self, _archive: &Path, _root: &Path, _members: &[&str]) -> Result<()> {
            unreachable!()
        }

        fn restore(&self, _archive: &Path, _root: &Path, _cleanup_members: &[&str]) -> Result<()> {
            unreachable!()
        }
    }

    #[test]
    fn a_post_login_capture_failure_removes_the_partial_profile_and_restores_the_previous_login() {
        let (_root, paths) = fixture();
        let before = std::fs::read(paths.config_json()).unwrap();
        let app = IncompleteLogin::new(paths.config_json());

        let error = capture_profile(
            &paths,
            "work",
            None,
            &app,
            WaitOpts {
                login: Duration::from_millis(10),
                org: Duration::from_millis(1),
                poll: Duration::from_millis(1),
            },
            &mut Vec::new(),
        )
        .unwrap_err();

        assert!(error.to_string().contains("OAuth token cache"), "{error}");
        assert_eq!(std::fs::read(paths.config_json()).unwrap(), before);
        assert!(!paths.profile_dir("work").exists());
        assert_eq!(
            app.steps.borrow().as_slice(),
            ["quit", "relaunch", "quit", "relaunch"]
        );
    }

    #[test]
    fn capturing_a_label_that_already_exists_is_refused_before_signing_out() {
        let (_root, paths) = fixture();
        write(
            &paths.profile_dir("work").join("meta.json"),
            r#"{"accountUuid":"uuid-work"}"#,
        );
        let recorder = Recorder::default();
        let before = std::fs::read(paths.config_json()).unwrap();

        let error = capture_profile(
            &paths,
            "work",
            None,
            &recorder,
            WaitOpts::default(),
            &mut Vec::new(),
        )
        .unwrap_err();

        assert!(error.to_string().contains("already exists"), "{error}");
        assert!(recorder.steps().is_empty(), "the app must not be touched");
        assert_eq!(std::fs::read(paths.config_json()).unwrap(), before);
    }

    #[test]
    fn an_unusable_label_is_refused() {
        let (_root, paths) = fixture();
        let recorder = Recorder::default();
        assert!(
            capture_profile(
                &paths,
                "../escape",
                None,
                &recorder,
                WaitOpts::default(),
                &mut Vec::new()
            )
            .is_err()
        );
        assert!(recorder.steps().is_empty());
    }
}
