//! Which Codex account `~/.codex/auth.json` holds, and moving that login
//! between managed accounts.
//!
//! Codex keeps one live login per `CODEX_HOME`, and the Codex CLI, the Codex
//! desktop app and the IDE extension read the same default one,
//! `~/.codex/auth.json`, unless they are configured otherwise. A named account
//! (`[[openai.accounts]]`) is another `auth.json`, usually made with
//! `CODEX_HOME=~/.codex-<label> codex login`. Making a named account "the one
//! Codex uses" therefore means moving its file into the default slot.
//!
//! Copying would leave two files holding the same *rotating* refresh token, and
//! whichever client refreshed first would invalidate the other. The switch
//! moves instead, in the same order as the Claude CLI switch in
//! [`crate::anthropic::cli_account`]: it saves the outgoing default login back
//! into its own account first, installs the target in the default slot, and
//! only then removes the target's named copy. [`OpenAiConfig::fetch_auth_path`]
//! routes reads for whichever label is *currently* active to the default slot
//! while its own file is gone.
//!
//! A moved-away account has no file left to identify it, so every account also
//! gets a marker next to its `auth.json` — one per credential file, never
//! shared — recording the ChatGPT account id it belongs to: the counterpart of
//! the `oauthAccount` block Claude Code keeps in `.claude.json`. It never holds
//! a token.
//!
//! Every credential directory a switch touches is locked for the whole move,
//! and ai-usagebar's own token refresh ([`crate::openai::fetch`]) takes the
//! same lock around its read–refresh–write, so the two cannot interleave. A
//! Codex process is outside that lock: before it refreshes it reloads
//! `auth.json` and skips the refresh when the account on disk changed, but a
//! refresh already in flight when the switch lands can still persist the old
//! account's tokens. Restarting open Codex sessions after a switch avoids that.
//!
//! [`OpenAiConfig::fetch_auth_path`]: crate::config::OpenAiConfig::fetch_auth_path

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use crate::cache::LockGuard;
use crate::config::OpenAiAccount;
use crate::display::sanitize_untrusted_path;
use crate::error::{AppError, Result};

/// Suffix of the identity marker kept beside each named account's credential
/// file: `auth.json` gets `.auth.json.ai-usagebar-account.json`.
const MARKER_SUFFIX: &str = ".ai-usagebar-account.json";

const LOCK_FILE: &str = ".ai-usagebar-codex-credentials.lock";

/// Long enough to wait out a token refresh holding the same lock.
const LOCK_TIMEOUT: Duration = Duration::from_secs(30);

/// The lock guarding the credential files in `auth_path`'s directory. Shared
/// by the switch, the adoption and the token refresh.
pub fn lock_path(auth_path: &Path) -> PathBuf {
    parent_of(auth_path).join(LOCK_FILE)
}

/// The ChatGPT account id an `auth.json` belongs to: `tokens.account_id`, or
/// the id token's `chatgpt_account_id` claim when an older file lacks it.
pub fn account_id_in(auth_path: &Path) -> Option<String> {
    let raw = std::fs::read(auth_path).ok()?;
    account_id_of(&raw)
}

fn account_id_of(raw: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(raw).ok()?;
    let tokens = value.get("tokens")?;
    if let Some(id) = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    {
        return Some(id.to_string());
    }
    let claims = crate::jwt::claims(tokens.get("id_token")?.as_str()?)?;
    claims
        .get("https://api.openai.com/auth")?
        .get("chatgpt_account_id")?
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// Where `account`'s identity marker lives: beside its credential file, named
/// after it, so two accounts sharing a directory never share a marker.
pub fn marker_path(account: &OpenAiAccount) -> PathBuf {
    let name = account
        .codex_auth_path
        .file_name()
        .map_or_else(|| "auth.json".into(), |name| name.to_string_lossy());
    parent_of(&account.codex_auth_path).join(format!(".{name}{MARKER_SUFFIX}"))
}

fn marker_account_id(account: &OpenAiAccount) -> Option<String> {
    let raw = std::fs::read(marker_path(account)).ok()?;
    let value: Value = serde_json::from_slice(&raw).ok()?;
    value
        .get("account_id")?
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

fn marker_bytes(account_id: &str) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec_pretty(
        &json!({ "account_id": account_id }),
    )?)
}

/// Who `account` is: its own file when it still has one, else its marker.
pub fn identity(account: &OpenAiAccount) -> Option<String> {
    account_id_in(&account.codex_auth_path).or_else(|| marker_account_id(account))
}

/// Which managed account the default `auth.json` belongs to, if any. `None`
/// as well when two accounts claim that identity, since either answer could
/// be wrong.
pub fn resolve_active_label(default_path: &Path, accounts: &[OpenAiAccount]) -> Option<String> {
    let live = account_id_in(default_path)?;
    let mut owners = accounts
        .iter()
        .filter(|account| identity(account).as_deref() == Some(live.as_str()));
    let owner = owners.next()?;
    if owners.next().is_some() {
        return None;
    }
    Some(owner.label.clone())
}

/// Whether `account` has a login anywhere: its own file, or the default slot
/// while it is the active account there. A marker alone is not a login.
pub fn has_login(default_path: &Path, accounts: &[OpenAiAccount], account: &OpenAiAccount) -> bool {
    account_id_in(&account.codex_auth_path).is_some()
        || resolve_active_label(default_path, accounts).as_deref() == Some(account.label.as_str())
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SwitchOpts {
    /// Overwrite the default slot even though the live login belongs to no
    /// managed account. That login cannot be saved anywhere first, so this
    /// genuinely discards it.
    pub force: bool,
    /// Validate everything and report, without writing anything — not even a
    /// lock file.
    pub dry_run: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SwitchOutcome {
    AlreadyActive,
    /// `outgoing` is the label whose login was saved back first, when there
    /// was one.
    Switched {
        outgoing: Option<String>,
    },
    /// `dry_run` was set; nothing was written.
    WouldSwitch {
        outgoing: Option<String>,
    },
}

/// Everything a switch will do, decided from a read-only look at the files.
struct Plan<'a> {
    target: &'a OpenAiAccount,
    target_blob: Vec<u8>,
    target_id: String,
    outgoing: Option<&'a OpenAiAccount>,
    outgoing_label: Option<String>,
    default_blob: Option<Vec<u8>>,
}

enum Decision<'a> {
    AlreadyActive,
    Move(Plan<'a>),
}

/// Make `label` the account the Codex CLI, app and IDE extension use.
///
/// The layout is validated before anything is written: every credential path
/// distinct and not a symlink, no identity claimed by two accounts, no active
/// account that also kept its own copy. The move then runs with every
/// directory it touches locked. The outgoing login is saved back **before**
/// anything is overwritten, the target is written to the default slot
/// **before** its named copy is removed, and a failure restores every file it
/// can, reporting any it could not.
pub fn switch_account(
    default_path: &Path,
    accounts: &[OpenAiAccount],
    label: &str,
    opts: SwitchOpts,
) -> Result<SwitchOutcome> {
    if opts.dry_run {
        return Ok(match decide(default_path, accounts, label, opts.force)? {
            Decision::AlreadyActive => SwitchOutcome::AlreadyActive,
            Decision::Move(plan) => SwitchOutcome::WouldSwitch {
                outgoing: plan.outgoing_label,
            },
        });
    }

    let target_path = find(accounts, label)?.codex_auth_path.as_path();
    let active = resolve_active_label(default_path, accounts);
    let outgoing_path = active
        .as_deref()
        .and_then(|active| accounts.iter().find(|account| account.label == active))
        .map(|account| account.codex_auth_path.as_path());
    let _locks = lock_dirs(
        [Some(default_path), Some(target_path), outgoing_path]
            .into_iter()
            .flatten(),
    )?;

    // Decide again under the locks: a refresh or another switch may have
    // changed the files since the first look.
    let plan = match decide(default_path, accounts, label, opts.force)? {
        Decision::AlreadyActive => return Ok(SwitchOutcome::AlreadyActive),
        Decision::Move(plan) => plan,
    };
    if plan
        .outgoing
        .map(|account| account.codex_auth_path.as_path())
        != outgoing_path
    {
        return Err(AppError::Other(
            "the active Codex account changed while switching; try again".into(),
        ));
    }

    let mut journal = Journal::default();
    let result = (|| -> Result<()> {
        if let (Some(account), Some(blob)) = (plan.outgoing, plan.default_blob.as_deref()) {
            journal.write(&account.codex_auth_path, blob)?;
            if let Some(id) = account_id_of(blob) {
                journal.write(&marker_path(account), &marker_bytes(&id)?)?;
            }
        }
        journal.write(default_path, &plan.target_blob)?;
        journal.write(&marker_path(plan.target), &marker_bytes(&plan.target_id)?)?;
        journal.remove(&plan.target.codex_auth_path)?;
        Ok(())
    })();
    if let Err(error) = result {
        let failures = journal.rollback();
        return Err(if failures.is_empty() {
            error
        } else {
            AppError::Other(format!(
                "{error}; these files could not be restored and need attention: {}",
                failures.join("; ")
            ))
        });
    }
    Ok(SwitchOutcome::Switched {
        outgoing: plan.outgoing_label,
    })
}

fn decide<'a>(
    default_path: &Path,
    accounts: &'a [OpenAiAccount],
    label: &str,
    force: bool,
) -> Result<Decision<'a>> {
    validate_layout(default_path, accounts)?;
    let target = find(accounts, label)?;
    let active = resolve_active_label(default_path, accounts);
    let default_blob = read_optional(default_path)?;

    if active.as_deref() == Some(label) && default_blob.is_some() {
        if target.codex_auth_path.exists() {
            return Err(AppError::Credentials(format!(
                "{label:?} is active in {} and also has its own login at {}; keep one of \
                 them before switching, so one account never has two live copies",
                sanitize_untrusted_path(default_path),
                sanitize_untrusted_path(&target.codex_auth_path)
            )));
        }
        return Ok(Decision::AlreadyActive);
    }
    if active.is_none() && default_blob.is_some() && !force {
        return Err(AppError::Credentials(format!(
            "the Codex login in {} is not managed here, so switching to {label:?} would \
             overwrite a login that cannot be saved first. Register it with \
             `ai-usagebar account add <label> --codex --adopt-current`, or pass --force to \
             discard it.",
            sanitize_untrusted_path(default_path)
        )));
    }

    let target_blob = read_optional(&target.codex_auth_path)?.ok_or_else(|| {
        AppError::Credentials(format!(
            "no stored Codex login for {label:?}; sign it in once with `CODEX_HOME={} codex login`",
            sanitize_untrusted_path(parent_of(&target.codex_auth_path))
        ))
    })?;
    let target_id = account_id_of(&target_blob).ok_or_else(|| {
        AppError::Credentials(format!(
            "the Codex login stored for {label:?} has no account id; sign it in again"
        ))
    })?;
    let outgoing = active
        .as_deref()
        .and_then(|active| accounts.iter().find(|account| account.label == active));
    if let Some(account) = outgoing
        && account.codex_auth_path.exists()
    {
        return Err(AppError::Credentials(format!(
            "{:?} is active and also has its own login at {}; keep one of them before \
             switching, so saving it back cannot overwrite a separate login",
            account.label,
            sanitize_untrusted_path(&account.codex_auth_path)
        )));
    }
    Ok(Decision::Move(Plan {
        target,
        target_blob,
        target_id,
        outgoing,
        outgoing_label: active,
        default_blob,
    }))
}

/// Refuse any layout where a move could land on, or through, another
/// account's file.
fn validate_layout(default_path: &Path, accounts: &[OpenAiAccount]) -> Result<()> {
    let mut seen: Vec<(PathBuf, String)> = vec![(normalized(default_path), "the default".into())];
    for account in accounts {
        let path = normalized(&account.codex_auth_path);
        if let Some((_, owner)) = seen.iter().find(|(other, _)| *other == path) {
            return Err(AppError::Credentials(format!(
                "Codex account {:?} uses the same credential file as {owner} ({}); every \
                 account needs its own file",
                account.label,
                sanitize_untrusted_path(&account.codex_auth_path)
            )));
        }
        seen.push((path, format!("account {:?}", account.label)));
    }
    let paths =
        std::iter::once(default_path).chain(accounts.iter().map(|a| a.codex_auth_path.as_path()));
    for path in paths {
        if std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return Err(AppError::Credentials(format!(
                "{} is a symlink; a switch would replace the link rather than the file it \
                 points to, so it refuses to move logins through it",
                sanitize_untrusted_path(path)
            )));
        }
    }
    let identities: Vec<(String, &str)> = accounts
        .iter()
        .filter_map(|account| Some((identity(account)?, account.label.as_str())))
        .collect();
    for (index, (id, label)) in identities.iter().enumerate() {
        if let Some((_, other)) = identities[index + 1..]
            .iter()
            .find(|(other, _)| other == id)
        {
            return Err(AppError::Credentials(format!(
                "Codex accounts {label:?} and {other:?} are the same ChatGPT account; remove \
                 one of them so a switch knows which login it moves"
            )));
        }
    }
    Ok(())
}

/// A path with its existing directories resolved, so `a/../b` and a symlinked
/// parent compare equal to the real location. The file itself may not exist.
fn normalized(path: &Path) -> PathBuf {
    let parent = parent_of(path);
    let parent = std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
    match path.file_name() {
        Some(name) => parent.join(name),
        None => parent,
    }
}

/// Register the login currently in the default slot as `account` without a
/// new sign-in: only its marker is written, so the token stays in one place.
pub fn adopt_current(
    default_path: &Path,
    accounts: &[OpenAiAccount],
    account: &OpenAiAccount,
) -> Result<()> {
    let _locks = lock_dirs([default_path, account.codex_auth_path.as_path()].into_iter())?;
    validate_layout(default_path, accounts)?;
    let live = account_id_in(default_path).ok_or_else(|| {
        AppError::Credentials(format!(
            "no Codex login with an account id at {}; run `codex login` first",
            sanitize_untrusted_path(default_path)
        ))
    })?;
    if let Some(owner) = accounts
        .iter()
        .find(|other| other.label != account.label && identity(other).as_deref() == Some(&live))
    {
        return Err(AppError::Credentials(format!(
            "the current Codex login already belongs to account {:?}",
            owner.label
        )));
    }
    if account.codex_auth_path.exists() {
        return Err(AppError::Credentials(format!(
            "{:?} already has its own Codex login at {}; adopting would leave two copies of \
             one account",
            account.label,
            sanitize_untrusted_path(&account.codex_auth_path)
        )));
    }
    crate::cache::atomic_write(&marker_path(account), &marker_bytes(&live)?)
}

/// Lock every distinct credential directory among `paths`, in a fixed order
/// so two callers never wait on each other. A missing directory is created
/// owner-only first, since the lock file would otherwise create it with
/// whatever the umask allows.
fn lock_dirs<'a>(paths: impl Iterator<Item = &'a Path>) -> Result<Vec<LockGuard>> {
    let mut locks = Vec::new();
    for path in paths {
        prepare_dir(parent_of(path))?;
        locks.push(normalized(&lock_path(path)));
    }
    locks.sort();
    locks.dedup();
    locks
        .iter()
        .map(|lock| crate::cache::acquire_lock(lock, LOCK_TIMEOUT))
        .collect()
}

fn prepare_dir(dir: &Path) -> Result<()> {
    if !dir.exists() {
        std::fs::create_dir_all(dir).map_err(|error| AppError::io_at(dir, error))?;
        restrict_dir(dir)?;
    }
    Ok(())
}

fn parent_of(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn find<'a>(accounts: &'a [OpenAiAccount], label: &str) -> Result<&'a OpenAiAccount> {
    accounts
        .iter()
        .find(|account| account.label == label)
        .ok_or_else(|| {
            let known: Vec<&str> = accounts.iter().map(|a| a.label.as_str()).collect();
            AppError::Credentials(format!(
                "no Codex account {label:?} in [[openai.accounts]]; known: {known:?}"
            ))
        })
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(AppError::io_at(path, error)),
    }
}

/// Every file the switch touched, with what it held before, so a failure can
/// put each one back.
#[derive(Default)]
struct Journal {
    entries: Vec<(PathBuf, Option<Vec<u8>>)>,
}

impl Journal {
    fn remember(&mut self, path: &Path) -> Result<()> {
        if self.entries.iter().all(|(seen, _)| seen != path) {
            let before = read_optional(path)?;
            self.entries.push((path.to_path_buf(), before));
        }
        Ok(())
    }

    fn write(&mut self, path: &Path, bytes: &[u8]) -> Result<()> {
        self.remember(path)?;
        prepare_dir(parent_of(path))?;
        crate::cache::atomic_write(path, bytes)
    }

    fn remove(&mut self, path: &Path) -> Result<()> {
        self.remember(path)?;
        std::fs::remove_file(path).map_err(|error| AppError::io_at(path, error))
    }

    /// Put every remembered file back, newest first, attempting all of them
    /// even after one fails. Returns the files that could not be restored.
    fn rollback(self) -> Vec<String> {
        let mut failures = Vec::new();
        for (path, before) in self.entries.into_iter().rev() {
            let restored = match &before {
                Some(bytes) if std::fs::read(&path).is_ok_and(|now| &now == bytes) => Ok(()),
                Some(bytes) => crate::cache::atomic_write(&path, bytes),
                None => match std::fs::remove_file(&path) {
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                        Err(AppError::io_at(&path, error))
                    }
                    _ => Ok(()),
                },
            };
            if let Err(error) = restored {
                failures.push(format!("{}: {error}", sanitize_untrusted_path(&path)));
            }
        }
        failures
    }
}

#[cfg(unix)]
fn restrict_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| AppError::io_at(path, error))
}

#[cfg(not(unix))]
fn restrict_dir(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth(account_id: &str, refresh: &str) -> String {
        format!(
            r#"{{"auth_mode":"chatgpt","tokens":{{"access_token":"a","refresh_token":"{refresh}","id_token":"x.y.z","account_id":"{account_id}"}}}}"#
        )
    }

    struct Fixture {
        dir: tempfile::TempDir,
        default: PathBuf,
        accounts: Vec<OpenAiAccount>,
    }

    impl Fixture {
        /// `~/.codex/auth.json` holds `main`'s login; `work` has its own file;
        /// `main` is registered but has no file, as after an adopt.
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let default = dir.path().join(".codex").join("auth.json");
            std::fs::create_dir_all(default.parent().unwrap()).unwrap();
            std::fs::write(&default, auth("acct-main", "rt-main")).unwrap();
            let account = |label: &str| OpenAiAccount {
                label: label.into(),
                codex_auth_path: dir.path().join(format!(".codex-{label}")).join("auth.json"),
            };
            let accounts = vec![account("main"), account("work")];
            std::fs::create_dir_all(accounts[1].codex_auth_path.parent().unwrap()).unwrap();
            std::fs::write(&accounts[1].codex_auth_path, auth("acct-work", "rt-work")).unwrap();
            Self {
                dir,
                default,
                accounts,
            }
        }

        fn adopt_main(&self) {
            adopt_current(&self.default, &self.accounts, &self.accounts[0]).unwrap();
        }

        fn switch(&self, label: &str) -> Result<SwitchOutcome> {
            switch_account(&self.default, &self.accounts, label, SwitchOpts::default())
        }

        fn refresh_token_at(path: &Path) -> String {
            let value: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            value["tokens"]["refresh_token"]
                .as_str()
                .unwrap()
                .to_string()
        }
    }

    fn walk(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(walk(&path));
            } else {
                out.push((path.clone(), std::fs::read(&path).unwrap()));
            }
        }
        out.sort();
        out
    }

    #[test]
    fn the_account_id_comes_from_the_tokens_block() {
        assert_eq!(
            account_id_of(auth("acct-1", "rt").as_bytes()).as_deref(),
            Some("acct-1")
        );
        assert_eq!(account_id_of(b"not json"), None);
    }

    #[test]
    fn an_unmanaged_default_login_resolves_to_no_label() {
        let fx = Fixture::new();
        assert_eq!(resolve_active_label(&fx.default, &fx.accounts), None);
    }

    #[test]
    fn adopting_marks_the_live_login_without_copying_it() {
        let fx = Fixture::new();
        fx.adopt_main();
        assert_eq!(
            resolve_active_label(&fx.default, &fx.accounts).as_deref(),
            Some("main")
        );
        assert!(!fx.accounts[0].codex_auth_path.exists());
        assert!(has_login(&fx.default, &fx.accounts, &fx.accounts[0]));
    }

    #[test]
    fn a_marker_alone_is_not_a_login() {
        let fx = Fixture::new();
        fx.adopt_main();
        std::fs::remove_file(&fx.default).unwrap();
        assert!(!has_login(&fx.default, &fx.accounts, &fx.accounts[0]));
        assert!(has_login(&fx.default, &fx.accounts, &fx.accounts[1]));
    }

    #[test]
    fn adopting_refuses_a_login_another_account_owns() {
        let fx = Fixture::new();
        std::fs::write(&fx.default, auth("acct-work", "rt-other")).unwrap();
        let error = adopt_current(&fx.default, &fx.accounts, &fx.accounts[0]).unwrap_err();
        assert!(error.to_string().contains("\"work\""), "{error}");
    }

    #[test]
    fn switching_from_an_unmanaged_login_is_refused_without_force() {
        let fx = Fixture::new();
        let error = fx.switch("work").unwrap_err();
        assert!(error.to_string().contains("not managed"), "{error}");
        assert_eq!(Fixture::refresh_token_at(&fx.default), "rt-main");
    }

    #[test]
    fn forcing_discards_the_unmanaged_login() {
        let fx = Fixture::new();
        let outcome = switch_account(
            &fx.default,
            &fx.accounts,
            "work",
            SwitchOpts {
                force: true,
                ..SwitchOpts::default()
            },
        )
        .unwrap();
        assert_eq!(outcome, SwitchOutcome::Switched { outgoing: None });
        assert_eq!(Fixture::refresh_token_at(&fx.default), "rt-work");
        assert!(!fx.accounts[1].codex_auth_path.exists());
    }

    #[test]
    fn switching_moves_both_logins_and_never_leaves_a_copy() {
        let fx = Fixture::new();
        fx.adopt_main();
        let outcome = fx.switch("work").unwrap();
        assert_eq!(
            outcome,
            SwitchOutcome::Switched {
                outgoing: Some("main".into())
            }
        );
        assert_eq!(Fixture::refresh_token_at(&fx.default), "rt-work");
        assert_eq!(
            Fixture::refresh_token_at(&fx.accounts[0].codex_auth_path),
            "rt-main"
        );
        assert!(!fx.accounts[1].codex_auth_path.exists());
        assert_eq!(
            resolve_active_label(&fx.default, &fx.accounts).as_deref(),
            Some("work")
        );
        assert!(has_login(&fx.default, &fx.accounts, &fx.accounts[1]));
    }

    #[test]
    fn switching_back_restores_the_original_layout() {
        let fx = Fixture::new();
        fx.adopt_main();
        fx.switch("work").unwrap();
        fx.switch("main").unwrap();
        assert_eq!(Fixture::refresh_token_at(&fx.default), "rt-main");
        assert_eq!(
            Fixture::refresh_token_at(&fx.accounts[1].codex_auth_path),
            "rt-work"
        );
        assert!(!fx.accounts[0].codex_auth_path.exists());
    }

    #[test]
    fn switching_to_the_active_account_changes_nothing() {
        let fx = Fixture::new();
        fx.adopt_main();
        assert_eq!(fx.switch("main").unwrap(), SwitchOutcome::AlreadyActive);
        assert_eq!(Fixture::refresh_token_at(&fx.default), "rt-main");
    }

    #[test]
    fn an_active_account_with_its_own_copy_is_refused() {
        let fx = Fixture::new();
        fx.adopt_main();
        std::fs::create_dir_all(fx.accounts[0].codex_auth_path.parent().unwrap()).unwrap();
        std::fs::write(
            &fx.accounts[0].codex_auth_path,
            auth("acct-main", "rt-main"),
        )
        .unwrap();
        let error = fx.switch("work").unwrap_err();
        assert!(error.to_string().contains("own login"), "{error}");
        assert_eq!(Fixture::refresh_token_at(&fx.default), "rt-main");
        assert!(fx.accounts[1].codex_auth_path.exists());
    }

    #[test]
    fn a_dry_run_writes_nothing_at_all() {
        let fx = Fixture::new();
        fx.adopt_main();
        let before = walk(fx.dir.path());
        let outcome = switch_account(
            &fx.default,
            &fx.accounts,
            "work",
            SwitchOpts {
                dry_run: true,
                ..SwitchOpts::default()
            },
        )
        .unwrap();
        assert_eq!(
            outcome,
            SwitchOutcome::WouldSwitch {
                outgoing: Some("main".into())
            }
        );
        assert_eq!(walk(fx.dir.path()), before);
    }

    #[test]
    fn switching_to_an_account_that_never_signed_in_changes_nothing() {
        let fx = Fixture::new();
        fx.adopt_main();
        std::fs::remove_file(&fx.accounts[1].codex_auth_path).unwrap();
        let error = fx.switch("work").unwrap_err();
        assert!(error.to_string().contains("codex login"), "{error}");
        assert_eq!(Fixture::refresh_token_at(&fx.default), "rt-main");
    }

    #[test]
    fn accounts_sharing_a_file_are_refused() {
        let mut fx = Fixture::new();
        fx.accounts[0].codex_auth_path = fx.accounts[1].codex_auth_path.clone();
        let error = fx.switch("work").unwrap_err();
        assert!(
            error.to_string().contains("same credential file"),
            "{error}"
        );
    }

    #[test]
    fn an_account_on_the_default_file_is_refused() {
        let mut fx = Fixture::new();
        fx.accounts[0].codex_auth_path = fx.default.clone();
        let error = fx.switch("work").unwrap_err();
        assert!(error.to_string().contains("the default"), "{error}");
    }

    #[test]
    fn accounts_in_one_directory_keep_separate_markers() {
        let fx = Fixture::new();
        let dir = fx.dir.path().join("shared");
        let a = OpenAiAccount {
            label: "a".into(),
            codex_auth_path: dir.join("a.json"),
        };
        let b = OpenAiAccount {
            label: "b".into(),
            codex_auth_path: dir.join("b.json"),
        };
        assert_ne!(marker_path(&a), marker_path(&b));
    }

    #[test]
    fn two_accounts_with_one_identity_are_refused() {
        let fx = Fixture::new();
        fx.adopt_main();
        std::fs::write(
            &fx.accounts[1].codex_auth_path,
            auth("acct-main", "rt-other"),
        )
        .unwrap();
        let error = fx.switch("work").unwrap_err();
        assert!(
            error.to_string().contains("same ChatGPT account"),
            "{error}"
        );
        assert_eq!(resolve_active_label(&fx.default, &fx.accounts), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_credential_file_is_refused() {
        let fx = Fixture::new();
        fx.adopt_main();
        let real = fx.dir.path().join("elsewhere.json");
        std::fs::rename(&fx.accounts[1].codex_auth_path, &real).unwrap();
        std::os::unix::fs::symlink(&real, &fx.accounts[1].codex_auth_path).unwrap();
        let error = fx.switch("work").unwrap_err();
        assert!(error.to_string().contains("symlink"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn a_rollback_reports_what_it_could_not_restore_and_restores_the_rest() {
        use std::os::unix::fs::PermissionsExt;

        let fx = Fixture::new();
        fx.adopt_main();
        let mut journal = Journal::default();
        journal
            .write(&fx.accounts[0].codex_auth_path, b"saved")
            .unwrap();
        journal.write(&fx.default, b"replaced").unwrap();
        journal.remove(&fx.accounts[1].codex_auth_path).unwrap();
        // The newest step cannot be undone; the older ones must still be.
        let locked = fx.accounts[1]
            .codex_auth_path
            .parent()
            .unwrap()
            .to_path_buf();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).unwrap();
        let failures = journal.rollback();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert_eq!(Fixture::refresh_token_at(&fx.default), "rt-main");
        assert!(!fx.accounts[0].codex_auth_path.exists());
    }

    #[test]
    fn an_unknown_label_lists_the_known_ones() {
        let fx = Fixture::new();
        let error = fx.switch("nope").unwrap_err();
        assert!(error.to_string().contains("\"main\""), "{error}");
    }
}
