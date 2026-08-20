//! `SyncPlan` to `PushBundle`: sealed chunks packed into remote-sized objects,
//! plus the manifest and the index object. The snapshot root is sealed by
//! [`root_for`], which the flip's rebuild closure calls on every attempt.
//!
//! This is the object that makes GitHub's 80-per-minute content-creation limit
//! irrelevant: 5,000 chunks become a handful of 48 MiB assets rather than 5,000
//! requests (REPO-06).
//!
//! # Manifest paths are root-relative, never absolute
//!
//! [`FilePlan::path`](crate::sync::plan::FilePlan) is an absolute local path
//! carrying the pushing user's home directory and username. Storing it would
//! make the bundle unresolvable on a second machine, leak the username to
//! anyone who obtains the repository, and — worst — be rejected by Phase 5's
//! traversal defence, so the bundle would only be restorable by disabling the
//! very check protecting the machine restoring it.
//!
//! [`manifest_path`] renders the root-prefixed relative form instead: the name of
//! the [`SyncRoots`] root the file was collected under, then the path beneath it.
//! A file under none of the roots is an error rather than a fallback to the
//! absolute path — there is no correct absolute path to fall back to.
//!
//! # The manifest describes what was packed, never what was planned
//!
//! [`plan::build`](crate::sync::plan::build) reads a file, and [`build`] reads
//! it again minutes later. The bundle's own contents are live transcripts that
//! are appended to while a multi-gigabyte push runs, so between the two reads a
//! file *will* change — this is the ordinary case, not a race anyone has to
//! engineer.
//!
//! So the chunk list in every [`FileEntry`], and the `true_len` beside it, come
//! from the bytes [`pack_file`] actually read. Naming the plan's ids instead
//! publishes a snapshot whose manifest names a chunk nothing ever sealed, and a
//! restore rightly refuses that rather than writing a partial tree — a push that
//! reports success and cannot be restored, which is D2's worst outcome.
//!
//! A file written *while* the packer reads it can still yield a torn read: a
//! prefix of one version and a tail of the next. That is what any backup of a
//! live file does, and it is internally consistent — every chunk it names was
//! sealed. That is the guarantee here, and it is the one that matters.
//!
//! # The two size constants, and which one governs
//!
//! [`should_seal`] compares against [`PACK_MAX`] (48 MiB) and never reads
//! `PACK_TARGET` (32 MiB), so packs fill to 48 MiB and the target is advisory.
//! Every size expectation in this file's tests is therefore derived from
//! `PACK_MAX`, including the worst-case pack-header guard — a guard built at the
//! advisory target would understate the real case by half again, and a guard
//! that understates its case is a guard that passes on the day it should fail.
//!
//! **Neither constant may be raised here.** `docs/sync-format.md` §7 records
//! that CAL-1 was never run, so the recorded fallback stands; and the pack
//! header is still a *single* sealed chunk, which gap-closure 1-09 deliberately
//! did not reach when it made manifests and index objects multi-chunk. Its
//! entry-count ceiling is a function of `PACK_MAX`.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;

use base64::Engine;
use zeroize::Zeroizing;

use crate::error::{AppError, Result};
use crate::sync::chunk::{Blob, seal_chunk};
use crate::sync::crypto::{ChunkId, Keys};
use crate::sync::index::ChunkLocation;
use crate::sync::keystore;
use crate::sync::model::{FileEntry, IndexEntry, IndexObject, Manifest, Root};
use crate::sync::pack::{PackWriter, should_seal};
use crate::sync::plan::SyncPlan;
use crate::sync::{CHUNK_SIZE, SyncRoots};

use super::{B64, BuiltPack, Pointer, PushBundle, PushCtx, RemoteIndexEntry};

/// Turn a plan into the bytes a push puts on the wire.
///
/// # The order the three objects are built in
///
/// One sentence in prose and three passes in code, and getting it wrong
/// produces a bundle Phase 5 cannot read while every test here still passes:
///
/// 1. the manifest is built, sealed and **packed**, and the writer is flushed,
///    because
/// 2. the index object describes every pack entry this snapshot references —
///    reused chunks, this run's data chunks, *and* the manifest's chunks. Build
///    it before the manifest is packed and the root's `manifest_chunks` name ids
///    the index object does not describe, so a restore cannot find the manifest
///    at all.
/// 3. the index object's own chunks are necessarily not described by the index
///    object — nothing describes itself — which is exactly why
///    [`PushBundle::index_chunks`] exists as a plaintext bootstrap in the
///    pointer.
///
/// The snapshot root is **not** built here — see [`root_for`]. It carries the
/// counter, which is a function of the pointer the flip actually lands against,
/// so it is sealed inside the rebuild closure that reruns on a conflict. This
/// returns the manifest's chunk ids in order for that closure to name.
///
/// Nothing here seals a new *kind* of object: packs, manifests, index objects
/// and roots are the four the format already defines, so Phase 1's deferred AAD
/// object-type separator stays untriggered.
pub fn build(ctx: &PushCtx<'_>, plan: &SyncPlan) -> Result<PushBundle> {
    // The plan's ids, deduplicated, asked of the chunk table once. This is a
    // *prediction* of what the packer will read — good enough to answer "is
    // this already published?", and never the manifest's source of truth.
    let mut planned: Vec<ChunkId> = Vec::new();
    let mut seen: HashSet<ChunkId> = HashSet::new();
    for file in &plan.file_plans {
        for raw in &file.chunk_ids {
            let id = ChunkId::from_bytes(*raw);
            if seen.insert(id) {
                planned.push(id);
            }
        }
    }
    let reusable = reusable(ctx, &planned);

    let mut packs = Packing::default();
    let mut files: Vec<FileEntry> = Vec::new();

    for file in &plan.file_plans {
        // A machine-bound store, not a file: its bytes come from the store
        // itself, and its manifest entry is the store's own fixed wire name —
        // never `manifest_path`, which would have no root to resolve against.
        //
        // Read again here rather than carried down from the plan, for the
        // reason this module's header gives: the manifest describes what the
        // packer sealed. A credential that rotated in between is sealed as it
        // is now, and one that vanished is simply not in the snapshot.
        if let Some(store) = file
            .path
            .to_str()
            .and_then(keystore::Store::from_manifest_path)
        {
            let Some(value) = ctx.roots.stores.read(store)? else {
                continue;
            };
            if value.is_empty() {
                continue;
            }
            let (true_len, chunks) = pack_store(ctx, &mut packs, value.as_bytes(), &reusable)?;
            files.push(FileEntry {
                // 0600 recorded for honesty; `restore::write` never applies a
                // manifest mode, and a store has no mode to apply one to.
                mode: 0o600,
                path: store.manifest_path().to_string(),
                true_len,
                chunks,
            });
            continue;
        }
        let planned: Vec<ChunkId> = file
            .chunk_ids
            .iter()
            .map(|raw| ChunkId::from_bytes(*raw))
            .collect();
        // SYNC-02, honoured rather than merely claimed: a file whose every chunk
        // the snapshot can already locate is not opened at all. Its length then
        // comes from the plaintext lengths of those same chunks and not from a
        // stat, because a stat may already describe a different file and a
        // `true_len` that disagrees with the chunk list beside it is a restore
        // that refuses.
        let (mode, true_len, chunks) = match reused_len(&planned, &reusable) {
            Some(true_len) => {
                let meta =
                    std::fs::metadata(&file.path).map_err(|e| AppError::io_at(&file.path, e))?;
                (mode_of(&meta), true_len, planned)
            }
            None => pack_file(ctx, &mut packs, &file.path, &reusable)?,
        };
        files.push(FileEntry {
            path: manifest_path(ctx.roots, &file.path)?,
            mode,
            true_len,
            chunks,
        });
    }

    // Every chunk the snapshot names, deduplicated, in first-seen order — read
    // off the **manifest that was just built**, not off the plan. Note this is
    // the union over the file entries, **not** `plan.new_chunk_ids`: a snapshot
    // must name a pack for every chunk it references, and taking only the plan's
    // new ids is precisely how `referenced_packs` ends up missing the packs that
    // hold all the unchanged data.
    let mut named: Vec<ChunkId> = Vec::new();
    let mut seen: HashSet<ChunkId> = HashSet::new();
    for file in &files {
        for id in &file.chunks {
            if seen.insert(*id) {
                named.push(*id);
            }
        }
    }

    // 1. The manifest, packed like any other chunk, and then sealed into a pack
    //    so every entry above names a pack that exists.
    let manifest = Manifest::new(files);
    let mut manifest_chunks: Vec<ChunkId> = Vec::new();
    for blob in manifest.seal(ctx.keys)? {
        manifest_chunks.push(blob.id);
        packs.push(blob, ctx.keys)?;
    }
    packs.flush(ctx.keys)?;

    // 2. The index object, over every entry this snapshot references.
    let mut entries: Vec<IndexEntry> = named
        .iter()
        .filter_map(|id| reusable.get(id).map(|at| entry_at(*id, at)))
        .collect();
    entries.extend(packs.entries.iter().copied());
    let index_object = IndexObject::new(entries, supersedes(ctx));

    let first_index_entry = packs.entries.len();
    for blob in index_object.seal(ctx.keys)? {
        packs.push(blob, ctx.keys)?;
    }
    packs.flush(ctx.keys)?;

    // 3. The index object's own chunks, as the plaintext bootstrap.
    let index_chunks: Vec<RemoteIndexEntry> = packs.entries[first_index_entry..]
        .iter()
        .map(|e| RemoteIndexEntry {
            id: e.id,
            pack: e.pack,
            offset: e.offset,
            clen: e.clen,
            true_len: e.true_len,
        })
        .collect();

    // 4. **The root is not built here.** It carries the snapshot counter, and
    //    the counter is a function of the pointer that the flip actually lands
    //    against — which a lost race changes underneath this run. Sealing it now
    //    would freeze a counter computed before the race and never recomputed
    //    after it, which is how two machines publish two snapshots at the same
    //    counter. `push::run`'s rebuild closure calls [`root_for`] instead, on
    //    every attempt, and this bundle carries only the manifest's chunk ids.

    // 5. The local chunk table learns where everything this run packed landed —
    //    after `finish`, when the packs' content addresses exist.
    ctx.index.record_chunks(&packs.rows())?;

    // **Every** pack the snapshot needs, reused ones included. A snapshot that
    // named only its new packs would let prune delete the packs holding all of
    // its unchanged data, which is the unrestorable backup D2 exists to prevent.
    let mut referenced_packs: Vec<ChunkId> = Vec::new();
    let mut counted: HashSet<ChunkId> = HashSet::new();
    for pack in index_object
        .entries
        .iter()
        .map(|e| e.pack)
        .chain(index_chunks.iter().map(|e| e.pack))
    {
        if counted.insert(pack) {
            referenced_packs.push(pack);
        }
    }

    Ok(PushBundle {
        packs: packs.done,
        manifest_chunks,
        index_chunks,
        referenced_packs,
    })
}

/// The chunks this snapshot may name without sealing them again.
///
/// Two conditions, and the second is the load-bearing one.
///
/// The local `chunk` table records what this machine **packed**, which is not
/// the same as what landed: a push that packs and then fails at upload leaves
/// rows pointing at packs no remote ever saw. Reusing one would publish a
/// snapshot referencing a pack that does not exist — an unrestorable backup,
/// D2's worst outcome, reached with nobody doing anything wrong. So a chunk is
/// reusable only when its pack is named by a snapshot the **pointer** already
/// carries, which is the remote's own evidence that the pack landed *and* has
/// not since been pruned.
///
/// Both halves fail towards not-reusable, and the cost of that is re-uploading
/// bytes that were already there.
fn reusable(ctx: &PushCtx<'_>, ids: &[ChunkId]) -> HashMap<ChunkId, ChunkLocation> {
    let published: HashSet<ChunkId> = ctx
        .previous
        .iter()
        .flat_map(|pointer| pointer.snapshots.iter())
        .flat_map(|snapshot| snapshot.packs.iter().copied())
        .collect();
    let mut known = ctx.index.chunk_locations(ids);
    known.retain(|_, at| published.contains(&at.pack));
    known
}

fn entry_at(id: ChunkId, at: &ChunkLocation) -> IndexEntry {
    IndexEntry {
        id,
        pack: at.pack,
        offset: at.offset,
        clen: at.clen,
        true_len: at.plen,
    }
}

/// Seal and pack every chunk of `path` the snapshot cannot already locate, and
/// return the file's mode, length and **chunk list as read** — see the module
/// docs on why the caller may not use the plan's list instead.
///
/// Streamed in [`CHUNK_SIZE`] blocks through **one** reused buffer (T-4-16): a
/// 115 MB transcript never exists as a 115 MB plaintext allocation, each block's
/// plaintext is overwritten by the next, and [`Zeroizing`] wipes the last one.
/// Reading the whole file into a `Vec` would hold every credential in the bundle
/// in memory at once for no gain.
///
/// Mode and length come from this same read — the open handle's own metadata and
/// the bytes actually counted — rather than from a separate `metadata` call on
/// the path, so the three halves of a manifest entry cannot describe three
/// different versions of the file.
fn pack_file(
    ctx: &PushCtx<'_>,
    packs: &mut Packing,
    path: &Path,
    reusable: &HashMap<ChunkId, ChunkLocation>,
) -> Result<(u32, u64, Vec<ChunkId>)> {
    let mut file = std::fs::File::open(path).map_err(|e| AppError::io_at(path, e))?;
    let mode = mode_of(&file.metadata().map_err(|e| AppError::io_at(path, e))?);
    let mut buf = Zeroizing::new(vec![0u8; CHUNK_SIZE]);
    let mut chunks: Vec<ChunkId> = Vec::new();
    let mut true_len = 0u64;
    loop {
        let read = fill(&mut file, &mut buf).map_err(|e| AppError::io_at(path, e))?;
        if read == 0 {
            return Ok((mode, true_len, chunks));
        }
        let block = &buf[..read];
        let id = ctx.keys.chunk_id(block);
        chunks.push(id);
        true_len = true_len.saturating_add(read as u64);
        // Both halves of the dedup, on the id of the block that was actually
        // read: already published, or already packed by this run.
        if !reusable.contains_key(&id) && !packs.holds(&id) {
            packs.push(seal_chunk(ctx.keys, block)?, ctx.keys)?;
        }
    }
}

/// Seal and pack a machine-bound store's value, which is already in memory.
///
/// The same two-halved dedup [`pack_file`] applies — already published, or
/// already packed by this run — so an unchanged credential costs nothing on the
/// wire. `value` is borrowed from a [`Zeroizing`] the caller owns; nothing here
/// copies it anywhere that outlives the call.
fn pack_store(
    ctx: &PushCtx<'_>,
    packs: &mut Packing,
    value: &[u8],
    reusable: &HashMap<ChunkId, ChunkLocation>,
) -> Result<(u64, Vec<ChunkId>)> {
    let mut chunks: Vec<ChunkId> = Vec::new();
    for block in value.chunks(CHUNK_SIZE) {
        let id = ctx.keys.chunk_id(block);
        chunks.push(id);
        if !reusable.contains_key(&id) && !packs.holds(&id) {
            packs.push(seal_chunk(ctx.keys, block)?, ctx.keys)?;
        }
    }
    Ok((value.len() as u64, chunks))
}

/// The plaintext length `ids` add up to, when every one of them is already
/// locatable — the SYNC-02 short-circuit's precondition and the `true_len` it
/// must then use, computed together because they have to come from one source.
fn reused_len(ids: &[ChunkId], reusable: &HashMap<ChunkId, ChunkLocation>) -> Option<u64> {
    ids.iter()
        .map(|id| reusable.get(id).map(|at| u64::from(at.plen)))
        .sum()
}

/// Read until `buf` is full or the file ends. `Read::read` is allowed to return
/// short, and a short read misaligned by one byte would re-chunk the whole file
/// into ids nothing recognises.
fn fill(source: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match source.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

/// The fill loop's state: one writer at a time, sealed when [`should_seal`] says
/// so, plus every entry's location once the pack holding it has an address.
#[derive(Default)]
struct Packing {
    writer: PackWriter,
    done: Vec<BuiltPack>,
    /// Filled in as each pack is finished — a pack's id does not exist until its
    /// header is sealed, so an entry recorded before that names nothing.
    entries: Vec<IndexEntry>,
    pending: Vec<IndexEntry>,
    held: HashSet<ChunkId>,
}

impl Packing {
    /// Has this run already packed `id`? The de-duplication decision lives at
    /// the call site rather than in [`Packing::push`], so a caller may
    /// deliberately pack the same bytes twice.
    fn holds(&self, id: &ChunkId) -> bool {
        self.held.contains(id)
    }

    fn push(&mut self, blob: Blob, keys: &Keys) -> Result<()> {
        // `should_seal` as it is, and no second size rule: a second size literal
        // in this file is how the header ceiling gets quietly re-broken.
        if !self.writer.is_empty() && should_seal(self.writer.len_bytes(), blob.ciphertext.len()) {
            self.flush(keys)?;
        }
        self.held.insert(blob.id);
        self.pending.push(IndexEntry {
            id: blob.id,
            pack: ChunkId::from_bytes([0; 32]), // filled in at `flush`
            offset: self.writer.len_bytes() as u64,
            clen: blob.ciphertext.len() as u32,
            true_len: blob.true_len,
        });
        self.writer.push(blob);
        Ok(())
    }

    /// Seal the open writer, if there is one, and stamp its address on to every
    /// entry it holds.
    fn flush(&mut self, keys: &Keys) -> Result<()> {
        if self.writer.is_empty() {
            return Ok(());
        }
        let (id, bytes) = std::mem::take(&mut self.writer).finish(keys)?;
        for mut entry in std::mem::take(&mut self.pending) {
            entry.pack = id;
            self.entries.push(entry);
        }
        self.done.push(BuiltPack { id, bytes });
        Ok(())
    }

    /// What [`crate::sync::index::Index::record_chunks`] takes.
    fn rows(&self) -> Vec<(ChunkId, ChunkId, u64, u32, u32)> {
        self.entries
            .iter()
            .map(|e| (e.id, e.pack, e.offset, e.clen, e.true_len))
            .collect()
    }
}

/// The index-object chunk ids the previous snapshot used, which the new one
/// supersedes.
fn supersedes(ctx: &PushCtx<'_>) -> Vec<ChunkId> {
    ctx.previous
        .as_ref()
        .and_then(|p| p.snapshots.last())
        .map(|s| s.index_chunks.iter().map(|e| e.id).collect())
        .unwrap_or_default()
}

/// The highest counter `pointer`'s snapshot roots carry, or 0 when there is no
/// pointer at all.
///
/// Read out of the sealed roots rather than off the pointer's shape, because
/// position in `snapshots` is remote-controlled and the counter inside a root is
/// not: a reader selects by counter, so the writer must too.
///
/// This is also the value the local rollback anchor is compared against
/// (`push::assert_no_rollback`), and there is deliberately **one** function for
/// both: the audit's carry-forward is explicit that the push-side rollback check
/// and the counter derivation must not be implemented twice differently.
///
/// Do **not** advance the local anchor from here. Phase 1's rule is that the
/// anchor advances only after a snapshot verifies, and this module is producing
/// one, not verifying it; [`super::run`] advances it after the flip lands.
pub(crate) fn highest_counter(pointer: Option<&Pointer>, keys: &Keys, repo_id: &str) -> u64 {
    let Some(pointer) = pointer else {
        return 0;
    };
    pointer
        .snapshots
        .iter()
        .filter_map(|s| B64.decode(&s.root).ok())
        .filter_map(|framed| Root::open(keys, &framed, repo_id).ok())
        .map(|root| root.counter)
        // A pointer whose roots this build cannot open — a damaged entry, or one
        // written by a format this build predates. The snapshot count is the
        // fallback the tracer used: monotone for this machine, and never a value
        // this bundle has already published.
        .max()
        .unwrap_or(pointer.snapshots.len() as u64)
}

/// Seal this run's snapshot root against the pointer that is **actually
/// arriving**, returning the framed root and the counter inside it.
///
/// Called from `push::run`'s rebuild closure rather than from [`build`], and
/// that placement is the whole point. `build` runs once; the closure runs again
/// on a 409. A counter derived where `build` runs is computed before the race
/// and never recomputed after it, so two machines that both read a pointer at
/// counter 6 both seal a root at 7 and the loser republishes its 7 alongside the
/// winner's. Two distinct snapshots at one counter make "select the newest by
/// counter" ambiguous, and `anchor::accept` reads an equal counter as a re-read
/// of a snapshot already seen — so one of the two backups is silently dropped by
/// the control that exists to protect backups.
///
/// The counter is the format's only ordering field and the rollback anchor's
/// subject, so it stays monotonic and meaningful: strictly one above the highest
/// counter the arriving pointer carries.
pub(crate) fn root_for(
    ctx: &PushCtx<'_>,
    arriving: Option<&Pointer>,
    manifest_chunks: &[ChunkId],
) -> Result<(Vec<u8>, u64)> {
    let counter = highest_counter(arriving, ctx.keys, &ctx.repo_id) + 1;
    let root = Root::new(
        counter,
        ctx.now,
        ctx.repo_id.clone(),
        manifest_chunks.to_vec(),
        ctx.kdf,
    )
    .seal(ctx.keys)?;
    Ok((root, counter))
}

/// The root-prefixed relative encoding — see the module docs.
///
/// The prefix is the *name of the root*, not its value, so nothing about this
/// machine's layout survives into the bundle. Phase 5 resolves it back against
/// that machine's own [`SyncRoots`].
pub fn manifest_path(roots: &SyncRoots, path: &Path) -> Result<String> {
    // Longest root first, so a nested root wins over its parent.
    let mut candidates = [
        ("config", roots.config_dir.as_path()),
        ("desktop-data", roots.desktop_data_dir.as_path()),
        ("desktop-profiles", roots.desktop_profiles_dir.as_path()),
        ("claude-home", roots.claude_home.as_path()),
    ];
    candidates.sort_by_key(|(_, root)| std::cmp::Reverse(root.as_os_str().len()));

    for (name, root) in candidates {
        if let Ok(rest) = path.strip_prefix(root) {
            let rest = rest
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            return Ok(format!("{name}/{rest}"));
        }
    }
    Err(AppError::Other(format!(
        "refusing to record {} in the manifest: it lies under none of the sync roots, and an \
         absolute path in a bundle is unresolvable on another machine",
        path.display()
    )))
}

/// Unix permission bits, or 0o600 where the platform has none.
fn mode_of(meta: &std::fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o7777
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        0o600
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SyncConfig;
    use crate::sync::crypto::{KdfParams, Keyfile, content_address};
    use crate::sync::github::token::TokenSource;
    use crate::sync::github::{Client, Endpoints, RepoRef};
    use crate::sync::index::Index;
    use crate::sync::pack::{PACK_MAX, PackEntry, PackHeader, blob_bytes, read_header};
    use crate::sync::plan::FilePlan;
    use crate::sync::{PACK_HEADER_VERSION, chunk};
    use chrono::{DateTime, Utc};
    use tempfile::TempDir;
    use zeroize::Zeroizing as Z;

    use super::super::{Pointer, SnapshotRecord};

    /// Microseconds instead of ~1.5 s and a gibibyte. Never use production
    /// parameters in a unit test: the AUR `check()` runs these on an
    /// installer's machine.
    const CHEAP: KdfParams = KdfParams {
        m_kib: 8,
        t: 1,
        p: 1,
    };

    const NOW: DateTime<Utc> = match DateTime::from_timestamp(1_700_000_000, 0) {
        Some(t) => t,
        None => panic!("a fixed timestamp"),
    };

    const REPO_ID: &str = "github:1";

    /// Deterministic xorshift: incompressible bytes with no random source and no
    /// clock anywhere near the test.
    fn incompressible(len: usize) -> Vec<u8> {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 24) as u8
            })
            .collect()
    }

    /// Everything a [`PushCtx`] borrows, owned in one place so a test can hand
    /// out contexts differing only in the pointer that arrived.
    struct Fixture {
        dir: TempDir,
        roots: SyncRoots,
        keys: Keys,
        index: Index,
        cfg: SyncConfig,
        client: Client,
        repo: RepoRef,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = TempDir::new().unwrap();
            let roots = SyncRoots::at(
                dir.path().join("config.toml"),
                dir.path().to_path_buf(),
                dir.path().join("desktop"),
                dir.path().join("profiles"),
                dir.path().join("claude-home"),
            );
            let keys =
                Keyfile::create_with_floor(b"correct horse battery staple", CHEAP, CHEAP.m_kib)
                    .expect("keyfile creation")
                    .1;
            let index = Index::at(&roots.index_file).unwrap();
            Self {
                dir,
                roots,
                keys,
                index,
                cfg: SyncConfig::default(),
                // Nothing under this module makes a request; the base is a
                // parked address so a regression would fail rather than dial
                // anything real.
                client: Client::new(
                    &Endpoints {
                        api_base: "http://127.0.0.1:1".into(),
                        uploads_base: "http://127.0.0.1:1".into(),
                    },
                    Z::new("github_pat_fixture_not_a_real_token".into()),
                    TokenSource::Env,
                )
                .unwrap(),
                repo: RepoRef::parse("o/n").unwrap(),
            }
        }

        fn ctx(&self, previous: Option<Pointer>) -> PushCtx<'_> {
            PushCtx {
                client: &self.client,
                repo: &self.repo,
                cfg: &self.cfg,
                roots: &self.roots,
                keys: &self.keys,
                kdf: CHEAP,
                index: &self.index,
                repo_id: REPO_ID.into(),
                keyfile_asset: "keyfile-x.json".into(),
                previous,
                allow_rollback: false,
                now: NOW,
            }
        }

        /// Write `bytes` under the config root and return the [`FilePlan`] the
        /// planner would have produced for it.
        fn seed(&self, name: &str, bytes: &[u8]) -> FilePlan {
            let path = self.roots.config_dir.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, bytes).unwrap();
            let chunk_ids: Vec<[u8; 32]> = chunk::split(bytes)
                .map(|block| *self.keys.chunk_id(block).as_bytes())
                .collect();
            FilePlan {
                path,
                sealed_chunks: chunk::sealed_chunk_count(bytes.len() as u64),
                new_chunk_ids: chunk_ids.clone(),
                chunk_ids,
                new_bytes: bytes.len() as u64,
                new_stored_bytes: bytes.len() as u64,
                reused: false,
            }
        }
    }

    // ---- the machine-bound stores (6-10) ----------------------------------

    /// The plan entry a store produces, and the store seeded to match.
    fn seed_store(fx: &Fixture, value: &str) -> FilePlan {
        fx.roots
            .stores
            .edit()
            .set(keystore::Store::ClaudeCodeOauth, value);
        let chunk_ids: Vec<[u8; 32]> = chunk::split(value.as_bytes())
            .map(|block| *fx.keys.chunk_id(block).as_bytes())
            .collect();
        FilePlan {
            path: std::path::PathBuf::from(keystore::Store::ClaudeCodeOauth.manifest_path()),
            sealed_chunks: chunk::sealed_chunk_count(value.len() as u64),
            new_chunk_ids: chunk_ids.clone(),
            chunk_ids,
            new_bytes: value.len() as u64,
            new_stored_bytes: value.len() as u64,
            reused: false,
        }
    }

    /// The manifest entry is the store's fixed wire name, its bytes really are
    /// in the packs, and no absolute path or username went with them.
    #[test]
    fn a_stores_manifest_entry_is_its_wire_name_and_its_bytes_are_sealed() {
        let fx = Fixture::new();
        let login = r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-x"}}"#;
        let plan = plan_of(vec![seed_store(&fx, login)]);

        let bundle = build(&fx.ctx(None), &plan).unwrap();
        let manifest = manifest_of(&bundle, &fx.keys);

        let entry = &manifest.files[0];
        assert_eq!(entry.path, "keystore/claude-code-oauth");
        assert_eq!(entry.true_len, login.len() as u64);
        assert!(!entry.path.starts_with('/'));
        assert!(!entry.path.contains(&fx.dir.path().display().to_string()));

        // The bytes are really there, read back through the format's own reader.
        let mut restored = Vec::new();
        for id in &entry.chunks {
            restored.extend_from_slice(&chunk_plaintext(&bundle, &fx.keys, id));
        }
        assert_eq!(restored, login.as_bytes());
    }

    /// The packer is the authority on what it sealed: a credential that rotated
    /// between planning and packing is sealed as it is *now*, and the manifest
    /// names the ids of the bytes that really went into a pack.
    #[test]
    fn a_credential_that_rotated_since_planning_is_sealed_as_it_now_is() {
        let fx = Fixture::new();
        let plan = plan_of(vec![seed_store(&fx, "the-old-token")]);
        fx.roots
            .stores
            .edit()
            .set(keystore::Store::ClaudeCodeOauth, "the-new-token");

        let bundle = build(&fx.ctx(None), &plan).unwrap();
        let entry = &manifest_of(&bundle, &fx.keys).files[0];

        let mut restored = Vec::new();
        for id in &entry.chunks {
            restored.extend_from_slice(&chunk_plaintext(&bundle, &fx.keys, id));
        }
        assert_eq!(restored, b"the-new-token");
        assert!(
            packed_ids(&bundle, &fx.keys)
                .is_superset(&entry.chunks.iter().copied().collect::<HashSet<ChunkId>>())
        );
    }

    /// A credential deleted between planning and packing leaves no entry at all
    /// — never a zero-length one, and never a chunk id nothing sealed.
    #[test]
    fn a_credential_that_vanished_since_planning_is_simply_not_in_the_snapshot() {
        let fx = Fixture::new();
        let plan = plan_of(vec![seed_store(&fx, "gone-by-the-time-we-pack")]);
        fx.roots
            .stores
            .edit()
            .set(keystore::Store::ClaudeCodeOauth, "");

        let bundle = build(&fx.ctx(None), &plan).unwrap();
        assert!(manifest_of(&bundle, &fx.keys).files.is_empty());
    }

    fn plan_of(files: Vec<FilePlan>) -> SyncPlan {
        let mut new_chunk_ids: Vec<[u8; 32]> = Vec::new();
        for file in &files {
            for id in &file.new_chunk_ids {
                if !new_chunk_ids.contains(id) {
                    new_chunk_ids.push(*id);
                }
            }
        }
        SyncPlan {
            categories: Vec::new(),
            new_chunk_ids,
            total_raw_bytes: 0,
            total_new_bytes: 0,
            total_new_stored_bytes: 0,
            files_opened: files.len(),
            append_check_miss_bytes: 0,
            index_rebuilt: false,
            file_plans: files,
        }
    }

    /// The pointer the flip would have published for `bundle` — which is what
    /// makes its packs reusable on the next push.
    ///
    /// The root is sealed here, through the same [`root_for`] the flip's rebuild
    /// closure calls, because the packer no longer produces one.
    fn published(fx: &Fixture, bundle: &PushBundle) -> Pointer {
        let (root, _) = root_for(&fx.ctx(None), None, &bundle.manifest_chunks).unwrap();
        Pointer {
            format: super::super::POINTER_VERSION,
            repo_id: REPO_ID.into(),
            keyfile: "keyfile-x.json".into(),
            snapshots: vec![SnapshotRecord {
                root: B64.encode(&root),
                index_chunks: bundle.index_chunks.clone(),
                packs: bundle.referenced_packs.clone(),
            }],
        }
    }

    /// Every chunk id the bundle's own packs really carry, read back through the
    /// format's own reader rather than from the builder's bookkeeping.
    fn packed_ids(bundle: &PushBundle, keys: &Keys) -> HashSet<ChunkId> {
        bundle
            .packs
            .iter()
            .flat_map(|p| read_header(keys, &p.bytes).unwrap().entries)
            .map(|e| e.id)
            .collect()
    }

    /// Which of the bundle's packs really holds `id`, read back through the
    /// format's own reader.
    fn pack_holding(bundle: &PushBundle, keys: &Keys, id: &ChunkId) -> ChunkId {
        bundle
            .packs
            .iter()
            .find(|p| {
                read_header(keys, &p.bytes)
                    .unwrap()
                    .entries
                    .iter()
                    .any(|e| e.id == *id)
            })
            .expect("no pack in this bundle holds that chunk")
            .id
    }

    /// The manifest this bundle carries, read back through the format's own
    /// readers rather than from the builder's bookkeeping.
    fn manifest_of(bundle: &PushBundle, keys: &Keys) -> Manifest {
        let chunks: Vec<(ChunkId, Vec<u8>)> = bundle
            .manifest_chunks
            .iter()
            .map(|id| (*id, chunk_bytes(bundle, keys, id)))
            .collect();
        Manifest::open(keys, &chunks).unwrap()
    }

    /// One chunk's **plaintext**, from whichever of this bundle's packs holds
    /// it — what a restore would really put back.
    fn chunk_plaintext(bundle: &PushBundle, keys: &Keys, id: &ChunkId) -> Vec<u8> {
        for pack in &bundle.packs {
            let header = read_header(keys, &pack.bytes).unwrap();
            if let Some(entry) = header.entries.into_iter().find(|e| e.id == *id) {
                return crate::sync::pack::open_blob(keys, &pack.bytes, &entry)
                    .unwrap()
                    .to_vec();
            }
        }
        panic!("no pack in this bundle holds that chunk");
    }

    /// One chunk's sealed bytes, from whichever of this bundle's packs holds it.
    fn chunk_bytes(bundle: &PushBundle, keys: &Keys, id: &ChunkId) -> Vec<u8> {
        for pack in &bundle.packs {
            let header = read_header(keys, &pack.bytes).unwrap();
            if let Some(entry) = header.entries.into_iter().find(|e| e.id == *id) {
                return blob_bytes(&pack.bytes, &entry).unwrap().to_vec();
            }
        }
        panic!("no pack in this bundle holds that chunk");
    }

    /// **The invariant that was broken**, asserted directly: every chunk id the
    /// manifest names is present in a pack this bundle publishes. Anything less
    /// direct — that the plan and the manifest agree, say — is an intermediate
    /// that can hold while the snapshot is still unrestorable.
    fn assert_every_named_chunk_was_packed(bundle: &PushBundle, keys: &Keys) {
        let packed = packed_ids(bundle, keys);
        for file in manifest_of(bundle, keys).files {
            for id in &file.chunks {
                assert!(
                    packed.contains(id),
                    "the manifest names a chunk of {} that no pack in this bundle holds",
                    file.path
                );
            }
        }
    }

    /// Slice one chunk out of whichever of this bundle's packs holds it.
    fn fetch(bundle: &PushBundle, at: &IndexEntry) -> Vec<u8> {
        let pack = bundle
            .packs
            .iter()
            .find(|p| p.id == at.pack)
            .expect("the entry names a pack this bundle built");
        let entry = PackEntry {
            id: at.id,
            offset: at.offset,
            clen: at.clen,
            true_len: at.true_len,
        };
        blob_bytes(&pack.bytes, &entry).unwrap().to_vec()
    }

    // ---- manifest paths -----------------------------------------------

    /// The defect Phase 5's planning found: an absolute path carries the
    /// username, is unresolvable elsewhere, and is exactly what the traversal
    /// defence is written to reject.
    #[test]
    fn a_manifest_path_is_root_relative_and_never_absolute() {
        let fx = Fixture::new();
        let roots = &fx.roots;

        let rendered = manifest_path(
            roots,
            &fx.dir.path().join("accounts/work/.credentials.json"),
        )
        .unwrap();
        assert_eq!(rendered, "config/accounts/work/.credentials.json");

        let home = manifest_path(roots, &roots.claude_home.join("projects/a.jsonl")).unwrap();
        assert_eq!(home, "claude-home/projects/a.jsonl");

        for path in [rendered, home] {
            assert!(!path.starts_with('/'), "{path}");
            assert!(!path.contains(".."), "{path}");
            assert!(
                !path.contains(fx.dir.path().to_str().unwrap()),
                "no local prefix survives: {path}"
            );
        }
    }

    #[test]
    fn a_file_under_no_root_is_an_error_rather_than_an_absolute_path() {
        let fx = Fixture::new();
        let err = manifest_path(&fx.roots, Path::new("/etc/passwd"))
            .expect_err("that is under no sync root");
        assert!(err.to_string().contains("unresolvable"), "{err}");
    }

    // ---- REPO-06: the fill loop ---------------------------------------

    /// **REPO-06, asserted as a pack count.** Thousands of chunks become a
    /// handful of objects, and the expected count is computed from [`PACK_MAX`]
    /// — the constant [`should_seal`] actually compares against — rather than
    /// written down as a literal that a change to the constant would not move.
    ///
    /// The blobs are sealed **once and cloned**, exactly as `pack.rs`'s own
    /// `PACK_MAX` test does: what is under test here is the boundary arithmetic,
    /// and three hundred real zstd-and-ChaCha passes over 256 KiB are not.
    #[test]
    fn the_fill_loop_turns_many_chunks_into_a_handful_of_self_addressing_packs() {
        let keys = Keyfile::create_with_floor(b"pw", CHEAP, CHEAP.m_kib)
            .unwrap()
            .1;
        let sealed = seal_chunk(&keys, &incompressible(CHUNK_SIZE)).unwrap();
        let (id, ciphertext, true_len) = (sealed.id, sealed.ciphertext, sealed.true_len);

        let per_pack = PACK_MAX / ciphertext.len();
        let blobs = 2 * per_pack + 3;

        let mut packs = Packing::default();
        for _ in 0..blobs {
            packs
                .push(
                    Blob {
                        id,
                        ciphertext: ciphertext.clone(),
                        true_len,
                    },
                    &keys,
                )
                .unwrap();
        }
        packs.flush(&keys).unwrap();

        assert_eq!(
            packs.done.len(),
            blobs.div_ceil(per_pack),
            "{blobs} chunks must become ceil({blobs}/{per_pack}) packs, not {blobs} objects"
        );
        assert!(
            packs.done.len() * 40 < blobs,
            "a handful of objects, not one per chunk"
        );

        for pack in &packs.done {
            assert!(
                pack.bytes.len() <= PACK_MAX,
                "a pack grew to {} past PACK_MAX",
                pack.bytes.len()
            );
            assert_eq!(
                content_address(&pack.bytes),
                pack.id,
                "a pack is named by its own bytes"
            );
            assert!(read_header(&keys, &pack.bytes).is_ok());
        }
        // Every entry names a pack that exists, and none carries the placeholder
        // written before the header was sealed.
        let built: HashSet<ChunkId> = packs.done.iter().map(|p| p.id).collect();
        assert_eq!(packs.entries.len(), blobs);
        assert!(packs.entries.iter().all(|e| built.contains(&e.pack)));
    }

    /// **T-4-14, built at [`PACK_MAX`] rather than at the advisory
    /// `PACK_TARGET`.**
    ///
    /// The pack header is still a *single* sealed chunk: `pack.rs` seals it
    /// through `chunk::seal_chunk`, which gap-closure 1-09 deliberately did not
    /// reach when it made manifests and index objects multi-chunk. This pins the
    /// slack that makes that sound.
    ///
    /// The worst case is built from the smallest blob the format admits **in
    /// practice** — a sealed full `CHUNK_SIZE` chunk, so ~192 entries at
    /// `PACK_MAX`. Building it at `PACK_TARGET` would understate the real case
    /// by half again, and a guard that understates its case is one that passes
    /// on the day it should fail.
    ///
    /// If this ever fails, the upgrade path is a format-2 **multi-chunk** header
    /// through the same `chunk::seal_all` / `reassemble` pair the manifest now
    /// uses. Raising `PACK_MAX` is what would break it.
    #[test]
    fn a_worst_case_pack_header_built_at_pack_max_still_seals_as_one_chunk() {
        let keys = Keyfile::create_with_floor(b"pw", CHEAP, CHEAP.m_kib)
            .unwrap()
            .1;
        let clen = seal_chunk(&keys, &incompressible(CHUNK_SIZE))
            .unwrap()
            .ciphertext
            .len();
        let entries = PACK_MAX / clen;

        let header = PackHeader {
            format: PACK_HEADER_VERSION,
            // Every field at its widest: a full-length id, and an offset near
            // the end of a 48 MiB pack, so no entry serializes shorter than a
            // real one would.
            entries: (0..entries)
                .map(|i| PackEntry {
                    id: ChunkId::from_bytes([0xff; 32]),
                    offset: (PACK_MAX - i * clen) as u64,
                    clen: clen as u32,
                    true_len: CHUNK_SIZE as u32,
                })
                .collect(),
        };

        let json = serde_json::to_vec(&header).unwrap();
        assert!(
            json.len() < CHUNK_SIZE,
            "a worst-case header of {entries} entries is {} bytes, past the {CHUNK_SIZE}-byte \
             single-chunk ceiling — see this test's doc comment for the upgrade path",
            json.len()
        );
        // Comfortably, not marginally: it must still seal after zstd and framing.
        assert!(seal_chunk(&keys, &json).is_ok());
        assert!(
            json.len() * 4 < CHUNK_SIZE,
            "only {} bytes of slack",
            CHUNK_SIZE - json.len()
        );
    }

    // ---- the three objects, and the order they are built in ------------

    /// A first push, walked back through Phase 1's own readers: the index object
    /// locates the manifest, the manifest opens, and the root opens under the
    /// caller's own `repo_id`.
    ///
    /// The ordering assertion is the one no other plan would catch: build the
    /// index object before the manifest is packed and every id in
    /// `manifest_chunks` names a location the index object does not describe, so
    /// a restore cannot find the manifest at all.
    #[test]
    fn a_first_push_produces_a_bundle_that_walks_back_through_phase_ones_readers() {
        let fx = Fixture::new();
        let body = incompressible(3 * CHUNK_SIZE + 17);
        let plan = plan_of(vec![fx.seed("accounts/work/.credentials.json", &body)]);

        let bundle = build(&fx.ctx(None), &plan).unwrap();
        let (framed, counter) = root_for(&fx.ctx(None), None, &bundle.manifest_chunks).unwrap();
        assert_eq!(counter, 1, "a first push starts at one");

        // The bootstrap: the index object's own chunks, named in the clear.
        assert!(!bundle.index_chunks.is_empty());
        let covered: HashSet<ChunkId> = bundle.packs.iter().map(|p| p.id).collect();
        for at in &bundle.index_chunks {
            assert!(
                covered.contains(&at.pack),
                "index_chunks must name a pack this bundle produces"
            );
        }
        let index_object = IndexObject::open(
            &fx.keys,
            &bundle
                .index_chunks
                .iter()
                .map(|at| {
                    (
                        at.id,
                        fetch(
                            &bundle,
                            &IndexEntry {
                                id: at.id,
                                pack: at.pack,
                                offset: at.offset,
                                clen: at.clen,
                                true_len: at.true_len,
                            },
                        ),
                    )
                })
                .collect::<Vec<_>>(),
        )
        .unwrap();

        let root = Root::open(&fx.keys, &framed, REPO_ID).unwrap();
        assert_eq!(root.counter, 1);
        assert_eq!(root.kdf, CHEAP);

        // **The ordering assertion.**
        let manifest_chunks: Vec<(ChunkId, Vec<u8>)> = root
            .manifest_chunks
            .iter()
            .map(|id| {
                let at = index_object
                    .resolve(id)
                    .expect("every manifest chunk resolves through the index object");
                (*id, fetch(&bundle, at))
            })
            .collect();
        let manifest = Manifest::open(&fx.keys, &manifest_chunks).unwrap();

        assert_eq!(manifest.files.len(), 1);
        let file = &manifest.files[0];
        assert_eq!(file.path, "config/accounts/work/.credentials.json");
        assert_eq!(file.true_len, body.len() as u64);
        assert_eq!(file.chunks.len(), 4);
        // …and every data chunk the manifest names resolves too.
        assert!(
            file.chunks
                .iter()
                .all(|id| index_object.resolve(id).is_some())
        );

        // Nothing about this machine survives into the bytes.
        let needle = fx.dir.path().to_str().unwrap().as_bytes();
        for pack in &bundle.packs {
            assert!(!pack.bytes.windows(needle.len()).any(|w| w == needle));
        }
        assert!(!framed.windows(needle.len()).any(|w| w == needle));
    }

    /// **The two-machine corruption this plan exists for.** `plan::build` reads a
    /// file and the packer reads it again minutes later; the bundle's contents
    /// are live transcripts that are appended to in between, so the planner's
    /// tail id names bytes that no longer exist. The packer never seals it — its
    /// `wanted` set is matched against the id of the block actually read — and a
    /// manifest built from the plan names it anyway. Push reports success and
    /// `sync pull --apply` refuses the snapshot.
    #[test]
    fn a_file_appended_to_between_planning_and_packing_names_only_chunks_it_sealed() {
        let fx = Fixture::new();
        let body = incompressible(2 * CHUNK_SIZE + 11);
        let file = fx.seed("claude-home/history.jsonl", &body);
        let path = file.path.clone();
        let plan = plan_of(vec![file]);

        // The append, after the plan and before the pack.
        let mut grown = body.clone();
        grown.extend_from_slice(&incompressible(4096));
        std::fs::write(&path, &grown).unwrap();

        let bundle = build(&fx.ctx(None), &plan).unwrap();

        assert_every_named_chunk_was_packed(&bundle, &fx.keys);
        let entry = manifest_of(&bundle, &fx.keys).files.remove(0);
        assert_eq!(
            entry.true_len,
            grown.len() as u64,
            "the length must come from the same read as the chunks"
        );
    }

    /// The other direction: a file that shrank. Two of the planner's ids now
    /// name bytes the packer never reads at all.
    #[test]
    fn a_file_truncated_between_planning_and_packing_names_only_chunks_it_sealed() {
        let fx = Fixture::new();
        let body = incompressible(2 * CHUNK_SIZE + 11);
        let file = fx.seed("claude-home/history.jsonl", &body);
        let path = file.path.clone();
        let plan = plan_of(vec![file]);

        let shrunk = body[..CHUNK_SIZE + 5].to_vec();
        std::fs::write(&path, &shrunk).unwrap();

        let bundle = build(&fx.ctx(None), &plan).unwrap();

        assert_every_named_chunk_was_packed(&bundle, &fx.keys);
        let entry = manifest_of(&bundle, &fx.keys).files.remove(0);
        assert_eq!(entry.true_len, shrunk.len() as u64);
        assert_eq!(
            entry.chunks.len(),
            2,
            "one sealed chunk and a five-byte tail"
        );
    }

    /// **T-4-13.** A second push over an unchanged tree re-seals no data chunk,
    /// and still names the pack holding every one of them. Omitting a reused
    /// pack is the exact input that makes prune delete live data.
    #[test]
    fn a_second_push_reseals_no_data_chunk_and_still_names_the_pack_holding_it() {
        let fx = Fixture::new();
        let body = incompressible(2 * CHUNK_SIZE);
        let plan = plan_of(vec![fx.seed("accounts/work/.credentials.json", &body)]);

        let first = build(&fx.ctx(None), &plan).unwrap();
        let second = build(&fx.ctx(Some(published(&fx, &first))), &plan).unwrap();

        let data: HashSet<ChunkId> = plan.file_plans[0]
            .chunk_ids
            .iter()
            .map(|raw| ChunkId::from_bytes(*raw))
            .collect();
        assert!(
            packed_ids(&second, &fx.keys).is_disjoint(&data),
            "a chunk the chunk table already locates is neither re-sealed nor re-packed"
        );
        for id in &data {
            assert!(
                second
                    .referenced_packs
                    .contains(&pack_holding(&first, &fx.keys, id)),
                "the pack holding the unchanged data must still be named"
            );
        }
        // Only the snapshot's own new objects were packed at all: the manifest,
        // and the index object in its own pack.
        assert_eq!(second.packs.len(), 2);
        // The file was never opened, so its manifest entry's length came from
        // the plaintext lengths of the chunks it names rather than from a stat.
        // It must still be the file's own length, or the entry describes a
        // length its own chunk list cannot produce.
        let entry = manifest_of(&second, &fx.keys).files.remove(0);
        assert_eq!(entry.true_len, body.len() as u64);
        assert_eq!(entry.chunks.len(), 2);
        assert!(entry.chunks.iter().all(|id| data.contains(id)));
        let published = published(&fx, &first);
        assert_eq!(
            root_for(&fx.ctx(None), Some(&published), &second.manifest_chunks)
                .unwrap()
                .1,
            2
        );
    }

    /// The gap `2-05-SUMMARY.md` recorded: a chunk shared with a file that
    /// failed its append check was re-uploaded, because the only evidence of
    /// "already present" lived in the `file` table.
    #[test]
    fn a_chunk_shared_with_a_file_that_failed_its_append_check_is_not_resealed() {
        let fx = Fixture::new();
        let body = incompressible(2 * CHUNK_SIZE);
        let first = build(&fx.ctx(None), &plan_of(vec![fx.seed("a.json", &body)])).unwrap();

        // The second file is byte-identical, and its plan claims every chunk is
        // new — which is exactly what a failed append check produces.
        let shared = fx.seed("b.json", &body);
        let data: HashSet<ChunkId> = shared
            .chunk_ids
            .iter()
            .map(|raw| ChunkId::from_bytes(*raw))
            .collect();
        let second = build(
            &fx.ctx(Some(published(&fx, &first))),
            &plan_of(vec![shared.clone()]),
        )
        .unwrap();

        assert!(
            packed_ids(&second, &fx.keys).is_disjoint(&data),
            "the plan called them new; the chunk table knows better"
        );
        assert!(data.iter().all(|id| {
            second
                .referenced_packs
                .contains(&pack_holding(&first, &fx.keys, id))
        }));
    }

    /// **The reason `reusable` asks the pointer and not just the chunk table.**
    ///
    /// A push that packs and then fails at upload leaves chunk rows pointing at
    /// a pack no remote ever saw. Reusing one would publish a snapshot
    /// referencing a pack that does not exist — an unrestorable backup. So a row
    /// whose pack no published snapshot names buys nothing.
    #[test]
    fn a_chunk_whose_pack_no_published_snapshot_names_is_packed_again() {
        let fx = Fixture::new();
        let body = incompressible(2 * CHUNK_SIZE);
        let plan = plan_of(vec![fx.seed("a.json", &body)]);

        let attempted = build(&fx.ctx(None), &plan).unwrap();
        // The flip never happened: the pointer is still absent.
        let retried = build(&fx.ctx(None), &plan).unwrap();

        let data: HashSet<ChunkId> = plan.file_plans[0]
            .chunk_ids
            .iter()
            .map(|raw| ChunkId::from_bytes(*raw))
            .collect();
        assert!(
            data.is_subset(&packed_ids(&retried, &fx.keys)),
            "the chunk table alone is not evidence that a pack landed"
        );
        // …and the same applies to a pointer that names some other pack.
        let stale = Pointer {
            snapshots: vec![SnapshotRecord {
                packs: vec![ChunkId::from_bytes([0xaa; 32])],
                ..published(&fx, &attempted).snapshots[0].clone()
            }],
            ..published(&fx, &attempted)
        };
        let third = build(&fx.ctx(Some(stale)), &plan).unwrap();
        assert!(data.is_subset(&packed_ids(&third, &fx.keys)));
    }

    /// One above the counter inside the newest *sealed root*, not one above the
    /// pointer's length: position in `snapshots` is remote-controlled and the
    /// counter is not.
    ///
    /// **And derived from the pointer that is passed in**, not from the one the
    /// packer ran against — which is what makes the conflict path able to
    /// recompute it after losing a race.
    #[test]
    fn the_counter_is_one_above_the_highest_the_pointers_roots_carry() {
        let fx = Fixture::new();

        let sealed = |counter: u64| {
            B64.encode(
                Root::new(counter, NOW, REPO_ID.into(), Vec::new(), CHEAP)
                    .seal(&fx.keys)
                    .unwrap(),
            )
        };
        let record = |root: String| SnapshotRecord {
            root,
            index_chunks: Vec::new(),
            packs: Vec::new(),
        };
        // Newest last by convention — but the highest counter wins regardless of
        // where a hostile remote puts it.
        let pointer = Pointer {
            format: super::super::POINTER_VERSION,
            repo_id: REPO_ID.into(),
            keyfile: "keyfile-x.json".into(),
            snapshots: vec![record(sealed(7)), record(sealed(3))],
        };
        assert_eq!(highest_counter(Some(&pointer), &fx.keys, REPO_ID), 7);
        // The ctx carries no pointer at all: the counter follows the argument,
        // never the context the packer was built against.
        assert_eq!(root_for(&fx.ctx(None), Some(&pointer), &[]).unwrap().1, 8);
        assert_eq!(root_for(&fx.ctx(None), None, &[]).unwrap().1, 1);
        assert_eq!(highest_counter(None, &fx.keys, REPO_ID), 0);

        // A root this build cannot open falls back to the snapshot count rather
        // than to zero: monotone for this machine, never a value already
        // published.
        let opaque = Pointer {
            snapshots: vec![record("not base64 at all".into()); 3],
            ..pointer
        };
        assert_eq!(highest_counter(Some(&opaque), &fx.keys, REPO_ID), 3);
    }
}
