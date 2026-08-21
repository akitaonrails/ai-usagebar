//! The inbound path: read a bundle off the remote and put it back on this
//! machine.
//!
//! Restore is where a compromised or merely *stale* bundle turns into local
//! writes over a working machine's config and credentials, so every default
//! here is biased toward refusing and explaining rather than helpfully
//! overwriting.
//!
//! # Every cross-module type is declared **here**
//!
//! `layout`, `fetch`, `merge`, `write`, `backup` and `report` exchange
//! [`RestoreOptions`], [`RestoreCtx`], [`Disposition`], [`ItemPlan`],
//! [`RestorePlan`], [`RestoreOutcome`], [`Applied`], [`BackupRecord`],
//! [`Resolved`] and [`PackSource`] — and not one of them is declared in a
//! sibling. That is the discipline plan 4-01 established and it is what lets
//! five plans fill five sibling modules in parallel worktrees without any of
//! them being able to break another's compile.
//!
//! # Dry-run is the absence of a flag, not a check
//!
//! [`RestoreOptions::apply`] defaults to false and the write half of [`run`]
//! sits behind an early return. There is no "did I remember to check dry-run?"
//! anywhere below that return, because the write path is not reachable from
//! there (D1).
//!
//! Owned by plan 5-01.

pub mod backup;
pub mod fetch;
pub mod layout;
pub mod merge;
pub mod report;
pub mod write;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use zeroize::Zeroizing;

use crate::config::SyncCategory;
use crate::error::{AppError, Result};
use crate::sync::SyncRoots;
use crate::sync::anchor::{self, Anchor};
use crate::sync::crypto::{ChunkId, Keys, content_address};
use crate::sync::github::{Client, RepoRef};
use crate::sync::model::{IndexObject, Manifest, Root};
use crate::sync::pack::{self, PackEntry};

/// Everything the user can ask a restore to do differently.
///
/// **Every field defaults to false, and `apply` is the one that lets a byte
/// reach the disk.** Dry-run is therefore the *absence* of `apply` rather than
/// a flag some code path has to remember to consult — D1 expressed in the type
/// rather than in a convention.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RestoreOptions {
    /// Write. Without it a restore plans, reports, and stops.
    pub apply: bool,
    /// Overwrite items whose local copy is newer.
    pub force: bool,
    /// The second, separate consent for a locally-newer **credential**.
    /// `force` alone never grants it (D2).
    pub force_credentials: bool,
    /// Accept a snapshot older than the one this machine has already seen.
    /// Never waives the `repo_id` check — see [`anchor::accept`].
    pub allow_rollback: bool,
    /// Discard the local change-detection index and reopen it empty.
    pub rebuild_index: bool,
    /// Answer the one interactive gate affirmatively.
    pub assume_yes: bool,
}

/// Everything a restore borrows from its caller.
///
/// Every path is **injected**; nothing under this module resolves `$HOME`, so
/// no test can write outside its own `TempDir`.
///
/// No `Debug`: it holds the passphrase.
pub struct RestoreCtx<'a> {
    pub client: &'a Client,
    pub repo: &'a RepoRef,
    pub roots: &'a SyncRoots,
    /// This machine's own bundle identifier, from the local pairing record and
    /// **never** from a remote response. Bound into the snapshot root's
    /// associated data, so a repo swap fails the Poly1305 tag rather than a
    /// string comparison.
    pub repo_id: &'a str,
    /// Opens the keyfile asset. A second machine has no local keyfile — that is
    /// the whole point of a restore — so the wrapped master key comes off the
    /// remote and this unwraps it.
    pub passphrase: &'a Zeroizing<String>,
    pub anchor_path: &'a Path,
    pub backups_dir: &'a Path,
    pub opts: RestoreOptions,
    pub now: DateTime<Utc>,
}

/// What a restore decided about one manifest entry, and why.
///
/// `SkipLocalNewer` and `Overwrite` are the same *fact* under different
/// options; keeping them distinct is what lets the report name exactly what
/// was lost rather than counting it (SYNC-06).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Disposition {
    /// Nothing at the destination.
    Create,
    /// Something at the destination, and the snapshot wins.
    Update,
    /// Already byte-identical. Decided by digest before any clock is consulted,
    /// which is what makes a second apply a no-op (D7).
    SkipIdentical,
    /// The local copy is newer and `force` was not given (SAFE-03, D2).
    SkipLocalNewer {
        local_mtime: DateTime<Utc>,
        remote_mtime: DateTime<Utc>,
    },
    /// The local copy is newer and is being overwritten anyway. Named in the
    /// summary afterwards.
    Overwrite {
        local_mtime: DateTime<Utc>,
        remote_mtime: DateTime<Utc>,
    },
    /// A locally-newer credential under `force`. Needs the second consent;
    /// `force` alone does not grant it.
    NeedsCredentialConfirm {
        local_mtime: DateTime<Utc>,
        remote_mtime: DateTime<Utc>,
    },
    /// Machine-bound or volatile state the bundle should not have carried, and
    /// which this side refuses regardless (D4).
    ExcludedByPolicy,
    /// The manifest path did not survive [`layout::from_manifest_path`]. It
    /// still appears in the report: dropping it silently would hide tampering.
    RejectedPath(String),
    /// A Claude Desktop token cache sealed by **another Mac's** login Keychain:
    /// it carries Chromium's `v10` safeStorage marker and this machine's
    /// `Claude Safe Storage` key does not open it, so Claude Desktop here could
    /// never read it either.
    ///
    /// Writing it would trade a working Desktop login for bytes nothing on this
    /// machine can decrypt — the user's own backup destroying the one thing the
    /// backup exists to protect. Refusing costs them a sign-in; writing costs
    /// them the session. macOS only: [`crate::safe_storage`]'s key store is the
    /// macOS login Keychain and no other platform has the question to ask.
    ForeignSafeStorage,
    /// A [`crate::sync::keystore`] store this machine already holds a
    /// **different** live credential in.
    ///
    /// Silently replacing the Claude login this tool exists to report on is the
    /// worst outcome this whole feature has, so it is never the default. The one
    /// consent that promotes it is `--force-credentials`; `--force` alone does
    /// not, exactly as for a locally-newer `.credentials.json`.
    ///
    /// It carries no timestamps and cannot: a Keychain item has no mtime this
    /// side can compare, so "is the local one newer?" has no answer here and is
    /// not pretended to. The question asked instead is the one that can be
    /// answered — *is it the same credential?* — by hashing what the store holds
    /// with the same [`Keys::chunk_id`] the push side used. Identical is
    /// [`Disposition::SkipIdentical`] and needs no consent at all, so a repeated
    /// pull onto the machine that pushed is silent.
    ReplacesLiveCredential,
}

impl Disposition {
    /// Would this item put bytes on the disk?
    pub fn writes(&self) -> bool {
        matches!(
            self,
            Disposition::Create | Disposition::Update | Disposition::Overwrite { .. }
        )
    }
}

/// One manifest entry, resolved and decided.
///
/// `dest` is `Option` because a rejected or excluded entry deliberately has
/// none — and still appears in the plan, which is how a tampered bundle becomes
/// visible instead of invisible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemPlan {
    pub manifest_path: String,
    pub dest: Option<PathBuf>,
    pub category: SyncCategory,
    pub true_len: u64,
    pub chunks: Vec<ChunkId>,
    pub disposition: Disposition,
}

/// What a restore *would* do. Produced whether or not `apply` was given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestorePlan {
    pub items: Vec<ItemPlan>,
    /// The snapshot's sealed counter — from the root, never from the pointer.
    pub counter: u64,
    /// The snapshot's own timestamp, and the remote mtime for every item in it.
    pub created_at: DateTime<Utc>,
    pub repo_id: String,
    pub packs_needed: usize,
    pub bytes_to_fetch: u64,
}

/// What the write half did. Returned by [`write::apply`]; plan 5-04 owns it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    pub written: usize,
    pub overwritten: Vec<String>,
    pub skipped: usize,
    /// The manifest path a partial restore stopped at.
    pub failed_at: Option<String>,
}

/// The archive taken before the first write. Plan 5-05 owns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupRecord {
    pub archive: PathBuf,
    /// What the archive's member paths are relative to.
    pub root: PathBuf,
    pub members: usize,
    pub bytes: u64,
}

impl BackupRecord {
    /// The copy-pasteable undo, printed on both the success and the
    /// partial-failure path.
    pub fn rollback_command(&self) -> String {
        backup::rollback_command(self)
    }
}

/// A plan plus what happened to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreOutcome {
    pub plan: RestorePlan,
    pub applied: bool,
    pub backup: Option<BackupRecord>,
    pub written: usize,
    pub overwritten: Vec<String>,
    pub skipped: usize,
    pub failed_at: Option<String>,
}

/// Everything [`fetch::resolve`] authenticated, ready for the rest of a
/// restore. Plan 5-02 fills the module that builds it.
pub struct Resolved {
    pub root: Root,
    pub manifest: Manifest,
    pub index: IndexObject,
    pub packs: PackSource,
}

/// The downloaded packs of one snapshot, and the only way to get a chunk's
/// bytes out of them.
///
/// # Why the accessors are synchronous
///
/// [`fetch::resolve`] downloads every pack a run needs before returning — the
/// bundle's own metadata packs always, and the data packs only when
/// `opts.apply` is set, so a dry run never pulls a byte of file content. With
/// the download already done, reading a chunk is a slice and an AEAD open, and
/// needs neither `async` nor `&mut self`. That is what lets
/// [`write::apply`] take a `&PackSource`.
///
/// # What is trusted here, and what is not
///
/// The pointer's `RemoteIndexEntry` says which *pack* holds a chunk, and that
/// is all it is believed about: the offset and length used to slice come from
/// each pack's own **sealed** header, read once in [`PackSource::new`]. The
/// pointer's unauthenticated `offset`/`clen` never index into anything.
pub struct PackSource {
    keys: Keys,
    packs: HashMap<ChunkId, Vec<u8>>,
    /// chunk id → (the pack that holds it, where inside that pack).
    located: HashMap<ChunkId, (ChunkId, PackEntry)>,
}

impl PackSource {
    /// The master key, and no packs yet. [`fetch::resolve`] fills it in rounds:
    /// the packs holding the index object, then the ones holding the manifest,
    /// then — only under `apply` — the ones holding file data.
    pub fn empty(keys: Keys) -> Self {
        Self {
            keys,
            packs: HashMap::new(),
            located: HashMap::new(),
        }
    }

    /// Take one downloaded pack and read its **sealed** header.
    ///
    /// A pack whose bytes do not hash to the id it was fetched under is refused
    /// here, before its header is opened: the asset name is a content address,
    /// so a substituted pack cannot keep the name it was served under.
    /// [`pack::read_header`] then bounds every offset and length in the header
    /// against the pack's real length, so nothing unauthenticated ever reaches
    /// a slice.
    pub fn add(&mut self, id: ChunkId, bytes: Vec<u8>) -> Result<()> {
        if self.packs.contains_key(&id) {
            return Ok(());
        }
        if content_address(&bytes) != id {
            return Err(AppError::Other(format!(
                "the pack served as {id} does not hash to that name — refusing it rather \
                 than opening bytes the remote substituted"
            )));
        }
        for entry in pack::read_header(&self.keys, &bytes)?.entries {
            self.located.insert(entry.id, (id, entry));
        }
        self.packs.insert(id, bytes);
        Ok(())
    }

    /// Already downloaded?
    pub fn holds_pack(&self, id: &ChunkId) -> bool {
        self.packs.contains_key(id)
    }

    /// The bundle's master key, for the object readers that take one.
    pub fn keys(&self) -> &Keys {
        &self.keys
    }

    /// How many packs a restore actually downloaded.
    pub fn packs(&self) -> usize {
        self.packs.len()
    }

    /// How many bytes those packs cost.
    pub fn bytes(&self) -> u64 {
        self.packs.values().map(|p| p.len() as u64).sum()
    }

    /// The **sealed** bytes of one chunk, for the object readers
    /// ([`Manifest::open`], [`IndexObject::open`]) that take ciphertext.
    pub fn sealed(&self, id: &ChunkId) -> Result<Vec<u8>> {
        let (pack_id, entry) = self.locate(id)?;
        let pack = self.packs.get(pack_id).ok_or_else(|| {
            AppError::Other(format!("chunk {id} names a pack that was not fetched"))
        })?;
        Ok(pack::blob_bytes(pack, entry)?.to_vec())
    }

    /// The **plaintext** of one chunk.
    pub fn chunk(&self, id: &ChunkId) -> Result<Zeroizing<Vec<u8>>> {
        let (pack_id, entry) = self.locate(id)?;
        let pack = self.packs.get(pack_id).ok_or_else(|| {
            AppError::Other(format!("chunk {id} names a pack that was not fetched"))
        })?;
        pack::open_blob(&self.keys, pack, entry)
    }

    fn locate(&self, id: &ChunkId) -> Result<&(ChunkId, PackEntry)> {
        self.located.get(id).ok_or_else(|| {
            AppError::Other(format!(
                "this snapshot names chunk {id}, which none of its packs carries — refusing \
                 rather than restoring a short file"
            ))
        })
    }
}

/// Restore one snapshot onto this machine.
///
/// The order below is a security property, not a style. Read it as seven
/// numbered steps, because two of them are only correct in this order:
///
/// 1. Read the local rollback anchor. A parse failure is an **error**, never
///    `None` — that is [`anchor::read_from`]'s rule and softening it here would
///    turn a damaged anchor into a free rollback. The path comes from
///    `ctx.anchor_path` and is never derived from the remote's claimed
///    `repo_id`.
/// 2. [`fetch::resolve`] — pointer, keyfile, root, manifest, index, packs. It
///    is handed the anchor and calls [`anchor::accept`] itself, because accept
///    must run against the **root's own sealed** `counter` and `repo_id`, not
///    against the plaintext pointer's copy of them.
/// 3. [`merge::plan`] — every manifest entry becomes exactly one [`ItemPlan`].
/// 4. If `!opts.apply`, return here. **The write path is not reachable past
///    this point** (D1).
/// 5. [`backup::take`] — before the first byte, even under `force`, even for a
///    partial restore.
/// 6. [`write::apply`].
/// 7. Advance the anchor — **only now**, and only if 2 through 6 all returned
///    `Ok` and nothing failed part-way. Advancing before verification lets a
///    forged high counter lock the user out of their own bundle permanently: a
///    denial of service anyone with repo write access could trigger at will,
///    and the risk plan 1-05 recorded against this phase. The value written is
///    the **root's** sealed counter, never the pointer's.
///
/// `progress` is the **same reporter `sync push` uses** — see
/// [`crate::sync::push::progress`]. A restore of 2.1 GiB spends ~1.5 s deriving
/// the key, minutes downloading packs and a while writing them, and until 6-12
/// every second of that was a terminal with nothing on it. `finish` is called on
/// both arms below, so a run that dies leaves the cursor on a fresh line rather
/// than in the middle of a bar the error message then lands on top of.
pub async fn run(
    ctx: RestoreCtx<'_>,
    progress: &mut dyn crate::sync::push::progress::Progress,
) -> Result<RestoreOutcome> {
    let outcome = restore(ctx, progress).await;
    progress.finish();
    outcome
}

/// [`run`]'s seven steps. Split out only so `finish` above covers every `?`.
async fn restore(
    ctx: RestoreCtx<'_>,
    progress: &mut dyn crate::sync::push::progress::Progress,
) -> Result<RestoreOutcome> {
    // 1.
    let local_anchor = anchor::read_from(ctx.anchor_path)?;

    // 2.
    let resolved = fetch::resolve(&ctx, local_anchor.as_ref(), progress).await?;

    // 3.
    let plan = merge::plan(&ctx, &resolved)?;

    // 4. Dry run: everything above was reads.
    if !ctx.opts.apply {
        return Ok(RestoreOutcome {
            plan,
            applied: false,
            backup: None,
            written: 0,
            overwritten: Vec::new(),
            skipped: 0,
            failed_at: None,
        });
    }

    // 5. Only what the restore would really touch, so the archive is exactly
    //    the reversal set.
    let targets: Vec<PathBuf> = plan
        .items
        .iter()
        .filter(|item| item.disposition.writes())
        .filter_map(|item| item.dest.clone())
        .collect();
    let backup = backup::take(&ctx, &targets)?;

    // 6.
    let applied = write::apply(&ctx, &plan, &resolved.packs, progress)?;

    // 7. A partial restore does not advance the anchor either: the machine has
    //    not seen this snapshot whole.
    if applied.failed_at.is_none() {
        anchor::write_to(
            ctx.anchor_path,
            &Anchor {
                repo_id: resolved.root.repo_id.clone(),
                counter: resolved.root.counter,
            },
        )?;
    }

    Ok(RestoreOutcome {
        plan,
        applied: true,
        backup,
        written: applied.written,
        overwritten: applied.overwritten,
        skipped: applied.skipped,
        failed_at: applied.failed_at,
    })
}

#[cfg(test)]
mod ordering_guard {
    /// SAFE-04 is an *order*, and until now only one integration test held it.
    ///
    /// Phase 5's verification swapped [`backup::take`] and [`write::apply`] in
    /// [`run`] and watched the entire 1520-test lib suite stay green — a
    /// safety property whose whole net was a single test in another file. This
    /// reads `run`'s own shipped source, so the swap fails here too, and it
    /// fails without constructing a remote, a keyfile or a passphrase.
    ///
    /// It asserts an order, not a presence: two calls in the wrong sequence
    /// still satisfies "both are called".
    ///
    /// It reads `restore`, not `run`: 6-12 made `run` a two-line wrapper that
    /// calls `Progress::finish` on both arms, and the seven steps moved into
    /// `restore` beneath it. The marker moved with the body it is about.
    #[test]
    fn the_archive_is_taken_before_the_first_byte_is_written() {
        let source = include_str!("mod.rs");
        let body = source
            .split_once("async fn restore(")
            .expect("`restore` holds the seven steps this guard is about")
            .1;
        let take = body
            .find("backup::take(")
            .expect("`run` archives before it writes");
        let apply = body.find("write::apply(").expect("`run` writes");
        assert!(
            take < apply,
            "backup::take must precede write::apply in `run`: an archive taken \
             afterwards restores what the run had already overwritten"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SyncConfig;
    use crate::sync::crypto::{KdfParams, Keyfile};
    use crate::sync::github::Endpoints;
    use crate::sync::github::token::TokenSource;
    use crate::sync::index::Index;
    use crate::sync::plan::{FilePlan, SyncPlan};
    use crate::sync::push::progress;
    use crate::sync::push::{
        self, Pointer, PushBundle, PushCtx, RELEASE_TAG, SnapshotRecord, keyfile_asset_name,
        pack_asset_name,
    };
    use crate::sync::{CHUNK_SIZE, chunk};
    use base64::Engine;
    use std::fs;
    use tempfile::TempDir;

    const B64: base64::engine::general_purpose::GeneralPurpose =
        base64::engine::general_purpose::STANDARD;

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

    /// A bundle produced by the **push** side, so the pair is exercised rather
    /// than a hand-rolled remote that could agree with a broken reader.
    struct Bundle {
        pointer: Pointer,
        keyfile_bytes: Vec<u8>,
        keyfile_name: String,
        packs: Vec<(String, Vec<u8>)>,
    }

    fn push_one_file(pusher: &SyncRoots, rel: &str, body: &[u8]) -> Bundle {
        push_bundle(pusher, |keys| {
            let path = pusher.config_dir.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, body).unwrap();
            file_plan_for(keys, path, body)
        })
    }

    /// The same push, but of Claude Code's login in the **machine-bound store**
    /// rather than of a file — which on macOS is where the credential actually
    /// is. See [`crate::sync::keystore`].
    fn push_login(pusher: &SyncRoots, value: &str) -> Bundle {
        push_bundle(pusher, |keys| {
            pusher
                .stores
                .edit()
                .set(crate::sync::keystore::Store::ClaudeCodeOauth, value);
            file_plan_for(
                keys,
                PathBuf::from(crate::sync::keystore::Store::ClaudeCodeOauth.manifest_path()),
                value.as_bytes(),
            )
        })
    }

    fn file_plan_for(keys: &Keys, path: PathBuf, body: &[u8]) -> FilePlan {
        let chunk_ids: Vec<[u8; 32]> = chunk::split(body)
            .map(|block| *keys.chunk_id(block).as_bytes())
            .collect();
        FilePlan {
            path,
            sealed_chunks: chunk::sealed_chunk_count(body.len() as u64),
            new_chunk_ids: chunk_ids.clone(),
            chunk_ids,
            new_bytes: body.len() as u64,
            new_stored_bytes: body.len() as u64,
            reused: false,
        }
    }

    /// One real push of one planned item, through the push side's own packer —
    /// so the pair is exercised rather than a hand-rolled remote that could
    /// agree with a broken reader.
    fn push_bundle(pusher: &SyncRoots, plan_one: impl FnOnce(&Keys) -> FilePlan) -> Bundle {
        let (keyfile, keys) =
            Keyfile::create_with_floor(PASSWORD.as_bytes(), CHEAP, CHEAP.m_kib).unwrap();
        let keyfile_bytes = serde_json::to_vec(&keyfile).unwrap();
        let keyfile_name = keyfile_asset_name(&content_address(&keyfile_bytes));

        let file_plan = plan_one(&keys);
        let body_len = file_plan.new_bytes;
        let plan = SyncPlan {
            categories: Vec::new(),
            new_chunk_ids: file_plan.new_chunk_ids.clone(),
            total_raw_bytes: body_len,
            total_new_bytes: body_len,
            total_new_stored_bytes: body_len,
            files_opened: 1,
            append_check_miss_bytes: 0,
            index_rebuilt: false,
            file_plans: vec![file_plan],
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
            roots: pusher,
            keys: &keys,
            kdf: CHEAP,
            index: &index,
            repo_id: REPO_ID.into(),
            keyfile_asset: keyfile_name.clone(),
            previous: None,
            now: NOW,
            allow_rollback: false,
        };
        let bundle: PushBundle = push::packer::build(&ctx, &plan).unwrap();
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
                snapshots: vec![SnapshotRecord {
                    root: B64.encode(&root),
                    index_chunks: bundle.index_chunks.clone(),
                    packs: bundle.referenced_packs.clone(),
                }],
            },
            keyfile_bytes,
            keyfile_name,
            packs: bundle
                .packs
                .iter()
                .map(|p| (pack_asset_name(&p.id), p.bytes.clone()))
                .collect(),
        }
    }

    /// Serve `bundle` from `server` exactly as GitHub would.
    async fn serve(server: &mut mockito::ServerGuard, bundle: &Bundle) {
        let pointer_json = serde_json::to_vec(&bundle.pointer).unwrap();
        server
            .mock("GET", "/repos/o/n/contents/sync/pointer.json")
            .with_status(200)
            .with_body(format!(
                r#"{{"sha":"deadbeef","content":"{}"}}"#,
                B64.encode(&pointer_json)
            ))
            .create_async()
            .await;
        server
            .mock(
                "GET",
                format!("/repos/o/n/releases/tags/{RELEASE_TAG}").as_str(),
            )
            .with_status(200)
            .with_body(format!(r#"{{"id":{RELEASE}}}"#))
            .create_async()
            .await;

        let mut listing = Vec::new();
        let mut assets: Vec<(&str, &[u8])> =
            vec![(bundle.keyfile_name.as_str(), &bundle.keyfile_bytes)];
        for (name, bytes) in &bundle.packs {
            assets.push((name.as_str(), bytes));
        }
        for (i, (name, bytes)) in assets.iter().enumerate() {
            let id = 100 + i as u64;
            listing.push(format!(
                r#"{{"id":{id},"name":"{name}","size":{},"state":"uploaded",
                    "created_at":"2023-11-14T22:13:20Z"}}"#,
                bytes.len()
            ));
            server
                .mock("GET", format!("/repos/o/n/releases/assets/{id}").as_str())
                .with_status(200)
                .with_body(*bytes)
                .create_async()
                .await;
        }
        server
            .mock(
                "GET",
                mockito::Matcher::Regex(format!("/releases/{RELEASE}/assets")),
            )
            .with_status(200)
            .with_body(format!("[{}]", listing.join(",")))
            .create_async()
            .await;
    }

    struct Restorer {
        _dir: TempDir,
        roots: SyncRoots,
        anchor_path: PathBuf,
        backups_dir: PathBuf,
        repo: RepoRef,
        password: Zeroizing<String>,
    }

    impl Restorer {
        fn new(dir: TempDir) -> Self {
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

    /// Every regular file anywhere under the restorer's roots.
    fn files_under(root: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    out.push(path);
                }
            }
        }
        out.sort();
        out
    }

    /// Every call the restore made, in order — no terminal, no writer, just the
    /// sequence and the figures.
    #[derive(Debug, Default)]
    struct Recording {
        calls: Vec<String>,
    }

    impl progress::Progress for Recording {
        fn phase(&mut self, label: &str, files: usize, bytes: u64) {
            self.calls.push(format!("phase {label} {files} {bytes}"));
        }
        fn start(&mut self, assets: usize, total_bytes: u64) {
            self.calls.push(format!("start {assets} {total_bytes}"));
        }
        fn stage(&mut self, stage: progress::Stage, items: usize, total_bytes: u64) {
            self.calls
                .push(format!("stage {} {items} {total_bytes}", stage.verb));
        }
        fn asset_done(&mut self, index: usize, _name: &str, bytes: u64) {
            self.calls.push(format!("done {index} {bytes}"));
        }
        fn finish(&mut self) {
            self.calls.push("finish".into());
        }
    }

    /// The reported defect: 2.1 GiB restored onto a second Mac under a silent
    /// terminal — "ETA, progress, status, nothing".
    ///
    /// Asserted as a **sequence**, because the order is the fix: the key
    /// derivation is announced before it starts (it is the first slow thing and
    /// it used to be the first silent one), every downloaded pack reports, and
    /// the write stage is entered with the count the run will really write.
    ///
    /// The download's byte total is the **release listing's** figure, and the
    /// per-pack figure is what actually arrived — deliberately not
    /// `plan.bytes_to_fetch`, which `merge::to_fetch` computes *after* the
    /// download from the index's `clen`s and which therefore both post-dates the
    /// bar and counts a different quantity (chunk payload, not pack asset).
    #[tokio::test]
    async fn every_slow_stretch_of_a_restore_reports_and_the_key_derivation_reports_first() {
        let push_dir = TempDir::new().unwrap();
        let bundle = push_one_file(
            &roots_at(push_dir.path(), "alice"),
            "accounts/work/settings.json",
            b"{\"a\":1}",
        );
        let mut server = mockito::Server::new_async().await;
        serve(&mut server, &bundle).await;

        let client = client_at(&server.url());
        let restorer = Restorer::new(TempDir::new().unwrap());
        let mut seen = Recording::default();
        let outcome = run(
            restorer.ctx(
                &client,
                RestoreOptions {
                    apply: true,
                    ..Default::default()
                },
            ),
            &mut seen,
        )
        .await
        .expect("the chain resolves");
        assert_eq!(outcome.written, 1);

        let calls = &seen.calls;
        assert_eq!(
            calls.first().map(String::as_str),
            Some("phase deriving the sync key (Argon2id) 0 0"),
            "the ~1.5 s Argon2id is the first slow thing and must be the first \
             thing said: {calls:?}"
        );
        let downloads: Vec<&String> = calls
            .iter()
            .filter(|c| c.starts_with("stage downloading"))
            .collect();
        assert!(
            !downloads.is_empty(),
            "the packs are the long stretch: {calls:?}"
        );
        for stage in &downloads {
            let bytes: u64 = stage.rsplit(' ').next().unwrap().parse().unwrap();
            assert!(bytes > 0, "a download stage knows its byte total: {stage}");
        }

        let write = calls
            .iter()
            .position(|c| c.starts_with("stage writing"))
            .expect("the write stage reports too");
        assert_eq!(calls[write], "stage writing 1 7", "{calls:?}");
        assert!(
            calls[..write]
                .iter()
                .any(|c| c.starts_with("stage downloading")),
            "packs are downloaded before they are written: {calls:?}"
        );
        assert_eq!(
            calls.last().map(String::as_str),
            Some("finish"),
            "the terminal is handed back on a fresh line: {calls:?}"
        );
        // Every stage's per-item calls are one apiece and never exceed the
        // total the stage announced — a bar that overshoots is a bar lying.
        let announced: usize = calls[write]
            .split(' ')
            .nth(2)
            .and_then(|n| n.parse().ok())
            .unwrap();
        assert_eq!(
            calls[write + 1..]
                .iter()
                .filter(|c| c.starts_with("done"))
                .count(),
            announced
        );
    }

    /// A dry run is the *planning* pass `sync pull` always makes first, and it
    /// deliberately fetches no file content (5-02) — so it must not announce a
    /// write stage it will never enter, and must not draw a bar for the packs it
    /// is not going to pull.
    #[tokio::test]
    async fn a_dry_run_narrates_only_what_it_actually_does() {
        let push_dir = TempDir::new().unwrap();
        let bundle = push_one_file(
            &roots_at(push_dir.path(), "alice"),
            "accounts/work/settings.json",
            b"{\"a\":1}",
        );
        let mut server = mockito::Server::new_async().await;
        serve(&mut server, &bundle).await;

        let client = client_at(&server.url());
        let restorer = Restorer::new(TempDir::new().unwrap());
        let mut seen = Recording::default();
        run(restorer.ctx(&client, RestoreOptions::default()), &mut seen)
            .await
            .expect("the chain resolves");

        assert!(
            !seen.calls.iter().any(|c| c.starts_with("stage writing")),
            "a dry run writes nothing and says so by saying nothing: {:?}",
            seen.calls
        );
        assert_eq!(seen.calls.last().map(String::as_str), Some("finish"));
    }

    /// The tracer, first half: the whole chain walked end to end, and **nothing
    /// written** (D1, UX-01).
    #[tokio::test]
    async fn a_default_run_plans_one_file_and_writes_nothing() {
        let push_dir = TempDir::new().unwrap();
        let bundle = push_one_file(
            &roots_at(push_dir.path(), "alice"),
            "accounts/work/.credentials.json",
            br#"{"token":"a-fixture-not-a-real-token"}"#,
        );
        let mut server = mockito::Server::new_async().await;
        serve(&mut server, &bundle).await;

        let client = client_at(&server.url());
        let restorer = Restorer::new(TempDir::new().unwrap());
        let outcome = run(
            restorer.ctx(&client, RestoreOptions::default()),
            &mut progress::Silent,
        )
        .await
        .expect("the chain resolves");

        assert!(!outcome.applied);
        assert_eq!(outcome.written, 0);
        assert_eq!(outcome.plan.items.len(), 1);
        assert_eq!(
            outcome.plan.items[0].manifest_path,
            "config/accounts/work/.credentials.json"
        );
        assert_eq!(outcome.plan.items[0].disposition, Disposition::Create);
        assert_eq!(outcome.plan.counter, 1);
        assert!(
            files_under(restorer.roots.config_dir.parent().unwrap()).is_empty(),
            "a dry run wrote something"
        );
        assert!(
            !restorer.anchor_path.exists(),
            "a dry run advanced the anchor"
        );
    }

    /// **Two Macs, and the credential that was missing from the bundle entirely.**
    ///
    /// Claude Code on macOS keeps its OAuth credential in the login Keychain,
    /// not in `~/.claude/.credentials.json`, so the collectors found no file and
    /// the `credentials` category restored to nothing usable. Here the whole
    /// chain runs — alice's store is read into a real push, served, and restored
    /// onto bob's — and the login arrives byte for byte.
    ///
    /// Both machines' stores are injected fixtures; no real login Keychain is
    /// within reach of this test. See [`crate::sync::keystore`].
    #[tokio::test]
    async fn a_keychain_login_pushed_on_one_mac_arrives_byte_for_byte_on_the_other() {
        use crate::sync::keystore::Store;

        let push_dir = TempDir::new().unwrap();
        let login = r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-fixture","refreshToken":"sk-ant-ort01-fixture"}}"#;
        let bundle = push_login(&roots_at(push_dir.path(), "alice"), login);

        let mut server = mockito::Server::new_async().await;
        serve(&mut server, &bundle).await;

        let client = client_at(&server.url());
        let restorer = Restorer::new(TempDir::new().unwrap());
        assert!(
            restorer
                .roots
                .stores
                .read(&Store::ClaudeCodeOauth)
                .unwrap()
                .is_none(),
            "the second Mac starts with no Claude login, which is the premise"
        );

        let outcome = run(
            restorer.ctx(
                &client,
                RestoreOptions {
                    apply: true,
                    ..Default::default()
                },
            ),
            &mut progress::Silent,
        )
        .await
        .expect("the chain resolves");

        assert_eq!(outcome.written, 1);
        assert!(outcome.failed_at.is_none());
        assert_eq!(
            outcome.plan.items[0].manifest_path,
            "keystore/claude-code-oauth"
        );
        assert_eq!(
            restorer
                .roots
                .stores
                .read(&Store::ClaudeCodeOauth)
                .unwrap()
                .map(|v| v.to_string())
                .as_deref(),
            Some(login),
            "the login did not survive the round trip"
        );

        // **And not a byte of it landed on the disk.** A synthetic manifest
        // entry that resolved to a file would be a live OAuth token written in
        // plaintext under the user's home directory.
        for path in files_under(restorer.roots.config_dir.parent().unwrap()) {
            let body = fs::read(&path).unwrap_or_default();
            assert!(
                !String::from_utf8_lossy(&body).contains("sk-ant-oat01-fixture"),
                "the credential was written to {}",
                path.display()
            );
        }
    }

    /// The tracer, second half: the same bundle, applied.
    #[tokio::test]
    async fn an_applied_run_writes_the_pushed_bytes_at_mode_0600() {
        let push_dir = TempDir::new().unwrap();
        let body = br#"{"token":"a-fixture-not-a-real-token"}"#;
        let bundle = push_one_file(
            &roots_at(push_dir.path(), "alice"),
            "accounts/work/.credentials.json",
            body,
        );
        let mut server = mockito::Server::new_async().await;
        serve(&mut server, &bundle).await;

        let client = client_at(&server.url());
        let restorer = Restorer::new(TempDir::new().unwrap());
        let outcome = run(
            restorer.ctx(
                &client,
                RestoreOptions {
                    apply: true,
                    ..Default::default()
                },
            ),
            &mut progress::Silent,
        )
        .await
        .expect("the chain resolves and writes");

        assert!(outcome.applied);
        assert_eq!(outcome.written, 1);

        let dest = restorer
            .roots
            .config_dir
            .join("accounts/work/.credentials.json");
        assert_eq!(fs::read(&dest).unwrap(), body);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&dest).unwrap().permissions().mode() & 0o7777;
            assert_eq!(mode, 0o600, "a restored credential must not be readable");
        }

        // No plaintext left behind beside it.
        assert_eq!(
            files_under(&restorer.roots.config_dir),
            vec![dest],
            "the write left something other than the destination behind"
        );
    }

    /// A bundle pushed under one username restores under another. The whole
    /// milestone exists for this line.
    #[tokio::test]
    async fn a_bundle_pushed_by_alice_restores_under_bobs_roots() {
        let push_dir = TempDir::new().unwrap();
        let bundle = push_one_file(
            &roots_at(push_dir.path(), "alice"),
            "config.toml",
            b"[sync]\nenabled = true\n",
        );
        let mut server = mockito::Server::new_async().await;
        serve(&mut server, &bundle).await;

        let client = client_at(&server.url());
        let restorer = Restorer::new(TempDir::new().unwrap());
        let outcome = run(
            restorer.ctx(
                &client,
                RestoreOptions {
                    apply: true,
                    ..Default::default()
                },
            ),
            &mut progress::Silent,
        )
        .await
        .unwrap();

        assert_eq!(outcome.written, 1);
        assert!(
            restorer.roots.config_dir.join("config.toml").exists(),
            "the file did not land under the restoring machine's roots"
        );
        assert!(
            !push_dir
                .path()
                .join("alice/.config")
                .join("marker")
                .exists(),
            "nothing may be written back to the pushing machine's tree"
        );
    }

    /// Step 7's whole reason for existing: a fetch that fails must leave the
    /// anchor byte-identical, or a forged high counter is a permanent lockout.
    #[tokio::test]
    async fn a_failed_fetch_leaves_the_anchor_untouched() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/repos/o/n/contents/sync/pointer.json")
            .with_status(200)
            .with_body(r#"{"sha":"x","content":"bm90IGpzb24="}"#)
            .create_async()
            .await;

        let restorer = Restorer::new(TempDir::new().unwrap());
        fs::create_dir_all(restorer.anchor_path.parent().unwrap()).unwrap();
        let before = Anchor {
            repo_id: REPO_ID.into(),
            counter: 7,
        };
        anchor::write_to(&restorer.anchor_path, &before).unwrap();
        let bytes_before = fs::read(&restorer.anchor_path).unwrap();

        let client = client_at(&server.url());
        let err = run(
            restorer.ctx(
                &client,
                RestoreOptions {
                    apply: true,
                    ..Default::default()
                },
            ),
            &mut progress::Silent,
        )
        .await
        .expect_err("a malformed pointer must refuse");
        assert!(!err.to_string().is_empty());
        assert_eq!(
            fs::read(&restorer.anchor_path).unwrap(),
            bytes_before,
            "a failed restore rewrote the rollback anchor"
        );
    }

    /// The anchor advances from the **root's** sealed counter, and only after
    /// the snapshot verified.
    #[tokio::test]
    async fn a_successful_apply_advances_the_anchor_from_the_sealed_counter() {
        let push_dir = TempDir::new().unwrap();
        let bundle = push_one_file(
            &roots_at(push_dir.path(), "alice"),
            "config.toml",
            b"[sync]\n",
        );
        let mut server = mockito::Server::new_async().await;
        serve(&mut server, &bundle).await;

        let client = client_at(&server.url());
        let restorer = Restorer::new(TempDir::new().unwrap());
        run(
            restorer.ctx(
                &client,
                RestoreOptions {
                    apply: true,
                    ..Default::default()
                },
            ),
            &mut progress::Silent,
        )
        .await
        .unwrap();

        let stored = anchor::read_from(&restorer.anchor_path).unwrap().unwrap();
        assert_eq!(stored.repo_id, REPO_ID);
        assert_eq!(stored.counter, 1);
    }

    /// `allow_rollback` is for an older snapshot of the *same* bundle, never
    /// for a counter borrowed from a different one by renaming. The orchestrator
    /// calls `anchor::accept` rather than reimplementing the comparison, so this
    /// still refuses.
    #[tokio::test]
    async fn a_repo_id_mismatch_still_refuses_under_allow_rollback() {
        let push_dir = TempDir::new().unwrap();
        let bundle = push_one_file(
            &roots_at(push_dir.path(), "alice"),
            "config.toml",
            b"[sync]\n",
        );
        let mut server = mockito::Server::new_async().await;
        serve(&mut server, &bundle).await;

        let restorer = Restorer::new(TempDir::new().unwrap());
        fs::create_dir_all(restorer.anchor_path.parent().unwrap()).unwrap();
        anchor::write_to(
            &restorer.anchor_path,
            &Anchor {
                repo_id: "github:999".into(),
                counter: 1,
            },
        )
        .unwrap();

        let client = client_at(&server.url());
        let err = run(
            restorer.ctx(
                &client,
                RestoreOptions {
                    apply: true,
                    allow_rollback: true,
                    ..Default::default()
                },
            ),
            &mut progress::Silent,
        )
        .await
        .expect_err("a borrowed counter must be refused");
        assert!(
            err.to_string().contains("anchored to bundle"),
            "the refusal came from somewhere other than the anchor: {err}"
        );
    }

    /// A hostile manifest entry is reported, not silently dropped, and produces
    /// no destination and no write (D5, T-5-08).
    #[test]
    fn a_rejected_path_is_visible_in_the_plan_and_has_no_destination() {
        let item = ItemPlan {
            manifest_path: "config/../../../../etc/shadow".into(),
            dest: None,
            category: SyncCategory::Config,
            true_len: 0,
            chunks: Vec::new(),
            disposition: Disposition::RejectedPath("it contains a `..` component".into()),
        };
        assert!(!item.disposition.writes());
        assert!(item.dest.is_none());
    }

    #[test]
    fn dry_run_is_the_default_and_no_option_is_on_by_accident() {
        let opts = RestoreOptions::default();
        assert!(!opts.apply);
        assert_eq!(opts, RestoreOptions::default());
        assert!(
            !(opts.force || opts.force_credentials || opts.allow_rollback || opts.assume_yes),
            "a consent option defaults to granted"
        );
    }

    #[test]
    fn the_rollback_command_is_copy_pasteable() {
        let record = BackupRecord {
            archive: PathBuf::from("/home/bob/.claude-acc/backups/sync-restore-20231114.tar.gz"),
            root: PathBuf::from("/home/bob"),
            members: 3,
            bytes: 1024,
        };
        assert_eq!(
            record.rollback_command(),
            "tar -xzf /home/bob/.claude-acc/backups/sync-restore-20231114.tar.gz -C /home/bob"
        );
    }

    /// A file larger than one chunk exercises ordered multi-chunk reassembly,
    /// which is where a scrambled restore would show up.
    #[tokio::test]
    async fn a_multi_chunk_file_is_restored_byte_for_byte() {
        let mut body = Vec::with_capacity(CHUNK_SIZE * 2 + 7);
        let mut state = 0x2545_f491_4f6c_dd1du64;
        for _ in 0..(CHUNK_SIZE * 2 + 7) {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            body.push((state >> 24) as u8);
        }

        let push_dir = TempDir::new().unwrap();
        let bundle = push_one_file(
            &roots_at(push_dir.path(), "alice"),
            "accounts/work/.credentials.json",
            &body,
        );
        let mut server = mockito::Server::new_async().await;
        serve(&mut server, &bundle).await;

        let client = client_at(&server.url());
        let restorer = Restorer::new(TempDir::new().unwrap());
        run(
            restorer.ctx(
                &client,
                RestoreOptions {
                    apply: true,
                    ..Default::default()
                },
            ),
            &mut progress::Silent,
        )
        .await
        .unwrap();

        let dest = restorer
            .roots
            .config_dir
            .join("accounts/work/.credentials.json");
        assert_eq!(fs::read(&dest).unwrap(), body);
    }
}
