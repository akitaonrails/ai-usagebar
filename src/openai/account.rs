//! Which Codex account `~/.codex/auth.json` holds, and moving that login
//! between managed accounts.
//!
//! Codex keeps exactly **one** live login per `CODEX_HOME`, and the Codex CLI,
//! the Codex desktop app and the IDE extension all read the same default one,
//! `~/.codex/auth.json`. A named account (`[[openai.accounts]]`) is another
//! `auth.json`, usually made with `CODEX_HOME=~/.codex-<label> codex login`.
//! Making a named account "the one Codex uses" therefore means moving its file
//! into the default slot.
//!
//! Copying would leave two files holding the same *rotating* refresh token, and
//! whichever client refreshed first would invalidate the other. The switch
//! moves instead, in the same order as the Claude CLI switch in
//! [`crate::anthropic::cli_account`]: it saves the outgoing default login back
//! into its own account first, installs the target in the default slot, and
//! only then removes the target's named copy. [`OpenAiConfig::resolve_auth_path`]
//! routes reads for whichever label is *currently* active to the default slot
//! while its own file is gone, so only one copy is ever live.
//!
//! A moved-away account has no file left to identify it, so every account also
//! gets a small marker next to its `auth.json` recording the ChatGPT account id
//! it belongs to — the counterpart of the `oauthAccount` block Claude Code keeps
//! in `.claude.json`. It never holds a token.
//!
//! A Codex process that is already running is not confused by the move: before
//! it refreshes, Codex reloads `auth.json` and skips the refresh when the
//! account id on disk changed, so it cannot write the old account's tokens over
//! the new one. It simply needs a restart to pick the new account up.
//!
//! [`OpenAiConfig::resolve_auth_path`]: crate::config::OpenAiConfig::resolve_auth_path

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

use crate::config::OpenAiAccount;
use crate::error::{AppError, Result};

/// Identity marker beside each named account's `auth.json`.
pub const MARKER_FILE: &str = ".ai-usagebar-account.json";

const LOCK_FILE: &str = ".ai-usagebar-account-switch.lock";

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

/// Where `account`'s identity marker lives.
pub fn marker_path(account: &OpenAiAccount) -> PathBuf {
    account
        .codex_auth_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(MARKER_FILE)
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

fn write_marker(account: &OpenAiAccount, account_id: &str) -> Result<()> {
    crate::cache::atomic_write(&marker_path(account), &marker_bytes(account_id)?)
}

/// Who `account` is: its own file when it still has one, else its marker.
pub fn identity(account: &OpenAiAccount) -> Option<String> {
    account_id_in(&account.codex_auth_path).or_else(|| marker_account_id(account))
}

/// Which managed account the default `auth.json` belongs to, if any.
pub fn resolve_active_label(default_path: &Path, accounts: &[OpenAiAccount]) -> Option<String> {
    let live = account_id_in(default_path)?;
    accounts
        .iter()
        .find(|account| identity(account).as_deref() == Some(live.as_str()))
        .map(|account| account.label.clone())
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SwitchOpts {
    /// Overwrite the default slot even though the live login belongs to no
    /// managed account. That login cannot be saved anywhere first, so this
    /// genuinely discards it.
    pub force: bool,
    /// Validate everything and report, without writing.
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

/// Make `label` the account the Codex CLI, app and IDE extension use.
///
/// Ordering is deliberate. The outgoing login is saved back **before**
/// anything is overwritten, the target is written to the default slot
/// **before** its named copy is removed, and any failure restores every file
/// touched so far — so no step can leave a login stranded or duplicated.
pub fn switch_account(
    default_path: &Path,
    accounts: &[OpenAiAccount],
    label: &str,
    opts: SwitchOpts,
) -> Result<SwitchOutcome> {
    let lock_path = default_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(LOCK_FILE);
    let _lock = crate::cache::acquire_lock(&lock_path, Duration::from_secs(2))?;

    let target = find(accounts, label)?;
    let active = resolve_active_label(default_path, accounts);
    let original_default = read_optional(default_path)?;

    if active.as_deref() == Some(label) && original_default.is_some() {
        return Ok(SwitchOutcome::AlreadyActive);
    }
    if active.is_none() && original_default.is_some() && !opts.force {
        return Err(AppError::Credentials(format!(
            "Codex is signed into an account that is not managed here, so switching to \
             {label:?} would overwrite a login that cannot be saved first. Register it with \
             `ai-usagebar account add <label> --codex --adopt-current`, or pass --force to \
             discard it."
        )));
    }

    let target_blob = read_optional(&target.codex_auth_path)?.ok_or_else(|| {
        AppError::Credentials(format!(
            "no stored Codex login for {label:?}; sign it in once with `CODEX_HOME={} codex login`",
            target
                .codex_auth_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .display()
        ))
    })?;
    let target_id = account_id_of(&target_blob).ok_or_else(|| {
        AppError::Credentials(format!(
            "the Codex login stored for {label:?} has no account id; sign it in again"
        ))
    })?;

    let outgoing = active
        .as_deref()
        .and_then(|outgoing| accounts.iter().find(|account| account.label == outgoing));
    if opts.dry_run {
        return Ok(SwitchOutcome::WouldSwitch { outgoing: active });
    }

    let mut journal = Journal::default();
    let result = (|| -> Result<()> {
        if let (Some(account), Some(blob)) = (outgoing, original_default.as_deref()) {
            journal.write(&account.codex_auth_path, blob)?;
            if let Some(id) = account_id_of(blob) {
                journal.write(&marker_path(account), &marker_bytes(&id)?)?;
            }
        }
        journal.write(default_path, &target_blob)?;
        journal.write(&marker_path(target), &marker_bytes(&target_id)?)?;
        journal.remove(&target.codex_auth_path)?;
        Ok(())
    })();
    if let Err(error) = result {
        return Err(match journal.rollback() {
            Ok(()) => error,
            Err(rollback) => AppError::Other(format!(
                "{error}; restoring the previous Codex logins also failed: {rollback}"
            )),
        });
    }
    Ok(SwitchOutcome::Switched { outgoing: active })
}

/// Register the login currently in the default slot as `account` without a
/// new sign-in: only its marker is written, so the token stays in one place.
pub fn adopt_current(
    default_path: &Path,
    accounts: &[OpenAiAccount],
    account: &OpenAiAccount,
) -> Result<()> {
    let live = account_id_in(default_path).ok_or_else(|| {
        AppError::Credentials(format!(
            "no Codex login with an account id at {}; run `codex login` first",
            default_path.display()
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
            account.codex_auth_path.display()
        )));
    }
    write_marker(account, &live)
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

fn marker_bytes(account_id: &str) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec_pretty(
        &json!({ "account_id": account_id }),
    )?)
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
        if let Some(dir) = path.parent()
            && !dir.exists()
        {
            std::fs::create_dir_all(dir).map_err(|error| AppError::io_at(dir, error))?;
            restrict_dir(dir)?;
        }
        crate::cache::atomic_write(path, bytes)
    }

    fn remove(&mut self, path: &Path) -> Result<()> {
        self.remember(path)?;
        std::fs::remove_file(path).map_err(|error| AppError::io_at(path, error))
    }

    fn rollback(self) -> Result<()> {
        for (path, before) in self.entries.into_iter().rev() {
            match before {
                Some(bytes) => crate::cache::atomic_write(&path, &bytes)?,
                None => match std::fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(AppError::io_at(&path, error)),
                },
            }
        }
        Ok(())
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
        _dir: tempfile::TempDir,
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
                _dir: dir,
                default,
                accounts,
            }
        }

        fn adopt_main(&self) {
            adopt_current(&self.default, &self.accounts, &self.accounts[0]).unwrap();
        }

        fn refresh_token_at(path: &Path) -> String {
            let value: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            value["tokens"]["refresh_token"]
                .as_str()
                .unwrap()
                .to_string()
        }
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
        let error =
            switch_account(&fx.default, &fx.accounts, "work", SwitchOpts::default()).unwrap_err();
        assert!(error.to_string().contains("not managed"), "{error}");
        assert_eq!(Fixture::refresh_token_at(&fx.default), "rt-main");
    }

    #[test]
    fn switching_moves_both_logins_and_never_leaves_a_copy() {
        let fx = Fixture::new();
        fx.adopt_main();
        let outcome =
            switch_account(&fx.default, &fx.accounts, "work", SwitchOpts::default()).unwrap();
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
    }

    #[test]
    fn switching_back_restores_the_original_layout() {
        let fx = Fixture::new();
        fx.adopt_main();
        switch_account(&fx.default, &fx.accounts, "work", SwitchOpts::default()).unwrap();
        switch_account(&fx.default, &fx.accounts, "main", SwitchOpts::default()).unwrap();
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
        let outcome =
            switch_account(&fx.default, &fx.accounts, "main", SwitchOpts::default()).unwrap();
        assert_eq!(outcome, SwitchOutcome::AlreadyActive);
        assert_eq!(Fixture::refresh_token_at(&fx.default), "rt-main");
    }

    #[test]
    fn a_dry_run_writes_nothing() {
        let fx = Fixture::new();
        fx.adopt_main();
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
        assert_eq!(Fixture::refresh_token_at(&fx.default), "rt-main");
        assert!(fx.accounts[1].codex_auth_path.exists());
    }

    #[test]
    fn switching_to_an_account_that_never_signed_in_changes_nothing() {
        let fx = Fixture::new();
        fx.adopt_main();
        std::fs::remove_file(&fx.accounts[1].codex_auth_path).unwrap();
        let error =
            switch_account(&fx.default, &fx.accounts, "work", SwitchOpts::default()).unwrap_err();
        assert!(error.to_string().contains("codex login"), "{error}");
        assert_eq!(Fixture::refresh_token_at(&fx.default), "rt-main");
    }

    #[test]
    fn a_failed_switch_restores_every_file() {
        let fx = Fixture::new();
        fx.adopt_main();
        let mut journal = Journal::default();
        journal
            .write(&fx.accounts[0].codex_auth_path, b"saved")
            .unwrap();
        journal.write(&fx.default, b"replaced").unwrap();
        journal.remove(&fx.accounts[1].codex_auth_path).unwrap();
        journal.rollback().unwrap();
        assert_eq!(Fixture::refresh_token_at(&fx.default), "rt-main");
        assert_eq!(
            Fixture::refresh_token_at(&fx.accounts[1].codex_auth_path),
            "rt-work"
        );
        assert!(!fx.accounts[0].codex_auth_path.exists());
    }

    #[test]
    fn an_unknown_label_lists_the_known_ones() {
        let fx = Fixture::new();
        let error =
            switch_account(&fx.default, &fx.accounts, "nope", SwitchOpts::default()).unwrap_err();
        assert!(error.to_string().contains("\"main\""), "{error}");
    }
}
