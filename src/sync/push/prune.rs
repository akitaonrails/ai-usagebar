//! Retention, and the one destructive pass in the crate.
//!
//! Plan 4-01 created this file with the safe minimum — nothing is deleted.
//! **Plan 4-05 owns it** and fills [`plan_deletions`]'s retention rules and the
//! delete pass.
//!
//! # The ordering rule is structural, and must stay that way
//!
//! D2 is absolute that a snapshot *record* is deleted before any pack, because
//! the reverse can leave a live snapshot pointing at a deleted pack — an
//! unrestorable backup, the worst outcome this feature can produce. That
//! ordering is **not a step in this file**: the truncated pointer is published by
//! the flip, and this file's deletion list is acted on only after
//! [`pointer::commit`](super::pointer::commit) has returned. The record is
//! therefore always gone from the remote before the first `DELETE` is issued.
//! Nobody may later "optimise" the delete pass to run in parallel with the flip.
//!
//! Say equally plainly what that ordering does **not** buy. It orders this
//! machine's own record against this machine's own deletes. It says nothing
//! about another machine's snapshot that has not been published yet. That gap
//! belongs to [`PRUNE_GRACE`](super::PRUNE_GRACE), and the two mitigations are
//! not interchangeable.

use std::collections::HashSet;

use chrono::{DateTime, TimeDelta, Utc};

use crate::error::{AppError, Result};
use crate::sync::crypto::ChunkId;
use crate::sync::github::gate;
use crate::sync::github::write::Asset;

use super::{Pointer, PushCtx};

/// Which snapshot records survive, and which assets may be deleted.
///
/// Pure — `now` and `grace` are parameters rather than reads — so every rule is
/// testable against a table rather than against a server.
///
/// # What survives
///
/// `pointer.snapshots` is truncated from the **oldest** end down to `keep`, and
/// the live pack set is the union of `packs` across every **surviving** record.
/// An asset is deletable only when all three hold:
///
/// 1. its name is one this build recognises — `pack-<64 hex>.bin` or
///    `keyfile-<64 hex>.json`. Anything else might be a future version's object
///    or something the user attached by hand, and a collector that deletes what
///    it does not understand turns every format addition into a data-loss bug;
/// 2. no surviving record names it. For a pack that is its id's absence from the
///    live set; for a keyfile it is not being the one `pointer.keyfile` names;
/// 3. it is older than `grace`.
///
/// Three exclusions are absolute, and each has its own test:
///
/// - **The asset named by `pointer.keyfile`, and this machine's own
///   `local_keyfile`.** No snapshot's `packs` list names either, so the naive
///   rule collects them — and the wrapped master key inside is the only route to
///   the data, for every machine, permanently. This is the single worst thing
///   this function could do, which is why the exclusion is not defined solely by
///   the pointer: that value is untrusted plaintext carried forward from the
///   remote by rebuild rule 3, so a pointer naming a keyfile that does not exist
///   would make an honest machine sweep the real one as an orphan. The caller
///   also holds a trustworthy local answer (`PushCtx::keyfile_asset`), and both
///   are excluded.
/// - **Anything younger than `grace`**, whatever the pointer says about it. Not
///   belt and braces: see [`PRUNE_GRACE`](super::PRUNE_GRACE) for the race it is
///   the only cover for.
/// - **A pointer with no surviving snapshots proposes nothing.** The union of an
///   empty set is empty, which read naively says "everything is garbage", and
///   that arithmetic is how a first-push race or a hand-edited pointer would
///   wipe a release.
///
/// `keep` is clamped to at least one. Config refuses `keep_snapshots = 0`, but
/// the truncated pointer this returns is *published* by
/// [`run_on_demand`], so a zero arriving by any other route must not empty the
/// snapshot list.
///
/// Keyfiles other than the published one are swept because an interrupted first
/// push, or a rekey on a bundle with no pointer, can leave a wrapper asset no
/// pointer names — an old password still opens it. Plan 4-06 found that and
/// could not close it in `rekey.rs`, because closing it there meant calling
/// `ensure_release` — which *creates* a release — purely to run a delete.
pub fn plan_deletions(
    pointer: &Pointer,
    local_keyfile: &str,
    assets: &[Asset],
    keep: usize,
    now: DateTime<Utc>,
    grace: TimeDelta,
) -> (Pointer, Vec<u64>) {
    let mut kept = pointer.clone();
    let keep = keep.max(1);
    if kept.snapshots.len() > keep {
        kept.snapshots.drain(..kept.snapshots.len() - keep);
    }
    if kept.snapshots.is_empty() {
        return (kept, Vec::new());
    }

    let live: HashSet<ChunkId> = kept
        .snapshots
        .iter()
        .flat_map(|s| s.packs.iter().copied())
        .collect();
    let doomed = assets
        .iter()
        .filter(|a| now - a.created_at > grace)
        .filter(|a| match asset_kind(&a.name) {
            Some(Kind::Pack(id)) => !live.contains(&id),
            Some(Kind::Keyfile) => a.name != kept.keyfile && a.name != local_keyfile,
            None => false,
        })
        .map(|a| a.id)
        .collect();
    (kept, doomed)
}

/// The two asset shapes this build writes. Anything else is somebody else's.
enum Kind {
    Pack(ChunkId),
    Keyfile,
}

/// Strict: the id must parse as 64 hex characters, and the affixes must be
/// exact. `pack-<hex>.bin.bak` is not a pack, and is therefore not ours.
fn asset_kind(name: &str) -> Option<Kind> {
    if let Some(hex) = name
        .strip_prefix("pack-")
        .and_then(|r| r.strip_suffix(".bin"))
    {
        return hex.parse().ok().map(Kind::Pack);
    }
    let hex = name.strip_prefix("keyfile-")?.strip_suffix(".json")?;
    hex.parse::<ChunkId>().ok().map(|_| Kind::Keyfile)
}

/// The delete pass after a successful flip.
///
/// `landed` is the pointer [`pointer::commit`](super::pointer::commit) returned
/// — the one that is actually on the remote — and **never** the one this run
/// built. That is the whole mitigation for the race D2 warns about: if another
/// machine won the flip, `landed` is *its* pointer, its snapshot records are in
/// the list, and its packs are consequently live. Deleting a competitor's pack
/// stops being unlikely and becomes impossible.
pub async fn run(
    ctx: &PushCtx<'_>,
    release_id: u64,
    landed: &Pointer,
    keep: usize,
    permit: &gate::Pushing,
) -> Result<usize> {
    let assets = ctx
        .client
        .list_assets(ctx.repo, release_id, ctx.now)
        .await?;
    let (_kept, doomed) = plan_deletions(
        landed,
        &ctx.keyfile_asset,
        &assets,
        keep,
        ctx.now,
        super::PRUNE_GRACE,
    );

    // Sequential rather than concurrent, deliberately: the whole set is a
    // handful of requests, deletion is the one irreversible operation in this
    // crate, and stopping at the first error leaves a state a human can read.
    // Leaving a few extra packs costs storage, and D2 says storage is exactly
    // what a prune failure is allowed to cost.
    let mut deleted = 0usize;
    let mut freed: Vec<ChunkId> = Vec::new();
    let mut refusal = None;
    for asset_id in doomed {
        match ctx
            .client
            .delete_asset(ctx.repo, asset_id, permit, ctx.now)
            .await
        {
            // A 404 is success — the asset is already gone, which is what was
            // asked for. `write::delete_asset` already treats it that way.
            Ok(()) => {
                deleted += 1;
                if let Some(Kind::Pack(pack)) = assets
                    .iter()
                    .find(|a| a.id == asset_id)
                    .and_then(|a| asset_kind(&a.name))
                {
                    freed.push(pack);
                }
            }
            Err(e) => {
                refusal = Some(e);
                break;
            }
        }
    }

    // The **confirmed** deletions only, and whichever way the pass ended: the
    // local index must stop claiming a chunk lives in a pack that is gone, or
    // the next push builds a snapshot naming assets that no longer exist. A row
    // that survives a failed delete is correct; one dropped for a pack still on
    // the remote costs a re-upload, which is the safe direction. The remote's
    // refusal is the more important error, so it wins if both happened.
    let forgotten = ctx.index.forget_chunks(&freed);
    if let Some(e) = refusal {
        return Err(e);
    }
    forgotten?;
    Ok(deleted)
}

/// The `ai-usagebar sync prune` entry point.
///
/// It **starts with the gate**, exactly as the push path does: this issues
/// writes to the remote, and every write path in the crate re-earns its
/// clearance rather than assuming one. A prune is a lower-stakes write than an
/// upload, but "lower stakes" is not a reason for a second rule.
///
/// It takes no release id, because obtaining one means calling `ensure_release`,
/// which is itself a write and therefore already needs the permit this function
/// mints.
///
/// The truncated pointer goes out through the same compare-and-swap a push uses,
/// so on-demand pruning drops its records in the same ordered way — and the
/// delete pass then runs against what **landed**, never against the pointer this
/// call built. Unlike the push path, a failure here **is** a failure: the user
/// asked for exactly this.
pub async fn run_on_demand(ctx: &PushCtx<'_>, keep: usize) -> Result<usize> {
    let permit = super::gate_now(
        ctx,
        ctx.cfg.includes(crate::config::SyncCategory::Credentials),
    )
    .await?;

    // Before the release, so a bundle with nothing published never *creates*
    // one purely to run a delete — which is precisely what plan 4-06 declined to
    // do. With no pointer there is nothing to prove an asset garbage against.
    let (current, sha) = super::pointer::load(ctx.client, ctx.repo, &ctx.repo_id, ctx.now).await?;
    let Some(current) = current else {
        return Ok(0);
    };

    // The rollback anchor, before liveness is computed over this pointer. This
    // is the path where a laundered rollback becomes irreversible:
    // `plan_deletions` treats every pack the dropped snapshots alone referenced
    // as unreferenced, and those packs are older than `PRUNE_GRACE`.
    super::assert_no_rollback(ctx, Some(&current))?;

    let release_id = ctx
        .client
        .ensure_release(ctx.repo, super::RELEASE_TAG, &permit, ctx.now)
        .await?;

    // `plan_deletions` is the one place the truncation rule lives; this closure
    // does not restate it. It runs again on a 409, against whatever won.
    let rebuild = |arriving: Option<&Pointer>| -> Result<Pointer> {
        let arriving = arriving.ok_or_else(|| {
            AppError::Other(
                "the snapshot pointer disappeared while pruning. Nothing was deleted — re-run \
                 `ai-usagebar sync prune`."
                    .into(),
            )
        })?;
        Ok(plan_deletions(
            arriving,
            &ctx.keyfile_asset,
            &[],
            keep,
            ctx.now,
            super::PRUNE_GRACE,
        )
        .0)
    };
    let (landed, _sha) = super::pointer::commit(
        ctx.client,
        ctx.repo,
        Some(&current),
        sha.as_deref(),
        rebuild,
        &permit,
        ctx.now,
    )
    .await?;

    run(ctx, release_id, &landed, keep, &permit).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::crypto::ChunkId;
    use crate::sync::github::write::ASSET_STATE_UPLOADED;
    use crate::sync::push::{POINTER_VERSION, SnapshotRecord, keyfile_asset_name, pack_asset_name};

    /// This machine's own keyfile asset — the trustworthy half of the exclusion
    /// set. Deliberately a name no fixture below plants, so every existing case
    /// still exercises the pointer's half; the one test that cares plants it.
    const LOCAL_KEYFILE: &str = "keyfile-                                 0000000000000000000000000000000000000000000000000000000000000001                                 .json";

    /// Fixed. Nothing here reads a clock: `plan_deletions` takes `now`.
    pub(super) const NOW: DateTime<Utc> = match DateTime::from_timestamp(1_700_000_000, 0) {
        Some(t) => t,
        None => panic!("a fixed timestamp"),
    };

    pub(super) fn id(byte: u8) -> ChunkId {
        ChunkId::from_bytes([byte; 32])
    }

    /// `age` is how long before [`NOW`] the remote created it — the only input
    /// the grace window reads.
    pub(super) fn asset(asset_id: u64, name: &str, age: TimeDelta) -> Asset {
        Asset {
            id: asset_id,
            name: name.to_owned(),
            size: 9,
            state: ASSET_STATE_UPLOADED.to_owned(),
            created_at: NOW - age,
            digest: None,
        }
    }

    pub(super) fn pack(asset_id: u64, byte: u8, age: TimeDelta) -> Asset {
        asset(asset_id, &pack_asset_name(&id(byte)), age)
    }

    pub(super) fn snapshot(packs: Vec<ChunkId>) -> SnapshotRecord {
        SnapshotRecord {
            root: format!("root-{}", packs.len()),
            index_chunks: Vec::new(),
            packs,
        }
    }

    pub(super) fn pointer(keyfile: &str, snapshots: Vec<SnapshotRecord>) -> Pointer {
        Pointer {
            format: POINTER_VERSION,
            repo_id: "github:1".into(),
            keyfile: keyfile.to_owned(),
            snapshots,
        }
    }

    pub(super) const OLD: TimeDelta = TimeDelta::hours(48);
    pub(super) const GRACE: TimeDelta = TimeDelta::hours(24);

    /// The base case: two surviving snapshots name A and B, C is nobody's.
    #[test]
    fn only_a_pack_no_surviving_snapshot_names_is_deleted() {
        let keyfile = keyfile_asset_name(&id(0xff));
        let p = pointer(
            &keyfile,
            vec![snapshot(vec![id(0xaa)]), snapshot(vec![id(0xaa), id(0xbb)])],
        );
        let assets = [
            pack(1, 0xaa, OLD),
            pack(2, 0xbb, OLD),
            pack(3, 0xcc, OLD),
            asset(4, &keyfile, OLD),
        ];

        let (kept, doomed) = plan_deletions(&p, LOCAL_KEYFILE, &assets, 10, NOW, GRACE);

        assert_eq!(doomed, vec![3], "only C");
        assert_eq!(kept.snapshots.len(), 2, "nothing to truncate at keep = 10");
    }

    /// Both sides of the boundary. An unreferenced pack uploaded by a machine
    /// that has not flipped yet is indistinguishable from garbage; the age floor
    /// is the only thing that tells them apart.
    #[test]
    fn an_asset_younger_than_the_grace_window_survives_being_unreferenced() {
        let p = pointer("keyfile-x.json", vec![snapshot(vec![id(0xaa)])]);

        for (age, expected) in [
            (TimeDelta::hours(1), vec![]),
            (GRACE, vec![]),
            (GRACE + TimeDelta::seconds(1), vec![7]),
            (OLD, vec![7]),
        ] {
            let (_, doomed) =
                plan_deletions(&p, LOCAL_KEYFILE, &[pack(7, 0xcc, age)], 10, NOW, GRACE);
            assert_eq!(doomed, expected, "at age {age}");
        }
    }

    /// Sharing is what makes ten snapshots cheap. A pack only the oldest
    /// *surviving* record names is still live.
    #[test]
    fn a_pack_only_the_oldest_surviving_snapshot_names_is_retained() {
        let p = pointer(
            "keyfile-x.json",
            vec![snapshot(vec![id(0xaa)]), snapshot(vec![id(0xbb)])],
        );
        let (_, doomed) = plan_deletions(&p, LOCAL_KEYFILE, &[pack(1, 0xaa, OLD)], 10, NOW, GRACE);
        assert!(doomed.is_empty(), "{doomed:?}");
    }

    /// **The worst thing this function could do.** No snapshot's `packs` list
    /// names the keyfile, so the naive rule deletes it — and the wrapped master
    /// key is the only route to the data, for every machine, permanently.
    #[test]
    fn the_keyfile_the_pointer_names_is_never_deleted() {
        let keyfile = keyfile_asset_name(&id(0xff));
        for snapshots in [
            vec![],
            vec![snapshot(vec![])],
            vec![snapshot(vec![id(0xaa)]), snapshot(vec![id(0xbb)])],
        ] {
            let p = pointer(&keyfile, snapshots);
            // Ancient, unreferenced by any `packs` list, and alone in the
            // release: every reason the naive rule would have to collect it.
            let (_, doomed) = plan_deletions(
                &p,
                LOCAL_KEYFILE,
                &[asset(1, &keyfile, TimeDelta::days(365))],
                1,
                NOW,
                GRACE,
            );
            assert!(doomed.is_empty(), "{doomed:?}");
        }
    }

    /// 4-06's gap: an interrupted first push, or a local-only rekey, can leave a
    /// keyfile asset no pointer names. Same grace window, same landed pointer.
    #[test]
    fn an_orphan_keyfile_no_pointer_names_is_swept_like_a_pack() {
        let live = keyfile_asset_name(&id(0xff));
        let orphan = keyfile_asset_name(&id(0xee));
        let p = pointer(&live, vec![snapshot(vec![id(0xaa)])]);
        let assets = [
            asset(1, &live, OLD),
            asset(2, &orphan, OLD),
            asset(3, &keyfile_asset_name(&id(0xdd)), TimeDelta::hours(2)),
        ];

        let (_, doomed) = plan_deletions(&p, LOCAL_KEYFILE, &assets, 10, NOW, GRACE);

        assert_eq!(doomed, vec![2], "the live one and the young one both stay");
    }

    /// **F-7.** `kept.keyfile` is untrusted plaintext carried forward from the
    /// remote, and T-4-36 calls deleting the keyfile the single worst thing this
    /// function can do. A pointer naming a keyfile that does not exist must not
    /// make an honest machine sweep its own.
    #[test]
    fn a_lying_pointer_cannot_make_this_machine_sweep_its_own_keyfile() {
        let mine = keyfile_asset_name(&id(0x11));
        let p = pointer(
            &keyfile_asset_name(&id(0xff)),
            vec![snapshot(vec![id(0xaa)])],
        );
        let orphan = keyfile_asset_name(&id(0xee));
        let assets = [asset(1, &mine, OLD), asset(2, &orphan, OLD)];

        let (_, doomed) = plan_deletions(&p, &mine, &assets, 10, NOW, GRACE);

        assert_eq!(
            doomed,
            vec![2],
            "the local wrapper survives a pointer that does not name it"
        );
    }

    /// A collector that deletes what it does not understand turns every format
    /// addition into a data-loss bug.
    #[test]
    fn an_asset_this_build_does_not_recognise_is_never_deleted() {
        let p = pointer("keyfile-x.json", vec![snapshot(vec![id(0xaa)])]);
        let hex = "aa".repeat(32);
        let strangers = [
            "manifest-v2.bin".to_owned(),
            format!("pack-{hex}"),                   // no extension
            format!("pack-{hex}.bin.bak"),           // trailing junk
            format!("pack-{}.bin", "zz".repeat(32)), // not hex
            format!("pack-{}.bin", "aa".repeat(16)), // too short
            "pack-.bin".to_owned(),
            format!("keyfile-{}.json", "zz".repeat(32)),
            "keyfile.json".to_owned(),
            "README.md".to_owned(),
        ];
        let assets: Vec<Asset> = strangers
            .iter()
            .enumerate()
            .map(|(i, name)| asset(i as u64 + 1, name, TimeDelta::days(365)))
            .collect();

        let (_, doomed) = plan_deletions(&p, LOCAL_KEYFILE, &assets, 10, NOW, GRACE);

        assert!(doomed.is_empty(), "{doomed:?}");
    }

    /// The packs freed by truncation are exactly what this pass exists to
    /// collect.
    #[test]
    fn truncation_drops_the_oldest_records_and_frees_their_packs() {
        let p = pointer(
            "keyfile-x.json",
            vec![
                snapshot(vec![id(0x01)]),
                snapshot(vec![id(0x02)]),
                snapshot(vec![id(0x03), id(0x02)]),
            ],
        );
        let assets = [pack(1, 0x01, OLD), pack(2, 0x02, OLD), pack(3, 0x03, OLD)];

        let (kept, doomed) = plan_deletions(&p, LOCAL_KEYFILE, &assets, 2, NOW, GRACE);

        assert_eq!(kept.snapshots, p.snapshots[1..], "oldest end only");
        assert_eq!(doomed, vec![1], "0x02 is still shared, 0x03 is the newest");
    }

    /// The union of an empty set is empty, which read naively says "everything
    /// is garbage". A first-push race or a hand-edited pointer would then wipe
    /// the release.
    #[test]
    fn a_pointer_with_no_snapshots_proposes_no_deletions() {
        let p = pointer("keyfile-x.json", vec![]);
        let assets = [pack(1, 0xaa, OLD), pack(2, 0xbb, OLD)];
        let (kept, doomed) = plan_deletions(&p, LOCAL_KEYFILE, &assets, 10, NOW, GRACE);
        assert!(doomed.is_empty(), "{doomed:?}");
        assert!(kept.snapshots.is_empty());
    }

    /// Config refuses `keep_snapshots = 0`, but this function's truncated
    /// pointer is *published*, so a zero arriving any other way must not empty
    /// the snapshot list.
    #[test]
    fn keep_of_zero_still_leaves_the_newest_record() {
        let p = pointer(
            "keyfile-x.json",
            vec![snapshot(vec![id(0x01)]), snapshot(vec![id(0x02)])],
        );
        let (kept, doomed) =
            plan_deletions(&p, LOCAL_KEYFILE, &[pack(1, 0x01, OLD)], 0, NOW, GRACE);
        assert_eq!(kept.snapshots, p.snapshots[1..]);
        assert_eq!(doomed, vec![1]);
    }
}

/// The remote half: the delete pass and the on-demand entry point.
///
/// Split from [`tests`] because it needs a whole `PushCtx` and a mock server,
/// while everything above proves its rules against a table.
#[cfg(test)]
mod delete_pass {
    use super::tests::{GRACE, NOW, OLD, asset, id, pack, pointer, snapshot};
    use super::*;

    use std::sync::{Arc, Mutex};

    use base64::Engine;
    use tempfile::TempDir;
    use zeroize::Zeroizing;

    use crate::config::SyncConfig;
    use crate::sync::SyncRoots;
    use crate::sync::crypto::{KdfParams, Keyfile, Keys, MIN_KDF_MEMORY_KIB};
    use crate::sync::github::gate::RepoFacts;
    use crate::sync::github::token::TokenSource;
    use crate::sync::github::{Client, Endpoints, RepoRef};
    use crate::sync::index::Index;
    use crate::sync::push::keyfile_asset_name;

    const B64: base64::engine::general_purpose::GeneralPurpose =
        base64::engine::general_purpose::STANDARD;

    // -- the harness -------------------------------------------------------

    fn repo() -> RepoRef {
        RepoRef::parse("o/n").unwrap()
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

    fn permit() -> gate::Pushing {
        let facts = RepoFacts {
            id: 1,
            private: true,
            visibility: "private".into(),
            owner_login: "o".into(),
            owner_id: 7,
            archived: false,
            fork: false,
            admin_permission: false,
        };
        gate::assert_pushable(&facts, &repo(), true, NOW)
            .expect("a private repository clears")
            .0
            .spend(NOW)
            .expect("freshly minted")
    }

    /// The format's own floor, never the shipped 1 GiB: the AUR `check()` runs
    /// these on an installer's machine.
    fn cheap() -> KdfParams {
        KdfParams {
            m_kib: MIN_KDF_MEMORY_KIB,
            t: 1,
            p: 1,
        }
    }

    /// Everything the context borrows, owned in one place so a test can hold it
    /// across the `await`. A `TempDir`, never a real `$HOME`.
    struct Local {
        _dir: TempDir,
        roots: SyncRoots,
        cfg: SyncConfig,
        keys: Keys,
        index: Index,
    }

    impl Local {
        fn new() -> Self {
            let dir = TempDir::new().unwrap();
            let roots = SyncRoots::at(
                dir.path().join("config.toml"),
                dir.path().to_path_buf(),
                dir.path().join("desktop"),
                dir.path().join("profiles"),
                dir.path().join("claude-home"),
            );
            let (_keyfile, keys) = Keyfile::create(b"prune-fixture-password", cheap()).unwrap();
            let index = Index::at(&roots.index_file).unwrap();
            Self {
                _dir: dir,
                roots,
                cfg: SyncConfig::default(),
                keys,
                index,
            }
        }

        fn ctx<'a>(&'a self, client: &'a Client, repo: &'a RepoRef) -> PushCtx<'a> {
            PushCtx {
                client,
                repo,
                cfg: &self.cfg,
                roots: &self.roots,
                keys: &self.keys,
                kdf: cheap(),
                index: &self.index,
                repo_id: "github:1".into(),
                keyfile_asset: keyfile_asset_name(&id(0xff)),
                previous: None,
                allow_rollback: false,
                now: NOW,
            }
        }
    }

    /// Every request the run made, in order, as `METHOD path` plus its body.
    type Trace = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

    fn record(trace: &Trace) -> impl Fn(&mockito::Request) -> bool + Send + Sync + 'static {
        let trace = Arc::clone(trace);
        move |req| {
            trace.lock().unwrap().push((
                format!("{} {}", req.method(), req.path()),
                req.body().cloned().unwrap_or_default(),
            ));
            true
        }
    }

    fn paths(trace: &Trace) -> Vec<String> {
        trace
            .lock()
            .unwrap()
            .iter()
            .map(|(p, _)| p.clone())
            .collect()
    }

    fn listing(assets: &[Asset]) -> String {
        let each: Vec<String> = assets
            .iter()
            .map(|a| {
                format!(
                    r#"{{"id":{},"name":"{}","size":{},"state":"{}","created_at":"{}"}}"#,
                    a.id,
                    a.name,
                    a.size,
                    a.state,
                    a.created_at.to_rfc3339()
                )
            })
            .collect();
        format!("[{}]", each.join(","))
    }

    fn repo_json(private: bool) -> String {
        format!(
            r#"{{"id":1,"private":{private},"visibility":"{}","owner":{{"login":"o","id":7}}}}"#,
            if private { "private" } else { "public" }
        )
    }

    fn contents_body(p: &Pointer) -> String {
        format!(
            r#"{{"sha":"blob1","content":"{}"}}"#,
            B64.encode(serde_json::to_vec(p).unwrap())
        )
    }

    /// The pointer decoded back out of the `PUT` body — what actually landed.
    fn published(trace: &Trace) -> Pointer {
        let (_, body) = trace
            .lock()
            .unwrap()
            .iter()
            .find(|(p, _)| p.starts_with("PUT"))
            .cloned()
            .expect("a pointer was published");
        let outer: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let raw = B64.decode(outer["content"].as_str().unwrap()).unwrap();
        serde_json::from_slice(&raw).unwrap()
    }

    // -- the delete pass ---------------------------------------------------

    #[tokio::test]
    async fn one_unreferenced_asset_is_one_delete_and_a_count_of_one() {
        let live = keyfile_asset_name(&id(0xff));
        let landed = pointer(&live, vec![snapshot(vec![id(0xaa)])]);
        let assets = [pack(1, 0xaa, OLD), pack(2, 0xcc, OLD), asset(3, &live, OLD)];

        let mut server = mockito::Server::new_async().await;
        let trace: Trace = Arc::default();
        let _list = server
            .mock("GET", mockito::Matcher::Regex(r"/releases/9/assets".into()))
            .with_status(200)
            .with_body(listing(&assets))
            .match_request(record(&trace))
            .create_async()
            .await;
        let kept = server
            .mock("DELETE", "/repos/o/n/releases/assets/1")
            .with_status(204)
            .expect(0)
            .create_async()
            .await;
        let doomed = server
            .mock("DELETE", "/repos/o/n/releases/assets/2")
            .with_status(204)
            .expect(1)
            .match_request(record(&trace))
            .create_async()
            .await;

        let local = Local::new();
        let client = client_at(&server.url());
        let repo = repo();
        let deleted = run(&local.ctx(&client, &repo), 9, &landed, 10, &permit())
            .await
            .unwrap();

        assert_eq!(deleted, 1);
        kept.assert_async().await;
        doomed.assert_async().await;
        assert!(
            paths(&trace)[0].starts_with("GET"),
            "listed before deleting"
        );
    }

    /// The asset is already gone, which is exactly what was asked for.
    #[tokio::test]
    async fn a_delete_answered_with_404_counts_as_success() {
        let landed = pointer("keyfile-x.json", vec![snapshot(vec![id(0xaa)])]);

        let mut server = mockito::Server::new_async().await;
        let _list = server
            .mock("GET", mockito::Matcher::Regex(r"/releases/9/assets".into()))
            .with_status(200)
            .with_body(listing(&[pack(2, 0xcc, OLD)]))
            .create_async()
            .await;
        let _gone = server
            .mock("DELETE", "/repos/o/n/releases/assets/2")
            .with_status(404)
            .with_body(r#"{"message":"Not Found"}"#)
            .create_async()
            .await;

        let local = Local::new();
        let client = client_at(&server.url());
        let repo = repo();
        assert_eq!(
            run(&local.ctx(&client, &repo), 9, &landed, 10, &permit())
                .await
                .unwrap(),
            1
        );
    }

    /// Sequential, stopping at the first error, is what makes a partial failure
    /// comprehensible — and `forget_chunks` then sees the confirmed set only.
    /// A row that survives a failed delete is correct; one dropped for a pack
    /// still on the remote costs a re-upload, which is the safe direction.
    #[tokio::test]
    async fn a_refused_delete_stops_the_pass_and_forgets_only_what_went() {
        let landed = pointer("keyfile-x.json", vec![snapshot(vec![id(0xaa)])]);
        let assets = [pack(2, 0xcc, OLD), pack(3, 0xdd, OLD)];

        let mut server = mockito::Server::new_async().await;
        let _list = server
            .mock("GET", mockito::Matcher::Regex(r"/releases/9/assets".into()))
            .with_status(200)
            .with_body(listing(&assets))
            .create_async()
            .await;
        let first = server
            .mock("DELETE", "/repos/o/n/releases/assets/2")
            .with_status(204)
            .expect(1)
            .create_async()
            .await;
        let second = server
            .mock("DELETE", "/repos/o/n/releases/assets/3")
            .with_status(403)
            .with_body(r#"{"message":"Forbidden"}"#)
            .expect(1)
            .create_async()
            .await;

        let local = Local::new();
        // One chunk in each of three packs: the one that goes, the one whose
        // delete is refused, and the live one.
        local
            .index
            .record_chunks(&[
                (id(0x01), id(0xcc), 0, 10, 9),
                (id(0x02), id(0xdd), 0, 10, 9),
                (id(0x03), id(0xaa), 0, 10, 9),
            ])
            .unwrap();

        let client = client_at(&server.url());
        let repo = repo();
        let err = run(&local.ctx(&client, &repo), 9, &landed, 10, &permit())
            .await
            .expect_err("the refusal is returned honestly");
        assert!(!err.to_string().is_empty(), "{err}");

        first.assert_async().await;
        second.assert_async().await;
        let known = local.index.chunk_locations(&[id(0x01), id(0x02), id(0x03)]);
        assert!(
            !known.contains_key(&id(0x01)),
            "the deleted pack is forgotten"
        );
        assert!(known.contains_key(&id(0x02)), "a failed delete keeps rows");
        assert!(known.contains_key(&id(0x03)), "a live pack keeps its rows");
    }

    /// T-4-35. `landed` is whatever `pointer::commit` returned, so a machine
    /// that won the flip has its records — and therefore its packs — in the
    /// list this pass reads. Every asset here is well past the grace window, so
    /// the age floor is not what is doing the work.
    #[tokio::test]
    async fn a_competing_machines_packs_are_never_proposed_for_deletion() {
        let landed = pointer(
            "keyfile-x.json",
            vec![snapshot(vec![id(0xaa)]), snapshot(vec![id(0xbb)])],
        );
        let assets = [pack(1, 0xaa, OLD), pack(2, 0xbb, OLD), pack(3, 0xcc, OLD)];

        let mut server = mockito::Server::new_async().await;
        let _list = server
            .mock("GET", mockito::Matcher::Regex(r"/releases/9/assets".into()))
            .with_status(200)
            .with_body(listing(&assets))
            .create_async()
            .await;
        let competitor = server
            .mock("DELETE", "/repos/o/n/releases/assets/2")
            .with_status(204)
            .expect(0)
            .create_async()
            .await;
        let _garbage = server
            .mock("DELETE", "/repos/o/n/releases/assets/3")
            .with_status(204)
            .create_async()
            .await;

        let local = Local::new();
        let client = client_at(&server.url());
        let repo = repo();
        assert_eq!(
            run(&local.ctx(&client, &repo), 9, &landed, 10, &permit())
                .await
                .unwrap(),
            1
        );
        competitor.assert_async().await;
    }

    // -- the on-demand entry point ----------------------------------------

    /// T-4-42b. The gate is first, and a repository that is no longer private
    /// stops the command before a single asset is listed.
    #[tokio::test]
    async fn run_on_demand_refuses_a_public_repository_before_listing_anything() {
        let mut server = mockito::Server::new_async().await;
        let trace: Trace = Arc::default();
        let _repo = server
            .mock("GET", "/repos/o/n")
            .with_status(200)
            .with_body(repo_json(false))
            .match_request(record(&trace))
            .create_async()
            .await;
        let listed = server
            .mock("GET", mockito::Matcher::Regex(r"/releases/9/assets".into()))
            .with_status(200)
            .with_body("[]")
            .expect(0)
            .create_async()
            .await;

        let local = Local::new();
        let client = client_at(&server.url());
        let repo = repo();
        let err = run_on_demand(&local.ctx(&client, &repo), 10)
            .await
            .expect_err("a public repository is refused");
        assert!(err.to_string().contains("REFUSING"), "{err}");
        assert_eq!(paths(&trace), vec!["GET /repos/o/n".to_owned()]);
        listed.assert_async().await;
    }

    /// D2's order on the on-demand path too: the record is dropped by the flip,
    /// and the flip has already returned before the first `DELETE` is issued.
    #[tokio::test]
    async fn run_on_demand_publishes_the_truncated_pointer_before_deleting_anything() {
        let current = pointer(
            "keyfile-x.json",
            vec![
                snapshot(vec![id(0x01)]),
                snapshot(vec![id(0x02)]),
                snapshot(vec![id(0x03)]),
            ],
        );
        let assets = [pack(1, 0x01, OLD), pack(2, 0x02, OLD), pack(3, 0x03, OLD)];

        let mut server = mockito::Server::new_async().await;
        let trace: Trace = Arc::default();
        let mut mocks = Vec::new();
        for mock in [
            server
                .mock("GET", "/repos/o/n")
                .with_status(200)
                .with_body(repo_json(true)),
            server
                .mock("GET", "/repos/o/n/contents/sync/pointer.json")
                .with_status(200)
                .with_body(contents_body(&current)),
            server
                .mock("GET", "/repos/o/n/releases/tags/ai-usagebar-sync-v1")
                .with_status(200)
                .with_body(r#"{"id":9}"#),
            server
                .mock("PUT", "/repos/o/n/contents/sync/pointer.json")
                .with_status(200)
                .with_body(r#"{"content":{"sha":"blob2"}}"#),
            server
                .mock("GET", mockito::Matcher::Regex(r"/releases/9/assets".into()))
                .with_status(200)
                .with_body(listing(&assets)),
            server
                .mock("DELETE", "/repos/o/n/releases/assets/1")
                .with_status(204),
        ] {
            mocks.push(mock.match_request(record(&trace)).create_async().await);
        }

        let local = Local::new();
        let client = client_at(&server.url());
        let repo = repo();
        assert_eq!(
            run_on_demand(&local.ctx(&client, &repo), 2).await.unwrap(),
            1
        );

        let seen = paths(&trace);
        let flip = seen.iter().position(|p| p.starts_with("PUT")).unwrap();
        let del = seen.iter().position(|p| p.starts_with("DELETE")).unwrap();
        assert!(flip < del, "{seen:?}");
        assert_eq!(published(&trace).snapshots, current.snapshots[1..]);
    }

    /// Nothing is published, so nothing is proven garbage — and creating a
    /// release purely to run a delete is exactly what 4-06 refused to do.
    #[tokio::test]
    async fn run_on_demand_with_no_pointer_creates_no_release_and_deletes_nothing() {
        let mut server = mockito::Server::new_async().await;
        let trace: Trace = Arc::default();
        let mut mocks = Vec::new();
        for mock in [
            server
                .mock("GET", "/repos/o/n")
                .with_status(200)
                .with_body(repo_json(true)),
            server
                .mock("GET", "/repos/o/n/contents/sync/pointer.json")
                .with_status(404)
                .with_body(r#"{"message":"Not Found"}"#),
        ] {
            mocks.push(mock.match_request(record(&trace)).create_async().await);
        }
        let release = server
            .mock("GET", "/repos/o/n/releases/tags/ai-usagebar-sync-v1")
            .with_status(200)
            .with_body(r#"{"id":9}"#)
            .expect(0)
            .create_async()
            .await;

        let local = Local::new();
        let client = client_at(&server.url());
        let repo = repo();
        assert_eq!(
            run_on_demand(&local.ctx(&client, &repo), 10).await.unwrap(),
            0
        );
        release.assert_async().await;
        assert!(
            !paths(&trace).iter().any(|p| p.starts_with("PUT")),
            "nothing was flipped"
        );
    }

    /// The grace window is not a constant this module re-declares.
    #[test]
    fn the_pass_uses_the_shared_grace_window() {
        assert_eq!(GRACE, super::super::PRUNE_GRACE);
    }
}
