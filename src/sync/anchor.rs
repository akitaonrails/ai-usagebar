//! The local monotonic rollback anchor: the last-seen snapshot counter,
//! persisted mode-0600 in the config directory (not the wipeable cache), so a
//! replayed-but-authentic old root is refused.
//!
//! **Why the AEAD cannot do this job.** Every other tampering attack this
//! format defends against produces something that fails to authenticate. A
//! rollback does not: an attacker with write access to the remote simply serves
//! an *older* snapshot root, which was genuinely produced by the real key and
//! verifies perfectly. Nothing inside the bundle can tell the client that a
//! newer one exists. Only state the attacker cannot reach — a counter kept on
//! this machine — detects it.
//!
//! **Why the config directory and not the cache.** The cache is documented as
//! wipeable and users (and packagers, and `rm -rf ~/.cache/*`) treat it that
//! way. A wiped anchor is a free rollback: it silently downgrades every future
//! fetch to first contact. The anchor is durable state, so it belongs beside
//! the configuration.
//!
//! Both entry points take a `&Path`, exactly as [`crate::cache::Cache::at`]
//! exists so nothing has to call `for_vendor` in a test. A thin wrapper
//! resolving the real location belongs to whichever phase owns the config
//! directory, not here — nothing in this module resolves a path.
//!
//! # The anchor path must be keyed to the *remote*, never to its `repo_id`
//!
//! This is a precondition on the caller, and it is load-bearing enough to be
//! stated rather than assumed. Whatever names the anchor file must come from
//! **local configuration — the remote's URL or account** — and never from the
//! `repo_id` the remote hands over.
//!
//! Sharding as `anchors/<repo_id>.json` is the obvious way to support several
//! bundles, and it is unavoidable eventually, because one [`Anchor`] holds
//! exactly one `repo_id`. It also silently nullifies this whole module. A served
//! root carrying a `repo_id` this machine has never seen would resolve to an
//! absent file, [`read_from`] would return `Ok(None)`, and [`accept`] returns
//! `Ok(())` for `None` *before* it compares anything: the `repo_id` guard below
//! becomes vacuous and every rollback is accepted as first contact. **A
//! `repo_id` this machine has not seen must read as a mismatch, not as first
//! contact**, which it does exactly when the path is keyed to the remote.
//!
//! The same rule stated the other way: first contact is a property of *this
//! machine and that remote*, and nothing the remote says may be allowed to
//! manufacture it. The format document says so in §9, for a re-implementer who
//! never reads this file.
//!
//! Note what this does *not* rest on. Since 1-10 the root's `repo_id` is bound
//! into its AEAD associated data, with the expected value supplied by the caller
//! from local configuration, so a root from another bundle fails its tag before
//! it is ever parsed. That closes the repo-swap leg cryptographically. The rule
//! above is still required for the *rollback* leg, which by construction cannot
//! be closed by any amount of authentication.
//!
//! [`accept`] is a pure function of the local anchor and the remote's claim, so
//! the rollback decision is exhaustively testable with no filesystem at all.
//!
//! Owned by plan 1-05.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};

/// The high-water mark for one bundle on this machine.
///
/// Holds no key material — the counter and the repository identifier are both
/// public — so `Debug` is derived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    /// Which bundle this counter belongs to. Two different bundles must never
    /// share a high-water mark.
    pub repo_id: String,
    /// The highest snapshot counter seen from that bundle.
    pub counter: u64,
}

/// Decide whether a remote snapshot may be accepted. Pure: the local anchor
/// arrives as an argument, so every branch is testable without touching a disk.
///
/// - `local == None` is **first contact**, and is accepted — which is precisely
///   why the caller must not let the remote decide whether the local anchor is
///   found (see the module docs). This is
///   trust-on-first-use, and it is an inherent residual gap rather than a bug: a
///   brand-new machine has nothing to compare against, so an attacker who
///   already controls the remote at the moment of the very first fetch can
///   serve an old snapshot and it will be believed. Every fetch afterwards is
///   protected. The gap is documented, not papered over — closing it would
///   require the user to carry a counter out of band, which is a different
///   trade than this design makes.
/// - A `repo_id` mismatch is always an error, at any counter. A counter is only
///   meaningful relative to the bundle it came from; comparing across bundles
///   would let an attacker reset the high-water mark by renaming.
/// - A counter *equal* to the anchor is fine — that is a re-read of the snapshot
///   already seen, which is the ordinary steady state.
/// - A *lower* counter is a rollback and is refused unless `allow_rollback`. The
///   message names the escape, because a user restoring an older snapshot on
///   purpose needs to know the word to type.
pub fn accept(
    local: Option<&Anchor>,
    remote_repo_id: &str,
    remote_counter: u64,
    allow_rollback: bool,
) -> Result<()> {
    let Some(local) = local else {
        return Ok(());
    };

    if local.repo_id != remote_repo_id {
        return Err(AppError::Other(format!(
            "this machine is anchored to bundle {:?} but the remote identifies as {:?} — \
             refusing, because a snapshot counter only means anything within one bundle",
            local.repo_id, remote_repo_id
        )));
    }

    if remote_counter < local.counter && !allow_rollback {
        return Err(AppError::Other(format!(
            "refusing a rolled-back snapshot: the remote offers counter {remote_counter} but \
             this machine has already seen {} from this bundle. An older snapshot is \
             authentic data replayed to hide a newer one. If you genuinely want the older \
             snapshot, re-run with --allow-rollback",
            local.counter
        )));
    }

    Ok(())
}

/// Read the anchor at `path`.
///
/// `Ok(None)` means the file is genuinely absent — first contact. A file that
/// exists but does not parse is an **error**, never `None`: treating corruption
/// as first contact would turn a damaged (or deliberately damaged) anchor into a
/// free rollback, which is precisely the attack this module exists to stop.
pub fn read_from(path: &Path) -> Result<Option<Anchor>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(AppError::io_at(path, e)),
    };
    let anchor: Anchor = serde_json::from_slice(&bytes).map_err(|e| {
        AppError::Other(format!(
            "the rollback anchor at {} exists but is unreadable ({e}) — refusing rather than \
             treating it as first contact, because that would silently permit a rollback. \
             Delete it only if you accept re-trusting whatever the remote currently offers",
            path.display()
        ))
    })?;
    Ok(Some(anchor))
}

/// Persist the anchor at `path`, atomically and mode 0600.
///
/// The atomic write is [`crate::cache::atomic_write`] — tempfile in the
/// destination's own directory, then `persist` — so this file inherits the
/// project's existing "no torn state file" invariant instead of reinventing it.
/// The mode is then set explicitly rather than trusting the temp file's
/// inherited one, the same belt-and-braces the Settings overlay applies to
/// `config.toml`.
pub fn write_to(path: &Path, anchor: &Anchor) -> Result<()> {
    let bytes = serde_json::to_vec(anchor)?;
    crate::cache::atomic_write(path, &bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| AppError::io_at(path, e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchored(counter: u64) -> Anchor {
        Anchor {
            repo_id: "bundle-a".into(),
            counter,
        }
    }

    #[test]
    fn first_contact_has_no_anchor_and_is_trusted() {
        assert!(accept(None, "bundle-a", 7, false).is_ok());
    }

    #[test]
    fn a_higher_or_equal_counter_is_accepted() {
        let local = anchored(7);
        assert!(accept(Some(&local), "bundle-a", 8, false).is_ok());
        // Equal is the steady state: the same snapshot read twice.
        assert!(accept(Some(&local), "bundle-a", 7, false).is_ok());
    }

    #[test]
    fn a_lower_counter_is_refused_and_the_message_names_the_escape() {
        let local = anchored(7);
        let err = accept(Some(&local), "bundle-a", 6, false)
            .expect_err("a rolled-back snapshot must be refused")
            .to_string();
        assert!(err.contains("--allow-rollback"));
        assert!(err.contains('6') && err.contains('7'));
    }

    #[test]
    fn a_lower_counter_is_accepted_when_rollback_is_explicitly_allowed() {
        assert!(accept(Some(&anchored(7)), "bundle-a", 6, true).is_ok());
    }

    #[test]
    fn a_mismatched_repo_id_is_refused_at_any_counter() {
        let local = anchored(7);
        for (counter, allow) in [(6, false), (7, false), (8, false), (99, true)] {
            let err = accept(Some(&local), "bundle-b", counter, allow)
                .expect_err("a counter from another bundle must never be compared")
                .to_string();
            assert!(err.contains("bundle-b"));
        }
    }

    #[test]
    fn write_then_read_round_trips_and_the_file_is_owner_only() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("rollback-anchor.json");
        let anchor = anchored(42);

        write_to(&path, &anchor).unwrap();
        assert_eq!(read_from(&path).unwrap(), Some(anchor));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn an_absent_anchor_is_first_contact_not_an_error() {
        let dir = tempfile::TempDir::new().unwrap();
        assert_eq!(read_from(&dir.path().join("never-written")).unwrap(), None);
    }

    #[test]
    fn a_corrupt_anchor_errors_instead_of_resetting_the_high_water_mark() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("rollback-anchor.json");
        std::fs::write(&path, b"{\"repo_id\": \"bundle-a\", \"counter\":").unwrap();

        let err = read_from(&path)
            .expect_err("a present but unparseable anchor must not read as first contact")
            .to_string();
        assert!(err.contains("would silently permit a rollback"));
    }

    #[test]
    fn overwriting_an_existing_anchor_keeps_it_owner_only() {
        // The mode must survive an update, not just creation — an anchor is
        // rewritten on every fetch.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("rollback-anchor.json");
        write_to(&path, &anchored(1)).unwrap();
        write_to(&path, &anchored(2)).unwrap();

        assert_eq!(read_from(&path).unwrap().unwrap().counter, 2);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
