//! The read chain: pointer → keyfile → snapshot root → index object →
//! manifest → packs.
//!
//! Everything this module touches comes off a remote the format treats as
//! hostile, and only the pointer is unauthenticated. So the order matters and
//! is worth stating: the pointer says *where* to look, the keyfile unwraps the
//! master key, and from the snapshot root onward every byte is under an AEAD
//! tag before it is parsed. Nothing the pointer claims about lengths, offsets
//! or ordering is believed — the pointer is trusted only to name a pack asset
//! and a chunk id, and the pack's own sealed header says where that chunk
//! really sits (see [`PackSource`]).
//!
//! # Restore cannot write to the remote
//!
//! There is no request-body call site in this file and no path that creates a
//! release: `Client::get_json`, `Client::list_assets`, `Client::get_contents`
//! (through `push::pointer::load`) and `Client::download_asset` are the four
//! verbs used, and not one of them takes a `gate::Pushing`. A missing release
//! is "nothing has been pushed yet", never a reason to make one.
//!
//! # Whole packs, not ranges
//!
//! CAL-1 — whether a private-repo release asset honours `Range:` after the 302
//! to signed storage — was scheduled in Phase 1 and again in Phase 3 plan 3-06,
//! and was **not run**. So `PACK_TARGET` stands at 32 MiB and a restore fetches
//! each pack it needs in full. `download_asset` caps at 64 MiB, comfortably
//! above [`PACK_ASSET_MAX`], so no streaming verb and no `reqwest` `stream` feature is
//! needed either.
//!
//! Naming the optimisation so a future measurement has somewhere to land: if
//! `Range:` is ever confirmed, [`PackSource`] gains a byte-range fetch keyed on
//! the `PackEntry`'s `offset` and `clen` and **nothing else in this file
//! changes** — the ceilings, the content-address check and the three rounds all
//! survive it. It is not implemented on an assumption: the pessimistic path is
//! correct either way, and the optimistic one is wrong if the measurement comes
//! back negative.
//!
//! Plan 5-01 filled the chain; plan 5-02 owns hardening it.

use std::collections::HashMap;

use base64::Engine;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::error::{AppError, Result};
use crate::sync::anchor::{self, Anchor};
use crate::sync::crypto::{ChunkId, Keyfile};
use crate::sync::github::write::{ASSET_STATE_UPLOADED, MAX_ASSET_BYTES};
use crate::sync::github::{Client, RepoRef};
use crate::sync::model::{IndexObject, Manifest, Root};
use crate::sync::pack::PACK_ASSET_MAX;
use crate::sync::push::{self, RELEASE_TAG, SnapshotRecord};

use super::{PackSource, Resolved, RestoreCtx};

const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD;

/// Snapshot records this build will walk in one pointer.
///
/// `keep_snapshots` defaults to 10 and prune truncates the published list to it
/// (see `config.rs` and `push::prune::plan_deletions`), so a legitimate pointer
/// holds ten records, plus whatever a machine mid-flip has appended. 256 is more
/// than an order of magnitude past that, and it is checked before a single asset
/// is fetched — the pointer is plaintext, so every list in it is a length the
/// remote chose (T-5-12).
///
/// There is no monthly retention tail. An earlier version of this comment said
/// there was, and `grep -rn monthly src/sync/` finds nothing: the ceiling was
/// right, its stated reason was invented.
const MAX_SNAPSHOTS_IN_POINTER: usize = 256;

/// Chunks the ordered manifest chunk list inside a snapshot root may name.
///
/// **This is the ceiling that matters most, and the reason is not the size of
/// the list.** `Root.manifest_chunks` is an unbounded `Vec<ChunkId>` and it is
/// perfectly safe *inside* the root's authenticated plaintext — but a reader
/// consumes it to decide **how many fetches to issue**, which is a decision
/// made from a list before the objects it names have authenticated anything.
/// That is exactly the shape Phase 1's handoff flagged as living in restore
/// (T-5-10).
///
/// `docs/sync-format.md` §5 measures the quantity this is derived from: at a
/// representative 229 bytes per entry, 1,600 files is 2 chunks and 5,700 files
/// — the whole `.claude` home with transcript sync on — is 5. 128 chunks is
/// roughly 146,000 files: an order of magnitude past the largest bundle the
/// format has ever been measured against.
const MAX_MANIFEST_CHUNKS: usize = 128;

/// Chunks of the index object the pointer's plaintext bootstrap may name.
///
/// The index object carries one ~180-byte JSON entry per chunk in the bundle
/// and is split at `CHUNK_SIZE` like anything else. [`MAX_RESTORE_BYTES`]
/// bounds a bundle at 49,152 chunks of 256 KiB, whose index is ~8.4 MiB and so
/// ~34 chunks. 256 is that with room to spare, and — unlike the manifest's list
/// — this one is entirely unauthenticated, so it is checked before the first
/// pack request (T-5-11).
const MAX_INDEX_CHUNKS: usize = 256;

/// Packs one restore will download — a bound on **requests**.
///
/// [`MAX_RESTORE_BYTES`] does not give this one: 512 one-byte packs cost 512
/// round trips and almost no transfer. At `PACK_TARGET` a legitimate 512-pack
/// snapshot would be 16 GiB of stored data, three orders of magnitude past a
/// config-and-credentials bundle.
const MAX_PACKS_PER_RESTORE: usize = 512;

/// Bytes one restore will download — a bound on **transfer**.
///
/// [`MAX_PACKS_PER_RESTORE`] does not give this one either: 512 packs at
/// [`PACK_ASSET_MAX`] would be 24 GiB. Derived as 256 full packs, which is where the
/// two ceilings cross; whichever binds first, binds, and each refusal names the
/// number a user who legitimately outgrows it has to raise.
const MAX_RESTORE_BYTES: u64 = 256 * PACK_ASSET_MAX as u64;

/// A sealed root is one chunk plus framing; base64 inflates by 4/3. Bounded
/// before the decode allocates.
const MAX_ROOT_B64: usize = 2 * crate::sync::CHUNK_SIZE;

/// The only field of a release this side reads.
#[derive(Deserialize)]
struct ReleaseRef {
    id: u64,
}

/// One release asset, reduced to what a download needs.
#[derive(Clone, Copy)]
struct AssetRef {
    id: u64,
    size: u64,
}

/// Walk the chain and return everything authenticated.
///
/// `local_anchor` arrives as an argument rather than being read here, because
/// the path it came from must not be derived from the remote's claimed
/// `repo_id` — that is [`anchor`]'s stated constraint, and `restore::run` step 1
/// is what honours it.
pub async fn resolve(ctx: &RestoreCtx<'_>, local_anchor: Option<&Anchor>) -> Result<Resolved> {
    // 1. The pointer. `push::pointer::load` already probes `format` before
    //    deserializing and refuses a `repo_id` that is not this machine's own;
    //    a second copy of either check here would be one more thing to diverge.
    let (pointer, _sha) = push::pointer::load(ctx.client, ctx.repo, ctx.repo_id, ctx.now).await?;
    let Some(pointer) = pointer else {
        return Err(AppError::Other(
            "this repository has no snapshot pointer, so nothing has been pushed to it yet — \
             run `ai-usagebar sync push` on the machine that has the data"
                .into(),
        ));
    };
    if pointer.snapshots.len() > MAX_SNAPSHOTS_IN_POINTER {
        return Err(AppError::Other(format!(
            "the snapshot pointer lists {} snapshots, past the {MAX_SNAPSHOTS_IN_POINTER} this \
             build will walk — refusing rather than following an unbounded list",
            pointer.snapshots.len()
        )));
    }
    if pointer.snapshots.is_empty() {
        return Err(AppError::Other(
            "the snapshot pointer names no snapshots, so there is nothing to restore".into(),
        ));
    }

    // 2. The release, read-only. A missing one is "nothing pushed yet".
    let release_id = find_release(ctx.client, ctx.repo).await?.ok_or_else(|| {
        AppError::Other(format!(
            "this repository has no `{RELEASE_TAG}` release, so the bundle's data is not there \
             even though a pointer is — nothing was restored"
        ))
    })?;

    // 3. One listing for the whole restore: the keyfile and every pack are
    //    looked up in it by name.
    let assets = asset_index(ctx.client, ctx.repo, release_id, ctx.now).await?;

    // 4. The keyfile named by the pointer. Its failure to open is deliberately
    //    the same error a wrong password gives — elaborating it would build the
    //    oracle Phase 1 refused to build.
    let keyfile_bytes = download(
        ctx.client,
        ctx.repo,
        ctx.now,
        &assets,
        &pointer.keyfile,
        "keyfile",
    )
    .await?;
    let keyfile: Keyfile = serde_json::from_slice(&keyfile_bytes).map_err(|_| {
        AppError::Other(format!(
            "the keyfile asset {:?} is not a readable sync keyfile — this bundle cannot be \
             opened",
            pointer.keyfile
        ))
    })?;
    let keys = keyfile.open(ctx.passphrase.as_bytes())?;

    // 5. Every snapshot root, opened under *this machine's* `repo_id`. It is
    //    bound into the root's associated data, so a root belonging to another
    //    bundle fails the Poly1305 tag before a field is parsed — do not
    //    "harden" this with a string comparison and think anything was added.
    let mut newest: Option<(Root, &SnapshotRecord)> = None;
    for record in &pointer.snapshots {
        if record.root.len() > MAX_ROOT_B64 {
            return Err(AppError::Other(
                "a snapshot record carries more base64 than a sealed root can be".into(),
            ));
        }
        let Ok(framed) = B64.decode(&record.root) else {
            continue;
        };
        // A root this build cannot open is skipped, not fatal: a pointer may
        // carry a record written by a format this build predates, and one such
        // record must not make every older snapshot unrestorable.
        let Ok(root) = Root::open(&keys, &framed, ctx.repo_id) else {
            continue;
        };
        // Ties are possible and are not hypothetical: the push side computes a
        // snapshot's counter from the pointer it read *before* the
        // compare-and-swap that publishes it, so two machines racing can
        // publish distinct snapshots under one counter. Break the tie on the
        // root's own **sealed** `created_at`, and then on the sealed bytes
        // themselves — both authenticated, so the plaintext list's order still
        // decides nothing (T-5-15). `record.root` is only a deterministic total
        // order here, not a trust decision: a tampered one never opened.
        let better = newest.as_ref().is_none_or(|(best, best_record)| {
            (root.counter, root.created_at, &record.root)
                > (best.counter, best.created_at, &best_record.root)
        });
        if better {
            newest = Some((root, record));
        }
    }
    let Some((root, record)) = newest else {
        return Err(AppError::Other(
            "none of the snapshots in this pointer could be opened — either the passphrase is \
             wrong, or the bundle was written by a newer ai-usagebar"
                .into(),
        ));
    };

    // 6. The rollback decision, against the root's **sealed** counter and
    //    `repo_id` — never the plaintext pointer's copies. `accept` only
    //    decides; `restore::run` step 7 persists, and only after this whole
    //    function has returned `Ok`. Advancing earlier would let a forged high
    //    counter lock the user out of their own bundle permanently.
    anchor::accept(
        local_anchor,
        &root.repo_id,
        root.counter,
        ctx.opts.allow_rollback,
    )?;

    if record.index_chunks.len() > MAX_INDEX_CHUNKS {
        return Err(AppError::Other(format!(
            "this snapshot describes its index in {} chunks, past the {MAX_INDEX_CHUNKS} this \
             build will read",
            record.index_chunks.len()
        )));
    }
    if root.manifest_chunks.len() > MAX_MANIFEST_CHUNKS {
        return Err(AppError::Other(format!(
            "this snapshot's manifest spans {} chunks, past the {MAX_MANIFEST_CHUNKS} this \
             build will read",
            root.manifest_chunks.len()
        )));
    }
    // The pointer's own claim about how much this restore would cost, checked
    // against both ceilings before the first pack is requested. The sizes come
    // from the release listing rather than from the pointer, so a snapshot that
    // simply lies about its pack set still cannot get past the count.
    if record.packs.len() > MAX_PACKS_PER_RESTORE {
        return Err(AppError::Other(format!(
            "this snapshot references {} packs, past the {MAX_PACKS_PER_RESTORE} one restore \
             will fetch",
            record.packs.len()
        )));
    }
    let claimed_bytes = record
        .packs
        .iter()
        .map(|id| {
            assets
                .get(&push::pack_asset_name(id))
                .map_or(0, |asset| asset.size)
        })
        .fold(0u64, u64::saturating_add);
    if claimed_bytes > MAX_RESTORE_BYTES {
        return Err(AppError::Other(format!(
            "this snapshot's packs come to {claimed_bytes} bytes, past the \
             {MAX_RESTORE_BYTES} one restore will download"
        )));
    }

    // 7. Round one: the packs holding the index object, which is the plaintext
    //    bootstrap — nothing describes itself.
    let mut packs = PackSource::empty(keys);
    let wanted: Vec<ChunkId> = record.index_chunks.iter().map(|e| e.pack).collect();
    fetch_packs(ctx, &assets, &mut packs, &wanted).await?;

    let index_sealed = sealed_in_order(
        &packs,
        record.index_chunks.iter().map(|e| e.id).collect::<Vec<_>>(),
    )?;
    let index = IndexObject::open(packs.keys(), &index_sealed)?;

    // 8. Round two: the packs holding the manifest, located through the index.
    let manifest_packs = packs_for(&index, &root.manifest_chunks)?;
    fetch_packs(ctx, &assets, &mut packs, &manifest_packs).await?;
    let manifest_sealed = sealed_in_order(&packs, root.manifest_chunks.clone())?;
    let manifest = Manifest::open(packs.keys(), &manifest_sealed)?;

    // 9. Round three: file data — **only** when this run will actually write.
    //    A dry run never pulls a byte of a user's file content, which is what
    //    keeps `PackSource`'s accessors synchronous.
    if ctx.opts.apply {
        let mut needed: Vec<ChunkId> = Vec::new();
        for file in &manifest.files {
            for id in &file.chunks {
                let entry = index.resolve(id).ok_or_else(|| missing_chunk(id))?;
                if !packs.holds_pack(&entry.pack) && !needed.contains(&entry.pack) {
                    needed.push(entry.pack);
                }
            }
        }
        // No second count check here: `fetch_packs` bounds the running total
        // across all three rounds, which is the number that matters and the
        // only one a second copy here could ever disagree with.
        fetch_packs(ctx, &assets, &mut packs, &needed).await?;
    }

    Ok(Resolved {
        root,
        manifest,
        index,
        packs,
    })
}

/// The release id for [`RELEASE_TAG`], or `None` when there is no such release.
///
/// Deliberately **not** `write::ensure_release`: that verb's 404 arm creates a
/// release, and restore must be structurally incapable of changing the remote.
/// `get_json` is the crate's one read verb and takes no push capability.
async fn find_release(client: &Client, repo: &RepoRef) -> Result<Option<u64>> {
    let path = format!(
        "/repos/{}/{}/releases/tags/{RELEASE_TAG}",
        repo.owner, repo.name
    );
    let (status, _headers, body) = client.get_json(&path).await?;
    if status.as_u16() == 404 {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(AppError::Http {
            status: status.as_u16(),
            body: "could not read the sync release from this repository".into(),
        });
    }
    let release: ReleaseRef = serde_json::from_slice(&body)
        .map_err(|_| AppError::Schema("the sync release listing was not readable".into()))?;
    Ok(Some(release.id))
}

/// One listing, by asset name. A torn upload (any state but `uploaded`) is not
/// offered: its bytes are incomplete by definition.
async fn asset_index(
    client: &Client,
    repo: &RepoRef,
    release_id: u64,
    now: DateTime<Utc>,
) -> Result<HashMap<String, AssetRef>> {
    let listed = client.list_assets(repo, release_id, now).await?;
    Ok(listed
        .into_iter()
        .filter(|a| a.state == ASSET_STATE_UPLOADED)
        .map(|a| {
            (
                a.name,
                AssetRef {
                    id: a.id,
                    size: a.size,
                },
            )
        })
        .collect())
}

/// Download one named asset, refusing a declared size this build will not hold.
async fn download(
    client: &Client,
    repo: &RepoRef,
    now: DateTime<Utc>,
    assets: &HashMap<String, AssetRef>,
    name: &str,
    what: &str,
) -> Result<Vec<u8>> {
    let asset = assets.get(name).ok_or_else(|| {
        AppError::Other(format!(
            "this bundle's pointer names the {what} asset {name:?}, which is not on the sync \
             release — the bundle cannot be opened. It may have been pruned, or the push that \
             would have uploaded it never finished"
        ))
    })?;
    if asset.size > MAX_ASSET_BYTES {
        return Err(AppError::Other(format!(
            "the {what} asset {name:?} is declared as {} bytes, past what this build will \
             download",
            asset.size
        )));
    }
    client.download_asset(repo, asset.id, now).await
}

/// The published keyfile asset's bytes, by the name a pointer gave.
///
/// The whole read chain a caller that holds a pointer and **no local keyfile**
/// needs: `sync setup`'s join path, which adopts an already-published bundle's
/// wrapper instead of minting a second master key for it (`github::setup::run`
/// step 3). It composes the same three verbs [`resolve`] does — none of which
/// takes a `gate::Pushing`, so this cannot change the remote either — and it
/// reuses [`download`]'s ceiling rather than inventing a second one: the size
/// is remote-chosen, and it is refused from the release listing before the
/// request that would allocate it.
///
/// It returns **bytes, not a [`Keyfile`]**. The caller writes them verbatim, so
/// the local file is byte-identical to the asset the pointer names — which is
/// what `push::upload::assert_keyfile_is_current` compares a push against.
/// Re-serializing here would be one more place for the two to disagree.
///
/// Nothing is authenticated at this point. The AEAD unwrap in [`Keyfile::open`]
/// is the only thing that ever will be, and the caller must not persist a byte
/// before it succeeds.
pub(crate) async fn published_keyfile(
    client: &Client,
    repo: &RepoRef,
    name: &str,
    now: DateTime<Utc>,
) -> Result<Vec<u8>> {
    let release_id = find_release(client, repo).await?.ok_or_else(|| {
        AppError::Other(format!(
            "this repository has a snapshot pointer but no `{RELEASE_TAG}` release, so the \
             keyfile that pointer names is not there — nothing was written"
        ))
    })?;
    let assets = asset_index(client, repo, release_id, now).await?;
    download(client, repo, now, &assets, name, "keyfile").await
}

/// Fetch every pack in `wanted` that is not already held.
///
/// Both ceilings are **cumulative across the three rounds** and both are
/// checked before the round's first byte is requested, from the sizes the
/// release listing declared. A bound applied after the download is not a bound,
/// and a bound applied per round would let three rounds cost three times the
/// ceiling. Already-held packs are skipped, which is what makes a pack shared
/// by two manifest entries — or by the manifest and a file — one download.
async fn fetch_packs(
    ctx: &RestoreCtx<'_>,
    assets: &HashMap<String, AssetRef>,
    packs: &mut PackSource,
    wanted: &[ChunkId],
) -> Result<()> {
    let mut round: Vec<(ChunkId, String)> = Vec::new();
    let mut round_bytes = 0u64;
    for id in wanted {
        if packs.holds_pack(id) || round.iter().any(|(seen, _)| seen == id) {
            continue;
        }
        let name = push::pack_asset_name(id);
        let size = assets.get(&name).map_or(0, |asset| asset.size);
        if size > PACK_ASSET_MAX as u64 {
            return Err(AppError::Other(format!(
                "the pack asset {name:?} is declared as {size} bytes, larger than the \
                 {PACK_ASSET_MAX} a pack can be — refusing it"
            )));
        }
        round_bytes = round_bytes.saturating_add(size);
        round.push((*id, name));
    }

    let total_packs = packs.packs() + round.len();
    if total_packs > MAX_PACKS_PER_RESTORE {
        return Err(AppError::Other(format!(
            "this restore would download {total_packs} packs, past the \
             {MAX_PACKS_PER_RESTORE} it will fetch"
        )));
    }
    let total_bytes = packs.bytes().saturating_add(round_bytes);
    if total_bytes > MAX_RESTORE_BYTES {
        return Err(AppError::Other(format!(
            "this restore would download {total_bytes} bytes, past the {MAX_RESTORE_BYTES} \
             it will fetch"
        )));
    }

    for (id, name) in round {
        let bytes = download(ctx.client, ctx.repo, ctx.now, assets, &name, "pack").await?;
        packs.add(id, bytes)?;
    }
    Ok(())
}

/// Which packs hold `ids`, according to the (authenticated) index object.
fn packs_for(index: &IndexObject, ids: &[ChunkId]) -> Result<Vec<ChunkId>> {
    let mut out: Vec<ChunkId> = Vec::new();
    for id in ids {
        let entry = index.resolve(id).ok_or_else(|| missing_chunk(id))?;
        if !out.contains(&entry.pack) {
            out.push(entry.pack);
        }
    }
    Ok(out)
}

/// The sealed bytes of `ids`, in the order given — the shape
/// [`Manifest::open`] and [`IndexObject::open`] take. Order is the caller's to
/// get right: a chunk carries no position.
fn sealed_in_order(packs: &PackSource, ids: Vec<ChunkId>) -> Result<Vec<(ChunkId, Vec<u8>)>> {
    ids.into_iter()
        .map(|id| packs.sealed(&id).map(|bytes| (id, bytes)))
        .collect()
}

fn missing_chunk(id: &ChunkId) -> AppError {
    AppError::Other(format!(
        "this snapshot names chunk {id}, which its index does not describe — refusing rather \
         than restoring a partial tree"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SyncConfig;
    use crate::sync::crypto::{KdfParams, Keys, content_address};
    use crate::sync::github::token::TokenSource;
    use crate::sync::github::{Client, Endpoints, RepoRef};
    use crate::sync::index::Index;
    use crate::sync::plan::{FilePlan, SyncPlan};
    use crate::sync::push::{Pointer, PushCtx, RemoteIndexEntry, keyfile_asset_name};
    use crate::sync::restore::RestoreOptions;
    use crate::sync::{SyncRoots, chunk};
    use chrono::{DateTime, Utc};
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;
    use zeroize::Zeroizing;

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
    const RELEASE: u64 = 9;
    const PASSWORD: &str = "correct horse battery staple";

    fn roots_at(dir: &Path, user: &str) -> SyncRoots {
        let home = dir.join(user);
        SyncRoots::at(
            home.join(".config/ai-usagebar/config.toml"),
            home.join(".config/ai-usagebar"),
            home.join("desktop"),
            home.join("profiles"),
            home.join(".claude"),
        )
    }

    fn client_at(base: &str) -> Client {
        Client::new(
            &Endpoints {
                api_base: base.into(),
                uploads_base: base.into(),
            },
            Zeroizing::new("github_pat_fixture_not_a_real_token".into()),
            TokenSource::Env,
        )
        .unwrap()
    }

    /// A distinct, deterministic id that names nothing — for the ceiling tests,
    /// which must refuse *before* anything is fetched and so never need the id
    /// to resolve.
    fn fake_id(n: u32) -> ChunkId {
        let mut bytes = [0u8; 32];
        bytes[..4].copy_from_slice(&n.to_le_bytes());
        ChunkId::from_bytes(bytes)
    }

    /// One release asset.
    ///
    /// `size` is what the *listing* declares and `bytes` is what a download
    /// returns, kept apart because every byte ceiling is checked against the
    /// declared number — which is the one a hostile remote chooses — and a test
    /// that could not make the two disagree could not reach the check.
    /// `bytes: None` is an asset that is listed and has no download mock at all,
    /// so a test asserting "refused before any fetch" fails loudly if the
    /// refusal did not happen.
    struct Asset {
        name: String,
        size: u64,
        bytes: Option<Vec<u8>>,
    }

    impl Asset {
        fn real(name: String, bytes: Vec<u8>) -> Self {
            Self {
                size: bytes.len() as u64,
                name,
                bytes: Some(bytes),
            }
        }

        /// Listed at a size it does not have, and undownloadable.
        fn listed_only(name: String, size: u64) -> Self {
            Self {
                name,
                size,
                bytes: None,
            }
        }
    }

    /// A bundle produced by the **push** side, so the pair is exercised rather
    /// than a hand-rolled remote that could agree with a broken reader. Every
    /// adversarial case below mutates one field of a real bundle.
    struct Bundle {
        pointer: Pointer,
        keyfile: crate::sync::crypto::Keyfile,
        keyfile_bytes: Vec<u8>,
        keyfile_name: String,
        packs: Vec<(String, Vec<u8>)>,
        _pusher: TempDir,
    }

    impl Bundle {
        fn keys(&self) -> Keys {
            self.keyfile.open(PASSWORD.as_bytes()).unwrap()
        }

        fn record(&mut self) -> &mut crate::sync::push::SnapshotRecord {
            &mut self.pointer.snapshots[0]
        }

        fn root(&self) -> Root {
            let framed = B64.decode(&self.pointer.snapshots[0].root).unwrap();
            Root::open(&self.keys(), &framed, REPO_ID).unwrap()
        }

        /// Re-seal the snapshot root after editing it — the only way to hand
        /// `resolve` a root the push side would never emit while keeping it a
        /// *real* sealed root rather than a hand-written fixture.
        fn reseal_root(&mut self, edit: impl FnOnce(&mut Root)) {
            let keys = self.keys();
            let mut root = self.root();
            edit(&mut root);
            self.pointer.snapshots[0].root = B64.encode(root.seal(&keys).unwrap());
        }

        fn assets(&self) -> Vec<Asset> {
            let mut out = vec![Asset::real(
                self.keyfile_name.clone(),
                self.keyfile_bytes.clone(),
            )];
            for (name, bytes) in &self.packs {
                out.push(Asset::real(name.clone(), bytes.clone()));
            }
            out
        }
    }

    fn build_bundle(files: impl IntoIterator<Item = (String, Vec<u8>)>) -> Bundle {
        let dir = TempDir::new().unwrap();
        let pusher = roots_at(dir.path(), "alice");
        let (keyfile, keys) =
            Keyfile::create_with_floor(PASSWORD.as_bytes(), CHEAP, CHEAP.m_kib).unwrap();
        let keyfile_bytes = serde_json::to_vec(&keyfile).unwrap();
        let keyfile_name = keyfile_asset_name(&content_address(&keyfile_bytes));

        let mut file_plans = Vec::new();
        let mut new_chunk_ids: Vec<[u8; 32]> = Vec::new();
        let mut raw = 0u64;
        for (rel, body) in files {
            let path = pusher.config_dir.join(&rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, &body).unwrap();
            let chunk_ids: Vec<[u8; 32]> = chunk::split(&body)
                .map(|block| *keys.chunk_id(block).as_bytes())
                .collect();
            for id in &chunk_ids {
                if !new_chunk_ids.contains(id) {
                    new_chunk_ids.push(*id);
                }
            }
            raw += body.len() as u64;
            file_plans.push(FilePlan {
                path,
                sealed_chunks: chunk::sealed_chunk_count(body.len() as u64),
                new_chunk_ids: chunk_ids.clone(),
                chunk_ids,
                new_bytes: body.len() as u64,
                new_stored_bytes: body.len() as u64,
                reused: false,
            });
        }

        let plan = SyncPlan {
            categories: Vec::new(),
            new_chunk_ids,
            total_raw_bytes: raw,
            total_new_bytes: raw,
            total_new_stored_bytes: raw,
            files_opened: file_plans.len(),
            append_check_miss_bytes: 0,
            index_rebuilt: false,
            file_plans,
        };

        let cfg = SyncConfig::default();
        let index = Index::at(&pusher.index_file).unwrap();
        // Parked: `packer::build` is pure, and a regression that made it dial
        // should fail rather than reach anything real.
        let client = client_at("http://127.0.0.1:1");
        let repo = RepoRef::parse("o/n").unwrap();
        let ctx = PushCtx {
            client: &client,
            repo: &repo,
            cfg: &cfg,
            roots: &pusher,
            keys: &keys,
            kdf: CHEAP,
            index: &index,
            repo_id: REPO_ID.into(),
            keyfile_asset: keyfile_name.clone(),
            previous: None,
            allow_rollback: false,
            now: NOW,
        };
        let bundle = push::packer::build(&ctx, &plan).unwrap();
        // 4-08 moved the root out of `PushBundle`: the counter is derived from
        // the pointer this push is racing against, inside the rebuild closure,
        // so two machines that both read counter 6 no longer both publish 7.
        // A fixture builds against no arriving pointer, which is a first push.
        let (root, _counter) = push::packer::root_for(&ctx, None, &bundle.manifest_chunks).unwrap();

        Bundle {
            pointer: Pointer {
                format: push::POINTER_VERSION,
                repo_id: REPO_ID.into(),
                keyfile: keyfile_name.clone(),
                snapshots: vec![crate::sync::push::SnapshotRecord {
                    root: B64.encode(&root),
                    index_chunks: bundle.index_chunks.clone(),
                    packs: bundle.referenced_packs.clone(),
                }],
            },
            keyfile: keyfile.clone(),
            keyfile_bytes,
            keyfile_name,
            packs: bundle
                .packs
                .iter()
                .map(|p| (push::pack_asset_name(&p.id), p.bytes.clone()))
                .collect(),
            _pusher: dir,
        }
    }

    fn one_file(rel: &str, body: &[u8]) -> Vec<(String, Vec<u8>)> {
        vec![(rel.into(), body.to_vec())]
    }

    /// The remote, served exactly as GitHub would. **Nothing here answers a
    /// request body**: there is no `POST`, `PUT` or `DELETE` mock in this
    /// module, so a restore that tried to write would get mockito's 501 and
    /// fail the test rather than pass quietly.
    struct Remote {
        url: String,
        _server: mockito::ServerGuard,
        _pointer: mockito::Mock,
        _release: mockito::Mock,
        listing: mockito::Mock,
        downloads: Vec<(String, mockito::Mock)>,
    }

    impl Remote {
        async fn serve(pointer: &Pointer, assets: &[Asset]) -> Remote {
            Remote::serve_expecting(pointer, assets, 1, 200).await
        }

        /// `listing_hits` is the number of `list_assets` calls the test asserts;
        /// `release_status` lets a test serve a repository that has a pointer
        /// and no release.
        async fn serve_expecting(
            pointer: &Pointer,
            assets: &[Asset],
            listing_hits: usize,
            release_status: usize,
        ) -> Remote {
            let mut server = mockito::Server::new_async().await;
            let pointer_json = serde_json::to_vec(pointer).unwrap();
            let pointer_mock = server
                .mock("GET", "/repos/o/n/contents/sync/pointer.json")
                .with_status(200)
                .with_body(format!(
                    r#"{{"sha":"deadbeef","content":"{}"}}"#,
                    B64.encode(&pointer_json)
                ))
                .create_async()
                .await;
            let release = server
                .mock(
                    "GET",
                    format!("/repos/o/n/releases/tags/{RELEASE_TAG}").as_str(),
                )
                .with_status(release_status)
                .with_body(format!(r#"{{"id":{RELEASE}}}"#))
                .create_async()
                .await;

            let mut rows = Vec::new();
            let mut downloads = Vec::new();
            for (i, asset) in assets.iter().enumerate() {
                let id = 100 + i as u64;
                rows.push(format!(
                    r#"{{"id":{id},"name":"{}","size":{},"state":"uploaded",
                        "created_at":"2023-11-14T22:13:20Z"}}"#,
                    asset.name, asset.size
                ));
                if let Some(bytes) = &asset.bytes {
                    downloads.push((
                        asset.name.clone(),
                        server
                            .mock("GET", format!("/repos/o/n/releases/assets/{id}").as_str())
                            .with_status(200)
                            .with_body(bytes.clone())
                            .expect(1)
                            .create_async()
                            .await,
                    ));
                }
            }
            let listing = server
                .mock(
                    "GET",
                    mockito::Matcher::Regex(format!("/releases/{RELEASE}/assets")),
                )
                .with_status(200)
                .with_body(format!("[{}]", rows.join(",")))
                .expect(listing_hits)
                .create_async()
                .await;

            Remote {
                url: server.url(),
                _server: server,
                _pointer: pointer_mock,
                _release: release,
                listing,
                downloads,
            }
        }

        /// Every asset was fetched exactly once — the assertion that a pack
        /// shared by two manifest entries, or by the manifest and a file, costs
        /// one download and not two.
        async fn assert_each_asset_fetched_once(&self) {
            for (name, mock) in &self.downloads {
                assert!(
                    mock.matched_async().await,
                    "{name} was not downloaded exactly once"
                );
            }
        }
    }

    /// The restoring machine. Every path is inside its own `TempDir`.
    struct Restorer {
        _dir: TempDir,
        roots: SyncRoots,
        anchor_path: PathBuf,
        backups_dir: PathBuf,
        repo: RepoRef,
        password: Zeroizing<String>,
    }

    impl Restorer {
        fn new() -> Self {
            let dir = TempDir::new().unwrap();
            let roots = roots_at(dir.path(), "bob");
            Self {
                anchor_path: dir.path().join("state/anchor.json"),
                backups_dir: dir.path().join("backups"),
                roots,
                _dir: dir,
                repo: RepoRef::parse("o/n").unwrap(),
                password: Zeroizing::new(PASSWORD.into()),
            }
        }

        fn with_password(mut self, pw: &str) -> Self {
            self.password = Zeroizing::new(pw.into());
            self
        }

        fn ctx<'a>(&'a self, client: &'a Client, opts: RestoreOptions) -> RestoreCtx<'a> {
            RestoreCtx {
                client,
                repo: &self.repo,
                roots: &self.roots,
                repo_id: REPO_ID,
                passphrase: &self.password,
                anchor_path: &self.anchor_path,
                backups_dir: &self.backups_dir,
                opts,
                now: NOW,
            }
        }
    }

    fn applying() -> RestoreOptions {
        RestoreOptions {
            apply: true,
            ..Default::default()
        }
    }

    // ---------------------------------------------------------------- task 1

    #[tokio::test]
    async fn the_chain_resolves_a_pushed_bundle_and_issues_one_listing() {
        let bundle = build_bundle(one_file("config.toml", b"[sync]\nenabled = true\n"));
        let remote = Remote::serve(&bundle.pointer, &bundle.assets()).await;
        let client = client_at(&remote.url);
        let restorer = Restorer::new();

        let resolved = resolve(&restorer.ctx(&client, applying()), None)
            .await
            .expect("a pushed bundle resolves");

        assert_eq!(resolved.root.counter, 1);
        assert_eq!(resolved.root.repo_id, REPO_ID);
        assert_eq!(resolved.manifest.files.len(), 1);
        assert_eq!(resolved.manifest.files[0].path, "config/config.toml");
        remote.listing.assert_async().await;
        remote.assert_each_asset_fetched_once().await;
    }

    #[tokio::test]
    async fn a_pointer_with_no_snapshots_says_nothing_has_been_pushed() {
        let mut bundle = build_bundle(one_file("config.toml", b"[sync]\n"));
        bundle.pointer.snapshots.clear();
        let remote = Remote::serve_expecting(&bundle.pointer, &bundle.assets(), 0, 200).await;
        let client = client_at(&remote.url);
        let restorer = Restorer::new();

        let err = resolve(&restorer.ctx(&client, applying()), None)
            .await
            .err()
            .expect("an empty snapshot list is not an empty success");
        assert!(
            err.to_string().contains("nothing to restore"),
            "an empty list must say so rather than restore zero files: {err}"
        );
        remote.listing.assert_async().await;
    }

    #[tokio::test]
    async fn a_pointer_past_the_snapshot_ceiling_is_refused_before_any_asset_is_fetched() {
        let mut bundle = build_bundle(one_file("config.toml", b"[sync]\n"));
        let record = bundle.pointer.snapshots[0].clone();
        while bundle.pointer.snapshots.len() <= MAX_SNAPSHOTS_IN_POINTER {
            bundle.pointer.snapshots.push(record.clone());
        }
        let listed = bundle.pointer.snapshots.len();
        let remote = Remote::serve_expecting(&bundle.pointer, &bundle.assets(), 0, 200).await;
        let client = client_at(&remote.url);
        let restorer = Restorer::new();

        let err = resolve(&restorer.ctx(&client, applying()), None)
            .await
            .err()
            .expect("an unbounded snapshot list must be refused");
        let text = err.to_string();
        assert!(
            text.contains(&listed.to_string())
                && text.contains(&MAX_SNAPSHOTS_IN_POINTER.to_string()),
            "the refusal names neither the observed value nor the ceiling: {text}"
        );
        remote.listing.assert_async().await;
    }

    #[tokio::test]
    async fn a_missing_keyfile_asset_is_named_rather_than_panicking() {
        let mut bundle = build_bundle(one_file("config.toml", b"[sync]\n"));
        bundle.pointer.keyfile = "sync-keyfile-0000000000000000.json".into();
        let remote = Remote::serve(&bundle.pointer, &bundle.assets()).await;
        let client = client_at(&remote.url);
        let restorer = Restorer::new();

        let err = resolve(&restorer.ctx(&client, applying()), None)
            .await
            .err()
            .expect("a pointer naming an absent keyfile must refuse");
        let text = err.to_string();
        assert!(
            text.contains("sync-keyfile-0000000000000000.json") && text.contains("keyfile"),
            "the refusal does not name the missing asset: {text}"
        );
    }

    #[tokio::test]
    async fn a_wrong_passphrase_gives_the_existing_error_and_elaborates_nothing() {
        let bundle = build_bundle(one_file("config.toml", b"[sync]\n"));
        let remote = Remote::serve(&bundle.pointer, &bundle.assets()).await;
        let client = client_at(&remote.url);
        let restorer = Restorer::new().with_password("not the passphrase");

        let err = resolve(&restorer.ctx(&client, applying()), None)
            .await
            .err()
            .expect("a wrong passphrase cannot open the keyfile");
        // Verbatim, and deliberately indistinguishable from a tampered keyfile:
        // elaborating it would build the oracle Phase 1 refused to build.
        assert_eq!(err.to_string(), "wrong password or corrupted keyfile");
    }

    #[tokio::test]
    async fn a_root_sealed_for_another_bundle_does_not_open_and_says_no_more() {
        let mut bundle = build_bundle(one_file("config.toml", b"[sync]\n"));
        // The pointer still claims this machine's bundle — so `pointer::load`
        // is satisfied and the refusal has to come from the root's AAD.
        bundle.reseal_root(|root| root.repo_id = "github:2".into());
        let remote = Remote::serve(&bundle.pointer, &bundle.assets()).await;
        let client = client_at(&remote.url);
        let restorer = Restorer::new();

        let err = resolve(&restorer.ctx(&client, applying()), None)
            .await
            .err()
            .expect("a root bound to another bundle must not open");
        let text = err.to_string();
        assert!(
            text.contains("none of the snapshots in this pointer could be opened"),
            "unexpected refusal: {text}"
        );
        assert!(
            !text.contains("github:2"),
            "a wrong repo must not be distinguishable from a wrong key: {text}"
        );
    }

    #[tokio::test]
    async fn the_newest_snapshot_is_chosen_by_sealed_counter_not_by_list_order() {
        for newest_first in [true, false] {
            let mut bundle = build_bundle(one_file("config.toml", b"[sync]\n"));
            let keys = bundle.keys();
            let mut high = bundle.root();
            high.counter = 5;
            let high_record = crate::sync::push::SnapshotRecord {
                root: B64.encode(high.seal(&keys).unwrap()),
                ..bundle.pointer.snapshots[0].clone()
            };
            if newest_first {
                bundle.pointer.snapshots.insert(0, high_record);
            } else {
                bundle.pointer.snapshots.push(high_record);
            }

            let remote = Remote::serve(&bundle.pointer, &bundle.assets()).await;
            let client = client_at(&remote.url);
            let restorer = Restorer::new();
            let resolved = resolve(&restorer.ctx(&client, applying()), None)
                .await
                .unwrap();
            assert_eq!(
                resolved.root.counter, 5,
                "list order decided the snapshot (newest_first={newest_first})"
            );
        }
    }

    /// Two machines racing can publish distinct snapshots under one counter —
    /// the push side computes it before the compare-and-swap. The tie must
    /// break on something sealed, or reordering the plaintext list changes the
    /// answer.
    #[tokio::test]
    async fn two_snapshots_at_one_counter_resolve_the_same_way_in_either_order() {
        let mut chosen = Vec::new();
        for reversed in [false, true] {
            let mut bundle = build_bundle(one_file("config.toml", b"[sync]\n"));
            let keys = bundle.keys();
            let mut twin = bundle.root();
            twin.created_at = NOW + chrono::TimeDelta::seconds(30);
            let twin_record = crate::sync::push::SnapshotRecord {
                root: B64.encode(twin.seal(&keys).unwrap()),
                ..bundle.pointer.snapshots[0].clone()
            };
            if reversed {
                bundle.pointer.snapshots.insert(0, twin_record);
            } else {
                bundle.pointer.snapshots.push(twin_record);
            }

            let remote = Remote::serve(&bundle.pointer, &bundle.assets()).await;
            let client = client_at(&remote.url);
            let restorer = Restorer::new();
            let resolved = resolve(&restorer.ctx(&client, applying()), None)
                .await
                .unwrap();
            assert_eq!(resolved.root.counter, 1);
            chosen.push(resolved.root.created_at);
        }
        assert_eq!(
            chosen[0], chosen[1],
            "a tied counter let the plaintext list's order pick the snapshot"
        );
        assert_eq!(
            chosen[0],
            NOW + chrono::TimeDelta::seconds(30),
            "the tie did not break on the root's own sealed timestamp"
        );
    }

    #[tokio::test]
    async fn a_lower_counter_is_refused_and_names_allow_rollback() {
        let bundle = build_bundle(one_file("config.toml", b"[sync]\n"));
        let remote = Remote::serve(&bundle.pointer, &bundle.assets()).await;
        let client = client_at(&remote.url);
        let restorer = Restorer::new();
        let seen = Anchor {
            repo_id: REPO_ID.into(),
            counter: 5,
        };

        let err = resolve(&restorer.ctx(&client, applying()), Some(&seen))
            .await
            .err()
            .expect("a rolled-back snapshot must be refused");
        assert!(
            err.to_string().contains("--allow-rollback"),
            "the refusal does not name the escape: {err}"
        );

        // And the escape works, against the root's own counter.
        let resolved = resolve(
            &restorer.ctx(
                &client,
                RestoreOptions {
                    allow_rollback: true,
                    ..applying()
                },
            ),
            Some(&seen),
        )
        .await
        .expect("--allow-rollback accepts an older snapshot of the same bundle");
        assert_eq!(resolved.root.counter, 1);
    }

    #[tokio::test]
    async fn a_repo_id_mismatch_is_refused_even_under_allow_rollback() {
        let bundle = build_bundle(one_file("config.toml", b"[sync]\n"));
        let remote = Remote::serve(&bundle.pointer, &bundle.assets()).await;
        let client = client_at(&remote.url);
        let restorer = Restorer::new();
        let borrowed = Anchor {
            repo_id: "github:999".into(),
            counter: 1,
        };

        let err = resolve(
            &restorer.ctx(
                &client,
                RestoreOptions {
                    allow_rollback: true,
                    ..applying()
                },
            ),
            Some(&borrowed),
        )
        .await
        .err()
        .expect("a counter borrowed from another bundle must be refused");
        assert!(
            err.to_string().contains("anchored to bundle"),
            "the refusal came from somewhere other than the anchor: {err}"
        );
    }

    /// `accept` decides; `restore::run` step 7 persists. Advancing here would
    /// let a forged high counter lock a user out of their own bundle for good.
    #[tokio::test]
    async fn resolve_persists_nothing() {
        let bundle = build_bundle(one_file("config.toml", b"[sync]\n"));
        let remote = Remote::serve(&bundle.pointer, &bundle.assets()).await;
        let client = client_at(&remote.url);
        let restorer = Restorer::new();

        resolve(&restorer.ctx(&client, applying()), None)
            .await
            .unwrap();

        assert!(
            !restorer.anchor_path.exists(),
            "resolve advanced the rollback anchor"
        );
        assert!(
            !restorer.roots.config_dir.exists(),
            "resolve wrote into the restoring machine's tree"
        );
    }

    #[tokio::test]
    async fn a_missing_release_is_nothing_pushed_yet_never_a_create() {
        let bundle = build_bundle(one_file("config.toml", b"[sync]\n"));
        let remote = Remote::serve_expecting(&bundle.pointer, &bundle.assets(), 0, 404).await;
        let client = client_at(&remote.url);
        let restorer = Restorer::new();

        let err = resolve(&restorer.ctx(&client, applying()), None)
            .await
            .err()
            .expect("a repository with no release has nothing to restore");
        assert!(
            err.to_string().contains("nothing was restored"),
            "unexpected refusal: {err}"
        );
        remote.listing.assert_async().await;
    }

    #[tokio::test]
    async fn an_oversized_root_string_is_refused_before_it_is_decoded() {
        let mut bundle = build_bundle(one_file("config.toml", b"[sync]\n"));
        bundle.record().root = "A".repeat(MAX_ROOT_B64 + 1);
        let remote = Remote::serve(&bundle.pointer, &bundle.assets()).await;
        let client = client_at(&remote.url);
        let restorer = Restorer::new();

        let err = resolve(&restorer.ctx(&client, applying()), None)
            .await
            .err()
            .expect("a root string larger than a root can be must be refused");
        assert!(
            err.to_string()
                .contains("more base64 than a sealed root can be"),
            "unexpected refusal: {err}"
        );
    }

    // ---------------------------------------------------------------- task 2

    #[tokio::test]
    async fn a_manifest_chunk_list_past_the_ceiling_is_refused_naming_the_number() {
        let mut bundle = build_bundle(one_file("config.toml", b"[sync]\n"));
        let over = MAX_MANIFEST_CHUNKS + 1;
        bundle.reseal_root(|root| {
            root.manifest_chunks = (0..over as u32).map(fake_id).collect();
        });
        let remote = Remote::serve(&bundle.pointer, &bundle.assets()).await;
        let client = client_at(&remote.url);
        let restorer = Restorer::new();

        let err = resolve(&restorer.ctx(&client, applying()), None)
            .await
            .err()
            .expect("an oversized manifest chunk list must be refused");
        let text = err.to_string();
        assert!(
            text.contains(&over.to_string()) && text.contains(&MAX_MANIFEST_CHUNKS.to_string()),
            "the refusal names neither the observed value nor the ceiling: {text}"
        );
    }

    #[tokio::test]
    async fn an_index_chunk_list_past_the_ceiling_is_refused_naming_the_number() {
        let mut bundle = build_bundle(one_file("config.toml", b"[sync]\n"));
        let template = bundle.pointer.snapshots[0].index_chunks[0].clone();
        let over = MAX_INDEX_CHUNKS + 1;
        bundle.record().index_chunks = (0..over as u32)
            .map(|n| RemoteIndexEntry {
                id: fake_id(n),
                ..template.clone()
            })
            .collect();
        let remote = Remote::serve(&bundle.pointer, &bundle.assets()).await;
        let client = client_at(&remote.url);
        let restorer = Restorer::new();

        let err = resolve(&restorer.ctx(&client, applying()), None)
            .await
            .err()
            .expect("an oversized index chunk list must be refused");
        let text = err.to_string();
        assert!(
            text.contains(&over.to_string()) && text.contains(&MAX_INDEX_CHUNKS.to_string()),
            "the refusal names neither the observed value nor the ceiling: {text}"
        );
    }

    #[tokio::test]
    async fn a_pack_list_past_the_count_ceiling_is_refused_naming_the_number() {
        let mut bundle = build_bundle(one_file("config.toml", b"[sync]\n"));
        let over = MAX_PACKS_PER_RESTORE + 1;
        bundle.record().packs = (0..over as u32).map(fake_id).collect();
        let remote = Remote::serve(&bundle.pointer, &bundle.assets()).await;
        let client = client_at(&remote.url);
        let restorer = Restorer::new();

        let err = resolve(&restorer.ctx(&client, applying()), None)
            .await
            .err()
            .expect("an oversized pack list must be refused");
        let text = err.to_string();
        assert!(
            text.contains(&over.to_string()) && text.contains(&MAX_PACKS_PER_RESTORE.to_string()),
            "the refusal names neither the observed value nor the ceiling: {text}"
        );
    }

    /// The count ceiling does not give this one: eight packs is nowhere near
    /// it, and at the 2 GiB the listing declares they are 16 GiB.
    #[tokio::test]
    async fn a_pack_set_past_the_byte_ceiling_is_refused_naming_the_number() {
        const HEAVY: u64 = 2 * 1024 * 1024 * 1024;
        let mut bundle = build_bundle(one_file("config.toml", b"[sync]\n"));
        let heavy: Vec<ChunkId> = (0..8u32).map(fake_id).collect();
        let mut assets = bundle.assets();
        for id in &heavy {
            // Listed at a size they do not have, and with no download mock at
            // all: reaching one would 501 rather than pass quietly.
            assets.push(Asset::listed_only(push::pack_asset_name(id), HEAVY));
        }
        bundle.record().packs = heavy;
        let expected = 8 * HEAVY;

        let remote = Remote::serve(&bundle.pointer, &assets).await;
        let client = client_at(&remote.url);
        let restorer = Restorer::new();

        let err = resolve(&restorer.ctx(&client, applying()), None)
            .await
            .err()
            .expect("a pack set past the byte ceiling must be refused");
        let text = err.to_string();
        assert!(
            text.contains(&expected.to_string()) && text.contains(&MAX_RESTORE_BYTES.to_string()),
            "the refusal names neither the observed value nor the ceiling: {text}"
        );
    }

    #[tokio::test]
    async fn a_pack_served_under_another_packs_name_is_refused_before_its_header_is_read() {
        let mut bundle = build_bundle(one_file("config.toml", b"[sync]\n"));
        for (_, bytes) in &mut bundle.packs {
            bytes[0] ^= 0xff;
        }
        let remote = Remote::serve(&bundle.pointer, &bundle.assets()).await;
        let client = client_at(&remote.url);
        let restorer = Restorer::new();

        let err = resolve(&restorer.ctx(&client, applying()), None)
            .await
            .err()
            .expect("a pack that does not hash to its own name must be refused");
        assert!(
            err.to_string().contains("does not hash to that name"),
            "the pack was opened before its content address was checked: {err}"
        );
    }

    /// `chunk::open_chunk` re-derives the id from the plaintext and binds it as
    /// associated data, so a chunk served under another chunk's id cannot open.
    /// Asserted here rather than inherited on trust from Phase 1.
    #[tokio::test]
    async fn a_chunk_opened_under_another_chunks_id_fails() {
        let bundle = build_bundle(vec![
            ("a.toml".into(), b"[sync]\nenabled = true\n".to_vec()),
            ("b.toml".into(), b"[sync]\nenabled = false\n".to_vec()),
        ]);
        let remote = Remote::serve(&bundle.pointer, &bundle.assets()).await;
        let client = client_at(&remote.url);
        let restorer = Restorer::new();
        let resolved = resolve(&restorer.ctx(&client, applying()), None)
            .await
            .unwrap();

        let ids: Vec<ChunkId> = resolved
            .manifest
            .files
            .iter()
            .flat_map(|f| f.chunks.clone())
            .collect();
        assert_eq!(ids.len(), 2, "two distinct bodies must be two chunks");
        let sealed = resolved.packs.sealed(&ids[0]).unwrap();
        let keys = bundle.keys();
        assert!(
            chunk::open_chunk(&keys, &ids[0], &sealed).is_ok(),
            "the chunk does not open under its own id"
        );
        assert!(
            chunk::open_chunk(&keys, &ids[1], &sealed).is_err(),
            "a chunk opened under another chunk's id succeeded"
        );
    }

    /// A manifest large enough to span chunks, with its last one withheld:
    /// reassembly must fail outright rather than hand back the entries that did
    /// arrive.
    #[tokio::test]
    async fn a_truncated_manifest_yields_no_partial_manifest() {
        let stem = "z".repeat(180);
        let files: Vec<(String, Vec<u8>)> = (0..1000)
            .map(|n| (format!("{stem}-{n:04}.json"), b"{}".to_vec()))
            .collect();
        let mut bundle = build_bundle(files);
        let whole = bundle.root().manifest_chunks.len();
        assert!(
            whole >= 2,
            "this test needs a multi-chunk manifest; it is {whole} chunk(s) — raise the file \
             count or the path length"
        );

        bundle.reseal_root(|root| {
            root.manifest_chunks.pop();
        });
        let remote = Remote::serve(&bundle.pointer, &bundle.assets()).await;
        let client = client_at(&remote.url);
        let restorer = Restorer::new();

        assert!(
            resolve(&restorer.ctx(&client, applying()), None)
                .await
                .is_err(),
            "a truncated manifest produced a Manifest"
        );
    }

    #[tokio::test]
    async fn a_manifest_chunk_the_index_does_not_resolve_is_named() {
        let mut bundle = build_bundle(one_file("config.toml", b"[sync]\n"));
        let orphan = fake_id(7);
        bundle.reseal_root(|root| root.manifest_chunks.push(orphan));
        let remote = Remote::serve(&bundle.pointer, &bundle.assets()).await;
        let client = client_at(&remote.url);
        let restorer = Restorer::new();

        let err = resolve(&restorer.ctx(&client, applying()), None)
            .await
            .err()
            .expect("a chunk the index does not describe must be refused");
        let text = err.to_string();
        assert!(
            text.contains(&orphan.to_string()),
            "the refusal does not name the missing chunk: {text}"
        );
        assert!(
            text.contains("refusing rather than restoring a partial tree"),
            "unexpected refusal: {text}"
        );
    }

    /// Two manifest entries sharing a chunk, and a metadata pack wanted by two
    /// of the three rounds: each asset is fetched once.
    #[tokio::test]
    async fn every_pack_is_downloaded_exactly_once() {
        let shared = b"{\"token\":\"a-fixture-not-a-real-token\"}".to_vec();
        let bundle = build_bundle(vec![
            ("accounts/one/.credentials.json".into(), shared.clone()),
            ("accounts/two/.credentials.json".into(), shared),
        ]);
        let remote = Remote::serve(&bundle.pointer, &bundle.assets()).await;
        let client = client_at(&remote.url);
        let restorer = Restorer::new();

        let resolved = resolve(&restorer.ctx(&client, applying()), None)
            .await
            .unwrap();

        assert_eq!(resolved.manifest.files.len(), 2);
        assert_eq!(
            resolved.manifest.files[0].chunks, resolved.manifest.files[1].chunks,
            "identical bodies must dedup to one chunk"
        );
        remote.assert_each_asset_fetched_once().await;
    }

    #[tokio::test]
    async fn the_same_chunk_twice_gives_identical_plaintext_and_no_second_download() {
        let body = b"{\"token\":\"a-fixture-not-a-real-token\"}";
        let bundle = build_bundle(one_file("accounts/work/.credentials.json", body));
        let remote = Remote::serve(&bundle.pointer, &bundle.assets()).await;
        let client = client_at(&remote.url);
        let restorer = Restorer::new();

        let resolved = resolve(&restorer.ctx(&client, applying()), None)
            .await
            .unwrap();
        let id = resolved.manifest.files[0].chunks[0];
        let first = resolved.packs.chunk(&id).unwrap();
        let second = resolved.packs.chunk(&id).unwrap();

        assert_eq!(&*first, body);
        assert_eq!(first, second);
        remote.assert_each_asset_fetched_once().await;
    }

    /// The pointer's `RemoteIndexEntry` is believed about **one** thing: which
    /// pack asset to fetch. Its `offset`, `clen` and `true_len` are
    /// unauthenticated integers that must never index into anything — the
    /// pack's own sealed header is what slices.
    #[tokio::test]
    async fn the_pointers_unauthenticated_offsets_never_index_into_anything() {
        let mut bundle = build_bundle(one_file("config.toml", b"[sync]\nenabled = true\n"));
        for entry in &mut bundle.record().index_chunks {
            entry.offset = u64::MAX;
            entry.clen = u32::MAX;
            entry.true_len = u32::MAX;
        }
        let remote = Remote::serve(&bundle.pointer, &bundle.assets()).await;
        let client = client_at(&remote.url);
        let restorer = Restorer::new();

        let resolved = resolve(&restorer.ctx(&client, applying()), None)
            .await
            .expect("absurd pointer offsets are inert, not fatal and not a panic");
        assert_eq!(resolved.manifest.files[0].path, "config/config.toml");
    }
}
