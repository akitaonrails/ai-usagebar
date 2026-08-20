//! Change the sync password without re-uploading the bundle (CRYPTO-04).
//!
//! Plan 4-01 created this file with its frozen signature and the gate.
//! **Plan 4-06 owns it** and fills the four remote steps, in this order and no
//! other: upload the new keyfile, flip the pointer to it, delete the old asset,
//! then re-list to confirm the deletion.
//!
//! # Why that order is the whole safety property
//!
//! If the old asset went first, an interruption between the delete and the flip
//! would leave a pointer naming an asset that no longer exists — the bundle
//! unreadable, with no recovery, from a command whose whole purpose is routine
//! maintenance. Upload-then-flip-then-delete instead means every interruption
//! lands on a bundle that still opens: before the flip the pointer still names
//! the **old** keyfile, which the old password still unwraps; after it, the new
//! one, which the new password does.
//!
//! # What this command is not
//!
//! A rewrap moves the wrapper, not the master key: the three data subkeys are
//! unchanged, so **not one pack byte moves** — that is CRYPTO-04's "without
//! re-uploading the entire bundle" — and it is deliberately **not revocation**.
//! Anyone holding a copy of the old keyfile can still open it with the old
//! password forever. Deleting the remote asset removes the copy *this project
//! published*; it cannot reach one already taken. Real revocation means a new
//! master key and a re-encrypted bundle, which is precisely the whole-bundle
//! re-upload CRYPTO-04 exists to avoid. The `sync rekey` arm in
//! [`crate::sync::cli`] says so on the way in and on the way out, and a test
//! below fails if that sentence is ever dropped.
//!
//! **Nothing under `src/sync/push/` reads a password.** Both arrive as
//! arguments, already `Zeroizing`, because the prompting happens at the CLI,
//! which owns the terminal — never argv and never an environment variable, which
//! is Phase 1's rule and is not relaxed here.

use std::path::Path;

use zeroize::Zeroizing;

use crate::config::SyncCategory;
use crate::error::{AppError, Result};
use crate::sync::crypto::{Keyfile, content_address};
use crate::sync::github::gate;

use super::{Pointer, PushCtx, RELEASE_TAG, keyfile_asset_name, pointer};

/// Rewrap the master key under `new_pw` and republish the keyfile, returning the
/// new keyfile asset's name.
///
/// The sequence, and every step of it is load-bearing:
///
/// 1. **Offline.** Read the local keyfile and [`Keyfile::rewrap`] it — Phase 1's
///    primitive, called and never approximated, because generating a fresh
///    master key here would orphan every pack on the remote and look like
///    success. The old password is verified by *unwrapping*, never by comparing,
///    and a wrong one fails here, before a single request, with Phase 1's one
///    indistinguishable message.
/// 2. **The gate**, before anything leaves the machine. This uploads the wrapped
///    master key, the single most valuable object in the bundle, and a
///    repository can be flipped public from the web UI between `sync setup` and
///    now — so the clearance is re-earned here exactly as a push re-earns it.
/// 3. Upload the new keyfile, flip the pointer to it, delete the old asset,
///    re-list to confirm it is gone.
///
/// No pack is uploaded, downloaded or deleted on any path.
pub async fn run(
    ctx: &PushCtx<'_>,
    old_pw: &Zeroizing<String>,
    new_pw: &Zeroizing<String>,
) -> Result<String> {
    let path = crate::sync::cli::keyfile_path(ctx.roots);
    let raw = std::fs::read(&path).map_err(|e| AppError::io_at(&path, e))?;
    let current: Keyfile = serde_json::from_slice(&raw).map_err(|_| {
        AppError::Other(format!("{} is not a readable sync keyfile", path.display()))
    })?;

    // Phase 1's floor, applied through Phase 1's own function at the parameters
    // this bundle lives at. Not restated here, and not lowered.
    if let crate::sync::passphrase::Strength::Rejected(why) =
        crate::sync::passphrase::check(new_pw, ctx.kdf)
    {
        return Err(AppError::Other(format!(
            "refusing to change the sync password: {why}"
        )));
    }

    // The same 32 bytes, a fresh salt, a new KEK. The subkeys do not move, which
    // is what keeps every existing pack readable.
    let rewrapped = current.rewrap(old_pw.as_bytes(), new_pw.as_bytes(), ctx.kdf)?;
    let body = serde_json::to_vec(&rewrapped)
        .map_err(|e| AppError::Other(format!("the new sync keyfile could not be written: {e}")))?;
    // Content-addressed over exactly the bytes about to be uploaded, so the new
    // asset can never collide with the old one and the two coexist for the
    // instant between the flip and the delete.
    let new_name = keyfile_asset_name(&content_address(&body));

    let permit = super::gate_now(ctx, ctx.cfg.includes(SyncCategory::Credentials)).await?;

    let (previous, sha) = pointer::load(ctx.client, ctx.repo, &ctx.repo_id, ctx.now).await?;
    let Some(previous) = previous else {
        // Nothing is published, so there is no remote wrapper to replace and
        // nothing to flip: uploading a keyfile no pointer names would just be
        // litter. The local half still happens — the password has changed — and
        // the next push publishes this keyfile through the ordinary path.
        write_local(&path, &body)?;
        return Ok(new_name);
    };
    // A rekey carries every arriving snapshot record forward untouched, so a
    // rolled-back pointer handed to it is republished with a fresh valid `sha` —
    // laundered, exactly as on the push path, and the next prune executes it.
    super::assert_no_rollback(ctx, Some(&previous))?;

    let old_name = previous.keyfile.clone();

    let release_id = ctx
        .client
        .ensure_release(ctx.repo, RELEASE_TAG, &permit, ctx.now)
        .await?;

    // 1. The new wrapper exists remotely before anything references it.
    ctx.client
        .upload_asset(
            ctx.repo,
            release_id,
            &new_name,
            body.clone(),
            &permit,
            ctx.now,
        )
        .await?;

    // 2. The flip. `..arriving.clone()` is the point: **only** the keyfile field
    //    moves, so every snapshot record — including a competitor's that landed
    //    while this ran — is carried forward untouched. A rekey changes nothing
    //    about the bundle's contents.
    let keyfile = new_name.clone();
    let rebuild = move |arriving: Option<&Pointer>| -> Result<Pointer> {
        let Some(arriving) = arriving else {
            return Err(AppError::Other(
                "the snapshot pointer disappeared while the password was being changed. \
                 Nothing was flipped and the old keyfile is untouched, so the bundle still \
                 opens under the old password. Re-run `ai-usagebar sync rekey`."
                    .into(),
            ));
        };
        Ok(Pointer {
            keyfile: keyfile.clone(),
            ..arriving.clone()
        })
    };
    pointer::commit(
        ctx.client,
        ctx.repo,
        Some(&previous),
        sha.as_deref(),
        rebuild,
        &permit,
        ctx.now,
    )
    .await?;

    // The local keyfile, only now — and before the delete rather than after it,
    // so that a delete that cannot be confirmed still leaves this machine able
    // to open its own remote bundle. A local keyfile replaced before a failed
    // flip would leave it unable to.
    write_local(&path, &body)?;

    // 3 and 4. D5: verifiably gone, not attempted.
    destroy(ctx, release_id, &old_name, &permit).await?;

    Ok(new_name)
}

/// Delete the old keyfile asset and **confirm** it is gone by re-listing.
///
/// The confirming re-list is D5 and is not optional. Choosing Release assets
/// over git objects was justified partly by this — an asset delete removes the
/// bytes, whereas a git object survives in history and makes "password change" a
/// comforting lie — and that payoff exists only if the deletion is checked. A
/// delete that returns success while the asset survives is a **failure**,
/// reported as one, naming the asset. It is not a warning: this is not prune,
/// where leftover bytes cost storage; here leftover bytes cost the entire point
/// of the command.
///
/// An asset that is already absent is not a failure — the pointer named a
/// keyfile no first push ever uploaded, which is exactly the state this leaves
/// behind anyway.
async fn destroy(
    ctx: &PushCtx<'_>,
    release_id: u64,
    old_name: &str,
    permit: &gate::Pushing,
) -> Result<()> {
    let assets = ctx
        .client
        .list_assets(ctx.repo, release_id, ctx.now)
        .await
        .map_err(|e| survived(old_name, &e.to_string()))?;
    let Some(old) = assets.iter().find(|a| a.name == old_name) else {
        return Ok(());
    };

    ctx.client
        .delete_asset(ctx.repo, old.id, permit, ctx.now)
        .await
        .map_err(|e| survived(old_name, &e.to_string()))?;

    let after = ctx
        .client
        .list_assets(ctx.repo, release_id, ctx.now)
        .await
        .map_err(|e| survived(old_name, &e.to_string()))?;
    if after.iter().any(|a| a.name == old_name) {
        return Err(survived(
            old_name,
            "GitHub accepted the delete and then listed the asset again",
        ));
    }
    Ok(())
}

/// The one message for every way the old wrapper can outlive its replacement.
fn survived(old_name: &str, why: &str) -> AppError {
    AppError::Other(format!(
        "the sync password was changed and the new keyfile is published, but the OLD keyfile \
         asset {old_name} is still reachable: {why}.\n\
         Anyone who can read the repository and knows the old password can still unwrap it. \
         Delete {old_name} by hand from the release's assets on GitHub, then confirm it is \
         gone."
    ))
}

/// Replace the local keyfile atomically, at mode 0600.
///
/// [`crate::cache::atomic_write`] is the project's tempfile-in-the-destination-
/// directory + `persist` helper — never `/tmp`, which is world-readable, often a
/// different filesystem where `persist` degrades to a copy leaving the original
/// behind, and may be tmpfs that survives in swap. The mode is then set
/// explicitly rather than trusting what the temp file inherited, the same
/// belt-and-braces `anchor.rs` and the Settings overlay apply.
fn write_local(path: &Path, body: &[u8]) -> Result<()> {
    crate::cache::atomic_write(path, body)?;
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
    use std::sync::{Arc, Mutex};

    use chrono::{DateTime, Utc};
    use tempfile::TempDir;

    use crate::config::SyncConfig;
    use crate::sync::SyncRoots;
    use crate::sync::crypto::{KdfParams, Keyfile, Keys, MIN_KDF_MEMORY_KIB};
    use crate::sync::github::token::TokenSource;
    use crate::sync::github::{Client, Endpoints, RepoRef};
    use crate::sync::index::Index;
    use crate::sync::push::{POINTER_VERSION, SnapshotRecord};

    const NOW: DateTime<Utc> = match DateTime::from_timestamp(1_700_000_000, 0) {
        Some(t) => t,
        None => panic!("a fixed timestamp"),
    };

    /// 20 characters, because `passphrase::check` rejects anything shorter at a
    /// KDF cost below the shipped default — which every test here runs at.
    const OLD_PW: &str = "old-sync-password-01";
    const NEW_PW: &str = "new-sync-password-02";

    /// The cheapest keyfile the format permits. **Never production parameters:**
    /// the AUR `check()` runs these on an installer's machine.
    fn cheap() -> KdfParams {
        KdfParams {
            m_kib: MIN_KDF_MEMORY_KIB,
            t: 1,
            p: 1,
        }
    }

    fn pw(s: &str) -> Zeroizing<String> {
        Zeroizing::new(s.to_owned())
    }

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

    fn roots_at(dir: &TempDir) -> SyncRoots {
        SyncRoots::at(
            dir.path().join("config.toml"),
            dir.path().to_path_buf(),
            dir.path().join("desktop"),
            dir.path().join("profiles"),
            dir.path().join("claude-home"),
        )
    }

    /// Everything the context borrows, owned in one place so a test can hold it
    /// across the `await`.
    struct Local {
        dir: TempDir,
        roots: SyncRoots,
        cfg: SyncConfig,
        keys: Keys,
        index: Index,
    }

    impl Local {
        /// A temp directory carrying a keyfile wrapped under [`OLD_PW`].
        fn new() -> Self {
            let dir = TempDir::new().unwrap();
            let roots = roots_at(&dir);
            let (keyfile, keys) = Keyfile::create(OLD_PW.as_bytes(), cheap()).unwrap();
            let path = crate::sync::cli::keyfile_path(&roots);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, serde_json::to_vec(&keyfile).unwrap()).unwrap();
            let index = Index::at(&roots.index_file).unwrap();
            Self {
                dir,
                roots,
                cfg: SyncConfig::default(),
                keys,
                index,
            }
        }

        fn keyfile_path(&self) -> std::path::PathBuf {
            crate::sync::cli::keyfile_path(&self.roots)
        }

        fn keyfile(&self) -> Keyfile {
            serde_json::from_slice(&std::fs::read(self.keyfile_path()).unwrap()).unwrap()
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
                keyfile_asset: "keyfile-local.json".into(),
                previous: None,
                allow_rollback: false,
                now: NOW,
            }
        }
    }

    fn published(keyfile: &str) -> Pointer {
        Pointer {
            format: POINTER_VERSION,
            repo_id: "github:1".into(),
            keyfile: keyfile.to_owned(),
            snapshots: vec![SnapshotRecord {
                root: "c2VhbGVk".into(),
                index_chunks: Vec::new(),
                packs: Vec::new(),
            }],
        }
    }

    fn contents_body(pointer: &Pointer) -> String {
        use base64::Engine;
        format!(
            r#"{{"sha":"blob1","content":"{}"}}"#,
            super::super::B64.encode(serde_json::to_vec(pointer).unwrap())
        )
    }

    fn repo_json(private: bool) -> String {
        format!(
            r#"{{"id":1,"private":{private},"visibility":"{}","owner":{{"login":"o","id":7}}}}"#,
            if private { "private" } else { "public" }
        )
    }

    fn asset_json(id: u64, name: &str) -> String {
        format!(
            r#"{{"id":{id},"name":"{name}","size":9,"state":"uploaded",
               "created_at":"2023-11-14T22:13:20Z"}}"#
        )
    }

    /// Every request the run made, in order, as `METHOD path`.
    type Trace = Arc<Mutex<Vec<String>>>;

    fn record(trace: &Trace) -> impl Fn(&mockito::Request) -> bool + Send + Sync + 'static {
        let trace = Arc::clone(trace);
        move |req| {
            trace
                .lock()
                .unwrap()
                .push(format!("{} {}", req.method(), req.path_and_query()));
            true
        }
    }

    /// What the remote does with the delete of the old keyfile asset.
    #[derive(Clone, Copy, PartialEq)]
    enum Deletion {
        /// Accepted, and the confirming re-list agrees.
        Gone,
        /// The request itself fails.
        Refused,
        /// **D5's case**: GitHub answers 204 and the asset is still listed.
        Survives,
    }

    /// The whole remote, wired so the two asset listings differ: the first still
    /// carries the old keyfile, every one after it does not.
    struct Remote {
        server: mockito::ServerGuard,
        trace: Trace,
        _mocks: Vec<mockito::Mock>,
    }

    impl Remote {
        async fn new(private: bool, pointer: Option<&Pointer>, deletion: Deletion) -> Self {
            let mut server = mockito::Server::new_async().await;
            let trace: Trace = Arc::default();
            let mut mocks = Vec::new();

            mocks.push(
                server
                    .mock("GET", "/repos/o/n")
                    .with_status(200)
                    .with_body(repo_json(private))
                    .match_request(record(&trace))
                    .create_async()
                    .await,
            );
            let contents = server.mock("GET", "/repos/o/n/contents/sync/pointer.json");
            mocks.push(
                match pointer {
                    Some(p) => contents.with_status(200).with_body(contents_body(p)),
                    None => contents.with_status(404).with_body(r#"{"message":"gone"}"#),
                }
                .match_request(record(&trace))
                .create_async()
                .await,
            );
            mocks.push(
                server
                    .mock("GET", "/repos/o/n/releases/tags/ai-usagebar-sync-v1")
                    .with_status(200)
                    .with_body(r#"{"id":9}"#)
                    .match_request(record(&trace))
                    .create_async()
                    .await,
            );
            mocks.push(
                server
                    .mock(
                        "POST",
                        mockito::Matcher::Regex(r"/releases/9/assets".into()),
                    )
                    .with_status(201)
                    .with_body(asset_json(42, "keyfile-new.json"))
                    .match_request(record(&trace))
                    .create_async()
                    .await,
            );
            mocks.push(
                server
                    .mock("PUT", "/repos/o/n/contents/sync/pointer.json")
                    .with_status(200)
                    .with_body(r#"{"content":{"sha":"blob2"}}"#)
                    .match_request(record(&trace))
                    .create_async()
                    .await,
            );
            mocks.push(
                server
                    .mock("DELETE", "/repos/o/n/releases/assets/7")
                    .with_status(if deletion == Deletion::Refused {
                        500
                    } else {
                        204
                    })
                    .match_request(record(&trace))
                    .create_async()
                    .await,
            );
            // The listing, which must answer differently before and after the
            // delete — one mock with a call counter rather than two mocks whose
            // matching order would be mockito's business rather than the test's.
            let listed = Arc::new(Mutex::new(0usize));
            let counter = Arc::clone(&listed);
            let survives = deletion != Deletion::Gone;
            mocks.push(
                server
                    .mock("GET", mockito::Matcher::Regex(r"/releases/9/assets".into()))
                    .with_status(200)
                    .with_body_from_request(move |_| {
                        let mut n = counter.lock().unwrap();
                        *n += 1;
                        if *n == 1 || survives {
                            format!("[{}]", asset_json(7, "keyfile-old.json")).into()
                        } else {
                            "[]".into()
                        }
                    })
                    .match_request(record(&trace))
                    .create_async()
                    .await,
            );

            Self {
                server,
                trace,
                _mocks: mocks,
            }
        }

        fn url(&self) -> String {
            self.server.url()
        }

        fn trace(&self) -> Vec<String> {
            self.trace.lock().unwrap().clone()
        }
    }

    /// The proof that the master key did not move: the same plaintext seals to
    /// the same ciphertext under the subkeys the new password opens.
    #[tokio::test]
    async fn a_rewrapped_keyfile_seals_a_chunk_to_the_very_same_bytes() {
        let local = Local::new();
        let plaintext = b"a chunk of a user's config";
        let id = local.keys.chunk_id(plaintext);
        let before = local.keys.seal(&id, plaintext).unwrap();

        let remote = Remote::new(true, Some(&published("keyfile-old.json")), Deletion::Gone).await;
        let client = client_at(&remote.url());
        let repo = repo();
        let name = run(&local.ctx(&client, &repo), &pw(OLD_PW), &pw(NEW_PW))
            .await
            .unwrap();

        let after_keys = local.keyfile().open(NEW_PW.as_bytes()).unwrap();
        assert_eq!(
            after_keys.chunk_id(plaintext),
            id,
            "the name key is the same"
        );
        assert_eq!(
            after_keys.seal(&id, plaintext).unwrap(),
            before,
            "byte for byte — a new master key would change this"
        );
        assert!(name.starts_with("keyfile-"), "{name}");
        // …and the old password no longer opens the local copy.
        assert!(local.keyfile().open(OLD_PW.as_bytes()).is_err());
    }

    /// The local file is replaced atomically at 0600, and only the local file:
    /// no `.tmp.` litter survives in the destination directory.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_local_keyfile_is_replaced_at_mode_0600_leaving_no_temporary_behind() {
        use std::os::unix::fs::PermissionsExt;

        let local = Local::new();
        let remote = Remote::new(true, Some(&published("keyfile-old.json")), Deletion::Gone).await;
        let client = client_at(&remote.url());
        let repo = repo();
        run(&local.ctx(&client, &repo), &pw(OLD_PW), &pw(NEW_PW))
            .await
            .unwrap();

        let path = local.keyfile_path();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "{mode:o}");
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .filter(|n| n.starts_with(".tmp."))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        drop(local.dir);
    }

    /// Upload, flip, delete, confirm — asserted as an **order**, not as a set,
    /// and with the gate's visibility read ahead of all four.
    #[tokio::test]
    async fn the_four_remote_steps_happen_in_that_order_and_no_pack_is_touched() {
        let local = Local::new();
        let remote = Remote::new(true, Some(&published("keyfile-old.json")), Deletion::Gone).await;
        let client = client_at(&remote.url());
        let repo = repo();
        run(&local.ctx(&client, &repo), &pw(OLD_PW), &pw(NEW_PW))
            .await
            .unwrap();

        let trace = remote.trace();
        let at = |needle: &str| {
            trace
                .iter()
                .position(|line| line.contains(needle))
                .unwrap_or_else(|| panic!("{needle} never happened in {trace:?}"))
        };
        let upload = at("POST /repos/o/n/releases/9/assets?name=keyfile-");
        let flip = at("PUT /repos/o/n/contents/sync/pointer.json");
        let delete = at("DELETE /repos/o/n/releases/assets/7");
        let confirm = trace
            .iter()
            .rposition(|line| line.starts_with("GET /repos/o/n/releases/9/assets"))
            .unwrap();

        assert_eq!(at("GET /repos/o/n"), 0, "the gate reads first: {trace:?}");
        assert!(at("GET /repos/o/n") < upload, "{trace:?}");
        assert!(upload < flip, "{trace:?}");
        assert!(flip < delete, "{trace:?}");
        assert!(delete < confirm, "{trace:?}");

        assert!(
            !trace.iter().any(|line| line.contains("pack-")),
            "a rekey moves no pack: {trace:?}"
        );
    }

    /// The password is verified by unwrapping, and a wrong one costs no request
    /// at all — not even the gate's.
    #[tokio::test]
    async fn a_wrong_old_password_fails_before_a_single_request() {
        let local = Local::new();
        let remote = Remote::new(true, Some(&published("keyfile-old.json")), Deletion::Gone).await;
        let client = client_at(&remote.url());
        let repo = repo();

        let err = run(
            &local.ctx(&client, &repo),
            &pw("not-the-old-password"),
            &pw(NEW_PW),
        )
        .await
        .expect_err("that is not the password");

        // Phase 1's single indistinguishable message — the same one a corrupted
        // keyfile gets.
        assert_eq!(err.to_string(), "wrong password or corrupted keyfile");
        assert!(remote.trace().is_empty(), "{:?}", remote.trace());
        // …and the local keyfile still opens under the old password.
        assert!(local.keyfile().open(OLD_PW.as_bytes()).is_ok());
    }

    /// The wrapped master key is the last thing that may reach a public
    /// repository, so the refusal precedes every request carrying a body.
    #[tokio::test]
    async fn a_public_repository_refuses_before_any_keyfile_asset_is_created() {
        let local = Local::new();
        let remote = Remote::new(false, Some(&published("keyfile-old.json")), Deletion::Gone).await;
        let client = client_at(&remote.url());
        let repo = repo();

        let err = run(&local.ctx(&client, &repo), &pw(OLD_PW), &pw(NEW_PW))
            .await
            .expect_err("a public repository is refused");
        assert!(err.to_string().contains("REFUSING TO PUSH"), "{err}");

        let trace = remote.trace();
        assert_eq!(trace, vec!["GET /repos/o/n".to_owned()], "{trace:?}");
        assert!(local.keyfile().open(OLD_PW.as_bytes()).is_ok());
    }

    /// Phase 1's floor, not a second copy of it: at a KDF cost below the shipped
    /// default, `passphrase::check` demands the generator's own length.
    #[tokio::test]
    async fn a_new_password_under_phase_ones_floor_is_refused_before_any_request() {
        let local = Local::new();
        let remote = Remote::new(true, Some(&published("keyfile-old.json")), Deletion::Gone).await;
        let client = client_at(&remote.url());
        let repo = repo();

        let err = run(&local.ctx(&client, &repo), &pw(OLD_PW), &pw("short"))
            .await
            .expect_err("that password is refused");
        assert!(
            err.to_string()
                .contains("refusing to change the sync password"),
            "{err}"
        );
        assert!(remote.trace().is_empty(), "{:?}", remote.trace());
    }

    /// An interruption between the upload and the flip: the pointer still names
    /// the old keyfile, nothing is deleted, and this machine still opens the
    /// bundle under the old password.
    ///
    /// The flip is attempted twice, not once: a 409 is what `pointer::commit`
    /// retries, exactly once, re-running `rebuild` against the winner. The
    /// count is asserted rather than left open so the retry stays bounded.
    #[tokio::test]
    async fn a_run_interrupted_before_the_flip_leaves_the_old_keyfile_published() {
        let local = Local::new();
        let mut server = mockito::Server::new_async().await;
        let trace: Trace = Arc::default();
        let _repo_mock = server
            .mock("GET", "/repos/o/n")
            .with_status(200)
            .with_body(repo_json(true))
            .match_request(record(&trace))
            .create_async()
            .await;
        let _contents = server
            .mock("GET", "/repos/o/n/contents/sync/pointer.json")
            .with_status(200)
            .with_body(contents_body(&published("keyfile-old.json")))
            .match_request(record(&trace))
            .create_async()
            .await;
        let _release = server
            .mock("GET", "/repos/o/n/releases/tags/ai-usagebar-sync-v1")
            .with_status(200)
            .with_body(r#"{"id":9}"#)
            .match_request(record(&trace))
            .create_async()
            .await;
        let _upload = server
            .mock(
                "POST",
                mockito::Matcher::Regex(r"/releases/9/assets".into()),
            )
            .with_status(201)
            .with_body(asset_json(42, "keyfile-new.json"))
            .match_request(record(&trace))
            .create_async()
            .await;
        let flip = server
            .mock("PUT", "/repos/o/n/contents/sync/pointer.json")
            .with_status(409)
            .with_body(r#"{"message":"conflict"}"#)
            .match_request(record(&trace))
            .expect(2)
            .create_async()
            .await;
        let delete = server
            .mock("DELETE", mockito::Matcher::Any)
            .with_status(204)
            .expect(0)
            .create_async()
            .await;

        let client = client_at(&server.url());
        let repo = repo();
        run(&local.ctx(&client, &repo), &pw(OLD_PW), &pw(NEW_PW))
            .await
            .expect_err("the flip failed");

        flip.assert_async().await;
        delete.assert_async().await;
        assert!(
            local.keyfile().open(OLD_PW.as_bytes()).is_ok(),
            "the local keyfile is replaced only after the flip returns"
        );
        assert!(local.keyfile().open(NEW_PW.as_bytes()).is_err());
    }

    /// D5: a delete GitHub accepted while the asset is still listed is a
    /// **failure**, named, and never reported as partial success.
    #[tokio::test]
    async fn an_old_asset_that_outlives_its_delete_is_reported_as_a_failure() {
        let local = Local::new();
        let remote = Remote::new(
            true,
            Some(&published("keyfile-old.json")),
            Deletion::Survives,
        )
        .await;
        let client = client_at(&remote.url());
        let repo = repo();

        let err = run(&local.ctx(&client, &repo), &pw(OLD_PW), &pw(NEW_PW))
            .await
            .expect_err("the old wrapper survived");
        let text = err.to_string();
        assert!(text.contains("keyfile-old.json"), "{text}");
        assert!(text.contains("still reachable"), "{text}");
        assert!(text.contains("old password"), "{text}");
        // …and it is an `Err`, not a partial success: the delete was accepted.
        assert!(
            remote
                .trace()
                .iter()
                .any(|l| l.starts_with("DELETE /repos/o/n/releases/assets/7")),
            "{:?}",
            remote.trace()
        );
        // The flip did land, so the new password is the working one from here.
        assert!(local.keyfile().open(NEW_PW.as_bytes()).is_ok());
    }

    /// The other way the wrapper outlives its replacement: the delete request
    /// itself fails. One message covers both, because the user's position is the
    /// same either way.
    #[tokio::test]
    async fn a_delete_that_the_remote_refuses_is_the_same_failure() {
        let local = Local::new();
        let remote = Remote::new(
            true,
            Some(&published("keyfile-old.json")),
            Deletion::Refused,
        )
        .await;
        let client = client_at(&remote.url());
        let repo = repo();

        let err = run(&local.ctx(&client, &repo), &pw(OLD_PW), &pw(NEW_PW))
            .await
            .expect_err("the delete was refused");
        assert!(err.to_string().contains("keyfile-old.json"), "{err}");
        assert!(err.to_string().contains("still reachable"), "{err}");
    }

    /// A bundle that was never pushed has no published wrapper to replace, so
    /// the remote is left alone entirely and only the local keyfile changes.
    #[tokio::test]
    async fn a_bundle_with_no_pointer_changes_the_local_keyfile_and_nothing_else() {
        let local = Local::new();
        let remote = Remote::new(true, None, Deletion::Gone).await;
        let client = client_at(&remote.url());
        let repo = repo();

        run(&local.ctx(&client, &repo), &pw(OLD_PW), &pw(NEW_PW))
            .await
            .unwrap();

        let trace = remote.trace();
        assert!(
            !trace
                .iter()
                .any(|l| l.starts_with("POST") || l.starts_with("PUT")),
            "{trace:?}"
        );
        assert!(local.keyfile().open(NEW_PW.as_bytes()).is_ok());
    }

    /// CRYPTO-07: no password, no key, and no keyfile byte — nor an
    /// eight-character prefix of one — reaches anything this module renders.
    #[tokio::test]
    async fn nothing_this_module_renders_carries_a_password_or_a_keyfile_byte() {
        let local = Local::new();
        let wrapped = local.keyfile().wrapped_master_key.clone();
        let remote = Remote::new(
            true,
            Some(&published("keyfile-old.json")),
            Deletion::Survives,
        )
        .await;
        let client = client_at(&remote.url());
        let repo = repo();

        let failure = run(&local.ctx(&client, &repo), &pw(OLD_PW), &pw(NEW_PW))
            .await
            .expect_err("the old wrapper survived")
            .to_string();
        let rewrapped = local.keyfile().wrapped_master_key.clone();

        let secrets = [
            OLD_PW,
            NEW_PW,
            &wrapped,
            &rewrapped,
            &wrapped[..8],
            &rewrapped[..8],
            &OLD_PW[..8],
            &NEW_PW[..8],
        ];
        for rendered in [&failure, &survived("keyfile-old.json", "why").to_string()] {
            for secret in secrets {
                assert!(!rendered.contains(secret), "{secret:?} in {rendered}");
            }
        }
    }

    /// The honest message is the `sync rekey` arm's, because that is what owns
    /// the terminal — and it is the whole reason this command is worth shipping
    /// rather than misleading. A later edit that drops it fails here.
    #[test]
    fn the_cli_says_plainly_that_a_password_change_is_not_revocation() {
        let cli = include_str!("../cli.rs");
        let arm = cli
            .split_once("fn rekey(")
            .expect("the rekey arm")
            .1
            .split_once("\nfn ")
            .expect("its end")
            .0;
        assert!(arm.contains("not revocation"), "the rekey arm must say it");
        assert!(
            arm.contains("old password"),
            "…and say what an old keyfile still opens"
        );
    }
}
