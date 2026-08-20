//! Every manifest entry, turned into exactly one decision.
//!
//! **Nothing is dropped on the floor.** An entry whose path is refused, whose
//! policy excludes it, or whose destination this machine will not write is
//! still an [`ItemPlan`] — with no `dest` and a [`Disposition`] that says why —
//! because a silently discarded entry is a tampered bundle nobody can see (D6,
//! T-5-21).
//!
//! Nothing here writes. [`plan`] reads local metadata and local bytes and
//! returns a decision; `write::apply` is the only module that touches a
//! destination, and it is unreachable without `RestoreOptions::apply` (D1).
//!
//! # Digest before timestamp, always
//!
//! Identity is decided by comparing the manifest's ordered chunk ids against
//! the local file's, and it short-circuits everything else — including both
//! consents. That is D7: re-running an interrupted restore must report the
//! conflicts it genuinely has, which is none. A timestamp check running first
//! would turn every already-restored file into a `SkipLocalNewer` and make a
//! resumed restore look like a disaster.
//!
//! The local chunk ids are hashed **off the disk**, with the same
//! [`Keys::chunk_id`] and the same [`CHUNK_SIZE`] buffers the push side used,
//! so an untouched file is recognised as identical across two machines. The
//! local SQLite index is deliberately never consulted: it is a cache keyed on
//! the *push* side's stat tuple, and one stale row would declare a file the
//! user has since edited identical and skip it (T-5-24).
//!
//! # The remote timestamp is the snapshot's, one value for every item
//!
//! [`crate::sync::model::FileEntry`] carries `path`, `mode`, `true_len` and the
//! chunk ids — **no mtime** — so the remote side of every comparison is
//! `Root::created_at`, when the snapshot was captured. Adding a per-file mtime
//! is a `MANIFEST_VERSION` bump to an already-shipping wire format for a
//! refinement nothing yet needs; the upgrade path, for whoever needs
//! sub-snapshot granularity, is a per-file `mtime_ns` under `MANIFEST_VERSION`
//! 3, read here in preference to `created_at` when present.
//!
//! `created_at` is when the remote copy was captured, so "local mtime is after
//! the capture" is exactly "this machine changed it since". Plan 5-04 makes
//! that exact rather than merely conservative by stamping every restored file's
//! mtime to that same `created_at`: a restored-then-untouched file compares
//! equal, not newer, so the next pull of a newer snapshot updates it cleanly.
//!
//! ## What the comparison assumes, and what a wrong guess costs
//!
//! Both timestamps come from **different machines' clocks** — `created_at` from
//! whichever laptop pushed, the mtime from this one — so the assumption is only
//! that the two are within a snapshot's age of each other, which NTP makes true
//! and a few seconds of drift does not break. It can still be wrong in either
//! direction, so neither direction is allowed to be destructive:
//!
//! - **This clock runs fast** (or the pusher's runs slow): an unchanged local
//!   file looks newer, so it is skipped and named in the report. The user
//!   re-runs with `--force`. A skip costs a second command.
//! - **This clock runs slow**: a locally-changed file looks older and is
//!   updated. That is the direction that loses data, and it is why D3's backup
//!   is taken before the first byte even when nothing looked like a conflict,
//!   and why every overwritten item is named in the outcome. The recovery is
//!   the `tar -xzf` line [`crate::sync::restore::BackupRecord::rollback_command`]
//!   prints.
//!
//! Digest-first is what keeps drift cheap in practice: a file that did not
//! change is `SkipIdentical` before either clock is read, so only genuinely
//! diverged files can be misjudged at all.
//!
//! # Credentials get the strictest arm
//!
//! A locally-newer credential under `force` is [`Disposition::NeedsCredentialConfirm`],
//! never `Overwrite`; only `force_credentials` **alongside** `force` promotes
//! it. Silently reverting a live rotating OAuth token to a stale one is a
//! failure this project has already shipped once, in the
//! two-stores-fighting-over-a-refresh-token form, and a third path into that
//! family is not being built. A credential that is *not* locally newer is an
//! ordinary `Update`: the second consent guards the loss, not the category.
//!
//! # A Claude Desktop token cache is only restorable where its key lives
//!
//! `desktop-profiles/*/config-tokenCache{,V2}` are Chromium **safeStorage**
//! values: `v10` followed by AES-128-CBC ciphertext under a random secret that
//! lives in the login Keychain of *the machine that wrote them*. Carried to a
//! second Mac they are inert — and restoring one over a live token cache
//! replaces a working Desktop login with bytes Desktop cannot read.
//!
//! The decision is made by **attempting the decryption**, never by recording
//! where a blob came from: the local key either opens it or it does not, and
//! that is exactly the question. A provenance field would be one more thing to
//! keep in sync, one more thing a bundle could lie about, and still would not
//! answer it. See [`foreign_safe_storage`]; the refusal is
//! [`Disposition::ForeignSafeStorage`] and it never writes.
//!
//! `.credentials.json` is deliberately untouched by all of this. It is plain
//! JSON, it is Claude Code's own OAuth credential, and it is what makes the CLI
//! work on the second machine — the portable half of the bundle.
//!
//! # Restore never plans a deletion
//!
//! A local file the manifest does not mention is left exactly as it is. That is
//! a decision, not an omission: a snapshot is what one machine had, not an
//! assertion about what every machine should have. The `synced.json` baseline
//! [`crate::claude_desktop::merge`] uses to tell a deletion from "never had it"
//! is the right machinery for a future selective restore (REC-02, deferred to
//! v2); reaching for it here would build a second reconciliation model for a
//! case v1 does not have.
//!
//! Owned by plan 5-03.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use zeroize::Zeroizing;

use crate::config::SyncCategory;
use crate::error::Result;
use crate::safe_storage;
use crate::sync::CHUNK_SIZE;
use crate::sync::crypto::{ChunkId, Keys};
use crate::sync::keystore::{self, Store};
use crate::sync::model::{FileEntry, IndexObject};
use crate::sync::scope::CREDENTIAL_FILE;

use super::{
    Disposition, ItemPlan, PackSource, Resolved, RestoreCtx, RestoreOptions, RestorePlan, layout,
};

/// What this machine has at a destination.
struct LocalFacts {
    mtime: DateTime<Utc>,
    /// The file's ordered chunk ids, or `None` when identity could not be — or
    /// need not be — established: an unreadable file, or one whose length
    /// already differs from the manifest's.
    ///
    /// `None` rather than an empty list on purpose. A zero-byte manifest entry
    /// has no chunks either, and an unreadable file that compared equal to it
    /// would be skipped as "identical" without a byte ever having been read.
    chunk_ids: Option<Vec<ChunkId>>,
}

/// What the snapshot has. `created_at` is the snapshot's, not the file's — see
/// the module doc.
struct RemoteFacts<'a> {
    chunk_ids: &'a [ChunkId],
    created_at: DateTime<Utc>,
}

/// The destination as it stands right now.
enum Local {
    /// Nothing there. The common case on a fresh machine.
    Absent,
    /// Something there that this restore will not write over, and why.
    Refused(String),
    File(LocalFacts),
}

/// Decide about every file in the snapshot, writing nothing.
///
/// Manifest order is preserved and the count is exact: N entries in, N
/// [`ItemPlan`]s out.
pub fn plan(ctx: &RestoreCtx<'_>, resolved: &Resolved) -> Result<RestorePlan> {
    // This machine's Claude Safe Storage key is now one more thing the injected
    // [`crate::sync::keystore::Stores`] answers for, so there is one rule for
    // every machine-bound secret rather than two — and still no way for a test
    // to reach a real login Keychain. `plan_with_safe_key` stays exactly as
    // 6-09 left it: the seam its own tests drive.
    let safe_key = ctx.roots.stores.safe_key();
    plan_with_safe_key(ctx, resolved, safe_key)
}

fn plan_with_safe_key(
    ctx: &RestoreCtx<'_>,
    resolved: &Resolved,
    safe_key: Option<safe_storage::Key>,
) -> Result<RestorePlan> {
    let keys = resolved.packs.keys();
    let created_at = resolved.root.created_at;

    let items: Vec<ItemPlan> = resolved
        .manifest
        .files
        .iter()
        .map(|file| {
            let category = category_of(&file.path);
            let (dest, disposition) = decide_entry(
                ctx,
                keys,
                file,
                category,
                created_at,
                safe_key.as_ref(),
                &resolved.packs,
            );
            ItemPlan {
                manifest_path: file.path.clone(),
                dest,
                category,
                true_len: file.true_len,
                chunks: file.chunks.clone(),
                disposition,
            }
        })
        .collect();

    let (packs_needed, bytes_to_fetch) = to_fetch(&resolved.index, &items);
    Ok(RestorePlan {
        items,
        counter: resolved.root.counter,
        created_at,
        repo_id: resolved.root.repo_id.clone(),
        packs_needed,
        bytes_to_fetch,
    })
}

/// Policy, then path, then the destination, then [`decide`].
///
/// The policy check runs on the **manifest path**, before any resolution, so a
/// bundle naming machine-bound state is refused whatever root it claims (D4).
/// Both refusals keep `dest: None`, which is the frozen contract for
/// [`Disposition::ExcludedByPolicy`] and [`Disposition::RejectedPath`].
fn decide_entry(
    ctx: &RestoreCtx<'_>,
    keys: &Keys,
    file: &FileEntry,
    category: SyncCategory,
    created_at: DateTime<Utc>,
    safe_key: Option<&safe_storage::Key>,
    packs: &PackSource,
) -> (Option<PathBuf>, Disposition) {
    // **Before anything resolves a path.** A `keystore/…` entry names a store,
    // not a file, and `layout::from_manifest_path` refuses that prefix by
    // design — so reaching it would report a live credential as a tampered path
    // instead of writing it where it belongs. `dest` stays `None` for every
    // store, which is what keeps `write::apply` from ever treating one as a
    // file: there is structurally no path for it to be written to.
    if keystore::Store::is_store_path(&file.path) {
        return (None, decide_store(ctx, keys, file));
    }
    if !layout::accept_for_write(Path::new(&file.path)) {
        return (None, Disposition::ExcludedByPolicy);
    }
    let dest = match layout::from_manifest_path(ctx.roots, &file.path) {
        Ok(dest) => dest,
        Err(why) => return (None, Disposition::RejectedPath(why.to_string())),
    };

    // Before the destination is even stat'ed: the refusal holds whether or not
    // a local token cache is there to lose, and no consent promotes it. `dest`
    // stays `None` for the same reason the other two refusals keep it — an
    // entry that will never be written has structurally nowhere to be written.
    if foreign_safe_storage(safe_key, packs, file) {
        return (None, Disposition::ForeignSafeStorage);
    }

    let local = match local_at(&dest, file, keys) {
        Local::Absent => None,
        Local::File(facts) => Some(facts),
        // A destination this machine will not write over. `RejectedPath` is the
        // only variant that carries a reason, and no consent promotes it —
        // which is the point for the symlink case (T-5-22).
        Local::Refused(why) => return (None, Disposition::RejectedPath(why)),
    };

    let remote = RemoteFacts {
        chunk_ids: &file.chunks,
        created_at,
    };
    let disposition = decide(
        local.as_ref(),
        &remote,
        credential_bearing(&file.path, category),
        &ctx.opts,
    );
    (Some(dest), disposition)
}

/// The whole decision, pure: local stat facts, the manifest entry and the
/// snapshot time arrive as arguments, so every branch is tested without a
/// filesystem and without a clock.
fn decide(
    local: Option<&LocalFacts>,
    remote: &RemoteFacts<'_>,
    credential: bool,
    opts: &RestoreOptions,
) -> Disposition {
    let Some(local) = local else {
        return Disposition::Create;
    };

    // D7, and it runs before either clock is read.
    if local.chunk_ids.as_deref() == Some(remote.chunk_ids) {
        return Disposition::SkipIdentical;
    }

    let (local_mtime, remote_mtime) = (local.mtime, remote.created_at);
    // Equal is not newer: a file 5-04 restored and nobody touched carries the
    // snapshot's own timestamp, and must update rather than look like a
    // conflict against the next snapshot.
    if local_mtime <= remote_mtime {
        return Disposition::Update;
    }
    if !opts.force {
        return Disposition::SkipLocalNewer {
            local_mtime,
            remote_mtime,
        };
    }
    if credential && !opts.force_credentials {
        return Disposition::NeedsCredentialConfirm {
            local_mtime,
            remote_mtime,
        };
    }
    Disposition::Overwrite {
        local_mtime,
        remote_mtime,
    }
}

/// Decide about one machine-bound store.
///
/// Four answers and no timestamps, because a store has none to compare:
///
/// | this machine holds | consent | answer |
/// |---|---|---|
/// | nothing, or no such store here | — | [`Disposition::Create`] / [`Disposition::ExcludedByPolicy`] |
/// | the same credential | — | [`Disposition::SkipIdentical`] |
/// | a **different** credential | none | [`Disposition::ReplacesLiveCredential`] |
/// | a **different** credential | `--force-credentials` | [`Disposition::Update`] |
///
/// Identity is decided by hashing what the store holds with the same
/// [`Keys::chunk_id`] the push side used — digest-first, the same rule
/// [`decide`] follows for a file, and the reason a repeated pull onto the
/// machine that pushed asks the user nothing. It reads no data pack, so a dry
/// run answers exactly what the applying run will.
///
/// **`--force` alone never promotes this**, matching
/// [`Disposition::NeedsCredentialConfirm`]. `--force` means "overwrite
/// something newer", and a Keychain item has no mtime for that to be about; the
/// single specific consent is the credential one.
///
/// An unknown `keystore/…` entry — from a build later than this one — is
/// [`Disposition::ExcludedByPolicy`]: named in the report, never written, and
/// never mistaken for a path.
fn decide_store(ctx: &RestoreCtx<'_>, keys: &Keys, file: &FileEntry) -> Disposition {
    let Some(store) = Store::from_manifest_path(&file.path) else {
        return Disposition::ExcludedByPolicy;
    };
    // A store this build has no way to write — a macOS Keychain entry arriving
    // on Linux, where Claude Code keeps a real file instead. Refused here, in
    // the planner, rather than failing part-way through the write.
    if !ctx.roots.stores.writable(store) {
        return Disposition::ExcludedByPolicy;
    }
    // A read failure is not "there is nothing here". A locked Keychain read as
    // an empty one would make the next line call a live credential absent and
    // replace it without ever asking.
    let Ok(local) = ctx.roots.stores.read(store) else {
        return Disposition::ReplacesLiveCredential;
    };
    let Some(local) = local.filter(|v| !v.is_empty()) else {
        return Disposition::Create;
    };
    if store_chunk_ids(keys, local.as_bytes()) == file.chunks {
        return Disposition::SkipIdentical;
    }
    if ctx.opts.force_credentials {
        return Disposition::Update;
    }
    Disposition::ReplacesLiveCredential
}

/// A store value's ordered chunk ids, hashed exactly as
/// [`crate::sync::push::packer`] hashed them, so "the same credential" means
/// the same bytes and nothing looser.
fn store_chunk_ids(keys: &Keys, value: &[u8]) -> Vec<ChunkId> {
    value.chunks(CHUNK_SIZE).map(|b| keys.chunk_id(b)).collect()
}

/// Is this incoming file a safeStorage blob that this machine cannot open?
///
/// Answered by attempting the decryption. **Only the boolean escapes**: the
/// plaintext is dropped through [`Zeroizing`] and neither it, the key, nor any
/// fragment of either reaches a return value, a log line or an error message.
///
/// `false` whenever the answer cannot be established, which is the honest
/// reading of "this is not known to be foreign". In particular a dry run has no
/// data packs to read — [`super::fetch::resolve`] downloads file content only
/// under `apply` — so a dry run reports a token cache's ordinary disposition
/// and the run that would actually write is the run that refuses. The gate
/// guards the write, and the write is where the loss would happen.
#[cfg(target_os = "macos")]
fn foreign_safe_storage(
    safe_key: Option<&safe_storage::Key>,
    packs: &PackSource,
    entry: &FileEntry,
) -> bool {
    /// A token cache is a few hundred bytes of base64. The ceiling is what
    /// keeps a 50 MB transcript from being decrypted into memory to answer a
    /// question its size has already answered — and, being under
    /// [`CHUNK_SIZE`], it also makes "one chunk" true rather than assumed.
    const MAX_LEN: u64 = 64 * 1024;

    if entry.true_len == 0 || entry.true_len > MAX_LEN {
        return false;
    }
    let [id] = entry.chunks[..] else {
        return false;
    };
    let Ok(bytes) = packs.chunk(&id) else {
        return false; // a dry run: the data packs were never downloaded
    };
    let Ok(value) = std::str::from_utf8(&bytes) else {
        return false;
    };
    if !safe_storage::looks_like_value(value) {
        return false;
    }
    // No local key at all is the same answer, and for the same reason: nothing
    // on this machine can read the blob, so writing it leaves Claude Desktop a
    // token cache it cannot decrypt — worse than leaving it none.
    safe_key.is_none_or(|key| {
        safe_storage::decrypt(key, value)
            .map(Zeroizing::new)
            .is_err()
    })
}

/// Not macOS: Chromium's safeStorage is a different scheme backed by a
/// different key store here, [`safe_storage::macos_key`] does not exist, and
/// restore behaves exactly as it always has.
#[cfg(not(target_os = "macos"))]
fn foreign_safe_storage(
    _safe_key: Option<&safe_storage::Key>,
    _packs: &PackSource,
    _entry: &FileEntry,
) -> bool {
    false
}

/// Would overwriting this entry cost a live secret?
///
/// The whole profile store, plus any `.credentials.json` under any root. The
/// file-name half is what catches `config/accounts/*/.credentials.json`, which
/// `scope` collects under [`SyncCategory::Config`] alongside `config.toml`, and
/// `claude-home/.credentials.json`, which it would file under `Routines` — a
/// category-only rule would miss both.
///
/// The name is a [`crate::sync::FixedName`], and the comparison it forces is
/// the whole of F-1's fix. This predicate ran byte-exactly until Phase 5's
/// audit: a manifest naming `.Credentials.json` classified as *not*
/// credential-bearing while `local_at`'s `symlink_metadata` — going through the
/// kernel, which folds case on APFS — found the **live** credential at that
/// exact path. `decide` then took the `Overwrite` arm, `write::apply`'s
/// `NeedsCredentialConfirm` tripwire had nothing to fire on, and the CLI's gate
/// filtered for a variant that no longer existed. `--force` alone reverted a
/// live OAuth token, which is precisely what D2 exists to prevent.
fn credential_bearing(manifest_path: &str, category: SyncCategory) -> bool {
    category == SyncCategory::Credentials
        || manifest_path
            .rsplit('/')
            .next()
            .is_some_and(|name| CREDENTIAL_FILE.matches(name))
}

/// Stat the destination and, when it is a plain file, hash it.
///
/// [`fs::symlink_metadata`] and never [`fs::metadata`]: a link planted at a
/// destination must be *seen* as a link rather than followed to whatever it
/// points at, which is the difference between refusing a write and performing
/// it somewhere the bundle chose (T-5-22). It is the only stat call in this
/// module.
///
/// Anything that is not a regular file — a link, a directory, a socket, a
/// device node — is refused by name. Nothing here clamps or repairs it: the
/// user is told what is in the way and decides.
fn local_at(dest: &Path, entry: &FileEntry, keys: &Keys) -> Local {
    let md = match fs::symlink_metadata(dest) {
        Ok(md) => md,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Local::Absent,
        Err(e) => {
            return Local::Refused(format!("{} could not be examined: {e}", dest.display()));
        }
    };

    if let Some(what) = not_a_plain_file(&md) {
        return Local::Refused(format!(
            "{} is {what}, and restore writes only over a regular file",
            dest.display()
        ));
    }

    let mtime = match md.modified() {
        Ok(t) => DateTime::<Utc>::from(t),
        Err(e) => {
            return Local::Refused(format!(
                "{} has no readable modification time, so it cannot be shown to be older \
                 than the snapshot: {e}",
                dest.display()
            ));
        }
    };

    Local::File(LocalFacts {
        mtime,
        chunk_ids: chunk_ids_of(dest, md.len(), entry, keys),
    })
}

/// The kind of thing in the way, or `None` for an ordinary file.
fn not_a_plain_file(md: &fs::Metadata) -> Option<&'static str> {
    if md.file_type().is_symlink() {
        Some("a symbolic link")
    } else if md.is_dir() {
        Some("a directory")
    } else if md.is_file() {
        None
    } else {
        Some("neither a regular file nor a directory")
    }
}

/// The local file's ordered chunk ids, or `None` when it is pointless or
/// impossible to compute them.
///
/// The length check first: a file whose size differs from the manifest's
/// `true_len` cannot share its chunk ids, and learning that from the stat
/// beats reading a 50 MB transcript to reach the same answer.
fn chunk_ids_of(path: &Path, size: u64, entry: &FileEntry, keys: &Keys) -> Option<Vec<ChunkId>> {
    if size != entry.true_len {
        return None;
    }
    let mut file = fs::File::open(path).ok()?;
    let mut ids = Vec::new();
    // Zeroizing: these are the plaintext bytes of, among other things, a live
    // OAuth token. One buffer, reused, wiped on the way out.
    let mut buf: Zeroizing<Vec<u8>> = Zeroizing::new(Vec::with_capacity(CHUNK_SIZE));
    loop {
        buf.clear();
        file.by_ref()
            .take(CHUNK_SIZE as u64)
            .read_to_end(&mut buf)
            .ok()?;
        if buf.is_empty() {
            break;
        }
        ids.push(keys.chunk_id(&buf));
        // A short read is EOF: `Take::read_to_end` fills otherwise.
        if buf.len() < CHUNK_SIZE {
            break;
        }
    }
    Some(ids)
}

/// The packs a real run would download, and their sealed size.
///
/// Counted from the items that will **actually be written**, never from every
/// entry in the manifest: a figure that counted skipped items would overstate
/// the cost of the operation, which is the direction that makes a safe restore
/// look alarming.
///
/// One `HashMap` pass rather than a `IndexObject::resolve` per chunk, which its
/// own doc asks of a caller resolving thousands of ids against one index.
fn to_fetch(index: &IndexObject, items: &[ItemPlan]) -> (usize, u64) {
    let pack_of: HashMap<ChunkId, ChunkId> = index.entries.iter().map(|e| (e.id, e.pack)).collect();
    let needed: HashSet<ChunkId> = items
        .iter()
        .filter(|item| item.disposition.writes())
        .flat_map(|item| item.chunks.iter())
        .filter_map(|id| pack_of.get(id).copied())
        .collect();
    let bytes = index
        .entries
        .iter()
        .filter(|e| needed.contains(&e.pack))
        .map(|e| u64::from(e.clen))
        .sum();
    (needed.len(), bytes)
}

/// Which category a bundle path belongs to, from its root prefix and the shape
/// beneath it — the same split `scope`'s collectors made on the way out.
fn category_of(manifest_path: &str) -> SyncCategory {
    let (prefix, rest) = manifest_path.split_once('/').unwrap_or((manifest_path, ""));
    match prefix {
        // A store is a credential and nothing else, which is what puts it under
        // the switch deciding whether it travels at all.
        keystore::PREFIX => SyncCategory::Credentials,
        "desktop-profiles" => SyncCategory::Credentials,
        // Claude Code's own credential file, which `scope` collects under
        // `Credentials`. Both directions must agree, or the report files it
        // under a category its owner never switched on.
        "claude-home"
            if rest
                .rsplit('/')
                .next()
                .is_some_and(|n| CREDENTIAL_FILE.matches(n)) =>
        {
            SyncCategory::Credentials
        }
        "desktop-data" if rest.starts_with("claude-code-sessions/") => SyncCategory::ChatIndex,
        "desktop-data" => SyncCategory::Routines,
        "claude-home" if rest.starts_with("projects/") => SyncCategory::Transcripts,
        "claude-home" => SyncCategory::Routines,
        _ => SyncCategory::Config,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::chunk;
    use crate::sync::crypto::{KdfParams, Keyfile};
    use crate::sync::github::token::TokenSource;
    use crate::sync::github::{Client, Endpoints, RepoRef};
    use crate::sync::model::{IndexEntry, Manifest, Root};
    use crate::sync::pack::PackWriter;
    use crate::sync::restore::PackSource;
    use crate::sync::{CHUNK_SIZE, SyncRoots};
    use std::fs;
    use std::time::{Duration, SystemTime};
    use tempfile::TempDir;
    use zeroize::Zeroizing;

    /// Microseconds instead of ~1.5 s and a gibibyte. The AUR `check()` runs
    /// these on an installer's machine.
    const CHEAP: KdfParams = KdfParams {
        m_kib: 8,
        t: 1,
        p: 1,
    };

    /// The snapshot's capture time. A fixed constant: nothing here reads a
    /// clock, which is what `decide` taking `created_at` as an argument buys.
    const SNAPSHOT: DateTime<Utc> = match DateTime::from_timestamp(1_700_000_000, 0) {
        Some(t) => t,
        None => panic!("a fixed timestamp"),
    };

    fn keys() -> Keys {
        Keyfile::create_with_floor(b"a-test-passphrase", CHEAP, CHEAP.m_kib)
            .expect("keyfile creation")
            .1
    }

    fn id(keys: &Keys, body: &[u8]) -> ChunkId {
        keys.chunk_id(body)
    }

    fn local(mtime: DateTime<Utc>, chunk_ids: Option<Vec<ChunkId>>) -> LocalFacts {
        LocalFacts { mtime, chunk_ids }
    }

    fn remote(chunk_ids: &[ChunkId]) -> RemoteFacts<'_> {
        RemoteFacts {
            chunk_ids,
            created_at: SNAPSHOT,
        }
    }

    fn opts(force: bool, force_credentials: bool) -> RestoreOptions {
        RestoreOptions {
            force,
            force_credentials,
            ..Default::default()
        }
    }

    const ORDINARY: bool = false;
    const CREDENTIAL: bool = true;

    // ---------------------------------------------------------------- decide

    #[test]
    fn an_absent_local_file_is_created() {
        let k = keys();
        let ids = vec![id(&k, b"remote")];
        assert_eq!(
            decide(None, &remote(&ids), ORDINARY, &opts(true, true)),
            Disposition::Create,
            "force must not change what a fresh machine does"
        );
    }

    /// D7: the digest short-circuits everything, so an interrupted restore
    /// re-run reports the conflicts it genuinely has — none.
    #[test]
    fn identical_chunk_ids_win_before_any_clock_is_consulted() {
        let k = keys();
        let ids = vec![id(&k, b"same"), id(&k, b"bytes")];
        for mtime in [
            SNAPSHOT - Duration::from_secs(3600),
            SNAPSHOT,
            SNAPSHOT + Duration::from_secs(3600),
        ] {
            for (force, force_creds) in [(false, false), (true, false), (true, true)] {
                for credential in [ORDINARY, CREDENTIAL] {
                    assert_eq!(
                        decide(
                            Some(&local(mtime, Some(ids.clone()))),
                            &remote(&ids),
                            credential,
                            &opts(force, force_creds),
                        ),
                        Disposition::SkipIdentical,
                        "{mtime} force={force} creds={force_creds} credential={credential}"
                    );
                }
            }
        }
    }

    #[test]
    fn an_older_local_file_is_updated_without_any_force() {
        let k = keys();
        let mine = vec![id(&k, b"mine")];
        let theirs = vec![id(&k, b"theirs")];
        assert_eq!(
            decide(
                Some(&local(SNAPSHOT - Duration::from_secs(1), Some(mine))),
                &remote(&theirs),
                ORDINARY,
                &RestoreOptions::default(),
            ),
            Disposition::Update
        );
    }

    /// SAFE-03: the skip carries both times so the report can say *why*.
    #[test]
    fn a_locally_newer_file_is_skipped_and_names_both_times() {
        let k = keys();
        let mine = vec![id(&k, b"mine")];
        let theirs = vec![id(&k, b"theirs")];
        let newer = SNAPSHOT + Duration::from_secs(1);
        assert_eq!(
            decide(
                Some(&local(newer, Some(mine))),
                &remote(&theirs),
                ORDINARY,
                &RestoreOptions::default(),
            ),
            Disposition::SkipLocalNewer {
                local_mtime: newer,
                remote_mtime: SNAPSHOT,
            }
        );
    }

    #[test]
    fn force_overwrites_a_locally_newer_ordinary_file() {
        let k = keys();
        let newer = SNAPSHOT + Duration::from_secs(1);
        assert_eq!(
            decide(
                Some(&local(newer, Some(vec![id(&k, b"mine")]))),
                &remote(&[id(&k, b"theirs")]),
                ORDINARY,
                &opts(true, false),
            ),
            Disposition::Overwrite {
                local_mtime: newer,
                remote_mtime: SNAPSHOT,
            }
        );
    }

    /// D2, the whole point of `force_credentials` being a separate field.
    #[test]
    fn force_alone_never_overwrites_a_locally_newer_credential() {
        let k = keys();
        let newer = SNAPSHOT + Duration::from_secs(1);
        assert_eq!(
            decide(
                Some(&local(newer, Some(vec![id(&k, b"live-token")]))),
                &remote(&[id(&k, b"stale-token")]),
                CREDENTIAL,
                &opts(true, false),
            ),
            Disposition::NeedsCredentialConfirm {
                local_mtime: newer,
                remote_mtime: SNAPSHOT,
            }
        );
    }

    #[test]
    fn the_second_consent_promotes_a_credential_and_only_alongside_force() {
        let k = keys();
        let newer = SNAPSHOT + Duration::from_secs(1);
        let call = |o: RestoreOptions| {
            decide(
                Some(&local(newer, Some(vec![id(&k, b"live-token")]))),
                &remote(&[id(&k, b"stale-token")]),
                CREDENTIAL,
                &o,
            )
        };
        assert_eq!(
            call(opts(true, true)),
            Disposition::Overwrite {
                local_mtime: newer,
                remote_mtime: SNAPSHOT,
            }
        );
        assert_eq!(
            call(opts(false, true)),
            Disposition::SkipLocalNewer {
                local_mtime: newer,
                remote_mtime: SNAPSHOT,
            },
            "`force_credentials` is a second consent on top of `force`, not a substitute for it"
        );
    }

    /// The second confirmation guards the *loss*, not the category.
    #[test]
    fn a_credential_that_is_not_locally_newer_is_an_ordinary_update() {
        let k = keys();
        assert_eq!(
            decide(
                Some(&local(
                    SNAPSHOT - Duration::from_secs(60),
                    Some(vec![id(&k, b"stale")])
                )),
                &remote(&[id(&k, b"fresh")]),
                CREDENTIAL,
                &RestoreOptions::default(),
            ),
            Disposition::Update
        );
    }

    /// The boundary, pinned from both sides: equal is not newer.
    #[test]
    fn an_mtime_equal_to_the_snapshot_time_updates_and_one_nanosecond_later_does_not() {
        let k = keys();
        let mine = vec![id(&k, b"mine")];
        let theirs = vec![id(&k, b"theirs")];
        let at = |mtime| {
            decide(
                Some(&local(mtime, Some(mine.clone()))),
                &remote(&theirs),
                ORDINARY,
                &RestoreOptions::default(),
            )
        };
        assert_eq!(at(SNAPSHOT), Disposition::Update);
        assert_eq!(
            at(SNAPSHOT - Duration::from_nanos(1)),
            Disposition::Update,
            "a hair older is still older"
        );
        assert!(matches!(
            at(SNAPSHOT + Duration::from_nanos(1)),
            Disposition::SkipLocalNewer { .. }
        ));
    }

    /// An unreadable file has no chunk ids, and "no chunk ids" must never
    /// compare equal to a zero-chunk manifest entry.
    #[test]
    fn a_local_file_whose_bytes_could_not_be_read_is_never_identical() {
        assert_eq!(
            decide(
                Some(&local(SNAPSHOT - Duration::from_secs(1), None)),
                &remote(&[]),
                ORDINARY,
                &RestoreOptions::default(),
            ),
            Disposition::Update,
            "an empty remote entry must not swallow an unreadable local file"
        );
        assert!(
            matches!(
                decide(
                    Some(&local(SNAPSHOT + Duration::from_secs(1), None)),
                    &remote(&[]),
                    ORDINARY,
                    &RestoreOptions::default(),
                ),
                Disposition::SkipLocalNewer { .. }
            ),
            "and it still gets SAFE-03's protection"
        );
    }

    // ------------------------------------------------ credential classification

    #[test]
    fn the_credential_arm_covers_the_profile_store_and_every_dot_credentials_json() {
        for path in [
            "desktop-profiles/work/meta.json",
            "desktop-profiles/work/token-cache.json",
            "config/accounts/work/.credentials.json",
            "claude-home/.credentials.json",
            // F-1: on macOS these name the very same files as the two above.
            "config/accounts/work/.Credentials.json",
            "claude-home/.CREDENTIALS.JSON",
            "config/accounts/work/.cReDeNtIaLs.jSoN",
        ] {
            assert!(
                credential_bearing(path, category_of(path)),
                "{path} would lose its second consent"
            );
        }
        for path in [
            "config/config.toml",
            "claude-home/scheduled-tasks/daily.json",
            "claude-home/projects/repo/session.jsonl",
            "desktop-data/claude-code-sessions/a/o/local_1.json",
        ] {
            assert!(
                !credential_bearing(path, category_of(path)),
                "{path} would demand a confirmation it does not need"
            );
        }
    }

    #[test]
    fn each_root_prefix_lands_in_the_category_its_collector_came_from() {
        for (path, expected) in [
            ("config/config.toml", SyncCategory::Config),
            (
                "config/accounts/work/.credentials.json",
                SyncCategory::Config,
            ),
            ("desktop-profiles/work/meta.json", SyncCategory::Credentials),
            (
                "desktop-data/claude-code-sessions/a/o/local_1.json",
                SyncCategory::ChatIndex,
            ),
            (
                "desktop-data/a/o/scheduled-tasks.json",
                SyncCategory::Routines,
            ),
            (
                "claude-home/scheduled-tasks/daily.json",
                SyncCategory::Routines,
            ),
            (
                "claude-home/projects/repo/s.jsonl",
                SyncCategory::Transcripts,
            ),
        ] {
            assert_eq!(category_of(path), expected, "{path}");
        }
    }

    // ------------------------------------------------------------------ plan

    /// Everything `plan` needs, with no network and no real `$HOME`.
    struct Machine {
        dir: TempDir,
        roots: SyncRoots,
        repo: RepoRef,
        passphrase: Zeroizing<String>,
    }

    impl Machine {
        fn new() -> Self {
            let dir = TempDir::new().unwrap();
            let home = dir.path().join("bob");
            let roots = SyncRoots::at(
                home.join(".config/ai-usagebar/config.toml"),
                home.join(".config/ai-usagebar"),
                home.join("desktop"),
                home.join("profiles"),
                home.join(".claude"),
            );
            Self {
                dir,
                roots,
                repo: RepoRef::parse("o/n").unwrap(),
                passphrase: Zeroizing::new("a-test-passphrase".into()),
            }
        }

        /// A destination, seeded with `body` and stamped with `mtime`.
        fn seed(&self, manifest_path: &str, body: &[u8], mtime: DateTime<Utc>) -> PathBuf {
            let dest = layout::from_manifest_path(&self.roots, manifest_path).unwrap();
            fs::create_dir_all(dest.parent().unwrap()).unwrap();
            fs::write(&dest, body).unwrap();
            let at = SystemTime::UNIX_EPOCH
                + Duration::from_nanos(
                    u64::try_from(mtime.timestamp_nanos_opt().unwrap()).unwrap(),
                );
            fs::File::options()
                .write(true)
                .open(&dest)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(at))
                .unwrap();
            dest
        }

        fn client(&self) -> Client {
            Client::new(
                &Endpoints {
                    api_base: "http://127.0.0.1:1".into(),
                    uploads_base: "http://127.0.0.1:1".into(),
                },
                Zeroizing::new("github_pat_fixture_not_a_real_token".into()),
                TokenSource::Env,
            )
            .unwrap()
        }

        fn ctx<'a>(&'a self, client: &'a Client, opts: RestoreOptions) -> RestoreCtx<'a> {
            RestoreCtx {
                client,
                repo: &self.repo,
                roots: &self.roots,
                repo_id: "github:1",
                passphrase: &self.passphrase,
                anchor_path: &self.roots.config_dir,
                backups_dir: &self.roots.config_dir,
                opts,
                now: SNAPSHOT,
            }
        }
    }

    /// [`snapshot`], but with the data packs really present — the state
    /// `plan` is in under `--apply`, and the only one in which the incoming
    /// bytes can be read at all.
    fn snapshot_with_packs(entries: &[(&str, &[u8])]) -> Resolved {
        let k = keys();
        let mut writer = PackWriter::new();
        let mut files = Vec::new();
        for (path, body) in entries {
            let chunks: Vec<ChunkId> = chunk::split(body)
                .map(|block| {
                    let blob = chunk::seal_chunk(&k, block).unwrap();
                    let id = blob.id;
                    writer.push(blob);
                    id
                })
                .collect();
            files.push(FileEntry {
                path: (*path).to_string(),
                mode: 0o600,
                true_len: body.len() as u64,
                chunks,
            });
        }
        let (pack_id, bytes) = writer.finish(&k).unwrap();
        let mut packs = PackSource::empty(k);
        packs.add(pack_id, bytes).unwrap();
        Resolved {
            root: Root::new(
                7,
                SNAPSHOT,
                "github:1".into(),
                Vec::new(),
                KdfParams::default(),
            ),
            manifest: Manifest::new(files),
            index: IndexObject::new(Vec::new(), Vec::new()),
            packs,
        }
    }

    fn applying() -> RestoreOptions {
        RestoreOptions {
            apply: true,
            ..Default::default()
        }
    }

    fn planned(m: &Machine, resolved: &Resolved, key: Option<safe_storage::Key>) -> RestorePlan {
        let client = m.client();
        plan_with_safe_key(&m.ctx(&client, applying()), resolved, key)
            .expect("planning is infallible here")
    }

    fn disposition_of<'a>(plan: &'a RestorePlan, path: &str) -> &'a Disposition {
        &plan
            .items
            .iter()
            .find(|i| i.manifest_path == path)
            .unwrap_or_else(|| panic!("{path} is missing from the plan"))
            .disposition
    }

    /// A snapshot of `(manifest path, body)` pairs, one chunk and one pack per
    /// entry — enough for every decision `plan` makes.
    fn snapshot(entries: &[(&str, &[u8])]) -> Resolved {
        let k = keys();
        let mut files = Vec::new();
        let mut index = Vec::new();
        for (i, (path, body)) in entries.iter().enumerate() {
            let chunk = k.chunk_id(body);
            files.push(FileEntry {
                path: (*path).to_string(),
                mode: 0o600,
                true_len: body.len() as u64,
                chunks: if body.is_empty() {
                    Vec::new()
                } else {
                    vec![chunk]
                },
            });
            index.push(IndexEntry {
                id: chunk,
                pack: ChunkId::from_bytes([i as u8; 32]),
                offset: 0,
                clen: 100 + i as u32,
                true_len: body.len() as u32,
            });
        }
        Resolved {
            root: Root::new(
                7,
                SNAPSHOT,
                "github:1".into(),
                Vec::new(),
                KdfParams::default(),
            ),
            manifest: Manifest::new(files),
            index: IndexObject::new(index, Vec::new()),
            packs: PackSource::empty(k),
        }
    }

    fn plan_with(machine: &Machine, resolved: &Resolved, opts: RestoreOptions) -> RestorePlan {
        let client = machine.client();
        plan(&machine.ctx(&client, opts), resolved).expect("planning is infallible here")
    }

    /// D6: N entries in, N `ItemPlan`s out — including the two that are refused.
    #[test]
    fn every_manifest_entry_becomes_exactly_one_item_plan_refusals_included() {
        let m = Machine::new();
        m.seed(
            "config/config.toml",
            b"[sync]\nenabled = true\n",
            SNAPSHOT - Duration::from_secs(60),
        );
        let identical = b"{\"tasks\":[]}";
        m.seed(
            "claude-home/scheduled-tasks/daily.json",
            identical,
            SNAPSHOT - Duration::from_secs(60),
        );
        m.seed(
            "config/accounts/work/.credentials.json",
            b"{\"token\":\"live\"}",
            SNAPSHOT + Duration::from_secs(60),
        );

        let resolved = snapshot(&[
            ("config/config.toml", b"[sync]\nenabled = false\n"),
            ("claude-home/scheduled-tasks/daily.json", identical),
            ("config/accounts/work/.credentials.json", b"{\"t\":\"old\"}"),
            ("claude-home/projects/new/session.jsonl", b"{}"),
            ("config/bridge-state.json", b"machine-bound"),
            ("config/../../../../etc/shadow", b"hostile"),
        ]);

        let plan = plan_with(&m, &resolved, RestoreOptions::default());
        assert_eq!(
            plan.items.len(),
            resolved.manifest.files.len(),
            "an entry was dropped on the floor"
        );

        let by_path = |p: &str| {
            plan.items
                .iter()
                .find(|i| i.manifest_path == p)
                .unwrap_or_else(|| panic!("{p} is missing from the plan"))
        };
        assert_eq!(
            by_path("config/config.toml").disposition,
            Disposition::Update
        );
        assert_eq!(
            by_path("claude-home/scheduled-tasks/daily.json").disposition,
            Disposition::SkipIdentical
        );
        assert!(matches!(
            by_path("config/accounts/work/.credentials.json").disposition,
            Disposition::SkipLocalNewer { .. }
        ));
        assert_eq!(
            by_path("claude-home/projects/new/session.jsonl").disposition,
            Disposition::Create
        );
        assert_eq!(
            by_path("config/bridge-state.json").disposition,
            Disposition::ExcludedByPolicy
        );
        assert!(matches!(
            by_path("config/../../../../etc/shadow").disposition,
            Disposition::RejectedPath(_)
        ));

        for refused in ["config/bridge-state.json", "config/../../../../etc/shadow"] {
            assert!(
                by_path(refused).dest.is_none(),
                "{refused} was handed a destination"
            );
        }
        assert!(
            plan.items.iter().filter(|i| i.disposition.writes()).count() == 2,
            "only the update and the create may be written"
        );
    }

    /// T-5-22: a link planted at a destination is seen as a link, and no
    /// combination of consents writes through it.
    #[test]
    #[cfg(unix)]
    fn a_symlink_at_the_destination_is_refused_under_every_force() {
        let m = Machine::new();
        let outside = m.dir.path().join("elsewhere/secret.txt");
        fs::create_dir_all(outside.parent().unwrap()).unwrap();
        fs::write(&outside, b"not yours").unwrap();

        let dest = layout::from_manifest_path(&m.roots, "config/config.toml").unwrap();
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&outside, &dest).unwrap();

        let resolved = snapshot(&[("config/config.toml", b"[sync]\n")]);
        for o in [opts(false, false), opts(true, false), opts(true, true)] {
            let plan = plan_with(&m, &resolved, o);
            let item = &plan.items[0];
            match &item.disposition {
                Disposition::RejectedPath(why) => assert!(
                    why.contains("symbolic link"),
                    "the refusal does not say why: {why}"
                ),
                other => panic!("a symlink destination became {other:?}"),
            }
            assert!(!item.disposition.writes());
            assert!(item.dest.is_none());
        }
        assert_eq!(fs::read(&outside).unwrap(), b"not yours");
    }

    #[test]
    fn a_directory_where_a_file_is_expected_is_refused_rather_than_planned() {
        let m = Machine::new();
        let dest = layout::from_manifest_path(&m.roots, "config/config.toml").unwrap();
        fs::create_dir_all(&dest).unwrap();

        let resolved = snapshot(&[("config/config.toml", b"[sync]\n")]);
        let plan = plan_with(&m, &resolved, opts(true, true));
        match &plan.items[0].disposition {
            Disposition::RejectedPath(why) => assert!(why.contains("directory"), "{why}"),
            other => panic!("a directory destination became {other:?}"),
        }
    }

    /// T-5-24: identity is what is on disk, not what a size or an index row
    /// claims.
    #[test]
    fn a_file_of_the_same_length_but_different_bytes_is_not_identical() {
        let m = Machine::new();
        m.seed(
            "config/config.toml",
            b"aaaaaaaa",
            SNAPSHOT - Duration::from_secs(60),
        );
        let resolved = snapshot(&[("config/config.toml", b"bbbbbbbb")]);
        let plan = plan_with(&m, &resolved, RestoreOptions::default());
        assert_eq!(plan.items[0].disposition, Disposition::Update);
    }

    /// D7: apply the same snapshot onto its own output and nothing is a
    /// conflict, whatever the local mtime says.
    #[test]
    fn a_second_apply_is_a_no_op_even_when_the_local_copy_is_newer() {
        let m = Machine::new();
        let body = b"{\"tasks\":[\"daily\"]}";
        m.seed(
            "claude-home/scheduled-tasks/daily.json",
            body,
            SNAPSHOT + Duration::from_secs(86_400),
        );
        let resolved = snapshot(&[("claude-home/scheduled-tasks/daily.json", body)]);
        let plan = plan_with(&m, &resolved, RestoreOptions::default());
        assert_eq!(plan.items[0].disposition, Disposition::SkipIdentical);
        assert_eq!(plan.packs_needed, 0);
        assert_eq!(plan.bytes_to_fetch, 0);
    }

    /// A file longer than one chunk is hashed in the same buffers the push side
    /// used, so it is recognised across machines rather than re-fetched.
    #[test]
    fn a_multi_chunk_file_is_recognised_as_identical() {
        let m = Machine::new();
        let body: Vec<u8> = (0..CHUNK_SIZE + 17).map(|i| (i % 251) as u8).collect();
        let dest = layout::from_manifest_path(&m.roots, "claude-home/projects/r/s.jsonl").unwrap();
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        fs::write(&dest, &body).unwrap();

        let k = keys();
        let chunks: Vec<ChunkId> = body.chunks(CHUNK_SIZE).map(|c| k.chunk_id(c)).collect();
        assert_eq!(chunks.len(), 2, "the fixture must span two chunks");

        let resolved = Resolved {
            root: Root::new(
                7,
                SNAPSHOT,
                "github:1".into(),
                Vec::new(),
                KdfParams::default(),
            ),
            manifest: Manifest::new(vec![FileEntry {
                path: "claude-home/projects/r/s.jsonl".into(),
                mode: 0o600,
                true_len: body.len() as u64,
                chunks,
            }]),
            index: IndexObject::new(Vec::new(), Vec::new()),
            packs: PackSource::empty(k),
        };
        let plan = plan_with(&m, &resolved, RestoreOptions::default());
        assert_eq!(plan.items[0].disposition, Disposition::SkipIdentical);
    }

    /// A dry run's headline number is what a real run would fetch — the packs
    /// behind the items that will actually be written, and no others.
    #[test]
    fn packs_needed_and_bytes_to_fetch_count_only_the_items_that_will_be_written() {
        let m = Machine::new();
        let kept = b"{\"tasks\":[]}";
        m.seed(
            "claude-home/scheduled-tasks/daily.json",
            kept,
            SNAPSHOT - Duration::from_secs(60),
        );
        let resolved = snapshot(&[
            ("claude-home/scheduled-tasks/daily.json", kept), // identical, pack 0
            ("config/config.toml", b"[sync]\n"),              // create,    pack 1
        ]);
        let plan = plan_with(&m, &resolved, RestoreOptions::default());
        assert_eq!(
            plan.packs_needed, 1,
            "the skipped item's pack is not needed"
        );
        assert_eq!(
            plan.bytes_to_fetch, 101,
            "only pack 1's sealed length is counted"
        );
    }

    /// Permissions, not a panic and not a silent skip.
    #[test]
    #[cfg(unix)]
    fn an_unreadable_local_file_is_planned_rather_than_panicking() {
        use std::os::unix::fs::PermissionsExt;

        let m = Machine::new();
        let dest = m.seed(
            "config/config.toml",
            b"[sync]\n",
            SNAPSHOT - Duration::from_secs(60),
        );
        fs::set_permissions(&dest, fs::Permissions::from_mode(0o000)).unwrap();
        if fs::File::open(&dest).is_ok() {
            return; // running as root: the premise does not hold
        }

        let resolved = snapshot(&[("config/config.toml", b"[sync]\nenabled = true\n")]);
        let plan = plan_with(&m, &resolved, RestoreOptions::default());
        assert_eq!(plan.items[0].disposition, Disposition::Update);
    }

    /// Restore is additive: a local file the snapshot never heard of is not in
    /// the plan at all, and nothing plans a deletion.
    #[test]
    fn a_local_file_the_manifest_does_not_mention_is_left_alone() {
        let m = Machine::new();
        let untouched = m.seed(
            "claude-home/scheduled-tasks/mine.json",
            b"{\"mine\":true}",
            SNAPSHOT + Duration::from_secs(1),
        );
        let resolved = snapshot(&[("config/config.toml", b"[sync]\n")]);
        let plan = plan_with(&m, &resolved, opts(true, true));
        assert_eq!(plan.items.len(), 1);
        assert!(
            !plan
                .items
                .iter()
                .any(|i| i.dest.as_deref() == Some(untouched.as_path()))
        );
        assert!(untouched.exists());
    }

    /// **F-1, at the seam the suite could not see.**
    ///
    /// Two tests already stood either side of this and both were correct.
    /// `force_alone_never_overwrites_a_locally_newer_credential` calls `decide`
    /// with the `credential` bool handed to it as `CREDENTIAL` — it proves the
    /// arm works when it is reached, never that it is reached.
    /// `the_credential_arm_covers_the_profile_store_and_every_dot_credentials_json`
    /// classifies, and every one of its fixtures was spelled one way. Nothing
    /// crossed from a *manifest string* to a *disposition*, which is the only
    /// place the defect lived: `.Credentials.json` classified as ordinary, took
    /// the `Overwrite` arm under `--force` alone, and on the case-insensitive
    /// volume this project's users run, that path is the live OAuth token.
    ///
    /// So this one goes through `plan`, from the spelling in the manifest to
    /// the disposition, with `force = true` and `force_credentials = false` —
    /// the exact flags the audit's PoC A3 used to revert a live credential.
    #[test]
    fn a_credential_reaches_the_second_consent_however_the_manifest_spells_it() {
        for spelling in [
            ".credentials.json",
            ".Credentials.json",
            ".CREDENTIALS.JSON",
            ".cReDeNtIaLs.jSoN",
        ] {
            let m = Machine::new();
            let path = format!("config/accounts/work/{spelling}");
            // Seeded at the manifest's own spelling, so the premise holds on a
            // case-sensitive volume too: this test proves the *classification*
            // crosses the seam, on every platform the suite runs on.
            m.seed(
                &path,
                b"{\"access_token\":\"live\"}",
                SNAPSHOT + Duration::from_secs(60),
            );
            let resolved = snapshot(&[(path.as_str(), b"{\"access_token\":\"stale\"}")]);

            let plan = plan_with(&m, &resolved, opts(true, false));
            assert!(
                matches!(
                    plan.items[0].disposition,
                    Disposition::NeedsCredentialConfirm { .. }
                ),
                "`--force` alone planned {:?} for a live credential the manifest spelled \
                 {spelling} — the second consent was skipped entirely",
                plan.items[0].disposition
            );

            // And the second consent still promotes it, so the fix widened the
            // gate rather than jamming it shut.
            let forced = plan_with(&m, &resolved, opts(true, true));
            assert!(matches!(
                forced.items[0].disposition,
                Disposition::Overwrite { .. }
            ));
        }
    }

    // ------------------------------------------- machine-bound credential stores

    /// A credential that is not a file: read on push, written on restore. Every
    /// store here is an injected [`keystore::Stores::fixture`], so no test in
    /// this module can reach a real login Keychain — which is the whole reason
    /// `SyncRoots::at` yields one.
    mod machine_bound_stores {
        use super::*;
        use crate::sync::keystore::Stores;
        use crate::sync::restore::{Applied, report, write};

        const LOGIN: &str = "keystore/claude-code-oauth";
        const LIVE: &str = r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-live"}}"#;
        const FROM_THE_BUNDLE: &str = r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-bundle"}}"#;

        /// A machine whose store already holds `value`, or holds nothing.
        fn machine_holding(value: Option<&str>) -> Machine {
            let m = Machine::new();
            if let Some(value) = value {
                m.roots.stores.edit().set(Store::ClaudeCodeOauth, value);
            }
            m
        }

        fn held(m: &Machine) -> Option<String> {
            m.roots
                .stores
                .edit()
                .get(Store::ClaudeCodeOauth)
                .map(str::to_string)
        }

        fn plan_of(m: &Machine, resolved: &Resolved, opts: RestoreOptions) -> RestorePlan {
            let client = m.client();
            plan(&m.ctx(&client, opts), resolved).expect("planning is infallible here")
        }

        fn apply_to(m: &Machine, plan: &RestorePlan, resolved: &Resolved) -> Applied {
            let client = m.client();
            write::apply(&m.ctx(&client, applying()), plan, &resolved.packs)
                .expect("the write half returns Ok even for a partial run")
        }

        /// **The headline.** A machine with no Claude login gets one, byte for
        /// byte, and it lands in the store rather than as a file anywhere.
        #[test]
        fn a_machine_with_no_login_receives_one_byte_for_byte() {
            let m = machine_holding(None);
            let resolved = snapshot_with_packs(&[(LOGIN, FROM_THE_BUNDLE.as_bytes())]);

            let plan = plan_of(&m, &resolved, applying());
            assert_eq!(disposition_of(&plan, LOGIN), &Disposition::Create);
            assert!(
                plan.items[0].dest.is_none(),
                "a store must never acquire a filesystem destination"
            );

            let applied = apply_to(&m, &plan, &resolved);
            assert_eq!(applied.written, 1);
            assert!(applied.failed_at.is_none());
            assert_eq!(held(&m).as_deref(), Some(FROM_THE_BUNDLE));
        }

        /// A repeated pull onto the machine that pushed asks nothing and writes
        /// nothing — digest-first, exactly as for a file (D7).
        #[test]
        fn the_same_login_already_here_is_identical_and_silent() {
            let m = machine_holding(Some(FROM_THE_BUNDLE));
            let resolved = snapshot_with_packs(&[(LOGIN, FROM_THE_BUNDLE.as_bytes())]);

            let plan = plan_of(&m, &resolved, RestoreOptions::default());
            assert_eq!(disposition_of(&plan, LOGIN), &Disposition::SkipIdentical);
            assert!(!plan.items[0].disposition.writes());
        }

        /// D2 for a store: `--force` alone is not the consent, and the live
        /// login is still there afterwards. Replacing the account this tool
        /// exists to report on is the worst outcome in the whole feature.
        #[test]
        fn force_alone_never_replaces_a_different_live_login() {
            let m = machine_holding(Some(LIVE));
            let resolved = snapshot_with_packs(&[(LOGIN, FROM_THE_BUNDLE.as_bytes())]);

            let plan = plan_of(&m, &resolved, opts(true, false));
            assert_eq!(
                disposition_of(&plan, LOGIN),
                &Disposition::ReplacesLiveCredential
            );
            assert!(!plan.items[0].disposition.writes());

            // Named, and the report says what it costs — including that the
            // pre-restore backup cannot archive it.
            let rendered = report::render_plan(&plan, false);
            assert!(rendered.contains(LOGIN), "{rendered}");
            assert!(rendered.contains("--force-credentials"), "{rendered}");
            assert!(rendered.contains("not archived"), "{rendered}");

            // And the write half refuses the whole run rather than guessing how
            // the confirmation would have been answered — the same tripwire
            // `NeedsCredentialConfirm` has. Nothing is written either way.
            let client = m.client();
            let err = write::apply(&m.ctx(&client, applying()), &plan, &resolved.packs)
                .expect_err("an unanswered credential consent must not reach the write path");
            assert!(err.to_string().contains("credential confirmation"), "{err}");
            assert_eq!(held(&m).as_deref(), Some(LIVE));
        }

        /// …and the second consent does promote it, so the gate widened rather
        /// than jammed shut.
        #[test]
        fn force_credentials_replaces_the_live_login_and_nothing_else_does() {
            let m = machine_holding(Some(LIVE));
            let resolved = snapshot_with_packs(&[(LOGIN, FROM_THE_BUNDLE.as_bytes())]);

            let plan = plan_of(
                &m,
                &resolved,
                RestoreOptions {
                    apply: true,
                    force_credentials: true,
                    ..Default::default()
                },
            );
            assert_eq!(disposition_of(&plan, LOGIN), &Disposition::Update);
            apply_to(&m, &plan, &resolved);
            assert_eq!(held(&m).as_deref(), Some(FROM_THE_BUNDLE));
        }

        /// A `keystore/…` entry from a build later than this one is named,
        /// refused, and never mistaken for a path — the forward-compatibility
        /// rule that lets a second store be added without breaking this build.
        #[test]
        fn an_unknown_store_is_refused_and_leaves_the_local_one_untouched() {
            let m = machine_holding(Some(LIVE));
            let resolved =
                snapshot_with_packs(&[("keystore/from-a-later-version", b"whatever-this-is")]);

            let plan = plan_of(&m, &resolved, applying());
            let item = &plan.items[0];
            assert_eq!(item.disposition, Disposition::ExcludedByPolicy);
            assert!(item.dest.is_none());

            apply_to(&m, &plan, &resolved);
            assert_eq!(held(&m).as_deref(), Some(LIVE));
        }

        /// **Never half-write a credential store.** A snapshot whose recorded
        /// length disagrees with the bytes it carries is refused *before* the
        /// store is touched, so the login that was there is still there.
        #[test]
        fn a_length_that_disagrees_refuses_the_item_and_keeps_the_existing_login() {
            let m = machine_holding(Some(LIVE));
            let mut resolved = snapshot_with_packs(&[(LOGIN, FROM_THE_BUNDLE.as_bytes())]);
            resolved.manifest.files[0].true_len += 1;

            let plan = plan_of(
                &m,
                &resolved,
                RestoreOptions {
                    apply: true,
                    force_credentials: true,
                    ..Default::default()
                },
            );
            assert_eq!(disposition_of(&plan, LOGIN), &Disposition::Update);

            let applied = apply_to(&m, &plan, &resolved);
            assert_eq!(applied.failed_at.as_deref(), Some(LOGIN));
            assert_eq!(applied.written, 0);
            assert_eq!(
                held(&m).as_deref(),
                Some(LIVE),
                "a refused write took the login with it"
            );
        }

        /// The same rule for bytes that are not a credential at all: refused
        /// whole, never written lossily.
        #[test]
        fn a_value_that_is_not_utf8_is_refused_rather_than_repaired() {
            let m = machine_holding(Some(LIVE));
            let resolved = snapshot_with_packs(&[(LOGIN, &[0xff, 0xfe, 0xfd])]);

            let plan = plan_of(
                &m,
                &resolved,
                RestoreOptions {
                    apply: true,
                    force_credentials: true,
                    ..Default::default()
                },
            );
            let applied = apply_to(&m, &plan, &resolved);
            assert_eq!(applied.failed_at.as_deref(), Some(LOGIN));
            assert_eq!(held(&m).as_deref(), Some(LIVE));
        }

        /// A store is a credential in both directions, so it lands under the
        /// category whose switch decided whether it travelled at all — and so
        /// does Claude Code's own credential *file*, which `scope` now collects
        /// under `Credentials` too.
        #[test]
        fn a_store_and_the_credential_file_are_both_filed_under_credentials() {
            assert_eq!(category_of(LOGIN), SyncCategory::Credentials);
            assert_eq!(
                category_of("keystore/from-a-later-version"),
                SyncCategory::Credentials
            );
            assert_eq!(
                category_of("claude-home/.credentials.json"),
                SyncCategory::Credentials
            );
            // Case-folded, like every other credential comparison in this file.
            assert_eq!(
                category_of("claude-home/.Credentials.json"),
                SyncCategory::Credentials
            );
            // And the rest of `claude-home` is untouched.
            assert_eq!(
                category_of("claude-home/scheduled-tasks/x.json"),
                SyncCategory::Routines
            );
        }

        /// The seam itself: a fresh `SyncRoots` has an empty *injected* store,
        /// never the machine's.
        #[test]
        fn every_machine_in_this_suite_has_an_injected_store() {
            let m = Machine::new();
            assert!(matches!(m.roots.stores, Stores::Fixture(_)));
            assert!(
                m.roots
                    .stores
                    .read(Store::ClaudeCodeOauth)
                    .unwrap()
                    .is_none()
            );
        }
    }

    // ------------------------------------------- Claude Desktop token caches

    /// A Claude Desktop token cache travels, and is only *restorable* where the
    /// key that sealed it lives.
    ///
    /// macOS-only, because the whole mechanism is. Every test here injects a
    /// key derived from a fixed fake secret through `plan_with_safe_key`'s
    /// seam: nothing in this module may read the real login Keychain, which is
    /// the reason the seam exists at all.
    #[cfg(target_os = "macos")]
    mod desktop_token_caches {
        use super::*;
        use crate::safe_storage;
        use crate::sync::restore::{report, write};

        /// `desktop-profiles/…` is the profile store `scope` files under
        /// `SyncCategory::Credentials` — the one the hazard lives in.
        const CACHE: &str = "desktop-profiles/work/config-tokenCacheV2";
        /// The portable half of the bundle, in the same run every time.
        const PORTABLE: &str = "config/accounts/work/.credentials.json";

        fn this_mac() -> safe_storage::Key {
            safe_storage::derive_key(b"the-key-in-this-machines-keychain")
        }

        fn another_mac() -> safe_storage::Key {
            safe_storage::derive_key(b"the-key-in-some-other-machines-keychain")
        }

        /// **The wiring, not the gate.** 6-09's tests all drive
        /// `plan_with_safe_key`, so nothing asserted that `plan` — the entry
        /// `restore::run` actually calls — reaches a key at all. A regression
        /// there disables the gate silently while every test below stays green.
        ///
        /// The key comes from the injected stores, so this asserts the
        /// production path end to end without going near a real Keychain.
        #[test]
        fn plan_takes_its_safe_storage_key_from_the_injected_stores() {
            let m = Machine::new();
            let theirs = safe_storage::encrypt(&another_mac(), br#"{"accessToken":"theirs"}"#);
            let resolved = snapshot_with_packs(&[(CACHE, theirs.as_bytes())]);
            let client = m.client();

            // This machine's key is *not* the one that sealed it.
            m.roots.stores.edit().set_safe_key(Some(this_mac()));
            let refused = plan(&m.ctx(&client, applying()), &resolved).unwrap();
            assert_eq!(
                disposition_of(&refused, CACHE),
                &Disposition::ForeignSafeStorage
            );

            // And with the key that did seal it, the same entry restores.
            m.roots.stores.edit().set_safe_key(Some(another_mac()));
            let accepted = plan(&m.ctx(&client, applying()), &resolved).unwrap();
            assert_eq!(disposition_of(&accepted, CACHE), &Disposition::Create);
        }

        /// Same key, same answer as before this gate existed: a snapshot of
        /// *this* machine restores onto it, which is what makes the disk-loss
        /// recovery the blobs are carried for actually work.
        #[test]
        fn a_blob_this_machines_key_opens_is_restored() {
            let m = Machine::new();
            let mine = safe_storage::encrypt(&this_mac(), br#"{"accessToken":"live"}"#);
            let resolved = snapshot_with_packs(&[(CACHE, mine.as_bytes())]);

            let plan = planned(&m, &resolved, Some(this_mac()));
            assert_eq!(disposition_of(&plan, CACHE), &Disposition::Create);

            let client = m.client();
            write::apply(&m.ctx(&client, applying()), &plan, &resolved.packs).unwrap();
            let dest = layout::from_manifest_path(&m.roots, CACHE).unwrap();
            assert_eq!(fs::read_to_string(&dest).unwrap(), mine);
        }

        /// The whole point. A blob from another Mac is refused, it is named in
        /// the report, and the live local session is still there — byte for
        /// byte — after a full `--apply` run.
        #[test]
        fn a_blob_from_another_machine_is_refused_and_the_live_session_survives() {
            let m = Machine::new();
            let live = safe_storage::encrypt(&this_mac(), br#"{"accessToken":"the-live-one"}"#);
            let dest = m.seed(CACHE, live.as_bytes(), SNAPSHOT - Duration::from_secs(3600));

            // Older than the snapshot, so without the gate this is a plain
            // `Update` and the login is gone.
            let theirs = safe_storage::encrypt(&another_mac(), br#"{"accessToken":"theirs"}"#);
            let resolved = snapshot_with_packs(&[(CACHE, theirs.as_bytes())]);

            let plan = planned(&m, &resolved, Some(this_mac()));
            let item = &plan.items[0];
            assert_eq!(item.disposition, Disposition::ForeignSafeStorage);
            assert!(!item.disposition.writes());
            assert!(item.dest.is_none(), "a refusal keeps no destination");

            // `false`: this asserts the refusal is *named*, which is the dry
            // run's whole job — the user sees it before the run that would
            // otherwise have written over a working login.
            let rendered = report::render_plan(&plan, false);
            assert!(
                rendered.contains(CACHE),
                "the refusal is not named:\n{rendered}"
            );
            assert!(
                rendered.contains("sign in to Claude Desktop on this Mac"),
                "the report does not say what to do about it:\n{rendered}"
            );

            let client = m.client();
            write::apply(&m.ctx(&client, applying()), &plan, &resolved.packs).unwrap();
            assert_eq!(
                fs::read_to_string(&dest).unwrap(),
                live,
                "the working login was overwritten by a blob nothing here can decrypt"
            );
        }

        /// No `Claude Safe Storage` key on this machine is the same answer: a
        /// token cache it cannot read is still a token cache it must not write,
        /// absent local file or not.
        #[test]
        fn a_blob_is_refused_when_this_machine_has_no_key_at_all() {
            let m = Machine::new();
            let theirs = safe_storage::encrypt(&another_mac(), br#"{"accessToken":"theirs"}"#);
            let resolved = snapshot_with_packs(&[(CACHE, theirs.as_bytes())]);

            let plan = planned(&m, &resolved, None);
            assert_eq!(plan.items[0].disposition, Disposition::ForeignSafeStorage);

            let client = m.client();
            write::apply(&m.ctx(&client, applying()), &plan, &resolved.packs).unwrap();
            assert!(
                !layout::from_manifest_path(&m.roots, CACHE)
                    .unwrap()
                    .exists(),
                "an unreadable token cache is worse for Desktop to find than none"
            );
        }

        /// The carve-out is narrow, and this is the assertion that says so:
        /// `.credentials.json` is plain JSON, it is Claude Code's own OAuth
        /// credential, it is what makes the CLI work on the second machine, and
        /// it restores in the very same run that refuses the token cache.
        #[test]
        fn a_dot_credentials_json_restores_in_the_same_run_that_refuses_a_token_cache() {
            let m = Machine::new();
            let theirs = safe_storage::encrypt(&another_mac(), b"{}");
            let portable = br#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-portable"}}"#;
            let resolved = snapshot_with_packs(&[(CACHE, theirs.as_bytes()), (PORTABLE, portable)]);

            let plan = planned(&m, &resolved, Some(this_mac()));
            assert_eq!(
                disposition_of(&plan, CACHE),
                &Disposition::ForeignSafeStorage
            );
            assert_eq!(disposition_of(&plan, PORTABLE), &Disposition::Create);

            let client = m.client();
            write::apply(&m.ctx(&client, applying()), &plan, &resolved.packs).unwrap();
            let dest = layout::from_manifest_path(&m.roots, PORTABLE).unwrap();
            assert_eq!(fs::read(&dest).unwrap(), portable);
        }

        /// The marker decides, not the path: an ordinary file sitting at the
        /// same shape of path is planned exactly as it always was.
        #[test]
        fn a_file_at_the_same_path_shape_that_is_not_a_safe_storage_value_is_untouched_by_the_gate()
        {
            let m = Machine::new();
            let plain = br#"{"not":"a safeStorage value"}"#;
            let resolved = snapshot_with_packs(&[(CACHE, plain)]);

            let plan = planned(&m, &resolved, Some(this_mac()));
            assert_eq!(plan.items[0].disposition, Disposition::Create);

            let client = m.client();
            write::apply(&m.ctx(&client, applying()), &plan, &resolved.packs).unwrap();
            assert_eq!(
                fs::read(layout::from_manifest_path(&m.roots, CACHE).unwrap()).unwrap(),
                plain
            );
        }

        /// A dry run has no data packs — `fetch::resolve` downloads file content
        /// only under `apply` — so it cannot answer the question and says so by
        /// not refusing. The run that writes is the run that refuses, which is
        /// where the loss would have happened.
        #[test]
        fn without_the_data_packs_the_gate_stays_out_of_the_way() {
            let m = Machine::new();
            let theirs = safe_storage::encrypt(&another_mac(), b"{}");
            let resolved = snapshot(&[(CACHE, theirs.as_bytes())]);
            let plan = planned(&m, &resolved, Some(this_mac()));
            assert_eq!(plan.items[0].disposition, Disposition::Create);
        }
    }

    /// The plan's own header comes from the root, never from the pointer.
    #[test]
    fn the_plan_carries_the_roots_counter_repo_id_and_capture_time() {
        let m = Machine::new();
        let resolved = snapshot(&[("config/config.toml", b"[sync]\n")]);
        let plan = plan_with(&m, &resolved, RestoreOptions::default());
        assert_eq!(plan.counter, 7);
        assert_eq!(plan.repo_id, "github:1");
        assert_eq!(plan.created_at, SNAPSHOT);
    }
}
