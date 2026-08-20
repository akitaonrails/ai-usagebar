//! The pairing record: which repository this machine is bound to, and what it
//! looked like when the binding was made.
//!
//! **Empty by design: plan 3-04 fills this file.** Plan 3-01 creates it so the
//! module tree is fixed up front and no two plans ever edit the same file.
//!
//! What it is for: `owner/name` is a *name*, and names on GitHub can be released
//! and re-taken. Recording the numeric `owner_id` and repository `id` from
//! [`RepoFacts`](super::gate::RepoFacts) at pairing time is what makes a
//! delete-and-resquat detectable on the next push — a login comparison alone
//! would not see it. It is also where a private→public transition is noticed as
//! *drift* rather than as a first-contact fact, which is what lets SAFE-02 name
//! the credentials to rotate.
//!
//! Where it lives: the config directory (from the injected
//! [`SyncRoots`](crate::sync::SyncRoots)), **not** the cache — a wipeable
//! pairing check silently degrades to first-contact trust. Written atomically at
//! mode 0600, like every other credential-adjacent file in this codebase.
//!
//! Owned by plan 3-04.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};
use crate::sync::SyncRoots;

use super::gate::RepoFacts;

/// The file name inside the config directory. Stable: a rename makes every
/// existing record unreadable, and an unreadable record is a pairing check that
/// silently degrades to first-contact trust — the exact window it exists to
/// close.
const PAIRING_FILE: &str = "sync-pairing.json";

/// What this machine paired with, and what that repository looked like at the
/// time.
///
/// Holds nothing secret — every field is public metadata GitHub serves — so
/// `Debug` is derived. The file is still mode 0600, because it is *integrity*
/// that matters here: an attacker who can rewrite it can wave a substituted
/// repository through.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pairing {
    /// `RepoFacts::id`. Survives a rename of the repository.
    pub repo_id: u64,
    /// `RepoFacts::owner_id`. This is the field that makes a delete-and-resquat
    /// of the *name* detectable — a login can be given up and taken by someone
    /// else, a numeric id cannot.
    pub owner_id: u64,
    /// Whether it was private when the pairing was made. A private→public
    /// transition read against this is SAFE-02's incident; a repository that was
    /// never private was never trusted with anything.
    pub private: bool,
    pub checked_at: DateTime<Utc>,
}

impl Pairing {
    /// The record to persist for `facts`, as of `now`.
    pub fn of(facts: &RepoFacts, now: DateTime<Utc>) -> Pairing {
        Pairing {
            repo_id: facts.id,
            owner_id: facts.owner_id,
            private: facts.private,
            checked_at: now,
        }
    }
}

/// Where the record lives in production: the **config** directory, never the
/// cache. A wiped cache must not silently reset the identity this record
/// defends — `rm -rf ~/.cache/*` is something users and packagers do.
///
/// The only production wrapper in this module, and nothing under `#[cfg(test)]`
/// calls it. Every other entry point takes a `&Path`, exactly as
/// [`Cache::at`](crate::cache::Cache::at) and `creds::read_from` already do.
pub fn default_path(roots: &SyncRoots) -> PathBuf {
    roots.config_dir.join(PAIRING_FILE)
}

/// Read the record at `path`.
///
/// `Ok(None)` is a genuinely absent file — first pairing, a normal path rather
/// than a failure. A file that exists but does not parse is an **error**: a
/// `Default` here would let a corrupted (or deliberately corrupted) record wave
/// through exactly the substitution the record exists to detect.
pub fn read_from(path: &Path) -> Result<Option<Pairing>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(AppError::io_at(path, e)),
    };
    let pairing: Pairing = serde_json::from_slice(&bytes).map_err(|e| {
        AppError::Other(format!(
            "the sync pairing record at {} exists but is unreadable ({e}) — refusing rather \
             than treating it as a first pairing, because that would re-trust whichever \
             repository currently answers to that name. Delete it only if you accept \
             pairing afresh with whatever is there now",
            path.display()
        ))
    })?;
    Ok(Some(pairing))
}

/// Persist the record at `path`, atomically and mode 0600.
///
/// [`crate::cache::atomic_write`] is the project's existing tempfile-in-the-
/// destination-directory + `persist` helper, so this inherits the no-torn-state
/// invariant rather than reinventing it; the mode is then set explicitly rather
/// than trusting what the temp file inherited, the same belt-and-braces
/// `anchor.rs` and the Settings overlay apply.
pub fn write_to(path: &Path, pairing: &Pairing) -> Result<()> {
    let bytes = serde_json::to_vec(pairing)?;
    crate::cache::atomic_write(path, &bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| AppError::io_at(path, e))?;
    }
    Ok(())
}

/// What [`check_drift`] decided.
///
/// There is no `proceed` flag: a refusal is an `Err`, so *holding* a
/// `DriftOutcome` is the permission to continue — the same shape as
/// [`PushClearance`](super::gate::PushClearance), for the same reason. Plan 3-07
/// wires this to `sync setup`; nothing here prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftOutcome {
    /// The record to persist once the operation succeeds. Always present: on a
    /// clean re-check it is the refreshed timestamp, and on first contact it is
    /// the pairing being established.
    pub record: Pairing,
    /// True when nothing was compared because there was no record. The caller
    /// may want to say so; it is not an error.
    pub first_contact: bool,
    pub warnings: Vec<String>,
}

/// Compare the pairing record against what GitHub says now.
///
/// Three things can have drifted, and they are not the same kind of event:
///
/// - **A different repository at the same name.** `owner_id` or `repo_id`
///   changed: the name was released and re-registered, or the repository was
///   deleted and recreated. Refused.
/// - **Private → public.** SAFE-02, and an *incident* rather than a
///   configuration error: this machine has been pushing to a repository that
///   later became readable by everyone. With credentials in the bundle the
///   message names them and says to rotate.
/// - **Public all along, credentials off.** D-04's closing paragraph: warned
///   about, not refused. There is nothing to rotate, but a chat index is still
///   personal data.
///
/// Every timestamp comes from the injected `now`; nothing here reads a clock.
pub fn check_drift(
    record: Option<&Pairing>,
    facts: &RepoFacts,
    credentials_in_bundle: bool,
    now: DateTime<Utc>,
) -> Result<DriftOutcome> {
    let fresh = Pairing::of(facts, now);

    let Some(record) = record else {
        // First contact establishes the pairing; there is nothing to compare
        // against. A public repository is not warned about twice — `assert_pushable`
        // owns that message, and it runs on this same path.
        return Ok(DriftOutcome {
            record: fresh,
            first_contact: true,
            warnings: Vec::new(),
        });
    };

    if record.owner_id != facts.owner_id {
        return Err(AppError::Other(format!(
            "REFUSING TO PUSH: this machine paired with owner id {}, but the repository at \
             that name is now owned by id {} ({:?}).\n\
             A GitHub account name can be released and re-registered by someone else, so the \
             same owner/name can point at a repository you have never seen. Nothing is \
             uploaded. Confirm which repository [sync] repo should name, then delete the \
             pairing record to pair afresh.",
            record.owner_id, facts.owner_id, facts.owner_login
        )));
    }

    if record.repo_id != facts.id {
        return Err(AppError::Other(format!(
            "REFUSING TO PUSH: this machine paired with repository id {}, but the repository \
             at that name is now id {}.\n\
             The original was deleted and something else was created under the same name. \
             Nothing is uploaded. Confirm which repository [sync] repo should name, then \
             delete the pairing record to pair afresh.",
            record.repo_id, facts.id
        )));
    }

    let mut warnings = Vec::new();
    if record.private && !facts.private {
        if credentials_in_bundle {
            return Err(AppError::Other(went_public_incident()));
        }
        warnings.push(
            "the backup repository was private when this machine paired with it and is public \
             now. The credentials category is off, so there is nothing to rotate — but the \
             chat indexes and config already pushed there are readable by anyone. Make it \
             private again."
                .to_owned(),
        );
    }

    Ok(DriftOutcome {
        record: fresh,
        first_contact: false,
        warnings,
    })
}

/// SAFE-02, said plainly and in this order: what happened, what to fix, what to
/// rotate — and that the last one is not optional, because a push may already
/// have landed while the repository was public and published bytes cannot be
/// un-published.
///
/// The categories are named rather than gestured at. "Rotate your credentials"
/// leaves the reader guessing which ones; these are the files the `credentials`
/// and `config` categories actually carry.
fn went_public_incident() -> String {
    "STOP — the backup repository was private when this machine paired with it, and it is \
     public now. Nothing was uploaded by this run.\n\
     \n\
     1. Make the repository private again, in its GitHub settings.\n\
     2. Rotate every credential the bundle carries. A previous push may already have landed \
     while the repository was public, and bytes that were published cannot be un-published:\n\
     \x20  - the saved Claude Desktop logins — each profile's `config-tokenCache`, \
     `config-tokenCacheV2` and `desktop-state/` in the claude-acc profile store: sign out \
     and sign in again on every account.\n\
     \x20  - the Claude OAuth credentials in `accounts/*/.credentials.json` beside \
     config.toml: re-run `claude login` for each account.\n\
     \x20  - any provider API key held inline in config.toml: re-issue it at the provider.\n\
     \n\
     Rotating is the only thing that undoes this. Making the repository private again does \
     not: whoever read it while it was public still holds what they read."
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::github::RepoRef;
    use crate::sync::github::gate::assert_pushable;
    use tempfile::TempDir;

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    fn facts(private: bool) -> RepoFacts {
        RepoFacts {
            id: 11,
            private,
            visibility: if private { "private" } else { "public" }.into(),
            owner_login: "o".into(),
            owner_id: 7,
            archived: false,
            fork: false,
            admin_permission: false,
        }
    }

    fn paired(private: bool) -> Pairing {
        Pairing::of(&facts(private), now())
    }

    #[test]
    fn write_then_read_round_trips_and_the_file_is_owner_only() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(PAIRING_FILE);

        write_to(&path, &paired(true)).unwrap();
        assert_eq!(read_from(&path).unwrap(), Some(paired(true)));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    /// The record is rewritten on every successful check, so the mode has to
    /// survive a replacement and not only a creation.
    #[test]
    fn a_rewrite_replaces_the_record_and_keeps_it_owner_only() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(PAIRING_FILE);
        write_to(&path, &paired(true)).unwrap();
        write_to(
            &path,
            &Pairing {
                repo_id: 99,
                ..paired(true)
            },
        )
        .unwrap();

        assert_eq!(read_from(&path).unwrap().unwrap().repo_id, 99);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    /// `atomic_write` creates the destination's directory, so the very first
    /// pairing on a machine whose config dir does not exist yet still works.
    #[test]
    fn the_record_is_created_in_a_directory_that_did_not_exist() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested").join(PAIRING_FILE);
        write_to(&path, &paired(true)).unwrap();
        assert_eq!(read_from(&path).unwrap(), Some(paired(true)));
    }

    #[test]
    fn an_absent_record_is_a_first_pairing_not_an_error() {
        let dir = TempDir::new().unwrap();
        assert_eq!(read_from(&dir.path().join("never-written")).unwrap(), None);
    }

    #[test]
    fn a_corrupt_record_errors_instead_of_deserialising_into_a_passing_default() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(PAIRING_FILE);
        std::fs::write(&path, br#"{"repo_id": 11, "owner_id":"#).unwrap();

        let err = read_from(&path)
            .expect_err("a present but unparseable record must not read as a first pairing")
            .to_string();
        assert!(err.contains("unreadable"), "{err}");
        assert!(err.contains("re-trust"), "{err}");
    }

    #[test]
    fn no_record_is_first_contact_and_yields_the_pairing_to_persist() {
        let out = check_drift(None, &facts(true), true, now()).unwrap();
        assert!(out.first_contact);
        assert_eq!(out.record, paired(true));
        assert!(out.warnings.is_empty());
    }

    #[test]
    fn an_unchanged_repository_refreshes_the_record_silently() {
        let out = check_drift(Some(&paired(true)), &facts(true), true, now()).unwrap();
        assert!(!out.first_contact);
        assert_eq!(out.record, paired(true));
        assert!(out.warnings.is_empty());
    }

    /// The whole reason the record carries numeric ids: a login can be released
    /// and taken by someone else, and the name would look unchanged.
    #[test]
    fn a_changed_owner_id_refuses_and_names_the_resquat() {
        let record = Pairing {
            owner_id: 4242,
            ..paired(true)
        };
        let err = check_drift(Some(&record), &facts(true), true, now())
            .expect_err("that is not the account this machine paired with")
            .to_string();
        assert!(err.contains("REFUSING TO PUSH"), "{err}");
        assert!(err.contains("released and re-registered"), "{err}");
        assert!(err.contains("4242") && err.contains('7'), "{err}");
    }

    #[test]
    fn a_changed_repo_id_refuses_distinctly_from_a_changed_owner() {
        let record = Pairing {
            repo_id: 4242,
            ..paired(true)
        };
        let err = check_drift(Some(&record), &facts(true), true, now())
            .expect_err("that is not the repository this machine paired with")
            .to_string();
        assert!(
            err.contains("deleted and something else was created"),
            "{err}"
        );

        let owner_err = check_drift(
            Some(&Pairing {
                owner_id: 4242,
                ..paired(true)
            }),
            &facts(true),
            true,
            now(),
        )
        .unwrap_err()
        .to_string();
        assert_ne!(err, owner_err);
    }

    /// SAFE-02. A repository that *was* private and is public now is an
    /// incident: this machine has already pushed to it.
    #[test]
    fn a_private_to_public_transition_is_an_incident_naming_what_to_rotate() {
        let err = check_drift(Some(&paired(true)), &facts(false), true, now())
            .expect_err("a repository that went public with credentials in the bundle")
            .to_string();

        assert!(err.contains("Rotate every credential"), "{err}");
        assert!(err.contains("cannot be un-published"), "{err}");
        // Named categories, not a gesture at "your credentials".
        assert!(err.contains("config-tokenCache"), "{err}");
        assert!(err.contains(".credentials.json"), "{err}");
        assert!(err.contains("config.toml"), "{err}");
        // The order the message has to say things in.
        let fix = err.find("Make the repository private again").unwrap();
        let rotate = err.find("Rotate every credential").unwrap();
        assert!(
            err.find("public now").unwrap() < fix && fix < rotate,
            "{err}"
        );
    }

    /// Distinct from the plain not-private refusal: a repository that was never
    /// private was never trusted with anything, and nothing was pushed to it
    /// under a promise of privacy.
    #[test]
    fn the_incident_differs_from_the_gates_plain_not_private_refusal() {
        let incident = check_drift(Some(&paired(true)), &facts(false), true, now())
            .unwrap_err()
            .to_string();
        let plain = assert_pushable(&facts(false), &RepoRef::parse("o/n").unwrap(), true, now())
            .unwrap_err()
            .to_string();
        assert_ne!(incident, plain);
        // A repository public from the start never had a record saying private.
        assert!(check_drift(Some(&paired(false)), &facts(false), true, now()).is_ok());
    }

    /// D-04's carve-out has to survive **both** functions, in the order
    /// `setup.rs` calls them. If either overruled the other this run would fail.
    #[test]
    fn with_credentials_off_a_repository_that_went_public_warns_through_both_checks() {
        let out = check_drift(Some(&paired(true)), &facts(false), false, now())
            .expect("with nothing to rotate this is a warning, not an incident");
        assert_eq!(out.warnings.len(), 1, "{:?}", out.warnings);
        assert!(
            out.warnings[0].contains("private again"),
            "{:?}",
            out.warnings
        );
        assert!(
            !out.warnings[0].contains("un-published"),
            "{:?}",
            out.warnings
        );
        assert_eq!(out.record, paired(false));

        let (clearance, gate_warnings) =
            assert_pushable(&facts(false), &RepoRef::parse("o/n").unwrap(), false, now())
                .expect("the gate must not overrule the carve-out check_drift just allowed");
        assert_eq!(clearance.checked_at(), now());
        assert_eq!(gate_warnings.len(), 1, "{gate_warnings:?}");
    }
}
