//! ROADMAP §Phase 4's success criteria, as named tests driven through the real
//! orchestrator against one stateful `mockito` fake.
//!
//! Five wave-2 plans each shipped one file green in isolation. Every test here
//! calls [`push::run`], `prune::run_on_demand` or `rekey::run` — never a module
//! on its own — because the failure this file exists to catch is the one unit
//! tests structurally cannot: modules that each pass alone and do not compose.
//! Two functions in this phase (`upload::ensure_keyfile` and
//! `progress::reporter`) shipped fully tested with **no call site at all**.
//!
//! Each test is named after the criterion it proves, so a reader comparing the
//! roadmap to the suite can do it by eye.
//!
//! # Hermetic, and it has to be
//!
//! The AUR `check()` runs `cargo test` during `makepkg` on installers'
//! machines. Nothing here reads a real `$HOME`, a real token, the Keychain, or
//! the network: every root comes from a `TempDir`, both `Endpoints` fields
//! point at one mockito server, `now` is a constant, and every keyfile is
//! wrapped at the cheapest KDF parameters the thing under test will accept.
//!
//! # Three fixture traps, each paid for once already
//!
//! 1. **Mockito evaluates every mock's `match_request` against every request
//!    clearing method and path.** Recording state in a matcher therefore counts
//!    requests that mock never answers. All recording here happens inside
//!    `with_body_from_request`, which runs only when the mock actually
//!    responds; matchers only *read* state, never write it.
//! 2. **Where two mocks share a method and a path, their matchers partition the
//!    space** rather than leaning on mockito's preference for a mock that has
//!    not met its expectation. That preference silently retires the first mock
//!    after one hit, which turns "every flip is refused" into "the first flip
//!    is refused" — and the tests that follow then pass for the wrong reason.
//! 3. **Asset ids are handed out per upload, never assumed to be 1.** A push
//!    uploads several assets — 4-02 packs the manifest and the index object
//!    alongside the data chunks, so even a one-file bundle produces multiple
//!    packs — and a fixture that assumes `pack-x.bin` is id 1 silently serves
//!    one pack's bytes back for another's verifying download.
//!
//! Counts that belong to another plan are asserted as **sets**. How many packs
//! a bundle produces is 4-02's business; that every pack the pointer names is
//! present, and that nothing else was destroyed, is this file's.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use base64::Engine;
use chacha20poly1305::XChaCha20Poly1305;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chrono::{DateTime, TimeDelta, Utc};
use serde::Serialize;
use tempfile::TempDir;
use zeroize::Zeroizing;

use ai_usagebar::config::{SyncCategory, SyncConfig};
use ai_usagebar::sync::SyncRoots;
use ai_usagebar::sync::anchor::{self, Anchor};
use ai_usagebar::sync::crypto::{
    ChunkId, KdfDoc, KdfParams, Keyfile, Keys, content_address, derive_kek,
};
use ai_usagebar::sync::github::token::TokenSource;
use ai_usagebar::sync::github::write::ASSET_STATE_UPLOADED;
use ai_usagebar::sync::github::{Client, Endpoints, RepoRef, pairing};
use ai_usagebar::sync::index::Index;
use ai_usagebar::sync::model::Root;
use ai_usagebar::sync::push::progress::Progress;
use ai_usagebar::sync::push::{
    self, PRUNE_GRACE, Pointer, PushCtx, PushOutcome, SnapshotRecord, prune, rekey,
};

const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD;

/// Microseconds instead of ~1.5 s and a gibibyte. Never production parameters:
/// see the module docs.
const CHEAP: KdfParams = KdfParams {
    m_kib: 8,
    t: 1,
    p: 1,
};

/// `crypto::MIN_KDF_MEMORY_KIB`, which is the **lowest** parameters a rekey can
/// run at: `Keyfile::rewrap` enforces the write-path floor and
/// `rewrap_with_floor` is `pub(crate)`, so a rekey test cannot use [`CHEAP`].
/// 8 MiB of Argon2id is milliseconds, which is what keeps that one test cheap.
const FLOOR: KdfParams = KdfParams {
    m_kib: 8 * 1024,
    t: 1,
    p: 1,
};

const PASSWORD: &[u8] = b"correct horse battery staple";
const NEW_PASSWORD: &[u8] = b"a different long enough sync password";

/// Fixed and injected. Nothing here reads the wall clock.
const NOW: DateTime<Utc> = match DateTime::from_timestamp(1_700_000_000, 0) {
    Some(t) => t,
    None => panic!("a fixed timestamp"),
};

/// **GitHub's clock, and it is deliberately not `NOW`.**
///
/// The fake used to stamp every upload at exactly `NOW` — zero skew, the single
/// value that cannot expose a `created_at >= ctx.now` filter. Phase 4's audit
/// found the incident cleanup selecting assets by comparing the remote's clock
/// to this machine's, which deletes nothing whenever the local clock runs a few
/// seconds fast (an incremental push is seconds long) and which a hostile remote
/// disables outright by backdating. Two clocks are never equal; a fixture that
/// pretends they are proves nothing about the code that compares them.
const REMOTE_CLOCK: DateTime<Utc> = match DateTime::from_timestamp(1_700_000_000 - 90, 0) {
    Some(t) => t,
    None => panic!("a fixed timestamp"),
};

/// Not a token. The fixture never reaches a real host, and `Client` only needs
/// something non-empty to put in a header mockito discards.
const TOKEN: &str = "github_pat_fixture_not_a_real_token";

const PRIVATE_BODY: &str = r#"{"id":1,"private":true,"visibility":"private",
    "owner":{"login":"o","id":7},"archived":false,"fork":false}"#;
const PUBLIC_BODY: &str = r#"{"id":1,"private":false,"visibility":"public",
    "owner":{"login":"o","id":7},"archived":false,"fork":false}"#;

/// `sync::cli::keyfile_path` is `pub(crate)`, so its one rule — the keyfile
/// lives beside `config.toml` and is never resolved from `$HOME` — is repeated
/// here rather than imported. A drift between the two shows up as
/// `ensure_keyfile` failing to find a file the fixture just wrote.
fn keyfile_path(roots: &SyncRoots) -> PathBuf {
    roots.config_dir.join("sync").join("keyfile.json")
}

// ---------------------------------------------------------------------------
// The local machine
// ---------------------------------------------------------------------------

/// A keyfile wrapping a fixed master key, assembled from the public fields.
///
/// `Keyfile::create_with_floor` is `pub(crate)` — a memory floor an outside
/// caller passes its own value for is not a floor — so a hermetic suite that
/// must stay in milliseconds builds its own, exactly as `tests/sync_vectors.rs`
/// and `tests/sync_adversarial.rs` already do.
fn wrap_by_hand(seed: u8, pw: &[u8], k: KdfParams) -> Keyfile {
    /// `docs/sync-format.md` §1's `{"format":…,"kdf":{…}}`, in declaration
    /// order, which *is* the canonical AAD byte order.
    #[derive(Serialize)]
    struct KeyfileAad<'a> {
        format: u32,
        kdf: &'a KdfDoc,
    }
    // One fixed nonce is safe only because `seed` picks the salt too, so two
    // keyfiles from this helper are two different KEKs as well as two different
    // master keys.
    const WRAP_NONCE: [u8; 24] = [0x5e; 24];
    const KEYFILE_VERSION: u32 = 1;

    let (master, salt) = ([seed; 32], [seed ^ 0xa5; 16]);
    let kdf = KdfDoc {
        algo: "argon2id".into(),
        version: 19,
        m_kib: k.m_kib,
        t: k.t,
        p: k.p,
        salt: B64.encode(salt),
    };
    let aad = serde_json::to_vec(&KeyfileAad {
        format: KEYFILE_VERSION,
        kdf: &kdf,
    })
    .expect("the AAD serializes");
    let kek = derive_kek(pw, &salt, k).expect("the cheap KDF seam");
    let wrapped = XChaCha20Poly1305::new((&*kek).into())
        .encrypt(
            &WRAP_NONCE.into(),
            Payload {
                msg: &master,
                aad: &aad,
            },
        )
        .expect("wrapping the fixed master key");

    Keyfile {
        format: KEYFILE_VERSION,
        kdf,
        nonce: B64.encode(WRAP_NONCE),
        wrapped_master_key: B64.encode(&wrapped),
    }
}

/// The keyfile's asset name, over the **canonical** serialization — the bytes
/// `ensure_keyfile` and `rekey` upload, not the pretty-printed bytes on disk.
fn keyfile_asset_of(keyfile: &Keyfile) -> String {
    let canonical = serde_json::to_vec(keyfile).expect("a keyfile serializes");
    push::keyfile_asset_name(&content_address(&canonical))
}

/// Everything a push resolves from the local machine, all inside one `TempDir`.
struct Local {
    _dir: TempDir,
    roots: SyncRoots,
    index: Index,
    keys: Keys,
    kdf: KdfParams,
    keyfile_asset: String,
    cfg: SyncConfig,
}

impl Local {
    fn new() -> Local {
        Local::at(CHEAP)
    }

    fn at(kdf: KdfParams) -> Local {
        let dir = TempDir::new().expect("a temp dir");
        let roots = SyncRoots::at(
            dir.path().join("config.toml"),
            dir.path().to_path_buf(),
            dir.path().join("desktop"),
            dir.path().join("profiles"),
            dir.path().join("claude-home"),
        );
        std::fs::create_dir_all(&roots.config_dir).expect("the config dir");
        std::fs::write(&roots.config_file, b"[anthropic]\nenabled = true\n").expect("a config");

        let keyfile = wrap_by_hand(0x11, PASSWORD, kdf);
        let keys = keyfile
            .open(PASSWORD)
            .expect("a hand-wrapped keyfile opens");
        let keyfile_asset = keyfile_asset_of(&keyfile);
        write_keyfile(&roots, &keyfile);

        pairing::write_to(
            &pairing::default_path(&roots),
            &pairing::Pairing {
                repo_id: 1,
                owner_id: 7,
                private: true,
                checked_at: NOW,
            },
        )
        .expect("a pairing record");

        let index = Index::at(&roots.index_file).expect("the local index");
        Local {
            _dir: dir,
            roots,
            index,
            keys,
            kdf,
            keyfile_asset,
            cfg: SyncConfig {
                // `Credentials` is on so the gate takes its strict arm — with it
                // off a public repository *warns and proceeds*, which is D-04's
                // deliberate carve-out and would quietly disarm the incident
                // test. `Config` is the category the fixture actually seeds.
                categories: vec![SyncCategory::Config, SyncCategory::Credentials],
                repo: Some("o/n".into()),
                ..SyncConfig::default()
            },
        }
    }

    /// Seed one file the `Config` collector picks up:
    /// `accounts/<label>/.credentials.json`, the only name that arm retains.
    fn seed(&self, label: &str, bytes: &[u8]) {
        let at = self
            .roots
            .config_dir
            .join("accounts")
            .join(label)
            .join(".credentials.json");
        std::fs::create_dir_all(at.parent().expect("a parent")).expect("the account dir");
        std::fs::write(&at, bytes).expect("a seeded file");
    }

    fn ctx<'a>(&'a self, client: &'a Client, repo: &'a RepoRef) -> PushCtx<'a> {
        PushCtx {
            client,
            repo,
            cfg: &self.cfg,
            roots: &self.roots,
            keys: &self.keys,
            kdf: self.kdf,
            index: &self.index,
            repo_id: push::repo_id_for(1),
            keyfile_asset: self.keyfile_asset.clone(),
            // Filled by `push::run` from the remote, after the gate. A caller
            // that populated it would have had to request before the gate.
            previous: None,
            allow_rollback: false,
            now: NOW,
        }
    }

    /// How many distinct chunks the current tree plans to.
    fn chunk_count(&self) -> usize {
        let plan = ai_usagebar::sync::plan::build_with_keys(
            &self.roots,
            &self.cfg,
            &self.index,
            NOW,
            &self.keys,
        )
        .expect("the plan builds");
        plan.file_plans
            .iter()
            .flat_map(|f| f.chunk_ids.iter().copied())
            .collect::<BTreeSet<_>>()
            .len()
    }
}

fn write_keyfile(roots: &SyncRoots, keyfile: &Keyfile) {
    let at = keyfile_path(roots);
    std::fs::create_dir_all(at.parent().expect("a parent")).expect("the keyfile dir");
    // Pretty-printed, exactly as `sync setup` writes it — so a fixture whose
    // asset name comes from the *canonical* form stays honest about the
    // difference `ensure_keyfile` turns on.
    std::fs::write(&at, serde_json::to_vec_pretty(keyfile).expect("json")).expect("the keyfile");
}

/// Distinct, moderately compressible bytes — a stand-in for the JSON this
/// bundle actually carries.
///
/// `seed` makes every file unique and the counter makes every 256 KiB *chunk*
/// unique. Identical chunks deduplicate, and a fixture whose chunks all
/// collapsed into one would prove nothing about how many packs a bundle needs.
fn payload(seed: u64, len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len + 64);
    let mut n = seed.wrapping_add(1);
    while out.len() < len {
        n = n
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        out.extend_from_slice(format!("{{\"k\":{n},\"v\":\"{:016x}\"}}\n", n >> 17).as_bytes());
    }
    out.truncate(len);
    out
}

// ---------------------------------------------------------------------------
// The remote
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct StoredAsset {
    id: u64,
    name: String,
    bytes: Vec<u8>,
    state: String,
    created_at: DateTime<Utc>,
}

/// One release, one pointer blob, and a log of everything asked of it.
#[derive(Default)]
struct RemoteState {
    assets: Vec<StoredAsset>,
    next_asset_id: u64,
    next_sha: u64,
    /// The bytes actually stored at `sync/pointer.json`, with its blob `sha`.
    pointer: Option<(String, Vec<u8>)>,
    /// `METHOD path`, one line per request a mock actually answered.
    requests: Vec<String>,
    /// Every asset name that ever landed, including ones since deleted — the
    /// cumulative history SYNC-07 measures the live set against.
    ever_uploaded: BTreeSet<String>,
    deleted: Vec<String>,
    /// How many times the visibility has been read.
    gate_reads: usize,
    /// The repository reads readable from this many visibility reads on.
    public_after: Option<usize>,
    /// Every pointer `PUT` is refused outright — the stand-in for a process
    /// killed after the uploads and before the flip.
    refuse_put: bool,
    /// The next pointer `PUT` answers 409, and this pointer becomes current.
    conflict_with: Option<Vec<u8>>,
    /// Assets uploaded from here on are stamped with this instead of
    /// [`REMOTE_CLOCK`].
    upload_clock: Option<DateTime<Utc>>,
}

impl RemoteState {
    fn note(&mut self, method: &str, path: &str) {
        self.requests.push(format!("{method} {path}"));
    }

    fn asset_json(a: &StoredAsset) -> String {
        format!(
            r#"{{"id":{},"name":"{}","size":{},"state":"{}","created_at":"{}"}}"#,
            a.id,
            a.name,
            a.bytes.len(),
            a.state,
            a.created_at.to_rfc3339()
        )
    }

    fn live_names(&self) -> BTreeSet<String> {
        self.assets.iter().map(|a| a.name.clone()).collect()
    }

    fn pointer_value(&self) -> Option<Pointer> {
        self.pointer
            .as_ref()
            .map(|(_, body)| serde_json::from_slice(body).expect("a pointer the fake stored"))
    }

    fn uploads(&self) -> usize {
        self.requests
            .iter()
            .filter(|r| r.starts_with("POST"))
            .count()
    }

    /// Put an asset on the release without an upload — used to age an orphan
    /// past `PRUNE_GRACE` and to seed a competitor's pack.
    fn plant(&mut self, name: &str, bytes: Vec<u8>, created_at: DateTime<Utc>) -> u64 {
        self.next_asset_id += 1;
        let id = self.next_asset_id;
        self.ever_uploaded.insert(name.to_owned());
        self.assets.push(StoredAsset {
            id,
            name: name.to_owned(),
            bytes,
            state: ASSET_STATE_UPLOADED.into(),
            created_at,
        });
        id
    }

    fn store_pointer(&mut self, body: Vec<u8>) -> String {
        self.next_sha += 1;
        let sha = format!("blob{}", self.next_sha);
        self.pointer = Some((sha.clone(), body));
        sha
    }
}

type Shared = Arc<Mutex<RemoteState>>;

/// A mockito server wired to one [`RemoteState`], serving the whole outbound
/// surface: the visibility read, the release, the asset listing, uploads,
/// verifying downloads, deletes, and both halves of the pointer.
struct Remote {
    server: mockito::ServerGuard,
    state: Shared,
}

impl Remote {
    async fn new() -> Remote {
        let mut server = mockito::Server::new_async().await;
        let state: Shared = Arc::default();

        // The gate, read twice per push and once per prune or rekey.
        let s = Arc::clone(&state);
        server
            .mock("GET", "/repos/o/n")
            .with_status(200)
            .with_body_from_request(move |_| {
                let mut st = s.lock().expect("the fake's lock");
                st.note("GET", "/repos/o/n");
                st.gate_reads += 1;
                let public = st.public_after.is_some_and(|n| st.gate_reads > n);
                if public { PUBLIC_BODY } else { PRIVATE_BODY }.into()
            })
            .create_async()
            .await;

        // The one release every asset hangs off.
        let s = Arc::clone(&state);
        server
            .mock("GET", "/repos/o/n/releases/tags/ai-usagebar-sync-v1")
            .with_status(200)
            .with_body_from_request(move |_| {
                s.lock().expect("lock").note("GET", "/releases/tags");
                br#"{"id":9}"#.to_vec()
            })
            .create_async()
            .await;

        // The asset listing, paginated exactly as `list_assets` expects: a
        // short page ends the loop, so a fake that ignored `page` would spin
        // forever on a release holding a multiple of `per_page` assets.
        let s = Arc::clone(&state);
        server
            .mock("GET", mockito::Matcher::Regex("/releases/9/assets".into()))
            .with_status(200)
            .with_body_from_request(move |req| {
                let mut st = s.lock().expect("lock");
                st.note("GET", "/releases/9/assets");
                let page = query_num(req.path_and_query(), "page").unwrap_or(1).max(1);
                let per = query_num(req.path_and_query(), "per_page")
                    .unwrap_or(100)
                    .max(1);
                let rows: Vec<String> = st
                    .assets
                    .iter()
                    .skip(((page - 1) * per) as usize)
                    .take(per as usize)
                    .map(RemoteState::asset_json)
                    .collect();
                format!("[{}]", rows.join(",")).into_bytes()
            })
            .create_async()
            .await;

        // Uploads. Ids are handed out here, per upload — never assumed.
        let s = Arc::clone(&state);
        server
            .mock("POST", mockito::Matcher::Regex("/releases/9/assets".into()))
            .with_status(201)
            .with_body_from_request(move |req| {
                let name = asset_name_in(req.path_and_query());
                let bytes = req.body().expect("an upload has a body").clone();
                let mut st = s.lock().expect("lock");
                st.note("POST", "/releases/9/assets");
                let at = st.upload_clock.unwrap_or(REMOTE_CLOCK);
                let id = st.plant(&name, bytes, at);
                let stored = st
                    .assets
                    .iter()
                    .find(|a| a.id == id)
                    .expect("just planted")
                    .clone();
                RemoteState::asset_json(&stored).into_bytes()
            })
            .create_async()
            .await;

        // The verifying download D3 makes a precondition of the flip. Serves
        // back what that *id* holds, which is the whole reason ids are handed
        // out per upload rather than assumed.
        let s = Arc::clone(&state);
        server
            .mock(
                "GET",
                mockito::Matcher::Regex(r"/releases/assets/\d+$".into()),
            )
            .with_status(200)
            .with_body_from_request(move |req| {
                let id = asset_id_in(req.path());
                let mut st = s.lock().expect("lock");
                st.note("GET", "/releases/assets/{id}");
                st.assets
                    .iter()
                    .find(|a| a.id == id)
                    .map(|a| a.bytes.clone())
                    .unwrap_or_default()
            })
            .create_async()
            .await;

        let s = Arc::clone(&state);
        server
            .mock(
                "DELETE",
                mockito::Matcher::Regex(r"/releases/assets/\d+$".into()),
            )
            .with_status(204)
            .with_body_from_request(move |req| {
                let id = asset_id_in(req.path());
                let mut st = s.lock().expect("lock");
                st.note("DELETE", "/releases/assets/{id}");
                if let Some(at) = st.assets.iter().position(|a| a.id == id) {
                    let gone = st.assets.remove(at);
                    st.deleted.push(gone.name);
                }
                Vec::new()
            })
            .create_async()
            .await;

        // The pointer read, in two mocks because a status code is fixed per
        // mock. The matchers are mutually exclusive and only *read* state.
        let s = Arc::clone(&state);
        server
            .mock("GET", "/repos/o/n/contents/sync/pointer.json")
            .match_request(move |_| s.lock().expect("lock").pointer.is_none())
            .with_status(404)
            .with_body_from_request({
                let s = Arc::clone(&state);
                move |_| {
                    s.lock().expect("lock").note("GET", "/contents/pointer");
                    br#"{"message":"Not Found"}"#.to_vec()
                }
            })
            .create_async()
            .await;
        let s = Arc::clone(&state);
        server
            .mock("GET", "/repos/o/n/contents/sync/pointer.json")
            .match_request(move |_| s.lock().expect("lock").pointer.is_some())
            .with_status(200)
            .with_body_from_request({
                let s = Arc::clone(&state);
                move |_| {
                    let mut st = s.lock().expect("lock");
                    st.note("GET", "/contents/pointer");
                    let (sha, body) = st.pointer.clone().expect("the matcher checked");
                    format!(r#"{{"sha":"{sha}","content":"{}"}}"#, B64.encode(&body)).into_bytes()
                }
            })
            .create_async()
            .await;

        // The flip, in three mocks for the same reason — and their matchers
        // partition the space rather than relying on mockito's preference for
        // an unsatisfied mock, which would retire the refusal after one hit.
        let s = Arc::clone(&state);
        server
            .mock("PUT", "/repos/o/n/contents/sync/pointer.json")
            .match_request(move |_| s.lock().expect("lock").refuse_put)
            .with_status(403)
            .with_body_from_request({
                let s = Arc::clone(&state);
                move |_| {
                    s.lock().expect("lock").note("PUT", "/contents/pointer 403");
                    br#"{"message":"Resource not accessible by personal access token"}"#.to_vec()
                }
            })
            .create_async()
            .await;
        let s = Arc::clone(&state);
        server
            .mock("PUT", "/repos/o/n/contents/sync/pointer.json")
            .match_request(move |_| {
                let st = s.lock().expect("lock");
                !st.refuse_put && st.conflict_with.is_some()
            })
            .with_status(409)
            .with_body_from_request({
                let s = Arc::clone(&state);
                move |_| {
                    // The competitor lands here, in the responder rather than in
                    // the matcher: this runs exactly when the 409 is served, so
                    // the re-read that follows sees the winner.
                    let mut st = s.lock().expect("lock");
                    st.note("PUT", "/contents/pointer 409");
                    let winner = st.conflict_with.take().expect("the matcher checked");
                    st.store_pointer(winner);
                    br#"{"message":"is at abc but expected def"}"#.to_vec()
                }
            })
            .create_async()
            .await;
        let s = Arc::clone(&state);
        server
            .mock("PUT", "/repos/o/n/contents/sync/pointer.json")
            .match_request({
                let s = Arc::clone(&state);
                move |_| {
                    let st = s.lock().expect("lock");
                    !st.refuse_put && st.conflict_with.is_none()
                }
            })
            .with_status(201)
            .with_body_from_request(move |req| {
                let body: serde_json::Value =
                    serde_json::from_slice(&req.body().expect("a PUT has a body").clone())
                        .expect("the pointer PUT is json");
                let content = B64
                    .decode(body["content"].as_str().expect("content is base64"))
                    .expect("valid base64");
                let mut st = s.lock().expect("lock");
                st.note("PUT", "/contents/pointer");
                let sha = st.store_pointer(content);
                format!(r#"{{"content":{{"sha":"{sha}"}}}}"#).into_bytes()
            })
            .create_async()
            .await;

        Remote { server, state }
    }

    fn client(&self) -> Client {
        Client::new(
            &Endpoints {
                api_base: self.server.url(),
                uploads_base: self.server.url(),
            },
            Zeroizing::new(TOKEN.into()),
            TokenSource::Env,
        )
        .expect("a client over the fake")
    }

    fn with<T>(&self, f: impl FnOnce(&mut RemoteState) -> T) -> T {
        f(&mut self.state.lock().expect("the fake's lock"))
    }
}

fn repo() -> RepoRef {
    RepoRef::parse("o/n").expect("a well-formed repo")
}

/// `…?name=pack-<hex>.bin` — the name GitHub assigns is the one the caller
/// asked for, so the fake echoes it rather than inventing one.
fn asset_name_in(path_and_query: &str) -> String {
    path_and_query
        .split_once("name=")
        .map(|(_, rest)| rest.split('&').next().unwrap_or(rest).to_owned())
        .unwrap_or_else(|| panic!("an upload names its asset: {path_and_query}"))
}

fn asset_id_in(path: &str) -> u64 {
    path.rsplit('/')
        .next()
        .and_then(|id| id.parse().ok())
        .unwrap_or_else(|| panic!("an asset path ends in its id: {path}"))
}

fn query_num(path_and_query: &str, key: &str) -> Option<u64> {
    path_and_query
        .split(['?', '&'])
        .find_map(|part| part.strip_prefix(key)?.strip_prefix('=')?.parse().ok())
}

// ---------------------------------------------------------------------------
// Progress
// ---------------------------------------------------------------------------

/// Records what a real reporter would render, so UX-04 is asserted rather than
/// looked at.
#[derive(Default)]
struct Recording {
    start: Option<(usize, u64)>,
    /// The running byte total after each completed asset.
    bytes_after: Vec<u64>,
    /// The zero-based indices reported, in arrival order.
    indices: Vec<usize>,
    finished: usize,
}

impl Progress for Recording {
    fn start(&mut self, assets: usize, total_bytes: u64) {
        self.start = Some((assets, total_bytes));
    }
    fn asset_done(&mut self, index: usize, _name: &str, bytes: u64) {
        self.indices.push(index);
        let running = self.bytes_after.last().copied().unwrap_or(0) + bytes;
        self.bytes_after.push(running);
    }
    fn finish(&mut self) {
        self.finished += 1;
    }
}

/// One push through the real orchestrator.
async fn push(local: &Local, remote: &Remote) -> ai_usagebar::error::Result<PushOutcome> {
    let client = remote.client();
    let repo = repo();
    push::run(local.ctx(&client, &repo), &mut Recording::default()).await
}

/// One push through the real orchestrator, with `--allow-rollback`.
async fn push_allowing_rollback(
    local: &Local,
    remote: &Remote,
) -> ai_usagebar::error::Result<PushOutcome> {
    let client = remote.client();
    let repo = repo();
    let mut ctx = local.ctx(&client, &repo);
    ctx.allow_rollback = true;
    push::run(ctx, &mut Recording::default()).await
}

/// The counter sealed inside every snapshot root the pointer carries, in
/// pointer order. Read through the format's own reader, which is the only thing
/// that makes a counter meaningful.
fn counters(pointer: &Pointer, local: &Local) -> Vec<u64> {
    pointer
        .snapshots
        .iter()
        .map(|s| {
            let framed = B64.decode(&s.root).expect("a base64 root");
            Root::open(&local.keys, &framed, &push::repo_id_for(1))
                .expect("a root this bundle's keys open")
                .counter
        })
        .collect()
}

/// Every pack asset name the pointer's snapshots reference.
fn referenced(pointer: &Pointer) -> BTreeSet<String> {
    pointer
        .snapshots
        .iter()
        .flat_map(|s| s.packs.iter())
        .map(push::pack_asset_name)
        .collect()
}

// ---------------------------------------------------------------------------
// Criterion 1 — one upload per pack, never one per chunk
// ---------------------------------------------------------------------------

/// **ROADMAP §Phase 4 success criterion 1** — "a first push issues one upload
/// request per pack, never one per chunk; a bundle of ~5,000 chunks completes
/// in under 10 HTTP requests total".
///
/// The first half is asserted exactly. The second is asserted **as measured**,
/// because the roadmap's two numbers cannot both be met by the shipped protocol
/// and the code is what wins:
///
/// - 5,000 chunks is 1.25 GiB at `CHUNK_SIZE`. Writing that during `makepkg`'s
///   `check()` is not acceptable, and making it compressible enough to avoid
///   writing it collapses the bundle into one pack whose header — still a
///   *single* sealed chunk, which gap-closure 1-09 deliberately did not reach —
///   passes `CHUNK_SIZE` at roughly 2,400 entries.
/// - "under 10 requests" is at the protocol's own floor. A first push spends
///   **nine** requests that have nothing to do with how much data there is: two
///   visibility reads (the gate and the re-gate), one pointer read, one release
///   lookup, three asset listings (the resume scan, `ensure_keyfile`'s, and the
///   prune's), the keyfile upload, and the flip — plus one upload and one
///   verifying download per pack. That download is D3's precondition for the
///   flip and is not optional. So "under 10" holds only for a bundle of zero
///   packs, which is not a bundle.
///
/// What REPO-06 actually requires is the ratio — "a small number of large
/// objects, never one request per chunk" — and the ratio is what is asserted.
#[tokio::test]
async fn a_first_push_issues_one_upload_per_pack_and_never_one_per_chunk() {
    let local = Local::new();
    // Twelve files of 4 MiB — ~190 chunks at 256 KiB. Enough that a per-chunk
    // protocol would be an order of magnitude louder, cheap enough to run
    // during an AUR install.
    for i in 0..12u64 {
        local.seed(&format!("acct{i}"), &payload(i, 4 * 1024 * 1024));
    }
    let chunks = local.chunk_count();
    let remote = Remote::new().await;

    let outcome = push(&local, &remote).await.expect("the push succeeds");
    let (requests, uploads, log) =
        remote.with(|st| (st.requests.len(), st.uploads(), st.requests.clone()));

    assert!(
        chunks > 150,
        "the fixture must hold many chunks or the ratio proves nothing: {chunks}"
    );
    // One upload per pack, plus exactly one keyfile — `ensure_keyfile`
    // publishes the wrapped master key, and it does so **after** the re-gate.
    assert_eq!(
        outcome.packs_uploaded + 1,
        uploads,
        "one upload per pack plus the keyfile, never one per chunk"
    );
    assert!(
        requests * 8 < chunks,
        "REPO-06: {requests} requests for {chunks} chunks is not 'a small number of large \
         objects'\n{log:#?}"
    );
    // The absolute number, pinned exactly rather than merely bounded: Phase 5
    // needs to know what one exchange with the remote costs, and a protocol
    // that grew a round trip per push should have to say so here rather than
    // sliding under an inequality.
    assert_eq!(
        requests,
        9 + 2 * outcome.packs_uploaded,
        "a first push of {} pack(s) took {requests} requests. The shape is nine fixed requests \
         plus an upload and a verifying download per pack; if that changed, say which request \
         was added or removed and why.\n{log:#?}",
        outcome.packs_uploaded
    );
}

// ---------------------------------------------------------------------------
// Criteria 2 and 3 — the kill, and the resume
// ---------------------------------------------------------------------------

/// A push that lands is recorded, so `sync status` can answer "last sync".
///
/// `Index::set_last_sync` shipped with **no production caller at all** — written,
/// tested, and never invoked — so `sync status` said `never` after every
/// successful push. Found by using the tool, not by any check: four real pushes
/// to a real repository all reported `never`.
#[tokio::test]
async fn a_landed_push_records_when_it_landed() {
    let local = Local::new();
    local.seed("first", &payload(1, 1024));
    let remote = Remote::new().await;

    assert!(
        Index::at(&local.roots.index_file)
            .unwrap()
            .last_sync()
            .is_none(),
        "nothing has landed yet"
    );
    push(&local, &remote).await.expect("the push lands");
    assert_eq!(
        Index::at(&local.roots.index_file).unwrap().last_sync(),
        Some(NOW),
        "a landed push records the run's own clock"
    );
}

/// **ROADMAP §Phase 4 success criteria 2 and 3**, together, because they are
/// one property: the flip is the only commit point, and what landed before it
/// is reused rather than re-sent. This is the most important test in the file.
///
/// # Why one refused push is enough, and once was not
///
/// `plan::build` emits `file_plans` in **two passes** — every file the index
/// already knows, then every file that changed — so the order used to depend on
/// what happened to be cached. The manifest is built from that list and travels
/// inside a pack, so a run that ordered the files differently sealed packs with
/// different content addresses, and the first re-run after an interruption
/// reused **nothing**; only the second re-run onwards reused anything.
///
/// `plan::build` now sorts each category's file plans by path, so the addresses
/// depend on what is on disk rather than on what was cached — or on the order
/// the filesystem happened to enumerate. This test asserts full reuse on the
/// **first** resume, which is what makes it a regression test for that.
#[tokio::test]
async fn a_push_killed_before_the_flip_leaves_the_previous_pointer_byte_identical() {
    let local = Local::new();
    local.seed("first", &payload(1, 2 * 1024 * 1024));
    let remote = Remote::new().await;

    push(&local, &remote).await.expect("the first push lands");
    let settled = remote.with(|st| st.pointer.clone().expect("a pointer was published"));

    // A second push with new data, refused at the flip. Everything above the
    // `PUT` still happens: the packs upload and verify.
    local.seed("second", &payload(2, 2 * 1024 * 1024));
    remote.with(|st| st.refuse_put = true);
    let err = push(&local, &remote)
        .await
        .expect_err("a refused flip is a failed push");
    assert!(
        !err.to_string().is_empty(),
        "every failure path carries a message"
    );

    assert_eq!(
        settled,
        remote.with(|st| st.pointer.clone().expect("still published")),
        "SYNC-04: the previous pointer is byte-identical after a killed push"
    );
    // …and every pack the surviving pointer names is still there, uploaded.
    let pointer: Pointer = serde_json::from_slice(&settled.1).expect("a stored pointer");
    remote.with(|st| {
        for name in referenced(&pointer) {
            let asset = st
                .assets
                .iter()
                .find(|a| a.name == name)
                .unwrap_or_else(|| panic!("{name} is referenced by the pointer but absent"));
            assert_eq!(asset.state, ASSET_STATE_UPLOADED, "{name}");
        }
    });

    let landed_before = remote.with(|st| st.live_names());

    // Criterion 3: the resume. Nothing goes back on the wire but the flip.
    remote.with(|st| {
        st.refuse_put = false;
        st.requests.clear();
    });
    let resumed = push(&local, &remote).await.expect("the resume lands");

    assert_eq!(
        resumed.packs_uploaded, 0,
        "SYNC-05: a resume re-uploads only what was missing, and nothing was"
    );
    assert!(
        resumed.packs_skipped > 0,
        "and it skipped by name, size and state rather than re-sending"
    );
    assert_eq!(
        remote.with(|st| st.uploads()),
        0,
        "not one asset body crossed the wire on the resume"
    );

    let final_pointer = remote.with(|st| st.pointer_value().expect("published"));
    let live = remote.with(|st| st.live_names());
    for name in referenced(&final_pointer) {
        assert!(
            live.contains(&name),
            "{name} is referenced by the pointer that landed but is not on the release"
        );
    }
    assert!(
        !referenced(&final_pointer).is_disjoint(&landed_before),
        "the packs the killed runs uploaded are the ones the resume published"
    );
}

/// **ROADMAP §Phase 4 success criterion 3**, the torn-upload half.
///
/// GitHub creates the asset record before the body finishes, so an interrupted
/// upload leaves a zombie whose name would collide forever. A resume deletes it
/// and re-uploads; it never skips on the name alone.
#[tokio::test]
async fn a_resume_deletes_a_torn_asset_rather_than_skipping_on_its_name() {
    let local = Local::new();
    local.seed("only", &payload(3, 1024 * 1024));
    let remote = Remote::new().await;

    // One refused push, so the next run rebuilds exactly the packs already on
    // the release. It takes one rather than two because `plan::build` sorts each
    // category's file plans by path — see the test above.
    remote.with(|st| st.refuse_put = true);
    push(&local, &remote).await.expect_err("refused");

    let torn = remote.with(|st| {
        let asset = st
            .assets
            .iter_mut()
            .find(|a| a.name.starts_with("pack-"))
            .expect("the refused runs uploaded packs");
        asset.state = "starter".into();
        asset.name.clone()
    });
    remote.with(|st| {
        st.refuse_put = false;
        st.deleted.clear();
        st.requests.clear();
    });

    push(&local, &remote).await.expect("the resume lands");

    remote.with(|st| {
        assert!(
            st.deleted.contains(&torn),
            "the zombie is deleted before the retry: {:?}",
            st.deleted
        );
        assert!(
            st.assets
                .iter()
                .any(|a| a.name == torn && a.state == ASSET_STATE_UPLOADED),
            "and it comes back in the uploaded state"
        );
        assert_eq!(st.uploads(), 1, "only the torn asset went back on the wire");
    });
}

// ---------------------------------------------------------------------------
// Criterion 4 — the stale-sha 409
// ---------------------------------------------------------------------------

/// **ROADMAP §Phase 4 success criterion 4.**
///
/// A 409 re-reads, re-plans against whoever won, and lands a pointer carrying
/// **both** machines' snapshot records. The prune that immediately follows runs
/// against what landed, so none of the competitor's packs is deleted — asserted
/// through the real orchestrator rather than through `plan_deletions`.
#[tokio::test]
async fn a_stale_sha_conflict_re_plans_and_the_prune_spares_the_competitor() {
    let local = Local::new();
    local.seed("mine", &payload(5, 1024 * 1024));
    let remote = Remote::new().await;

    push(&local, &remote).await.expect("the first push lands");
    let published = remote.with(|st| st.pointer_value().expect("published"));

    // A competing machine's snapshot: a record naming a pack only it uploaded,
    // aged well past the grace window so nothing but the pointer protects it.
    let rival_pack = ChunkId::from_bytes([0x7c; 32]);
    let rival_name = push::pack_asset_name(&rival_pack);
    let mut competitor = published.clone();
    competitor.snapshots.push(SnapshotRecord {
        root: B64.encode(b"a competitor's sealed root"),
        index_chunks: Vec::new(),
        packs: vec![rival_pack],
    });
    remote.with(|st| {
        st.plant(
            &rival_name,
            b"the rival's pack".to_vec(),
            NOW - TimeDelta::days(30),
        );
        st.conflict_with = Some(serde_json::to_vec(&competitor).expect("json"));
        st.requests.clear();
        st.deleted.clear();
    });

    local.seed("mine2", &payload(6, 1024 * 1024));
    let outcome = push(&local, &remote)
        .await
        .expect("the conflict is survived");

    let (puts, reads) = remote.with(|st| {
        (
            st.requests.iter().filter(|r| r.starts_with("PUT")).count(),
            st.requests
                .iter()
                .filter(|r| r.as_str() == "GET /contents/pointer")
                .count(),
        )
    });
    assert_eq!(puts, 2, "one conflict, one bounded retry, and no third");
    assert!(
        reads >= 2,
        "the conflict re-reads before it re-plans: {reads}"
    );

    let landed = remote.with(|st| st.pointer_value().expect("published"));
    let roots: BTreeSet<&str> = landed.snapshots.iter().map(|s| s.root.as_str()).collect();
    assert!(
        roots.contains(B64.encode(b"a competitor's sealed root").as_str()),
        "the competitor's record is carried forward, never dropped"
    );
    assert!(
        landed.snapshots.len() > competitor.snapshots.len(),
        "and this run's own record is appended to it rather than instead of it"
    );
    assert_eq!(
        outcome.snapshots_kept,
        landed.snapshots.len(),
        "the outcome reports the pointer that landed, not the one this run built"
    );

    remote.with(|st| {
        assert!(
            !st.deleted.contains(&rival_name),
            "REPO-07: no asset the competing pointer references is deleted: {:?}",
            st.deleted
        );
        assert!(
            st.live_names().contains(&rival_name),
            "and it is still on the release"
        );
    });
}

// ---------------------------------------------------------------------------
// The security audit's three blocking findings
// ---------------------------------------------------------------------------

/// **NEW-1.** Two machines, which is the entire point of this milestone.
///
/// A and B both read a pointer whose highest counter is `n` and both compute
/// `n + 1`. A flips first; B gets a 409. The counter used to be derived where
/// the *packer* ran — once, before the race — so B's re-`PUT` republished a
/// second snapshot claiming `n + 1`. Two distinct snapshots at one counter make
/// "select the newest by counter" ambiguous, and `anchor::accept` reads an equal
/// counter as *a re-read of the snapshot already seen*: B's backup is silently
/// dropped by the control built to protect backups. Rule 1's dedup compares root
/// **bytes**, which differ, so it cannot see the collision.
///
/// The fix derives the counter inside the rebuild closure — the only code that
/// runs again after the race — and re-seals the root with it.
#[tokio::test]
async fn a_flip_lost_to_another_machine_republishes_at_a_higher_counter_never_the_same_one() {
    // Two machines: `wrap_by_hand` is deterministic, so both hold the same
    // master key and the same keyfile address, and each has its own TempDir —
    // its own index, its own pairing record, its own rollback anchor.
    let a = Local::new();
    let b = Local::new();
    assert_eq!(a.keyfile_asset, b.keyfile_asset, "one bundle, two machines");
    let remote = Remote::new().await;

    a.seed("shared", &payload(11, 512 * 1024));
    b.seed("shared", &payload(11, 512 * 1024));
    push(&a, &remote).await.expect("the first push lands");

    // Both machines are now looking at this pointer, whose highest counter is 1.
    let contended = remote.with(|st| st.pointer.clone().expect("published"));
    let seen: Pointer = serde_json::from_slice(&contended.1).expect("a stored pointer");
    assert_eq!(counters(&seen, &a), vec![1]);

    // A pushes and wins, producing counter 2.
    a.seed("a-only", &payload(12, 512 * 1024));
    push(&a, &remote).await.expect("machine A wins the race");
    let winner = remote.with(|st| st.pointer.clone().expect("published"));

    // Rewind the remote to what B read, and arm the 409 with A's pointer: B is
    // about to discover it lost. B has no anchor — it has never pushed — so this
    // is first contact for it, exactly as it would be for a genuine second
    // machine, and not a rollback.
    remote.with(|st| {
        st.pointer = Some(contended.clone());
        st.conflict_with = Some(winner.1.clone());
    });

    b.seed("b-only", &payload(13, 512 * 1024));
    push(&b, &remote).await.expect("machine B survives the 409");

    let landed = remote.with(|st| st.pointer_value().expect("published"));
    let published = counters(&landed, &a);

    let unique: BTreeSet<u64> = published.iter().copied().collect();
    assert_eq!(
        unique.len(),
        published.len(),
        "no two snapshots may claim one counter: {published:?}"
    );
    assert_eq!(
        published,
        vec![1, 2, 3],
        "the loser re-seals one above the winner rather than reusing its own"
    );
    assert_eq!(
        landed.snapshots.len(),
        3,
        "and every machine's snapshot survives: {published:?}"
    );

    // Both snapshots survive an anchor round-trip. Read oldest to newest with
    // the anchor advancing, every one is strictly newer than the mark — so none
    // reads as "already seen", which is the failure the collision caused.
    let mut mark = Anchor {
        repo_id: push::repo_id_for(1),
        counter: 0,
    };
    for counter in &published {
        anchor::accept(Some(&mark), &mark.repo_id.clone(), *counter, false)
            .unwrap_or_else(|e| panic!("counter {counter} must read as new: {e}"));
        assert!(
            *counter > mark.counter,
            "{counter} is not strictly above the high-water mark {}",
            mark.counter
        );
        mark.counter = *counter;
    }
}

/// **NEW-2 / T-4-04.** The accept named a control that was not on this path.
///
/// An attacker with repo write — squarely in the declared model — replaces the
/// pointer with an authentic *older* copy of itself. Every root in it opens,
/// `repo_id` matches, nothing errors. The next honest push used to carry those
/// records forward, append its own and flip, **laundering the rollback into a
/// legitimately-written pointer** — and then prune computed liveness over the
/// laundered pointer and deleted every pack the rollback orphaned. Those packs
/// are older than 24 h, so `PRUNE_GRACE` does not cover them: reversible tamper
/// became irreversible deletion, executed by the victim, exit 0.
#[tokio::test]
async fn a_pointer_rolled_back_to_an_authentic_older_copy_is_refused_rather_than_laundered() {
    let local = Local::new();
    local.seed("first", &payload(21, 512 * 1024));
    let remote = Remote::new().await;

    push(&local, &remote).await.expect("the first push lands");
    let old = remote.with(|st| st.pointer.clone().expect("published"));

    local.seed("second", &payload(22, 512 * 1024));
    push(&local, &remote).await.expect("the second push lands");
    let current = remote.with(|st| st.pointer_value().expect("published"));
    assert_eq!(counters(&current, &local), vec![1, 2]);

    // Age every pack past the grace window, so nothing but the pointer stands
    // between prune and the data the rollback orphans.
    remote.with(|st| {
        for asset in &mut st.assets {
            asset.created_at = NOW - TimeDelta::days(30);
        }
        // The tamper: an authentic older copy of the pointer, byte for byte.
        st.pointer = Some(old.clone());
        st.deleted.clear();
    });
    let doomed = referenced(&current);

    local.seed("third", &payload(23, 512 * 1024));
    let err = push(&local, &remote)
        .await
        .expect_err("a rolled-back pointer must not be pushed onto");
    let text = err.to_string();
    assert!(text.contains("rolled-back snapshot"), "{text}");
    assert!(
        text.contains("--allow-rollback"),
        "the message names the escape, and the escape exists: {text}"
    );

    remote.with(|st| {
        assert_eq!(
            st.pointer.clone().expect("still published"),
            old,
            "nothing was laundered: the tampered pointer was not republished"
        );
        assert!(
            st.deleted.is_empty(),
            "and prune never ran, so nothing was deleted: {:?}",
            st.deleted
        );
        for name in &doomed {
            assert!(
                st.live_names().contains(name),
                "{name} was orphaned by the rollback and is still on the release"
            );
        }
    });

    // The on-demand prune is the path that would perform the deletion, and it
    // refuses on the same evidence rather than trusting the pointer it is
    // handed.
    let client = remote.client();
    let repo = repo();
    let refused = prune::run_on_demand(&local.ctx(&client, &repo), 10)
        .await
        .expect_err("prune must not compute liveness over a rolled-back pointer");
    assert!(refused.to_string().contains("rolled-back snapshot"));

    // And the escape is real: a user who knows why the remote went back gets
    // through, which is what keeps the refusal's message honest.
    push_allowing_rollback(&local, &remote)
        .await
        .expect("--allow-rollback is the documented way through");
}

/// **NEW-3 / T-4-45.** A rekey has to stick across machines, or it is cosmetic.
///
/// Machine A rekeys: it uploads the new wrapper, flips, deletes the old one and
/// re-lists to confirm — truthfully, at that instant. Machine B, which has not
/// rekeyed, then runs an ordinary push, and `ensure_keyfile` used to publish
/// **whatever keyfile is on this machine's disk** — putting the wrapper A
/// destroyed straight back on the release, where the old password opens it.
/// Prune calls it an orphan, but `PRUNE_GRACE` holds it 24 h and B's next push
/// resets `created_at`, so it is never collected.
#[tokio::test]
async fn a_machine_that_missed_a_rekey_refuses_rather_than_republishing_the_old_wrapper() {
    let a = Local::at(FLOOR);
    let b = Local::at(FLOOR);
    a.seed("shared", &payload(31, 256 * 1024));
    b.seed("shared", &payload(31, 256 * 1024));
    let remote = Remote::new().await;

    push(&a, &remote).await.expect("A's first push lands");
    let old_wrapper = remote.with(|st| st.pointer_value().expect("published").keyfile);
    assert_eq!(old_wrapper, b.keyfile_asset, "B holds the same wrapper");

    let client = remote.client();
    let repo = repo();
    let new_wrapper = rekey::run(
        &a.ctx(&client, &repo),
        &Zeroizing::new(String::from_utf8(PASSWORD.to_vec()).expect("ascii")),
        &Zeroizing::new(String::from_utf8(NEW_PASSWORD.to_vec()).expect("ascii")),
    )
    .await
    .expect("A changes the sync password");
    assert!(
        !remote.with(|st| st.live_names().contains(&old_wrapper)),
        "D5: A verifiably deleted the old wrapper"
    );

    remote.with(|st| {
        st.requests.clear();
        st.deleted.clear();
    });
    b.seed("b-only", &payload(32, 256 * 1024));
    let err = push(&b, &remote)
        .await
        .expect_err("a stale machine must not publish the superseded wrapper");

    let text = err.to_string();
    assert!(
        text.contains("changed on another machine"),
        "the user is told plainly what happened: {text}"
    );
    assert!(
        text.contains(&new_wrapper) && text.contains(&old_wrapper),
        "and which wrapper is which: {text}"
    );
    assert!(
        text.contains("copy") && text.contains("keyfile.json"),
        "and how to catch this machine up: {text}"
    );
    assert!(
        text.contains("re-encrypts no"),
        "and that the data is unaffected: {text}"
    );

    remote.with(|st| {
        assert!(
            !st.live_names().contains(&old_wrapper),
            "the wrapper the rekey destroyed is not resurrected"
        );
        assert_eq!(
            st.pointer_value().expect("published").keyfile,
            new_wrapper,
            "and the pointer still names the new one"
        );
        assert_eq!(
            st.uploads(),
            0,
            "the refusal comes before a byte is sent: {:?}",
            st.requests
        );
    });

    // The catch-up the message names actually works: copy A's keyfile onto B.
    let caught_up: Keyfile = serde_json::from_slice(
        &std::fs::read(keyfile_path(&a.roots)).expect("A's rewrapped keyfile"),
    )
    .expect("a keyfile");
    write_keyfile(&b.roots, &caught_up);
    push(&b, &remote)
        .await
        .expect("B pushes once it holds the current wrapper");
}

// ---------------------------------------------------------------------------
// Criterion 5 — rekey
// ---------------------------------------------------------------------------

/// **ROADMAP §Phase 4 success criterion 5.**
///
/// A rekey rewraps the same master key: not one pack byte moves, the pointer
/// names the new wrapper, and the old keyfile asset is **gone** — D5's whole
/// justification for choosing release assets over git objects.
///
/// Note the path it does *not* take. `rekey` never calls
/// `upload::ensure_keyfile`: during a rekey the local keyfile is still the old
/// one until after the flip, so it uploads the freshly rewrapped bytes
/// directly. A fresh salt means the new address provably does not exist yet.
///
/// Runs at [`FLOOR`] rather than [`CHEAP`], because `Keyfile::rewrap` enforces
/// the write-path memory floor and the seam that takes it as an argument is
/// `pub(crate)` — deliberately, since a floor a caller supplies is not a floor.
#[tokio::test]
async fn a_rekey_leaves_every_pack_byte_identical_and_destroys_the_old_keyfile() {
    let local = Local::at(FLOOR);
    local.seed("data", &payload(7, 1024 * 1024));
    let remote = Remote::new().await;

    push(&local, &remote).await.expect("the first push lands");
    let packs_before: HashMap<String, Vec<u8>> = remote.with(|st| {
        st.assets
            .iter()
            .filter(|a| a.name.starts_with("pack-"))
            .map(|a| (a.name.clone(), a.bytes.clone()))
            .collect()
    });
    let old_keyfile = remote.with(|st| st.pointer_value().expect("published").keyfile);
    assert!(
        remote.with(|st| st.live_names().contains(&old_keyfile)),
        "the first push published the wrapped master key, not merely its address"
    );

    let client = remote.client();
    let repo = repo();
    let new_name = rekey::run(
        &local.ctx(&client, &repo),
        &Zeroizing::new(String::from_utf8(PASSWORD.to_vec()).expect("ascii")),
        &Zeroizing::new(String::from_utf8(NEW_PASSWORD.to_vec()).expect("ascii")),
    )
    .await
    .expect("the rekey succeeds");

    let packs_after: HashMap<String, Vec<u8>> = remote.with(|st| {
        st.assets
            .iter()
            .filter(|a| a.name.starts_with("pack-"))
            .map(|a| (a.name.clone(), a.bytes.clone()))
            .collect()
    });
    assert_eq!(
        packs_before, packs_after,
        "CRYPTO-04: a password change moves the wrapper, not one pack byte"
    );

    remote.with(|st| {
        assert!(
            !st.live_names().contains(&old_keyfile),
            "D5: the old wrapper is verifiably gone, not merely superseded"
        );
        assert!(
            st.live_names().contains(&new_name),
            "and the new one is on the release"
        );
        assert_eq!(
            st.pointer_value().expect("published").keyfile,
            new_name,
            "the pointer names the new wrapper"
        );
    });

    // The local keyfile now opens under the new password and not the old one,
    // and it still carries the same master key — which is what keeps every pack
    // above readable.
    let raw = std::fs::read(keyfile_path(&local.roots)).expect("the local keyfile");
    let rewrapped: Keyfile = serde_json::from_slice(&raw).expect("a keyfile");
    assert_eq!(
        keyfile_asset_of(&rewrapped),
        new_name,
        "the bytes on disk are the bytes that were published"
    );
    assert!(
        rewrapped.open(PASSWORD).is_err(),
        "the old password no longer opens this machine's keyfile"
    );
    let fresh = rewrapped
        .open(NEW_PASSWORD)
        .expect("the new password opens it");
    assert_eq!(
        fresh.chunk_id(b"the same master key, or not"),
        local.keys.chunk_id(b"the same master key, or not"),
        "the three subkeys are unchanged, which is what CRYPTO-04 is about"
    );
}

// ---------------------------------------------------------------------------
// The grace window, through the orchestrator
// ---------------------------------------------------------------------------

/// The in-flight-competitor guard, driven through `prune::run_on_demand`.
///
/// `PRUNE_GRACE` and the `landed` pointer are **not interchangeable**: the
/// landed pointer closes the *committed* competitor, and the 24-hour floor
/// closes the *in-flight* one a landed pointer cannot see by definition. This
/// asserts the second end to end; `plan_deletions`' own tests assert it in
/// isolation.
#[tokio::test]
async fn an_unreferenced_asset_survives_inside_the_grace_window_and_not_outside_it() {
    let local = Local::new();
    local.seed("data", &payload(8, 512 * 1024));
    let remote = Remote::new().await;
    push(&local, &remote).await.expect("the first push lands");

    // Two orphans no snapshot names: one uploaded an hour ago by a machine that
    // has not flipped yet, one abandoned a month ago.
    let fresh = push::pack_asset_name(&ChunkId::from_bytes([0x01; 32]));
    let stale = push::pack_asset_name(&ChunkId::from_bytes([0x02; 32]));
    remote.with(|st| {
        st.plant(&fresh, b"mid-push".to_vec(), NOW - TimeDelta::hours(1));
        st.plant(
            &stale,
            b"garbage".to_vec(),
            NOW - PRUNE_GRACE - TimeDelta::hours(1),
        );
    });

    let client = remote.client();
    let repo = repo();
    let deleted = prune::run_on_demand(&local.ctx(&client, &repo), 10)
        .await
        .expect("the prune runs");

    assert_eq!(deleted, 1, "exactly the aged orphan");
    remote.with(|st| {
        let live = st.live_names();
        assert!(
            live.contains(&fresh),
            "no asset younger than PRUNE_GRACE is ever deleted"
        );
        assert!(
            !live.contains(&stale),
            "and genuine garbage past the window goes"
        );
    });
}

/// `prune::run_on_demand` returns before `ensure_release` when nothing is
/// published — creating a release purely to run a delete is wrong, and 4-06
/// declined to do it. The consequence the documentation has to state is pinned
/// here: an orphan keyfile on a bundle that never published a pointer stays
/// until a first push succeeds.
#[tokio::test]
async fn an_on_demand_prune_with_no_published_pointer_deletes_nothing_and_creates_no_release() {
    let local = Local::new();
    let remote = Remote::new().await;
    let orphan = push::keyfile_asset_name(&ChunkId::from_bytes([0x03; 32]));
    remote.with(|st| {
        st.plant(
            &orphan,
            b"an abandoned wrapper".to_vec(),
            NOW - TimeDelta::days(9),
        );
    });

    let client = remote.client();
    let repo = repo();
    let deleted = prune::run_on_demand(&local.ctx(&client, &repo), 10)
        .await
        .expect("a prune with nothing published is not an error");

    assert_eq!(deleted, 0);
    remote.with(|st| {
        assert!(
            st.live_names().contains(&orphan),
            "the orphan wrapper is still there"
        );
        assert!(
            !st.requests.iter().any(|r| r.contains("/releases/tags")),
            "and no release was created purely to run a delete: {:?}",
            st.requests
        );
    });
}

// ---------------------------------------------------------------------------
// Criterion 6 — remote size tracks live data
// ---------------------------------------------------------------------------

/// **ROADMAP §Phase 4 success criterion 6.**
///
/// Twelve pushes of a growing file against `keep_snapshots = 3`. The claim is
/// that remote size tracks live data rather than cumulative history, so both
/// numbers are measured and compared rather than asserted against a constant —
/// how many packs a generation costs is 4-02's business.
#[tokio::test]
async fn repeated_syncs_of_a_growing_file_leave_fewer_assets_than_were_ever_uploaded() {
    let mut local = Local::new();
    local.cfg.keep_snapshots = 3;
    let remote = Remote::new().await;

    // Every asset is stamped well in the past, so `PRUNE_GRACE` does not hold
    // the superseded generations back: this measurement is about retention.
    remote.with(|st| st.upload_clock = Some(NOW - TimeDelta::days(30)));

    let mut body = payload(9, 256 * 1024);
    for round in 0..12u64 {
        body.extend_from_slice(&payload(100 + round, 192 * 1024));
        local.seed("growing", &body);
        push(&local, &remote)
            .await
            .unwrap_or_else(|e| panic!("push {round} failed: {e}"));
    }

    let (live, ever, pointer) = remote.with(|st| {
        (
            st.live_names(),
            st.ever_uploaded.clone(),
            st.pointer_value().expect("published"),
        )
    });

    assert!(
        live.len() < ever.len(),
        "SYNC-07: {} assets live against {} ever uploaded — remote size must track live data",
        live.len(),
        ever.len()
    );
    assert!(
        pointer.snapshots.len() <= 3,
        "retention truncates from the oldest end: {} snapshots kept",
        pointer.snapshots.len()
    );
    // And the half that matters more: nothing the surviving pointer names was
    // collected. An unrestorable backup is the worst thing a prune can produce.
    for name in referenced(&pointer) {
        assert!(live.contains(&name), "{name} is referenced but was deleted");
    }
    assert!(
        live.contains(&pointer.keyfile),
        "the wrapped master key is never collected"
    );
}

// ---------------------------------------------------------------------------
// Criterion 7 — progress, and actionable failures
// ---------------------------------------------------------------------------

/// **ROADMAP §Phase 4 success criterion 7.**
///
/// A push driven with a recording reporter emits advancing asset and byte
/// counts, and the failure path returns an error whose message names actions.
/// The reporter is the one `progress::reporter` chooses between; that function
/// shipped, like `ensure_keyfile`, with no call site at all.
#[tokio::test]
async fn a_long_push_reports_advancing_counts_and_every_failure_names_an_action() {
    let local = Local::new();
    for i in 0..4u64 {
        local.seed(&format!("a{i}"), &payload(20 + i, 2 * 1024 * 1024));
    }
    let remote = Remote::new().await;

    let client = remote.client();
    let repo = repo();
    let mut recorder = Recording::default();
    let outcome = push::run(local.ctx(&client, &repo), &mut recorder)
        .await
        .expect("the push succeeds");

    let (assets, total_bytes) = recorder.start.expect("start is called exactly once");
    assert_eq!(
        assets, outcome.packs_uploaded,
        "the reporter is told what is actually being sent"
    );
    assert!(total_bytes > 0, "measured bytes, never a projection");
    assert_eq!(
        recorder.bytes_after.len(),
        assets,
        "one report per completed asset"
    );
    assert!(
        recorder.bytes_after.windows(2).all(|w| w[1] > w[0]),
        "the byte count advances rather than freezing: {:?}",
        recorder.bytes_after
    );
    assert_eq!(
        recorder.bytes_after.last().copied(),
        Some(total_bytes),
        "and it arrives at the total it started from"
    );
    assert_eq!(
        recorder.indices.iter().collect::<BTreeSet<_>>().len(),
        assets,
        "every asset is reported once, and no index twice"
    );
    assert_eq!(
        recorder.finished, 1,
        "finish runs once, whether or not the run succeeded"
    );

    // The failure path: a repository that reads private on the first gate and
    // readable on the re-gate. What this run uploaded is destroyed, nothing is
    // flipped, and the message names three actions.
    let settled = remote.with(|st| {
        st.public_after = Some(st.gate_reads + 1);
        st.deleted.clear();
        st.pointer.clone().expect("published")
    });
    local.seed("more", &payload(31, 512 * 1024));
    let err = push(&local, &remote)
        .await
        .expect_err("a repository that turned readable mid-push refuses");
    let text = err.to_string();
    for action in [
        "Make the repository private again",
        "Rotate the sync token",
        "sync rekey",
    ] {
        assert!(text.contains(action), "the message names an action: {text}");
    }
    assert!(
        text.contains("not revocation"),
        "and it repeats that a rekey is not revocation: {text}"
    );
    assert_eq!(
        remote.with(|st| st.pointer.clone().expect("published")),
        settled,
        "nothing was flipped"
    );
    let (deleted, live) = remote.with(|st| (st.deleted.clone(), st.live_names()));
    // **F-4.** Every asset this run uploaded, selected from what `upload::run`
    // observed itself sending — not from a `created_at >= now` comparison
    // between GitHub's clock and this machine's, which `REMOTE_CLOCK` would
    // now make a guaranteed no-op.
    assert!(
        !deleted.is_empty(),
        "the incident path deletes what this run uploaded"
    );
    let pointer: Pointer = serde_json::from_slice(&settled.1).expect("a stored pointer");
    for name in referenced(&pointer) {
        assert!(
            live.contains(&name),
            "{name} belongs to the previous snapshot and must survive the incident path"
        );
    }
}

// ---------------------------------------------------------------------------
// Phase 1 carry-forwards — two guards over the source itself
// ---------------------------------------------------------------------------

/// **No fifth kind of object is sealed under `chunk_key`.**
///
/// The format defines four — pack blobs, manifests, index objects and snapshot
/// roots — and Phase 1's deferred AAD object-type separator stays untriggered
/// only while that is true. This reads the push module's own sources and pins
/// the complete set of sealing call sites in production code.
///
/// Crude on purpose. A guard that understood Rust syntax would be a program
/// with its own bugs; this one fails loudly and says what to do.
#[test]
fn the_push_path_seals_only_the_formats_four_object_kinds() {
    /// Everything before a file's own `#[cfg(test)]` module. A test that seals
    /// something is a test, not a format change.
    fn production(source: &str) -> &str {
        source
            .split_once("\n#[cfg(test)]\n")
            .map(|(before, _)| before)
            .unwrap_or(source)
    }

    // The complete set, each annotated with the kind it seals. Anything else
    // under `src/sync/push/` is a fifth kind until someone proves otherwise.
    const KNOWN: [(&str, &str); 4] = [
        (
            "packs.push(seal_chunk(ctx.keys, block)?, ctx.keys)?;",
            "a pack blob",
        ),
        ("for blob in manifest.seal(ctx.keys)? {", "the manifest"),
        (
            "for blob in index_object.seal(ctx.keys)? {",
            "the index object",
        ),
        (
            ".seal(ctx.keys)?;",
            "the snapshot root — the tail of `Root::new(…)`",
        ),
    ];

    let sources: [(&str, &str); 6] = [
        ("mod.rs", include_str!("../src/sync/push/mod.rs")),
        ("packer.rs", include_str!("../src/sync/push/packer.rs")),
        ("upload.rs", include_str!("../src/sync/push/upload.rs")),
        ("pointer.rs", include_str!("../src/sync/push/pointer.rs")),
        ("prune.rs", include_str!("../src/sync/push/prune.rs")),
        ("rekey.rs", include_str!("../src/sync/push/rekey.rs")),
    ];

    let mut found: Vec<String> = Vec::new();
    for (name, source) in sources {
        for (n, line) in production(source).lines().enumerate() {
            let code = line.trim();
            if code.starts_with("//") {
                continue;
            }
            if !(code.contains("seal_chunk(")
                || code.contains(".seal(")
                || code.contains("seal_all("))
            {
                continue;
            }
            assert!(
                KNOWN.iter().any(|(known, _)| *known == code),
                "src/sync/push/{name}:{} seals something under a key, and it is not one of the \
                 format's four object kinds:\n\
                 \x20   {code}\n\
                 A fifth kind sealed under `chunk_key` is the trigger for Phase 1's deferred AAD \
                 object-type separator. That is a versioned format change — a new format number, \
                 a reader that accepts both, and a migration — not an edit. Raise it; do not add \
                 the call. If this line really is one of the four and only its spelling moved, \
                 update KNOWN in this test and say which kind it seals.",
                n + 1
            );
            found.push(code.to_owned());
        }
    }

    for (known, kind) in KNOWN {
        assert!(
            found.iter().any(|f| f == known),
            "the guard no longer finds the call site that seals {kind} (`{known}`). It has \
             stopped matching the code rather than the code having stopped sealing — which would \
             make every assertion above vacuous."
        );
    }
}

/// **Both pack size constants are pinned, and `PACK_MAX` is named as the one
/// that governs.**
///
/// `pack::should_seal` compares against `PACK_MAX` and never reads
/// `PACK_TARGET`, so a guard on the target alone stays green while someone
/// raises the real ceiling — which is exactly the change this guard exists to
/// catch. The entry count of a pack's **single-chunk** header rises with
/// `PACK_MAX`, and gap-closure 1-09 deliberately did not reach `pack.rs`.
#[test]
fn both_pack_size_constants_are_pinned_and_pack_max_is_the_one_that_governs() {
    use ai_usagebar::sync::pack::{PACK_MAX, PACK_TARGET, should_seal};

    assert_eq!(
        PACK_TARGET,
        32 * 1024 * 1024,
        "PACK_TARGET moved. It is advisory — `should_seal` never reads it — but it is recorded \
         in docs/sync-format.md §7 as the CAL-1 fallback, so moving it means updating that too."
    );
    assert_eq!(
        PACK_MAX,
        48 * 1024 * 1024,
        "PACK_MAX moved, and **PACK_MAX is the constant that governs**: `pack::should_seal` \
         compares against it and never reads PACK_TARGET, so a guard on the target alone would \
         have stayed green through this change. A larger ceiling means more entries in a pack's \
         header, and that header is still a *single* sealed chunk — gap-closure 1-09 made \
         manifests and index objects multi-chunk and deliberately did not reach pack.rs, so its \
         entry ceiling is a function of this constant. Before moving it, re-run 4-02's \
         worst-case pack-header test and read 4-CONTEXT.md's 'Risk propagated from Phase 1 \
         verification'. Past ~256 MiB it also means adding reqwest's `stream` feature and \
         writing packs to a tempfile."
    );
    // Non-vacuity: the pins above mean nothing if `should_seal` stopped reading
    // the constant they are pinned against.
    assert!(
        !should_seal(0, PACK_MAX),
        "a pack may reach PACK_MAX exactly"
    );
    assert!(should_seal(0, PACK_MAX + 1), "and never pass it");
    assert!(
        !should_seal(0, PACK_TARGET),
        "the target is advisory: should_seal does not fire at it"
    );
}

/// Nothing in this suite may reach a real path, a real token, or the network.
///
/// The AUR `check()` runs `cargo test` during `makepkg`, so an ambient read
/// here is an install failure on a stranger's machine. Each needle is spelled
/// in two halves so this list does not match itself.
#[test]
fn nothing_in_this_suite_resolves_a_real_home_or_a_real_token() {
    let source = include_str!("sync_push_e2e.rs");
    for forbidden in [
        concat!("SyncRoots", "::resolve"),
        concat!("index", "::default_path"),
        concat!("std::env", "::var"),
        concat!("KdfParams", "::default"),
        concat!("Keyfile", "::create("),
    ] {
        assert!(
            !source.contains(forbidden),
            "this suite must never reach `{forbidden}`"
        );
    }
    assert!(
        source.contains("m_kib: 8,"),
        "and it must wrap its keyfiles at the cheap KDF parameters"
    );
}
