//! Pack assets on to the release: the resume scan, the bounded uploader, and
//! the verification D3 makes a precondition of the flip.
//!
//! Nothing in this file prints. Rendering belongs to the [`Progress`]
//! implementations, which keeps the uploader testable with
//! [`Silent`](super::progress::Silent).
//!
//! # Why there is no `JoinSet` here
//!
//! Plan 4-03 was written against a `tokio::task::JoinSet` refilled to four.
//! Two things merged since make that unimplementable rather than merely
//! unattractive, and both are structural:
//!
//! - every write verb takes a [`gate::Pushing`] **by reference**, and `Pushing`
//!   is not `Clone`. A spawned task must be `'static`, so it cannot hold the
//!   borrow that proves the gate was earned. Minting a second permit here to
//!   work around that would put gate logic in this module, which is exactly
//!   what T-4-21 says must not exist.
//! - [`PushCtx`] holds an `&Index`, whose `rusqlite::Connection` is `Send` but
//!   not `Sync`, so a future borrowing the context is not `Send` and cannot be
//!   spawned at all.
//!
//! What replaces it keeps the same semantics — up to four uploads outstanding,
//! refilled as each completes — with a window of boxed futures polled by hand.
//! Concurrency without `'static`: no crate is added, and the borrow of the
//! permit survives.

use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::task::Poll;

use crate::error::{AppError, Result};
use crate::sync::crypto::{Keyfile, content_address};
use crate::sync::github::gate;
use crate::sync::github::write::{ASSET_STATE_UPLOADED, Asset};

use super::progress::Progress;
use super::{BuiltPack, PushCtx, keyfile_asset_name, pack_asset_name};

/// One outstanding upload: the pack's position in the pending list — which is
/// what progress reports — and the future doing the work.
type InFlight<'a> = (usize, Pin<Box<dyn Future<Output = Result<()>> + 'a>>);

/// How many bodies may be on the wire at once.
///
/// The research's documented ceiling is 100 concurrent requests, but with
/// `PACK_MAX`-sized bodies a push is bandwidth-bound long before it is
/// request-bound, and a low cap keeps a laptop's uplink usable while it runs
/// (T-4-23). Four 48 MiB bodies is also the memory ceiling of this module.
const MAX_IN_FLIGHT: usize = 4;

/// What one upload pass actually sent.
///
/// `names` exists because the incident path has to delete exactly what this run
/// put on the release, and the only trustworthy answer is the one this process
/// observed. Reconstructing it by comparing the remote's `created_at` against
/// this machine's clock is wrong in both directions — see
/// [`went_public_mid_push`](super::went_public_mid_push).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Uploaded {
    /// The asset names this run uploaded, in the order they were sent.
    pub names: Vec<String>,
    /// Packs already present under the same content address.
    pub skipped: usize,
    /// Measured from the packs' own lengths, never from a projection.
    pub bytes: u64,
}

/// Upload `packs`, reporting exactly what was sent.
///
/// **Step 1, the resume scan.** One `list_assets`, then a three-way decision
/// per pack — see [`decide`]. Resume is free because a pack's name *is* its
/// content address: a changed pack gets a different name, so "already
/// uploaded" has an exact answer and no local record of what a previous run
/// did is needed (D4, SYNC-05).
///
/// **Step 2, the uploads**, at most [`MAX_IN_FLIGHT`] at a time. Every request
/// goes through `write::upload_asset`, which already routes through the shared
/// retry helper, so D7's rate-limit discipline is inherited rather than
/// re-implemented — there is deliberately no second retry loop here (T-4-24).
///
/// **Step 3, verification**, which D3 makes a precondition of the flip rather
/// than a nicety: every asset this run uploaded is fetched back and its content
/// address compared against the pack's id. A mismatch fails this function, so
/// the orchestrator never reaches the pointer `PUT` and no pointer can
/// reference a pack that did not verify (T-4-20).
///
/// Say plainly what that check is and is not: a corrupt pack would in any case
/// fail its per-blob Poly1305 tags on read, so this catches transport and
/// packaging bugs, not attacks. It costs one extra download of newly-uploaded
/// data — on a 115 MB first push, 115 MB — and that is the price D3 sets. A
/// later phase can drop it only if GitHub is found to populate the asset
/// `digest` field, which the live probe in `tests/live.rs` is there to answer.
///
/// Skipped assets are **not** re-downloaded: they carry a content-addressed
/// name, a torn upload is caught by the state check, and re-verifying data an
/// earlier run already verified would double the traffic of every resume.
///
/// `permit` is the gate's, minted inside the push. It authorises every write
/// here; the flip gets a second one, minted by the re-gate afterwards.
pub async fn run(
    ctx: &PushCtx<'_>,
    release_id: u64,
    packs: &[BuiltPack],
    permit: &gate::Pushing,
    progress: &mut dyn Progress,
) -> Result<Uploaded> {
    let existing = ctx
        .client
        .list_assets(ctx.repo, release_id, ctx.now)
        .await?;

    let mut pending: Vec<&BuiltPack> = Vec::with_capacity(packs.len());
    let mut skipped = 0usize;
    for pack in packs {
        let name = pack_asset_name(&pack.id);
        match decide(&existing, &name, pack.bytes.len() as u64) {
            Decision::Present => skipped += 1,
            // GitHub creates the asset record before the body finishes, so an
            // interrupted upload leaves a zombie whose name would collide
            // forever. Deleting it is what makes a resume a continuation.
            Decision::Torn(asset_id) => {
                ctx.client
                    .delete_asset(ctx.repo, asset_id, permit, ctx.now)
                    .await?;
                pending.push(pack);
            }
            Decision::Absent => pending.push(pack),
        }
    }

    // Measured from the packs' own lengths, never from a projection.
    let bytes: u64 = pending.iter().map(|p| p.bytes.len() as u64).sum();
    progress.start(pending.len(), bytes);
    let outcome = upload_all(ctx, release_id, &pending, permit, progress).await;
    progress.finish();
    outcome?;
    Ok(Uploaded {
        names: pending.iter().map(|p| pack_asset_name(&p.id)).collect(),
        skipped,
        bytes,
    })
}

/// Refuse when the arriving pointer names a keyfile this machine does not hold.
///
/// **A different address means another machine rekeyed and this one is stale.**
/// The old wrapper is exactly what that rekey verifiably deleted; republishing
/// it puts it back where the old password opens it, and `PRUNE_GRACE` then
/// protects it for 24 h while every subsequent push from this machine resets
/// `created_at` — so it is never collected and the password change was
/// cosmetic.
///
/// It refuses rather than silently skipping the upload. A skip would leave the
/// bundle correct and the *user* wrong: the old password would keep opening this
/// machine's local keyfile forever while they believed the password had changed
/// everywhere. The data is unaffected either way — a rekey rewraps the master
/// key and re-encrypts nothing — so the refusal costs a manual file copy, not a
/// backup.
///
/// `previous == None` is a first push and the only case that legitimately
/// publishes from local state.
pub(crate) fn assert_keyfile_is_current(ctx: &PushCtx<'_>) -> Result<()> {
    let Some(published) = ctx.previous.as_ref().map(|p| p.keyfile.as_str()) else {
        return Ok(());
    };
    let local = keyfile_asset_name(&content_address(&canonical_keyfile(ctx)?));
    if published == local {
        return Ok(());
    }
    let path = crate::sync::cli::keyfile_path(ctx.roots);
    Err(AppError::Other(format!(
        "STOP — the sync password was changed on another machine. The bundle names {published}, \
         but this machine's keyfile is {local}, the superseded wrapper.\n\
         Refusing to push: publishing it would put the old wrapper back on the remote, where \
         the old password opens it again — undoing the password change. Nothing was uploaded \
         and the snapshot pointer is untouched.\n\
         Your data is unaffected. A password change rewraps the master key and re-encrypts no \
         pack, so every byte already on the remote is still readable under the new password.\n\
         To catch this machine up, copy {} from the machine where the password was changed, \
         replacing the local file, then re-run the same command.",
        path.display()
    )))
}

/// Publish the local keyfile asset if the release does not already carry it.
///
/// **Without this a first push publishes a pointer naming an asset that does
/// not exist**, and a second machine cannot bootstrap from the bundle at all —
/// which is the whole purpose of the milestone. `Pointer.keyfile` is set from a
/// content address of the local keyfile, but before this function only `rekey`
/// ever *uploaded* one. Plan 4-01 recorded the gap; it is closed here.
///
/// Idempotent by content address, which is what makes it safe to call on every
/// push rather than only the first: the asset's name is
/// [`keyfile_asset_name`] over the canonical serialization, so an unchanged
/// keyfile always resolves to the same name and a listing that already shows it
/// uploaded ends the function without a request body.
///
/// The bytes are the keyfile's **canonical** serialization —
/// `serde_json::to_vec` of what is on disk, which is what
/// `cli::keyfile_asset_for` addresses and what `rekey` uploads. Not the file's
/// literal bytes: setup writes it pretty-printed, and uploading those would
/// publish an asset whose name addresses different bytes than it holds. Nothing
/// here re-wraps, re-derives or re-encrypts anything — getting a keyfile's bytes
/// wrong is an unrecoverable bundle.
///
/// **It publishes only a keyfile the arriving pointer already names.** This
/// reads whatever is on *this* machine's disk, and if another machine has
/// rekeyed that is the superseded wrapper — the asset D5 verifiably deleted.
/// Re-uploading it resurrects it, and prune's orphan-keyfile sweep cannot reach
/// it because `PRUNE_GRACE` retains anything younger than 24 h and the next push
/// from the same stale machine resets `created_at`. So
/// [`assert_keyfile_is_current`] refuses first, here and again at the top of
/// `push::run` where the refusal costs nothing. Rekey does not call this at all
/// — during a rekey the local keyfile is still the old one until after the flip.
pub async fn ensure_keyfile(
    ctx: &PushCtx<'_>,
    release_id: u64,
    permit: &gate::Pushing,
) -> Result<()> {
    assert_keyfile_is_current(ctx)?;
    let bytes = canonical_keyfile(ctx)?;
    let name = keyfile_asset_name(&content_address(&bytes));
    let existing = ctx
        .client
        .list_assets(ctx.repo, release_id, ctx.now)
        .await?;
    match decide(&existing, &name, bytes.len() as u64) {
        Decision::Present => return Ok(()),
        Decision::Torn(asset_id) => {
            ctx.client
                .delete_asset(ctx.repo, asset_id, permit, ctx.now)
                .await?;
        }
        Decision::Absent => {}
    }
    ctx.client
        .upload_asset(ctx.repo, release_id, &name, bytes, permit, ctx.now)
        .await?;
    Ok(())
}

/// The local keyfile's canonical bytes, read through the injected
/// [`SyncRoots`](crate::sync::SyncRoots) like every other collector — never
/// from a resolver that reads a real `$HOME`.
///
/// Round-tripped through [`Keyfile`] rather than hashed as it sits on disk,
/// because the on-disk form is pretty-printed and the asset name addresses the
/// compact one. No password is involved: this reads the wrapped blob and does
/// not open it.
fn canonical_keyfile(ctx: &PushCtx<'_>) -> Result<Vec<u8>> {
    let path = crate::sync::cli::keyfile_path(ctx.roots);
    let raw = std::fs::read(&path).map_err(|e| AppError::io_at(&path, e))?;
    let keyfile: Keyfile = serde_json::from_slice(&raw).map_err(|_| {
        AppError::Other(format!(
            "{} is not a readable sync keyfile, so the bundle's keyfile asset cannot be \
             published. Nothing was uploaded.",
            path.display()
        ))
    })?;
    serde_json::to_vec(&keyfile)
        .map_err(|e| AppError::Other(format!("the sync keyfile could not be serialized: {e}")))
}

/// What the resume scan decided about one asset name.
#[derive(Debug, PartialEq, Eq)]
enum Decision {
    /// Name, size and state all agree — skip it. All three, deliberately:
    /// the name alone is attacker-supplied and a torn upload carries the right
    /// one (T-4-19).
    Present,
    /// Present under the right name but not in the uploaded state, or at the
    /// wrong size. Delete this asset id, then upload.
    Torn(u64),
    Absent,
}

fn decide(existing: &[Asset], name: &str, size: u64) -> Decision {
    match existing.iter().find(|a| a.name == name) {
        None => Decision::Absent,
        Some(a) if a.size == size && a.state == ASSET_STATE_UPLOADED => Decision::Present,
        Some(a) => Decision::Torn(a.id),
    }
}

/// Up to [`MAX_IN_FLIGHT`] uploads outstanding, refilled as each completes.
///
/// A hand-polled window rather than a `JoinSet` — see the module docs for why
/// spawning is not available here. Each slot is one boxed future borrowing the
/// context and the permit; `poll_fn` polls every outstanding one and returns
/// the first that is ready, which is `join_next` without the `'static` bound.
/// The first failure returns, dropping — and so cancelling — the rest.
async fn upload_all(
    ctx: &PushCtx<'_>,
    release_id: u64,
    pending: &[&BuiltPack],
    permit: &gate::Pushing,
    progress: &mut dyn Progress,
) -> Result<()> {
    let mut queued = pending.iter().enumerate();
    let mut in_flight: Vec<InFlight<'_>> = Vec::with_capacity(MAX_IN_FLIGHT);

    loop {
        while in_flight.len() < MAX_IN_FLIGHT {
            let Some((index, pack)) = queued.next() else {
                break;
            };
            in_flight.push((index, Box::pin(upload_one(ctx, release_id, pack, permit))));
        }
        if in_flight.is_empty() {
            return Ok(());
        }

        let (slot, finished) = poll_fn(|cx| {
            for (slot, (_, task)) in in_flight.iter_mut().enumerate() {
                if let Poll::Ready(out) = task.as_mut().poll(cx) {
                    return Poll::Ready((slot, out));
                }
            }
            Poll::Pending
        })
        .await;

        // Removed before the `?`: a future that returned `Ready` must never be
        // polled again, and the early return drops the whole vector anyway.
        let (index, _done) = in_flight.remove(slot);
        finished?;
        let pack = pending[index];
        progress.asset_done(index, &pack_asset_name(&pack.id), pack.bytes.len() as u64);
    }
}

/// One pack: upload it, fetch it back, and prove it is the bytes that were
/// sent.
async fn upload_one(
    ctx: &PushCtx<'_>,
    release_id: u64,
    pack: &BuiltPack,
    permit: &gate::Pushing,
) -> Result<()> {
    let name = pack_asset_name(&pack.id);
    let asset = ctx
        .client
        .upload_asset(
            ctx.repo,
            release_id,
            &name,
            pack.bytes.clone(),
            permit,
            ctx.now,
        )
        .await?;

    let fetched = ctx
        .client
        .download_asset(ctx.repo, asset.id, ctx.now)
        .await?;
    if content_address(&fetched) != pack.id {
        return Err(AppError::Other(format!(
            "the pack uploaded as {name} does not read back as the bytes that were sent. \
             Nothing was published — the snapshot pointer is untouched — and re-running \
             the command re-uploads it. If this repeats, report it at \
             https://github.com/akitaonrails/ai-usagebar/issues."
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SyncConfig;
    use crate::sync::SyncRoots;
    use crate::sync::crypto::{KdfParams, Keyfile, Keys};
    use crate::sync::github::token::TokenSource;
    use crate::sync::github::{Client, Endpoints, RepoRef};
    use crate::sync::index::Index;
    use crate::sync::push::progress::Silent;
    use chrono::{DateTime, Utc};
    use mockito::Matcher;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;
    use zeroize::Zeroizing;

    const TOKEN: &str = "github_pat_fixture_not_a_real_token";
    const RELEASE: u64 = 9;

    const NOW: DateTime<Utc> = match DateTime::from_timestamp(1_700_000_000, 0) {
        Some(t) => t,
        None => panic!("a fixed timestamp"),
    };

    /// Microseconds instead of a gibibyte and 1.5 s. The AUR `check()` runs
    /// these on an installer's machine.
    const CHEAP: KdfParams = KdfParams {
        m_kib: 8,
        t: 1,
        p: 1,
    };

    /// A permit through the only door there is — there is no constructor to
    /// shortcut, so this is also a standing check that the gate clears a
    /// private repository.
    fn permit() -> gate::Pushing {
        let facts = gate::RepoFacts {
            id: 1,
            private: true,
            visibility: "private".into(),
            owner_login: "o".into(),
            owner_id: 7,
            archived: false,
            fork: false,
            admin_permission: false,
        };
        gate::assert_pushable(&facts, &RepoRef::parse("o/n").unwrap(), true, NOW)
            .expect("a private repository clears")
            .0
            .spend(NOW)
            .expect("freshly minted")
    }

    /// Everything a [`PushCtx`] borrows, owned in one place so the context can
    /// be handed out by reference. Every path is under a temp directory: no
    /// test here reads a real `$HOME`.
    struct Local {
        _dir: TempDir,
        roots: SyncRoots,
        cfg: SyncConfig,
        keys: Keys,
        index: Index,
        repo: RepoRef,
        client: Client,
    }

    impl Local {
        fn at(base: &str) -> Self {
            let dir = TempDir::new().unwrap();
            let roots = SyncRoots::at(
                dir.path().join("config.toml"),
                dir.path().to_path_buf(),
                dir.path().join("desktop"),
                dir.path().join("profiles"),
                dir.path().join("claude-home"),
            );
            let index = Index::at(&roots.index_file).unwrap();
            let keys = Keyfile::create_with_floor(b"a-test-passphrase", CHEAP, CHEAP.m_kib)
                .unwrap()
                .1;
            let client = Client::new(
                &Endpoints {
                    api_base: base.into(),
                    uploads_base: base.into(),
                },
                Zeroizing::new(TOKEN.into()),
                TokenSource::Env,
            )
            .unwrap();
            Self {
                _dir: dir,
                roots,
                cfg: SyncConfig::default(),
                keys,
                index,
                repo: RepoRef::parse("o/n").unwrap(),
                client,
            }
        }

        fn ctx(&self) -> PushCtx<'_> {
            PushCtx {
                client: &self.client,
                repo: &self.repo,
                cfg: &self.cfg,
                roots: &self.roots,
                keys: &self.keys,
                kdf: CHEAP,
                index: &self.index,
                repo_id: "github:1".into(),
                keyfile_asset: "keyfile-unset.json".into(),
                previous: None,
                allow_rollback: false,
                now: NOW,
            }
        }
    }

    fn pack(fill: u8, len: usize) -> BuiltPack {
        let bytes = vec![fill; len];
        BuiltPack {
            id: content_address(&bytes),
            bytes,
        }
    }

    fn asset_json(id: u64, name: &str, size: usize, state: &str) -> String {
        format!(
            r#"{{"id":{id},"name":"{name}","size":{size},"state":"{state}",
               "created_at":"2023-11-14T22:13:20Z"}}"#
        )
    }

    /// The resume scan's one `GET`. `body` is the JSON array it answers with.
    async fn mock_listing(server: &mut mockito::Server, body: String) -> mockito::Mock {
        server
            .mock("GET", "/repos/o/n/releases/9/assets")
            .match_query(Matcher::Any)
            .with_status(200)
            .with_body(body)
            .create_async()
            .await
    }

    /// The upload and the verifying download for one pack, wired as a pair so
    /// the download answers with the very bytes that were sent.
    async fn mock_pack(
        server: &mut mockito::Server,
        pack: &BuiltPack,
        asset_id: u64,
    ) -> (mockito::Mock, mockito::Mock) {
        let name = pack_asset_name(&pack.id);
        let upload = server
            .mock("POST", "/repos/o/n/releases/9/assets")
            .match_query(Matcher::UrlEncoded("name".into(), name.clone()))
            .with_status(201)
            .with_body(asset_json(
                asset_id,
                &name,
                pack.bytes.len(),
                ASSET_STATE_UPLOADED,
            ))
            .expect(1)
            .create_async()
            .await;
        let download = server
            .mock(
                "GET",
                format!("/repos/o/n/releases/assets/{asset_id}").as_str(),
            )
            .with_status(200)
            .with_body(pack.bytes.clone())
            .create_async()
            .await;
        (upload, download)
    }

    // ---- the resume scan, D4 ----------------------------------------------

    #[tokio::test]
    async fn an_asset_present_at_a_matching_size_and_state_is_skipped_and_the_rest_upload() {
        let mut server = mockito::Server::new_async().await;
        let packs = [pack(1, 10), pack(2, 20), pack(3, 30)];
        let landed = pack_asset_name(&packs[0].id);
        let _list = mock_listing(
            &mut server,
            format!("[{}]", asset_json(50, &landed, 10, ASSET_STATE_UPLOADED)),
        )
        .await;
        let skipped_upload = server
            .mock("POST", "/repos/o/n/releases/9/assets")
            .match_query(Matcher::UrlEncoded("name".into(), landed))
            .expect(0)
            .create_async()
            .await;
        let (b_up, _b_down) = mock_pack(&mut server, &packs[1], 101).await;
        let (c_up, _c_down) = mock_pack(&mut server, &packs[2], 102).await;

        let local = Local::at(&server.url());
        let sent = run(&local.ctx(), RELEASE, &packs, &permit(), &mut Silent)
            .await
            .unwrap();

        assert_eq!((sent.names.len(), sent.skipped), (2, 1));
        // **F-4.** The names are the ones this run observed itself sending, so
        // the incident path never has to infer them from the remote's clock.
        assert_eq!(
            sent.names,
            vec![pack_asset_name(&packs[1].id), pack_asset_name(&packs[2].id)],
            "and the pack an earlier run landed is not among them"
        );
        // Measured from the packs' own lengths, never projected.
        assert_eq!(
            sent.bytes,
            (packs[1].bytes.len() + packs[2].bytes.len()) as u64
        );
        skipped_upload.assert_async().await;
        b_up.assert_async().await;
        c_up.assert_async().await;
    }

    #[tokio::test]
    async fn an_asset_in_any_other_state_is_deleted_before_the_pack_is_uploaded() {
        let mut server = mockito::Server::new_async().await;
        let packs = [pack(1, 10)];
        let name = pack_asset_name(&packs[0].id);
        let _list = mock_listing(
            &mut server,
            format!("[{}]", asset_json(50, &name, 10, "starter")),
        )
        .await;
        let delete = server
            .mock("DELETE", "/repos/o/n/releases/assets/50")
            .with_status(204)
            .expect(1)
            .create_async()
            .await;
        let (upload, _download) = mock_pack(&mut server, &packs[0], 101).await;

        let local = Local::at(&server.url());
        let sent = run(&local.ctx(), RELEASE, &packs, &permit(), &mut Silent)
            .await
            .unwrap();

        assert_eq!((sent.names.len(), sent.skipped), (1, 0));
        delete.assert_async().await;
        upload.assert_async().await;
    }

    #[tokio::test]
    async fn an_asset_whose_size_disagrees_is_deleted_before_the_pack_is_uploaded() {
        let mut server = mockito::Server::new_async().await;
        let packs = [pack(1, 10)];
        let name = pack_asset_name(&packs[0].id);
        // The state says uploaded, and the name is the content address — only
        // the size disagrees, and that is enough.
        let _list = mock_listing(
            &mut server,
            format!("[{}]", asset_json(50, &name, 7, ASSET_STATE_UPLOADED)),
        )
        .await;
        let delete = server
            .mock("DELETE", "/repos/o/n/releases/assets/50")
            .with_status(204)
            .expect(1)
            .create_async()
            .await;
        let (upload, _download) = mock_pack(&mut server, &packs[0], 101).await;

        let local = Local::at(&server.url());
        run(&local.ctx(), RELEASE, &packs, &permit(), &mut Silent)
            .await
            .unwrap();

        delete.assert_async().await;
        upload.assert_async().await;
    }

    /// T-4-26: assets belonging to other snapshots are prune's business, and
    /// only after a successful flip.
    #[tokio::test]
    async fn an_asset_matching_no_pack_in_this_run_is_left_completely_alone() {
        let mut server = mockito::Server::new_async().await;
        let packs = [pack(1, 10)];
        let stranger = pack_asset_name(&pack(9, 99).id);
        let _list = mock_listing(
            &mut server,
            format!("[{}]", asset_json(50, &stranger, 99, ASSET_STATE_UPLOADED)),
        )
        .await;
        let delete = server
            .mock("DELETE", Matcher::Any)
            .expect(0)
            .create_async()
            .await;
        let (upload, _download) = mock_pack(&mut server, &packs[0], 101).await;

        let local = Local::at(&server.url());
        run(&local.ctx(), RELEASE, &packs, &permit(), &mut Silent)
            .await
            .unwrap();

        delete.assert_async().await;
        upload.assert_async().await;
    }

    // ---- the ceiling on bodies in flight -----------------------------------

    #[derive(Default)]
    struct Flight {
        started: usize,
        finished: usize,
        max: usize,
    }

    /// Which pack an upload request names, by the `name=` in its query.
    ///
    /// The match lives **inside** the mock's own predicate rather than in a
    /// `match_query` beside it, and that is not a style choice: mockito
    /// evaluates every candidate mock's `match_request` closure while it looks
    /// for one that matches, so a counter incremented in a closure that also
    /// relies on a separate matcher counts requests the mock never answered.
    /// One mock, one predicate, and the side effect happens only on a real hit.
    fn upload_of(packs: &[(String, BuiltPack)], path_and_query: &str) -> Option<usize> {
        packs
            .iter()
            .position(|(name, _)| path_and_query.contains(&format!("name={name}")))
    }

    /// Which pack a download request names, by the asset id in its path.
    fn download_of(path: &str) -> Option<usize> {
        let id: u64 = path
            .strip_prefix("/repos/o/n/releases/assets/")?
            .parse()
            .ok()?;
        id.checked_sub(FIRST_ASSET_ID).map(|i| i as usize)
    }

    const FIRST_ASSET_ID: u64 = 200;

    /// Six packs, four at a time. `started - finished` is the number of packs
    /// whose upload has reached the server and whose verifying download has
    /// not, which is exactly the window this function bounds.
    #[tokio::test]
    async fn no_more_than_four_bodies_are_in_flight_at_once() {
        let mut server = mockito::Server::new_async().await;
        let packs: Vec<BuiltPack> = (1..=6u8).map(|i| pack(i, 10 + i as usize)).collect();
        let _list = mock_listing(&mut server, "[]".into()).await;

        let named: Arc<Vec<(String, BuiltPack)>> = Arc::new(
            packs
                .iter()
                .map(|p| (pack_asset_name(&p.id), p.clone()))
                .collect(),
        );
        let flight: Arc<Mutex<Flight>> = Arc::default();

        let counted = Arc::clone(&flight);
        let matching = Arc::clone(&named);
        let bodies = Arc::clone(&named);
        let _upload = server
            .mock("POST", Matcher::Any)
            .match_request(move |req| {
                if upload_of(&matching, req.path_and_query()).is_none() {
                    return false;
                }
                let mut f = counted.lock().unwrap();
                f.started += 1;
                f.max = f.max.max(f.started - f.finished);
                true
            })
            .with_status(201)
            .with_body_from_request(move |req| {
                let i = upload_of(&bodies, req.path_and_query()).expect("matched already");
                let (name, p) = &bodies[i];
                asset_json(
                    FIRST_ASSET_ID + i as u64,
                    name,
                    p.bytes.len(),
                    ASSET_STATE_UPLOADED,
                )
                .into_bytes()
            })
            .create_async()
            .await;

        let counted = Arc::clone(&flight);
        let bodies = Arc::clone(&named);
        let _download = server
            .mock("GET", Matcher::Any)
            .match_request(move |req| match download_of(req.path()) {
                Some(_) => {
                    counted.lock().unwrap().finished += 1;
                    true
                }
                None => false,
            })
            .with_status(200)
            .with_body_from_request(move |req| {
                let i = download_of(req.path()).expect("matched already");
                bodies[i].1.bytes.clone()
            })
            .create_async()
            .await;

        let local = Local::at(&server.url());
        let sent = run(&local.ctx(), RELEASE, &packs, &permit(), &mut Silent)
            .await
            .unwrap();

        assert_eq!(sent.names.len(), 6);
        // The literal, not `MAX_IN_FLIGHT`: asserted against the constant this
        // test passes at any cap, because the observed maximum simply follows
        // it. Raising the ceiling must turn this red and make someone justify
        // the memory (T-4-23).
        assert_eq!(
            flight.lock().unwrap().max,
            4,
            "four bodies concurrent, and never a fifth"
        );
    }

    // ---- verification, D3's precondition for the flip ----------------------

    #[tokio::test]
    async fn a_download_that_reads_back_different_bytes_fails_the_run() {
        let mut server = mockito::Server::new_async().await;
        let packs = [pack(1, 10)];
        let name = pack_asset_name(&packs[0].id);
        let _list = mock_listing(&mut server, "[]".into()).await;
        let _upload = server
            .mock("POST", "/repos/o/n/releases/9/assets")
            .match_query(Matcher::Any)
            .with_status(201)
            .with_body(asset_json(101, &name, 10, ASSET_STATE_UPLOADED))
            .create_async()
            .await;
        let _download = server
            .mock("GET", "/repos/o/n/releases/assets/101")
            .with_status(200)
            .with_body(vec![0xff; 10])
            .create_async()
            .await;

        let local = Local::at(&server.url());
        let err = run(&local.ctx(), RELEASE, &packs, &permit(), &mut Silent)
            .await
            .expect_err("altered bytes must fail the run, so the caller never flips");
        assert!(err.to_string().contains("does not read back"), "{err}");
    }

    // ---- the keyfile asset, without which no second machine can bootstrap --

    impl Local {
        /// Write a cheap keyfile where `cli::keyfile_path` looks for it, and
        /// return its canonical bytes and the asset name they address.
        fn seed_keyfile(&self) -> (Vec<u8>, String) {
            let (keyfile, _) =
                Keyfile::create_with_floor(b"a-test-passphrase", CHEAP, CHEAP.m_kib).unwrap();
            let path = crate::sync::cli::keyfile_path(&self.roots);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            // Pretty-printed, exactly as `setup::write_keyfile` writes it — the
            // asset name addresses the *canonical* form, and a function that
            // hashed the file as it sits would publish a name for bytes it did
            // not upload.
            std::fs::write(&path, serde_json::to_vec_pretty(&keyfile).unwrap()).unwrap();
            let canonical = serde_json::to_vec(&keyfile).unwrap();
            let name = keyfile_asset_name(&content_address(&canonical));
            (canonical, name)
        }
    }

    #[tokio::test]
    async fn a_first_push_uploads_the_keyfile_the_pointer_will_name() {
        let mut server = mockito::Server::new_async().await;
        let _list = mock_listing(&mut server, "[]".into()).await;
        let local = Local::at(&server.url());
        let (canonical, name) = local.seed_keyfile();

        let upload = server
            .mock("POST", "/repos/o/n/releases/9/assets")
            .match_query(Matcher::UrlEncoded("name".into(), name.clone()))
            // The bytes are the keyfile's own, unaltered: nothing here
            // re-wraps, re-derives or re-encrypts.
            .match_body(Matcher::from(canonical.clone()))
            .with_status(201)
            .with_body(asset_json(60, &name, canonical.len(), ASSET_STATE_UPLOADED))
            .expect(1)
            .create_async()
            .await;

        ensure_keyfile(&local.ctx(), RELEASE, &permit())
            .await
            .unwrap();
        upload.assert_async().await;
    }

    /// Idempotent by content address, which is what makes it safe to call on
    /// every push rather than only the first.
    #[tokio::test]
    async fn a_keyfile_already_present_and_uploaded_is_not_uploaded_again() {
        let mut server = mockito::Server::new_async().await;
        let local = Local::at(&server.url());
        let (canonical, name) = local.seed_keyfile();
        let _list = mock_listing(
            &mut server,
            format!(
                "[{}]",
                asset_json(60, &name, canonical.len(), ASSET_STATE_UPLOADED)
            ),
        )
        .await;
        let upload = server
            .mock("POST", "/repos/o/n/releases/9/assets")
            .expect(0)
            .create_async()
            .await;

        ensure_keyfile(&local.ctx(), RELEASE, &permit())
            .await
            .unwrap();
        upload.assert_async().await;
    }

    /// The same zombie the pack scan deletes: GitHub creates the asset record
    /// before the body finishes, and a torn keyfile would otherwise hold the
    /// name forever — leaving the pointer naming an unreadable asset.
    #[tokio::test]
    async fn a_torn_keyfile_asset_is_deleted_before_it_is_uploaded_again() {
        let mut server = mockito::Server::new_async().await;
        let local = Local::at(&server.url());
        let (canonical, name) = local.seed_keyfile();
        let _list = mock_listing(
            &mut server,
            format!("[{}]", asset_json(60, &name, canonical.len(), "starter")),
        )
        .await;
        let delete = server
            .mock("DELETE", "/repos/o/n/releases/assets/60")
            .with_status(204)
            .expect(1)
            .create_async()
            .await;
        let upload = server
            .mock("POST", "/repos/o/n/releases/9/assets")
            .match_query(Matcher::UrlEncoded("name".into(), name.clone()))
            .with_status(201)
            .with_body(asset_json(61, &name, canonical.len(), ASSET_STATE_UPLOADED))
            .expect(1)
            .create_async()
            .await;

        ensure_keyfile(&local.ctx(), RELEASE, &permit())
            .await
            .unwrap();
        delete.assert_async().await;
        upload.assert_async().await;
    }

    /// D7: a 401 retried is a slower failure. `with_retry`'s own arms are
    /// tested in `write.rs`; this asserts the uploader inherits them rather
    /// than adding a second loop.
    #[tokio::test]
    async fn an_unauthorized_upload_fails_on_the_first_attempt() {
        let mut server = mockito::Server::new_async().await;
        let packs = [pack(1, 10)];
        let _list = mock_listing(&mut server, "[]".into()).await;
        let upload = server
            .mock("POST", "/repos/o/n/releases/9/assets")
            .match_query(Matcher::Any)
            .with_status(401)
            .with_body(r#"{"message":"Bad credentials"}"#)
            .expect(1)
            .create_async()
            .await;

        let local = Local::at(&server.url());
        run(&local.ctx(), RELEASE, &packs, &permit(), &mut Silent)
            .await
            .expect_err("a 401 is terminal");
        upload.assert_async().await;
    }
}
