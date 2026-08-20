//! The two-machine scenario, literally: machine A's roots push a bundle,
//! machine B's roots pull it, and the two trees are compared byte for byte.
//!
//! Phase 5 shipped seven modules that each passed alone. This file is the only
//! place the **pair** is exercised — `push::run` produces every byte the remote
//! serves, and `restore::run` consumes it — because the failure a unit suite
//! structurally cannot catch is two halves that each agree with themselves. A
//! hand-written pointer or a hand-rolled pack would test the test author's idea
//! of the format; every fixture here is a bundle the push side really produced,
//! and every adversarial case mutates exactly **one** thing in it.
//!
//! # Root B is deliberately shaped differently
//!
//! Machine B's `config_dir`, `desktop_data_dir`, `desktop_profiles_dir` and
//! `claude_home` all have different leaf names from A's, under a different
//! username, under a different `TempDir`. That is what proves 5-01's
//! relocatable path encoding: if the two layouts matched, a manifest full of
//! absolute paths would pass this file, which is precisely the bug the encoding
//! exists to prevent.
//!
//! # Where the command path stops here, and why
//!
//! `sync::cli::pull_with_parts` — the arm that owns the two interactive gates,
//! the `is_terminal` decision and the exit-code table — is private, and the
//! production `pull` reads the sync password off the process's real stdin.
//! Driving it from an integration test would mean feeding a real stdin, which
//! the AUR `check()` runs with on an installer's machine. So this file drives
//! the two orchestrators the command wires together, `push::run` and
//! `restore::run`, exactly as `tests/sync_push_e2e.rs` drives `push::run`; the
//! gate wiring and the exit codes above them are pinned by plan 5-07's own
//! suite in `src/sync/cli.rs`. What is asserted here is what only composition
//! can show.
//!
//! # Hermetic, and it has to be
//!
//! The AUR `check()` runs `cargo test` during `makepkg`. Nothing here reads a
//! real `$HOME`, a real token, the Keychain, or the network: every root is a
//! `TempDir`, both `Endpoints` fields point at one mockito server, `now` is a
//! constant, every local mtime is stamped rather than read from the wall clock,
//! and every keyfile is wrapped at the cheapest KDF parameters the thing under
//! test will accept.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
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
use ai_usagebar::sync::crypto::{KdfDoc, KdfParams, Keyfile, Keys, content_address, derive_kek};
use ai_usagebar::sync::github::setup::{self, SetupPrompt};
use ai_usagebar::sync::github::token::{TokenChain, TokenSource};
use ai_usagebar::sync::github::write::ASSET_STATE_UPLOADED;
use ai_usagebar::sync::github::{Client, Endpoints, RepoRef, pairing};
use ai_usagebar::sync::index::Index;
use ai_usagebar::sync::model::Root;
use ai_usagebar::sync::push::progress::Progress;
use ai_usagebar::sync::push::{self, Pointer, PushCtx, PushOutcome, prune, rekey};
use ai_usagebar::sync::restore::{
    self, Disposition, RestoreCtx, RestoreOptions, RestoreOutcome, report,
};

const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD;

/// Microseconds instead of ~1.5 s and a gibibyte. Never production parameters:
/// the AUR `check()` runs these on an installer's machine.
const CHEAP: KdfParams = KdfParams {
    m_kib: 8,
    t: 1,
    p: 1,
};

/// `crypto::MIN_KDF_MEMORY_KIB` — the lowest parameters a **rekey** can run at,
/// because `Keyfile::rewrap` enforces the write-path floor. 8 MiB of Argon2id is
/// milliseconds, which is what keeps the one rekey test cheap.
const FLOOR: KdfParams = KdfParams {
    m_kib: 8 * 1024,
    t: 1,
    p: 1,
};

const PASSWORD: &str = "correct horse battery staple";
const NEW_PASSWORD: &str = "a different long enough sync password";

/// Fixed and injected. Nothing here reads the wall clock.
const NOW: DateTime<Utc> = match DateTime::from_timestamp(1_700_000_000, 0) {
    Some(t) => t,
    None => panic!("a fixed timestamp"),
};

/// GitHub's clock, deliberately not [`NOW`]: two clocks are never equal, and a
/// fixture that pretends they are proves nothing about code comparing them.
const REMOTE_CLOCK: DateTime<Utc> = match DateTime::from_timestamp(1_700_000_000 - 90, 0) {
    Some(t) => t,
    None => panic!("a fixed timestamp"),
};

/// Not a token. The fixture never reaches a real host.
const TOKEN: &str = "github_pat_fixture_not_a_real_token";

const PRIVATE_BODY: &str = r#"{"id":1,"private":true,"visibility":"private",
    "owner":{"login":"o","id":7},"archived":false,"fork":false}"#;

/// A distinctive byte string seeded into the credential fixture's plaintext.
///
/// SAFE-05's wording is that decrypted plaintext must not survive at a path
/// outside a destination directory, so the refusal tests search **for this
/// string** across everything under machine B rather than asserting a file
/// count. A count passes while a copy sits one directory over.
const MARKER: &str = "PLAINTEXT-MARKER-6f2b1d";

// ---------------------------------------------------------------------------
// The local machines
// ---------------------------------------------------------------------------

/// A keyfile wrapping a fixed master key, assembled from the public fields.
///
/// `Keyfile::create_with_floor` is `pub(crate)` — a memory floor an outside
/// caller supplies its own value for is not a floor — so a hermetic suite that
/// must stay in milliseconds builds its own, exactly as `tests/sync_vectors.rs`,
/// `tests/sync_adversarial.rs` and `tests/sync_push_e2e.rs` already do.
///
/// `seed` picks the master key **and** the salt, so two keyfiles from this
/// helper are two different KEKs as well as two different master keys — which is
/// what makes the one fixed wrap nonce safe.
fn wrap_by_hand(seed: u8, pw: &[u8], k: KdfParams) -> Keyfile {
    /// `docs/sync-format.md` §1's `{"format":…,"kdf":{…}}`, in declaration
    /// order, which *is* the canonical AAD byte order.
    #[derive(Serialize)]
    struct KeyfileAad<'a> {
        format: u32,
        kdf: &'a KdfDoc,
    }
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

/// The keyfile's asset name over the **canonical** serialization — the bytes
/// `ensure_keyfile` and `rekey` upload, not the pretty-printed bytes on disk.
fn keyfile_asset_of(keyfile: &Keyfile) -> String {
    push::keyfile_asset_name(&content_address(
        &serde_json::to_vec(keyfile).expect("a keyfile serializes"),
    ))
}

/// `sync::cli::keyfile_path` is `pub(crate)`, so its one rule — the keyfile
/// lives beside `config.toml` and is never resolved from `$HOME` — is repeated
/// here rather than imported, exactly as `tests/sync_push_e2e.rs` does.
fn keyfile_path(roots: &SyncRoots) -> PathBuf {
    roots.config_dir.join("sync").join("keyfile.json")
}

/// `cli::backups_dir`, which is private for the same reason. `~/.claude-acc/`'s
/// sibling of the profile store, derived from the injected roots and never from
/// `$HOME` — which is what keeps every archive inside its own `TempDir` (D3).
fn backups_dir(roots: &SyncRoots) -> PathBuf {
    roots
        .desktop_profiles_dir
        .parent()
        .unwrap_or(&roots.desktop_profiles_dir)
        .join("backups")
}

/// The four roots of one machine, and **nothing about them is shared with the
/// other machine's**. Every leaf name differs, so a manifest entry that only
/// resolved because the two layouts happened to match would fail.
fn alice_roots(dir: &Path) -> SyncRoots {
    let home = dir.join("Users/alice");
    SyncRoots::at(
        home.join(".config/ai-usagebar/config.toml"),
        home.join(".config/ai-usagebar"),
        home.join("Library/Application Support/Claude"),
        home.join(".claude-acc/profiles"),
        home.join(".claude"),
    )
}

fn bob_roots(dir: &Path) -> SyncRoots {
    let home = dir.join("var/home/bob");
    SyncRoots::at(
        home.join("cfg/aub/settings.toml"),
        home.join("cfg/aub"),
        home.join("state/claude-desktop"),
        home.join("acc-store/profiles"),
        home.join("dot-claude"),
    )
}

/// One machine: its roots, its index, its keyfile, its pairing record, its
/// anchor. Both a pusher and a restorer, because the milestone's whole story is
/// a machine that does one and then the other.
struct Machine {
    dir: TempDir,
    roots: SyncRoots,
    index: Index,
    keys: Keys,
    kdf: KdfParams,
    keyfile_asset: String,
    cfg: SyncConfig,
    password: Zeroizing<String>,
    repo_id: String,
}

impl Machine {
    fn alice() -> Machine {
        Machine::at(alice_roots, 0x11, CHEAP, PASSWORD)
    }

    fn bob() -> Machine {
        Machine::at(bob_roots, 0x11, CHEAP, PASSWORD)
    }

    fn at(
        layout: fn(&Path) -> SyncRoots,
        keyfile_seed: u8,
        kdf: KdfParams,
        password: &str,
    ) -> Machine {
        let dir = TempDir::new().expect("a temp dir");
        let roots = layout(dir.path());
        for root in [
            &roots.config_dir,
            &roots.desktop_data_dir,
            &roots.desktop_profiles_dir,
            &roots.claude_home,
        ] {
            std::fs::create_dir_all(root).expect("a root directory");
        }

        let keyfile = wrap_by_hand(keyfile_seed, password.as_bytes(), kdf);
        let keys = keyfile
            .open(password.as_bytes())
            .expect("a hand-wrapped keyfile opens");
        let keyfile_asset = keyfile_asset_of(&keyfile);
        let at = keyfile_path(&roots);
        std::fs::create_dir_all(at.parent().expect("a parent")).expect("the keyfile dir");
        // Pretty-printed, exactly as `sync setup` writes it, so a fixture whose
        // asset name comes from the canonical form stays honest about it.
        std::fs::write(&at, serde_json::to_vec_pretty(&keyfile).expect("json"))
            .expect("the keyfile");

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
        Machine {
            dir,
            index,
            keys,
            kdf,
            keyfile_asset,
            cfg: SyncConfig {
                categories: SyncCategory::ALL.to_vec(),
                repo: Some("o/n".into()),
                ..SyncConfig::default()
            },
            password: Zeroizing::new(password.into()),
            repo_id: push::repo_id_for(1),
            roots,
        }
    }

    fn push_ctx<'a>(&'a self, client: &'a Client, repo: &'a RepoRef) -> PushCtx<'a> {
        PushCtx {
            client,
            repo,
            cfg: &self.cfg,
            roots: &self.roots,
            keys: &self.keys,
            kdf: self.kdf,
            index: &self.index,
            repo_id: self.repo_id.clone(),
            keyfile_asset: self.keyfile_asset.clone(),
            // Filled by `push::run` from the remote, after the gate.
            previous: None,
            allow_rollback: false,
            now: NOW,
        }
    }

    fn restore_ctx<'a>(
        &'a self,
        client: &'a Client,
        repo: &'a RepoRef,
        anchor_path: &'a Path,
        backups: &'a Path,
        opts: RestoreOptions,
    ) -> RestoreCtx<'a> {
        RestoreCtx {
            client,
            repo,
            roots: &self.roots,
            repo_id: &self.repo_id,
            passphrase: &self.password,
            anchor_path,
            backups_dir: backups,
            opts,
            now: NOW,
        }
    }

    /// `<config_dir>/sync-anchor-o-n.json`, through 4-08's own helper. Restore
    /// must read and advance the **same** file the push path does; a second
    /// implementation is how a defence ends up with two copies that disagree.
    fn anchor_path(&self) -> PathBuf {
        push::anchor_path(&self.roots, &repo())
    }

    fn backups(&self) -> PathBuf {
        backups_dir(&self.roots)
    }

    /// Write one file under a root, creating parents, stamped at `NOW` so no
    /// comparison in this file depends on the wall clock.
    fn seed(&self, root: &Path, rel: &str, body: &[u8]) -> PathBuf {
        self.write_at(root, rel, body, NOW)
    }

    /// A later edit to a file that has already been pushed.
    ///
    /// **Stamped after `NOW` deliberately, and this is a fixture trap paid for
    /// once already.** The local index keys change detection on
    /// `(path, size, mtime_ns, inode)`, and `fs::write` over an existing file
    /// keeps its inode — so an edit of the *same length* at the *same* mtime is
    /// invisible to the planner, the file is never re-read, and the next
    /// snapshot silently carries the old bytes. A test asserting that the edit
    /// arrived then fails for a reason that has nothing to do with restore.
    fn edit(&self, root: &Path, rel: &str, body: &[u8]) -> PathBuf {
        self.write_at(root, rel, body, NOW + TimeDelta::minutes(1))
    }

    fn write_at(&self, root: &Path, rel: &str, body: &[u8], at: DateTime<Utc>) -> PathBuf {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("the seed dir");
        std::fs::write(&path, body).expect("a seeded file");
        set_mtime(&path, at);
        path
    }

    /// Everything under this machine's whole `TempDir` — roots, backups
    /// directory, anchor, keyfile and index alike.
    fn everything(&self) -> Vec<PathBuf> {
        files_under(self.dir.path())
    }

    /// Everything a *restore* could have put here: the machine's whole tree,
    /// minus the files it was born with (keyfile, pairing, index), minus the
    /// rollback anchor, and minus the pre-restore archive — all of which are
    /// state or safety machinery rather than restored content.
    ///
    /// Asserting emptiness by **walking** is the point: the return value under
    /// test cannot also be the evidence for itself (T-5-70). The archive is
    /// covered separately, by asserting the backups directory does not exist at
    /// all after a refusal.
    fn restored(&self) -> Vec<PathBuf> {
        let anchor = self.anchor_path();
        let mine = self.roots.config_dir.join("sync");
        let pairing = pairing::default_path(&self.roots);
        let backups = self.backups();
        self.everything()
            .into_iter()
            .filter(|p| {
                *p != anchor && *p != pairing && !p.starts_with(&mine) && !p.starts_with(&backups)
            })
            .collect()
    }

    fn anchor_bytes(&self) -> Option<Vec<u8>> {
        std::fs::read(self.anchor_path()).ok()
    }
}

/// The bundle's own view of a tree: manifest path → (bytes, unix mode).
///
/// Keyed through the **push side's** encoder, so comparing A's view with B's is
/// comparing relative positions inside the bundle rather than two absolute
/// paths that could only ever match by accident. Files that encode under no
/// root (the backups directory, anything above the roots) are not part of a
/// bundle and are skipped.
fn bundle_view(roots: &SyncRoots) -> BTreeMap<String, (Vec<u8>, u32)> {
    let mut out = BTreeMap::new();
    for root in [
        &roots.config_dir,
        &roots.desktop_data_dir,
        &roots.desktop_profiles_dir,
        &roots.claude_home,
    ] {
        for path in files_under(root) {
            let Ok(wire) = push::packer::manifest_path(roots, &path) else {
                continue;
            };
            // This machine's own sync state — keyfile, index, pairing record,
            // rollback anchor — is machinery, not bundle content, and no
            // collector picks any of it up.
            if wire.starts_with("config/sync/")
                || wire.starts_with("config/sync-anchor-")
                || wire == "config/sync-pairing.json"
            {
                continue;
            }
            out.insert(
                wire,
                (
                    std::fs::read(&path).expect("a readable file"),
                    mode_of(&path),
                ),
            );
        }
    }
    out
}

/// The two machines hold the same bundle content.
///
/// Content only, never modes: a file seeded on A carries whatever the host's
/// umask gave it, while every restored file is 0600 by design. Criterion 1
/// asserts the modes; everywhere else the question is whether the bytes arrived.
fn assert_same_content(a: &SyncRoots, b: &SyncRoots) {
    let (sent, landed) = (bundle_view(a), bundle_view(b));
    assert_eq!(
        sent.keys().collect::<Vec<_>>(),
        landed.keys().collect::<Vec<_>>(),
        "the two machines hold different files"
    );
    for (wire, (bytes, _)) in &sent {
        assert_eq!(
            &landed[wire].0, bytes,
            "{wire} differs between the two machines"
        );
    }
}

fn mode_of(path: &Path) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::symlink_metadata(path)
            .expect("a stat-able path")
            .permissions()
            .mode()
            & 0o7777
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        0o600
    }
}

fn set_mtime(path: &Path, at: DateTime<Utc>) {
    let times = std::fs::FileTimes::new().set_modified(std::time::SystemTime::from(at));
    std::fs::File::options()
        .write(true)
        .open(path)
        .expect("the seeded file reopens")
        .set_times(times)
        .expect("a filesystem that takes a timestamp");
}

/// Every regular file (and every symlink) anywhere under `root`, sorted.
fn files_under(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(md) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if md.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// Does `MARKER`'s plaintext exist anywhere under `root`?
///
/// The only assertion that covers SAFE-05's actual wording. A test that checked
/// the destination alone would pass while a copy sat one directory over.
fn plaintext_anywhere_under(root: &Path) -> Vec<PathBuf> {
    files_under(root)
        .into_iter()
        .filter(|p| {
            std::fs::read(p)
                .map(|bytes| bytes.windows(MARKER.len()).any(|w| w == MARKER.as_bytes()))
                .unwrap_or(false)
        })
        .collect()
}

/// Distinct, moderately compressible bytes — a stand-in for the JSON the bundle
/// really carries. `seed` makes every file unique and the counter makes every
/// 256 KiB chunk unique, so nothing collapses into one deduplicated chunk.
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

/// One realistic file in every one of the five categories, plus the three
/// shapes a naive round trip gets wrong: a file larger than `CHUNK_SIZE`, a name
/// containing a space, and one nested three directories deep.
fn seed_a_full_tree(m: &Machine) {
    // Config: config.toml itself, and one account credential carrying MARKER.
    std::fs::write(&m.roots.config_file, b"[sync]\nrepo = \"o/n\"\n").expect("a config");
    set_mtime(&m.roots.config_file, NOW);
    m.seed(
        &m.roots.config_dir,
        "accounts/work/.credentials.json",
        format!(r#"{{"token":"not-a-real-token","note":"{MARKER}"}}"#).as_bytes(),
    );

    // Credentials: the claude-acc profile store.
    m.seed(
        &m.roots.desktop_profiles_dir,
        "work/meta.json",
        br#"{"label":"work"}"#,
    );
    m.seed(
        &m.roots.desktop_profiles_dir,
        "work/config-tokenCache",
        b"an opaque desktop token cache",
    );

    // Routines: a name with a space, and one nested three directories deep.
    m.seed(
        &m.roots.claude_home,
        "scheduled-tasks/daily routine.json",
        br#"{"cron":"0 9 * * *"}"#,
    );
    m.seed(
        &m.roots.claude_home,
        "scheduled-tasks/one/two/three/deep.json",
        br#"{"cron":"@weekly"}"#,
    );

    // Chat index.
    m.seed(
        &m.roots.desktop_data_dir,
        "claude-code-sessions/acct/org/local_1.json",
        br#"{"sessions":[]}"#,
    );

    // Transcripts: the one file above CHUNK_SIZE, so ordered multi-chunk
    // reassembly is exercised — which is where a scrambled restore shows up.
    m.seed(
        &m.roots.claude_home,
        "projects/repo/session.jsonl",
        &payload(7, ai_usagebar::sync::CHUNK_SIZE * 2 + 7),
    );
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
    pointer: Option<(String, Vec<u8>)>,
    /// Asset names actually served to a downloader, in order.
    downloaded: Vec<String>,
    deleted: Vec<String>,
    /// Every pointer `PUT` is refused — a process killed after the uploads and
    /// before the flip.
    refuse_put: bool,
    /// The next pointer `PUT` answers 409, and this pointer becomes current.
    conflict_with: Option<Vec<u8>>,
    /// Asset names withheld from a download, answering 404 instead.
    withheld: BTreeSet<String>,
    /// Asset names whose first ciphertext byte is flipped on the way out.
    tampered: BTreeSet<String>,
    uploads: usize,
}

impl RemoteState {
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

    fn store_pointer(&mut self, body: Vec<u8>) -> String {
        self.next_sha += 1;
        let sha = format!("blob{}", self.next_sha);
        self.pointer = Some((sha.clone(), body));
        sha
    }

    fn plant(&mut self, name: &str, bytes: Vec<u8>, created_at: DateTime<Utc>) -> u64 {
        self.next_asset_id += 1;
        let id = self.next_asset_id;
        self.assets.push(StoredAsset {
            id,
            name: name.to_owned(),
            bytes,
            state: ASSET_STATE_UPLOADED.into(),
            created_at,
        });
        id
    }
}

type Shared = Arc<Mutex<RemoteState>>;

/// A mockito server over one [`RemoteState`], serving the whole surface both
/// halves use: the visibility read, the release, the listing, uploads, the
/// verifying/restoring downloads, deletes, and both halves of the pointer.
///
/// Two fixture traps this inherits from `tests/sync_push_e2e.rs`, each already
/// paid for once: recording state happens only inside `with_body_from_request`
/// (a matcher runs against every request, including ones its mock never
/// answers), and where two mocks share a method and path their matchers
/// **partition** the space rather than leaning on mockito's preference for an
/// unsatisfied mock, which silently retires the first after one hit.
struct Remote {
    server: mockito::ServerGuard,
    state: Shared,
}

impl Remote {
    async fn new() -> Remote {
        let mut server = mockito::Server::new_async().await;
        let state: Shared = Arc::default();

        server
            .mock("GET", "/repos/o/n")
            .with_status(200)
            .with_body(PRIVATE_BODY)
            .create_async()
            .await;

        server
            .mock("GET", "/repos/o/n/releases/tags/ai-usagebar-sync-v1")
            .with_status(200)
            .with_body(r#"{"id":9}"#)
            .create_async()
            .await;

        // Paginated exactly as `list_assets` expects: a short page ends the
        // loop, so a fake ignoring `page` would spin forever on a release
        // holding a multiple of `per_page` assets.
        let s = Arc::clone(&state);
        server
            .mock("GET", mockito::Matcher::Regex("/releases/9/assets".into()))
            .with_status(200)
            .with_body_from_request(move |req| {
                let st = s.lock().expect("lock");
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

        // Uploads. Ids are handed out per upload, never assumed.
        let s = Arc::clone(&state);
        server
            .mock("POST", mockito::Matcher::Regex("/releases/9/assets".into()))
            .with_status(201)
            .with_body_from_request(move |req| {
                let name = asset_name_in(req.path_and_query());
                let bytes = req.body().expect("an upload has a body").clone();
                let mut st = s.lock().expect("lock");
                st.uploads += 1;
                let id = st.plant(&name, bytes, REMOTE_CLOCK);
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

        // The download every restore reads through, and the verifying download
        // D3 makes a precondition of the flip. Withholding and tampering both
        // live here so an adversarial case is one mutation of a real bundle.
        let s = Arc::clone(&state);
        server
            .mock(
                "GET",
                mockito::Matcher::Regex(r"/releases/assets/\d+$".into()),
            )
            .match_request({
                let s = Arc::clone(&state);
                move |req| {
                    let st = s.lock().expect("lock");
                    let id = asset_id_in(req.path());
                    !st.assets
                        .iter()
                        .any(|a| a.id == id && st.withheld.contains(&a.name))
                }
            })
            .with_status(200)
            .with_body_from_request(move |req| {
                let id = asset_id_in(req.path());
                let mut st = s.lock().expect("lock");
                let Some(asset) = st.assets.iter().find(|a| a.id == id).cloned() else {
                    return Vec::new();
                };
                st.downloaded.push(asset.name.clone());
                let mut bytes = asset.bytes;
                if st.tampered.contains(&asset.name) && !bytes.is_empty() {
                    // One ciphertext byte, deep enough inside to be a blob's
                    // body rather than the framing.
                    let at = bytes.len() / 2;
                    bytes[at] ^= 0x01;
                }
                bytes
            })
            .create_async()
            .await;
        let s = Arc::clone(&state);
        server
            .mock(
                "GET",
                mockito::Matcher::Regex(r"/releases/assets/\d+$".into()),
            )
            .match_request(move |req| {
                let st = s.lock().expect("lock");
                let id = asset_id_in(req.path());
                st.assets
                    .iter()
                    .any(|a| a.id == id && st.withheld.contains(&a.name))
            })
            .with_status(404)
            .with_body(r#"{"message":"Not Found"}"#)
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
            .with_body(r#"{"message":"Not Found"}"#)
            .create_async()
            .await;
        let s = Arc::clone(&state);
        server
            .mock("GET", "/repos/o/n/contents/sync/pointer.json")
            .match_request({
                let s = Arc::clone(&state);
                move |_| s.lock().expect("lock").pointer.is_some()
            })
            .with_status(200)
            .with_body_from_request(move |_| {
                let st = s.lock().expect("lock");
                let (sha, body) = st.pointer.clone().expect("the matcher checked");
                format!(r#"{{"sha":"{sha}","content":"{}"}}"#, B64.encode(&body)).into_bytes()
            })
            .create_async()
            .await;

        // The flip, in three mocks for the same reason — matchers partitioning
        // the space rather than relying on mock preference.
        let s = Arc::clone(&state);
        server
            .mock("PUT", "/repos/o/n/contents/sync/pointer.json")
            .match_request(move |_| s.lock().expect("lock").refuse_put)
            .with_status(403)
            .with_body(r#"{"message":"Resource not accessible by personal access token"}"#)
            .create_async()
            .await;
        let s = Arc::clone(&state);
        server
            .mock("PUT", "/repos/o/n/contents/sync/pointer.json")
            .match_request({
                let s = Arc::clone(&state);
                move |_| {
                    let st = s.lock().expect("lock");
                    !st.refuse_put && st.conflict_with.is_some()
                }
            })
            .with_status(409)
            .with_body_from_request(move |_| {
                // The competitor lands here, in the responder rather than the
                // matcher: this runs exactly when the 409 is served, so the
                // re-read that follows sees the winner.
                let mut st = s.lock().expect("lock");
                let winner = st.conflict_with.take().expect("the matcher checked");
                st.store_pointer(winner);
                br#"{"message":"is at abc but expected def"}"#.to_vec()
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

/// `sync push`'s progress reporter, reduced to nothing. UX-04 is 4-07's.
#[derive(Default)]
struct Silent;
impl Progress for Silent {
    fn start(&mut self, _assets: usize, _total_bytes: u64) {}
    fn asset_done(&mut self, _index: usize, _name: &str, _bytes: u64) {}
    fn finish(&mut self) {}
}

/// One push through the real orchestrator.
async fn push(m: &Machine, remote: &Remote) -> ai_usagebar::error::Result<PushOutcome> {
    let client = remote.client();
    let repo = repo();
    push::run(m.push_ctx(&client, &repo), &mut Silent).await
}

/// One restore through the real orchestrator, with this machine's own anchor
/// and backups directory — the same two paths `sync pull` derives.
async fn pull(
    m: &Machine,
    remote: &Remote,
    opts: RestoreOptions,
) -> ai_usagebar::error::Result<RestoreOutcome> {
    let client = remote.client();
    let repo = repo();
    let anchor_path = m.anchor_path();
    let backups = m.backups();
    restore::run(m.restore_ctx(&client, &repo, &anchor_path, &backups, opts)).await
}

/// `sync setup`'s prompt seam, scripted — the second machine's operator.
///
/// **`store_token` is overridden and has to be.** Its production default writes
/// the **real** macOS login Keychain, and the AUR `check()` runs `cargo test`
/// during `makepkg` on installers' machines: a test that reached it through the
/// production call would clobber the user's own sync token. The seam exists for
/// exactly this.
///
/// [`SetupPrompt::passphrase`] **panics**, which is the load-bearing assertion:
/// it is the generate path's ask, and reaching it means setup minted a second
/// master key for a bundle that already has one — the whole failure this test
/// exists to catch. The join path asks
/// [`SetupPrompt::existing_passphrase`] instead.
struct Joining {
    existing: Vec<String>,
    said: Vec<String>,
    asked: usize,
}

impl Joining {
    fn typing(password: &str) -> Joining {
        Joining {
            existing: vec![password.into()],
            said: Vec::new(),
            asked: 0,
        }
    }
}

impl SetupPrompt for Joining {
    fn say(&mut self, line: &str) {
        self.said.push(line.to_owned());
    }
    fn confirm(&mut self, _question: &str, _default_yes: bool) -> ai_usagebar::error::Result<bool> {
        Ok(true)
    }
    fn passphrase(&mut self, _generated: &str) -> ai_usagebar::error::Result<Zeroizing<String>> {
        panic!("a machine joining a published bundle must never be offered a generated password")
    }
    fn existing_passphrase(&mut self) -> ai_usagebar::error::Result<Zeroizing<String>> {
        self.asked += 1;
        Ok(Zeroizing::new(if self.existing.is_empty() {
            String::new()
        } else {
            self.existing.remove(0)
        }))
    }
    fn categories(
        &mut self,
        current: &[SyncCategory],
    ) -> ai_usagebar::error::Result<Vec<SyncCategory>> {
        Ok(current.to_vec())
    }
    /// 8 MiB rather than a gibibyte, for the same reason as everywhere else.
    fn kdf(&self) -> KdfParams {
        FLOOR
    }
    /// Recorded by *not happening*: the real one writes the login Keychain.
    fn store_token(&self, _token: &str, _file: &Path) -> ai_usagebar::error::Result<TokenSource> {
        Ok(TokenSource::File)
    }
}

/// A second machine as `sync setup` really leaves it — no keyfile, no pairing
/// record and no index until the flow writes them — paired against `remote`.
///
/// Returns the [`Machine`] the rest of this file's helpers take, built from
/// what setup put on disk rather than from a hand-wrapped fixture. That is the
/// point: every other two-machine test here hands B a keyfile built by
/// `wrap_by_hand` with A's seed, which is precisely the step the product could
/// not perform.
async fn set_up_second_machine(
    remote: &Remote,
    password: &str,
) -> ai_usagebar::error::Result<(Machine, Joining)> {
    let dir = TempDir::new().expect("a temp dir");
    let roots = bob_roots(dir.path());
    let prompt = set_up_at(remote, &roots, password).await?;

    let keyfile: Keyfile =
        serde_json::from_slice(&std::fs::read(keyfile_path(&roots)).expect("setup wrote one"))
            .expect("a readable keyfile");
    let keys = keyfile
        .open(password.as_bytes())
        .expect("the adopted keyfile");
    let index = Index::at(&roots.index_file).expect("the local index");
    Ok((
        Machine {
            dir,
            index,
            keys,
            kdf: keyfile.kdf.params(),
            keyfile_asset: keyfile_asset_of(&keyfile),
            cfg: SyncConfig {
                categories: SyncCategory::ALL.to_vec(),
                repo: Some("o/n".into()),
                ..SyncConfig::default()
            },
            password: Zeroizing::new(password.into()),
            repo_id: push::repo_id_for(1),
            roots,
        },
        prompt,
    ))
}

/// `sync setup` against `remote`, under roots the caller owns — so a refusal
/// case can assert on the paths the flow did **not** write after the `TempDir`
/// would otherwise have gone with the `Machine` that was never built.
async fn set_up_second_machine_at(
    remote: &Remote,
    roots: &SyncRoots,
    password: &str,
) -> ai_usagebar::error::Result<()> {
    set_up_at(remote, roots, password).await.map(|_| ())
}

async fn set_up_at(
    remote: &Remote,
    roots: &SyncRoots,
    password: &str,
) -> ai_usagebar::error::Result<Joining> {
    for root in [
        &roots.config_dir,
        &roots.desktop_data_dir,
        &roots.desktop_profiles_dir,
        &roots.claude_home,
    ] {
        std::fs::create_dir_all(root).expect("a root directory");
    }
    let cfg = SyncConfig {
        categories: SyncCategory::ALL.to_vec(),
        repo: Some("o/n".into()),
        ..SyncConfig::default()
    };

    let mut prompt = Joining::typing(password);
    setup::run(
        &cfg,
        roots,
        &Endpoints {
            api_base: remote.server.url(),
            uploads_base: remote.server.url(),
        },
        &TokenChain {
            env_value: Some(Zeroizing::new(TOKEN.into())),
            ..TokenChain::default()
        },
        &mut prompt,
        NOW,
    )
    .await?;
    Ok(prompt)
}

fn applying() -> RestoreOptions {
    RestoreOptions {
        apply: true,
        ..Default::default()
    }
}

/// Every refusal test calls this: a failed pull must leave the anchor **byte**
/// identical. Advancing on a failed verify would let anyone with repo write
/// access lock the user out of their own bundle permanently — the risk plan
/// 1-05 recorded against this phase, expressed as an assertion rather than a
/// review note (T-5-72).
fn assert_anchor_frozen(m: &Machine, before: Option<Vec<u8>>, what: &str) {
    assert_eq!(
        m.anchor_bytes(),
        before,
        "{what} moved the rollback anchor; a failed pull must never advance it"
    );
}

fn dispositions(outcome: &RestoreOutcome) -> BTreeMap<&str, &Disposition> {
    outcome
        .plan
        .items
        .iter()
        .map(|i| (i.manifest_path.as_str(), &i.disposition))
        .collect()
}

// ---------------------------------------------------------------------------
// Criterion 1 — the round trip
// ---------------------------------------------------------------------------

/// **ROADMAP §Phase 5 criterion 1** — a pushed tree pulls into a
/// differently-shaped second root byte for byte, credentials at 0600.
///
/// This is the line the milestone exists for. Every leaf name in B's layout
/// differs from A's, and B's username differs too, so a manifest that had
/// smuggled an absolute path through would resolve to nothing here.
#[tokio::test]
async fn criterion_1_a_pushed_tree_restores_byte_for_byte_under_a_second_machines_roots() {
    let a = Machine::alice();
    let b = Machine::bob();
    seed_a_full_tree(&a);
    let remote = Remote::new().await;

    push(&a, &remote).await.expect("the first push lands");
    let outcome = pull(&b, &remote, applying())
        .await
        .expect("the second machine restores");

    assert!(outcome.applied);
    assert!(outcome.failed_at.is_none(), "{:?}", outcome.failed_at);

    let sent = bundle_view(&a.roots);
    let landed = bundle_view(&b.roots);
    assert!(
        sent.len() >= 8,
        "the fixture stopped covering every category: {:?}",
        sent.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        sent.keys().collect::<Vec<_>>(),
        landed.keys().collect::<Vec<_>>(),
        "the two machines disagree about which files the bundle holds"
    );
    for (wire, (bytes, _)) in &sent {
        assert_eq!(
            &landed[wire].0, bytes,
            "{wire} did not survive the round trip byte for byte"
        );
    }
    assert_eq!(outcome.written, sent.len(), "every item was written");

    // Every category, and the three shapes a naive round trip gets wrong.
    for wire in [
        "config/config.toml",
        "config/accounts/work/.credentials.json",
        "desktop-profiles/work/meta.json",
        "claude-home/scheduled-tasks/daily routine.json",
        "claude-home/scheduled-tasks/one/two/three/deep.json",
        "desktop-data/claude-code-sessions/acct/org/local_1.json",
        "claude-home/projects/repo/session.jsonl",
    ] {
        assert!(landed.contains_key(wire), "{wire} is missing on machine B");
    }
    assert!(
        landed["claude-home/projects/repo/session.jsonl"].0.len() > ai_usagebar::sync::CHUNK_SIZE,
        "the multi-chunk file shrank"
    );

    // Nothing landed outside B's four roots, and nothing carries A's username.
    for path in b.restored() {
        assert!(
            [
                &b.roots.config_dir,
                &b.roots.desktop_data_dir,
                &b.roots.desktop_profiles_dir,
                &b.roots.claude_home,
            ]
            .iter()
            .any(|root| path.starts_with(root)),
            "{path:?} landed outside every sync root"
        );
        assert!(
            !path.to_string_lossy().contains("alice"),
            "{path:?} carries the pushing machine's username"
        );
    }

    #[cfg(unix)]
    {
        for (wire, (_, mode)) in &landed {
            assert_eq!(*mode, 0o600, "{wire} was not restored at mode 0600");
        }
        // Directories the restore created are closed to other users; the ones
        // the machine was born with keep whatever their owner gave them.
        assert_eq!(
            mode_of(&b.roots.claude_home.join("scheduled-tasks/one/two/three")),
            0o700,
            "a directory the restore created is readable by others"
        );
    }

    // The anchor advanced, from the root's own sealed counter.
    let stored = anchor::read_from(&b.anchor_path())
        .expect("a readable anchor")
        .expect("a successful apply advances it");
    assert_eq!(stored.counter, 1);
    assert_eq!(stored.repo_id, push::repo_id_for(1));
}

// ---------------------------------------------------------------------------
// Criterion 2 — idempotence (D7)
// ---------------------------------------------------------------------------

/// **D7 idempotence** — a second apply writes nothing and reports
/// no conflict.
///
/// It re-runs the **whole** restore rather than re-applying the plan in hand:
/// D7's claim is about re-running an interrupted restore, and re-applying a
/// stale plan would prove something weaker. Identity is decided by digest before
/// either clock is read, which is what keeps the second run from reporting two
/// hundred phantom conflicts.
#[tokio::test]
async fn d7_a_second_apply_of_the_same_snapshot_writes_nothing_and_reports_no_conflict() {
    let a = Machine::alice();
    let b = Machine::bob();
    seed_a_full_tree(&a);
    let remote = Remote::new().await;

    push(&a, &remote).await.expect("the push lands");
    let first = pull(&b, &remote, applying()).await.expect("the restore");
    assert!(first.written > 0);
    let after_first = bundle_view(&b.roots);

    let second = pull(&b, &remote, applying())
        .await
        .expect("the restore re-runs");

    assert_eq!(second.written, 0, "a second apply wrote files");
    assert!(second.overwritten.is_empty(), "{:?}", second.overwritten);
    assert_eq!(second.skipped, second.plan.items.len());
    for (wire, disposition) in dispositions(&second) {
        assert_eq!(
            *disposition,
            Disposition::SkipIdentical,
            "{wire} is not identical on the second run"
        );
    }
    assert_eq!(after_first, bundle_view(&b.roots), "a byte moved");
    assert!(
        second.backup.is_none(),
        "a run that writes nothing took an archive"
    );
}

// ---------------------------------------------------------------------------
// D1 — the dry run
// ---------------------------------------------------------------------------

/// A dry run writes nothing **anywhere**, and never downloads a pack it would
/// only need for file data.
///
/// The second half is 5-02 made structural: `fetch::resolve`'s third download
/// round is behind `opts.apply`, so a dry run pulls the metadata packs and stops.
/// Two pushes are what make that observable — snapshot 2's manifest and index
/// live in the packs push 2 sealed, while the file data from push 1 lives in a
/// pack of its own. With a single push every pack also carries metadata and the
/// assertion would be vacuous.
#[tokio::test]
async fn criterion_2_a_dry_run_writes_nothing_at_all_and_never_downloads_a_pack_for_file_data() {
    let a = Machine::alice();
    let b = Machine::bob();
    seed_a_full_tree(&a);
    let remote = Remote::new().await;

    push(&a, &remote).await.expect("the first push lands");
    a.seed(&a.roots.claude_home, "scheduled-tasks/second.json", b"{}");
    push(&a, &remote).await.expect("the second push lands");

    remote.with(|st| st.downloaded.clear());
    let dry = pull(&b, &remote, RestoreOptions::default())
        .await
        .expect("a dry run resolves");
    let dry_packs: BTreeSet<String> = remote.with(|st| st.downloaded.iter().cloned().collect());

    assert!(!dry.applied);
    assert_eq!(dry.written, 0);
    assert!(dry.backup.is_none(), "a dry run took an archive");
    assert!(
        dry.plan.packs_needed > 0,
        "the plan claims nothing to fetch"
    );
    assert!(
        b.restored().is_empty(),
        "a dry run wrote {:?} under machine B",
        b.restored()
    );
    assert!(
        !b.backups().exists(),
        "a dry run created the backups directory"
    );
    assert!(!b.anchor_path().exists(), "a dry run advanced the anchor");

    remote.with(|st| st.downloaded.clear());
    pull(&b, &remote, applying()).await.expect("the apply");
    let apply_packs: BTreeSet<String> = remote.with(|st| st.downloaded.iter().cloned().collect());

    assert!(
        !dry_packs.is_empty(),
        "the dry run fetched nothing at all, so it proves nothing about what it skipped"
    );
    assert!(
        dry_packs.is_subset(&apply_packs),
        "the dry run fetched something the apply did not: {dry_packs:?} vs {apply_packs:?}"
    );
    assert!(
        dry_packs.len() < apply_packs.len(),
        "the dry run downloaded every pack the apply did — a data-only pack was fetched \
         before anything asked for one: {dry_packs:?}"
    );
}

// ---------------------------------------------------------------------------
// Criterion 4 — the backup (SAFE-04, D3)
// ---------------------------------------------------------------------------

/// **ROADMAP §Phase 5 criterion 4** — the archive exists before the first write,
/// and the command it prints restores the prior tree exactly.
///
/// A rollback command that is well-formed and *wrong* looks identical to one
/// that works, so the rendered string is executed through `/bin/sh` and the tree
/// compared — contents and modes both. A credential archived at 0600 and
/// restored at 0644 would be a leak created by the safety mechanism.
#[tokio::test]
async fn criterion_4_the_backup_precedes_the_first_write_and_its_printed_command_restores_exactly()
{
    if !Path::new("/bin/sh").exists() || !Path::new("/usr/bin/tar").exists() {
        eprintln!("skipping: this host has no /bin/sh or /usr/bin/tar");
        return;
    }
    let a = Machine::alice();
    let b = Machine::bob();
    seed_a_full_tree(&a);
    let remote = Remote::new().await;

    push(&a, &remote).await.expect("the first push lands");
    pull(&b, &remote, applying()).await.expect("the restore");
    let before = bundle_view(&b.roots);

    // A changes two files and pushes again; B pulls the newer snapshot, which
    // is the run that overwrites and therefore the run that archives.
    a.edit(
        &a.roots.claude_home,
        "scheduled-tasks/daily routine.json",
        br#"{"cron":"0 6 * * *"}"#,
    );
    a.edit(
        &a.roots.config_dir,
        "accounts/work/.credentials.json",
        format!(r#"{{"token":"rotated","note":"{MARKER}"}}"#).as_bytes(),
    );
    push(&a, &remote).await.expect("the second push lands");

    let outcome = pull(&b, &remote, applying())
        .await
        .expect("the second restore");
    let record = outcome
        .backup
        .clone()
        .expect("an overwriting restore archives first");
    assert!(record.members > 0 && record.bytes > 0);
    let changed: Vec<String> = outcome
        .plan
        .items
        .iter()
        .filter(|i| i.disposition.writes())
        .map(|i| i.manifest_path.clone())
        .collect();
    assert_eq!(changed.len(), 2, "the fixture stopped changing two files");

    assert_ne!(before, bundle_view(&b.roots), "the restore changed nothing");

    // **The archive holds the bytes that were there before the write**, which
    // is what proves it was taken first. Timestamps cannot say so: 5-04 stamps
    // every restored file with the snapshot's own `created_at`, so a comparison
    // of mtimes would be comparing the snapshot's clock to the archive's.
    let scratch = TempDir::new().expect("a scratch dir");
    let extracted = std::process::Command::new("/usr/bin/tar")
        .arg("-xzf")
        .arg(&record.archive)
        .arg("-C")
        .arg(scratch.path())
        .status()
        .expect("tar runs");
    assert!(extracted.success(), "the archive did not extract");
    for wire in [
        "claude-home/scheduled-tasks/daily routine.json",
        "config/accounts/work/.credentials.json",
    ] {
        let dest = restore::layout::from_manifest_path(&b.roots, wire).expect("a bundle path");
        let member = dest
            .strip_prefix(&record.root)
            .expect("a member of the archive");
        assert_eq!(
            std::fs::read(scratch.path().join(member)).expect("the archived copy"),
            before[wire].0,
            "{wire} was archived *after* it was overwritten, which is no undo at all"
        );
    }

    // The archive is exactly the reversal set — the items the restore wrote
    // over, and nothing else. Clobber those, as a bad restore would, and run
    // the printed undo.
    assert_eq!(
        record.members,
        changed.len(),
        "the archive is not the reversal set"
    );
    for wire in &changed {
        let dest = restore::layout::from_manifest_path(&b.roots, wire).expect("a bundle path");
        std::fs::write(&dest, b"clobbered").expect("a writable destination");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        }
    }
    let command = record.rollback_command();
    assert!(command.starts_with("tar -xzf "), "{command}");
    let status = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(&command)
        .status()
        .expect("the rollback command runs");
    assert!(status.success(), "`{command}` exited {status}");

    let back = bundle_view(&b.roots);
    for wire in &changed {
        let (bytes, mode) = &before[wire.as_str()];
        let (back_bytes, back_mode) = back
            .get(wire.as_str())
            .unwrap_or_else(|| panic!("{wire} was not brought back"));
        assert_eq!(back_bytes, bytes, "{wire} came back with different bytes");
        // A credential archived at 0600 and restored at 0644 would be a leak
        // created by the safety mechanism itself.
        assert_eq!(back_mode, mode, "{wire} came back at a different mode");
    }
}

// ---------------------------------------------------------------------------
// SYNC-06 — the conflict is reported, never silent
// ---------------------------------------------------------------------------

/// **SYNC-06 / SAFE-03 (D2, D6)** — a locally-newer file is skipped and named,
/// `--force` overwrites it and names it in the summary, and a locally-*older*
/// edit is simply updated.
///
/// The default for a conflict is a skip with a report, never a helpful
/// overwrite: a wrong push costs a re-push, a wrong restore costs the work on
/// the machine in front of you.
#[tokio::test]
async fn criterion_6_a_newer_local_file_is_skipped_and_named_and_force_overwrites_and_names_it() {
    const ROUTINE: &str = "claude-home/scheduled-tasks/daily routine.json";
    let a = Machine::alice();
    let b = Machine::bob();
    seed_a_full_tree(&a);
    let remote = Remote::new().await;

    push(&a, &remote).await.expect("the first push lands");
    pull(&b, &remote, applying()).await.expect("the restore");

    // A pushes a competing edit.
    a.edit(
        &a.roots.claude_home,
        "scheduled-tasks/daily routine.json",
        br#"{"cron":"from alice"}"#,
    );
    push(&a, &remote).await.expect("the second push lands");

    let local = b
        .roots
        .claude_home
        .join("scheduled-tasks/daily routine.json");
    let newer = br#"{"cron":"edited on bob, later"}"#;
    std::fs::write(&local, newer).expect("a local edit");
    set_mtime(&local, NOW + TimeDelta::hours(1));

    // 1. Skipped, named, and untouched.
    let skipped = pull(&b, &remote, applying()).await.expect("the pull");
    assert!(matches!(
        dispositions(&skipped)[ROUTINE],
        Disposition::SkipLocalNewer { .. }
    ));
    assert_eq!(std::fs::read(&local).unwrap(), newer, "a skip wrote anyway");
    let rendered = report::render_plan(&skipped.plan, false);
    assert!(
        rendered.contains("scheduled-tasks/daily routine.json"),
        "the skipped item is not named in the report:\n{rendered}"
    );
    assert!(
        rendered.contains("--force"),
        "the report does not name the flag that resolves it:\n{rendered}"
    );

    // 2. `--force` overwrites it, and the summary names what it replaced.
    let forced = pull(
        &b,
        &remote,
        RestoreOptions {
            apply: true,
            force: true,
            ..Default::default()
        },
    )
    .await
    .expect("the forced pull");
    assert!(matches!(
        dispositions(&forced)[ROUTINE],
        Disposition::Overwrite { .. }
    ));
    assert_eq!(
        std::fs::read(&local).unwrap(),
        br#"{"cron":"from alice"}"#,
        "--force did not replace the local file"
    );
    assert!(
        forced.overwritten.iter().any(|p| p == ROUTINE),
        "SYNC-06: the overwritten item is counted rather than named: {:?}",
        forced.overwritten
    );
    assert!(
        report::render_outcome(&forced).contains("scheduled-tasks/daily routine.json"),
        "the summary does not name what it overwrote"
    );

    // 3. A local edit *older* than the snapshot needs no flag at all.
    std::fs::write(&local, b"{}").expect("a local edit");
    set_mtime(&local, NOW - TimeDelta::hours(1));
    let updated = pull(&b, &remote, applying()).await.expect("the pull");
    assert_eq!(*dispositions(&updated)[ROUTINE], Disposition::Update);
    assert_eq!(std::fs::read(&local).unwrap(), br#"{"cron":"from alice"}"#);
}

// ---------------------------------------------------------------------------
// The resume, from the far side
// ---------------------------------------------------------------------------

/// A push killed before the flip leaves the previous pointer byte-identical, the
/// **first** re-run re-uploads nothing, and the resumed bundle is what the second
/// machine restores.
///
/// The "first re-run" half is the regression test: pack content addresses depend
/// on the order blobs land inside a pack, so before `plan::build` sorted each
/// category's file plans by path, a resume reused **nothing** and only the
/// second re-run onwards reused anything. Asserting full reuse on the first
/// resume is what pins it. This file adds the half a push-only suite cannot
/// show: that what the resume published is what actually restores.
#[tokio::test]
async fn a_push_killed_before_the_flip_costs_one_refused_push_and_the_resume_restores_the_tree() {
    let a = Machine::alice();
    let b = Machine::bob();
    seed_a_full_tree(&a);
    let remote = Remote::new().await;

    push(&a, &remote).await.expect("the first push lands");
    let settled = remote.with(|st| st.pointer.clone().expect("published"));

    // New data, refused at the flip. Everything above the PUT still happens.
    a.seed(
        &a.roots.claude_home,
        "projects/repo/second.jsonl",
        &payload(9, 300 * 1024),
    );
    remote.with(|st| st.refuse_put = true);
    let err = push(&a, &remote)
        .await
        .expect_err("a refused flip is a failed push");
    assert!(!err.to_string().is_empty(), "every failure names something");
    assert_eq!(
        settled,
        remote.with(|st| st.pointer.clone().expect("still published")),
        "SYNC-04: the previous pointer is byte-identical after a killed push"
    );

    // The first re-run. Nothing goes back on the wire but the flip.
    remote.with(|st| {
        st.refuse_put = false;
        st.uploads = 0;
    });
    let resumed = push(&a, &remote).await.expect("the resume lands");
    assert_eq!(
        resumed.packs_uploaded, 0,
        "SYNC-05: the *first* resume re-uploaded packs it already had"
    );
    assert!(
        resumed.packs_skipped > 0,
        "and it skipped rather than re-sent"
    );
    assert_eq!(
        remote.with(|st| st.uploads),
        0,
        "not one asset body crossed the wire on the resume"
    );

    // And the resumed bundle is a bundle: the second machine restores it whole.
    let outcome = pull(&b, &remote, applying()).await.expect("the restore");
    assert!(outcome.failed_at.is_none());
    assert_same_content(&a.roots, &b.roots);
}

// ---------------------------------------------------------------------------
// Two machines racing (4-08 NEW-1)
// ---------------------------------------------------------------------------

/// Two machines that both read one pointer publish at **distinct** counters, both
/// snapshots survive, and a pull takes the newest.
///
/// Before 4-08 the counter was sealed from the pointer read before the
/// compare-and-swap and never recomputed, so both machines published at the same
/// one: the dedup compared root *bytes*, which differ, so it could not see the
/// collision, and `anchor::accept` reads an equal counter as "already seen" — so
/// restoring A's snapshot made B's distinct snapshot read as a re-read and the
/// anchor silently dropped a backup. The counter is now derived inside the
/// rebuild closure, which is the only code that runs again after the race.
#[tokio::test]
async fn two_machines_racing_publish_distinct_counters_and_a_pull_takes_the_newest() {
    // `wrap_by_hand` is deterministic, so both hold the same master key and the
    // same keyfile address while keeping separate trees, indexes and anchors.
    let a = Machine::alice();
    let a2 = Machine::at(bob_roots, 0x11, CHEAP, PASSWORD);
    let restorer = Machine::at(alice_roots, 0x11, CHEAP, PASSWORD);
    assert_eq!(
        a.keyfile_asset, a2.keyfile_asset,
        "one bundle, two machines"
    );
    let remote = Remote::new().await;

    seed_a_full_tree(&a);
    seed_a_full_tree(&a2);
    push(&a, &remote).await.expect("the first push lands");
    let contended = remote.with(|st| st.pointer.clone().expect("published"));

    // A pushes again and wins, producing counter 2.
    a.seed(
        &a.roots.claude_home,
        "scheduled-tasks/a-only.json",
        b"{\"a\":1}",
    );
    push(&a, &remote).await.expect("machine A wins the race");
    let winner = remote.with(|st| st.pointer.clone().expect("published"));

    // Rewind to what A2 read and arm the 409 with A's pointer: A2 is about to
    // discover it lost.
    remote.with(|st| {
        st.pointer = Some(contended);
        st.conflict_with = Some(winner.1.clone());
    });
    a2.seed(
        &a2.roots.claude_home,
        "scheduled-tasks/b-only.json",
        b"{\"b\":1}",
    );
    push(&a2, &remote)
        .await
        .expect("machine A2 survives the 409");

    let landed = remote.with(|st| st.pointer_value().expect("published"));
    let counters: Vec<u64> = landed
        .snapshots
        .iter()
        .map(|s| {
            let framed = B64.decode(&s.root).expect("a base64 root");
            Root::open(&a.keys, &framed, &push::repo_id_for(1))
                .expect("a root this bundle's keys open")
                .counter
        })
        .collect();
    let unique: BTreeSet<u64> = counters.iter().copied().collect();
    assert_eq!(
        unique.len(),
        counters.len(),
        "no two snapshots may claim one counter: {counters:?}"
    );
    assert_eq!(
        counters,
        vec![1, 2, 3],
        "the loser re-seals one above the winner rather than reusing its own"
    );

    // The newest snapshot is the loser's, and that is what a pull takes — by the
    // counter *inside* each sealed root, not by position in the plaintext list.
    let outcome = pull(&restorer, &remote, applying())
        .await
        .expect("the restore");
    assert_eq!(outcome.plan.counter, 3);
    assert!(
        restorer
            .roots
            .claude_home
            .join("scheduled-tasks/b-only.json")
            .exists(),
        "the pull did not take the highest counter"
    );
    assert!(
        !restorer
            .roots
            .claude_home
            .join("scheduled-tasks/a-only.json")
            .exists(),
        "the loser's snapshot is a snapshot of the loser's machine, not a merge"
    );
}

// ---------------------------------------------------------------------------
// Criterion 3 — the refusals
// ---------------------------------------------------------------------------

/// **ROADMAP §Phase 5 criterion 5**, the rollback half — an authentic *older*
/// pointer served in place of the current one is refused, and `prune` does not
/// then delete the packs it orphaned.
///
/// This is the one attack that authenticates perfectly: the old data really was
/// written by the user's key. Only state the attacker cannot reach detects it.
/// 4-08 put the check on all three paths that publish a pointer, `prune` among
/// them, because guarding only `push` left `sync prune` as the executioner.
#[tokio::test]
async fn criterion_5_a_rolled_back_pointer_is_refused_and_prune_does_not_collect_its_orphans() {
    let a = Machine::alice();
    let b = Machine::bob();
    seed_a_full_tree(&a);
    let remote = Remote::new().await;

    push(&a, &remote).await.expect("the first push lands");
    let old = remote.with(|st| st.pointer.clone().expect("published"));

    a.seed(
        &a.roots.claude_home,
        "projects/repo/newer.jsonl",
        &payload(31, 300 * 1024),
    );
    push(&a, &remote).await.expect("the second push lands");

    // B restores the current bundle, which is what gives it a high-water mark.
    pull(&b, &remote, applying()).await.expect("the restore");
    let anchored = b.anchor_bytes();
    let landed = bundle_view(&b.roots);
    assert!(
        b.roots
            .claude_home
            .join("projects/repo/newer.jsonl")
            .exists()
    );

    // The remote is replaced with its own earlier bytes. Nothing is forged.
    remote.with(|st| st.pointer = Some(old));
    let err = pull(&b, &remote, applying())
        .await
        .expect_err("a rolled-back pointer must be refused");
    assert!(
        err.to_string().contains("--allow-rollback"),
        "the refusal does not name the escape it has: {err}"
    );
    assert_anchor_frozen(&b, anchored.clone(), "a refused rollback");
    assert_eq!(landed, bundle_view(&b.roots), "a refused pull wrote anyway");

    // And prune, run against the same tampered pointer, deletes nothing — the
    // path that would have turned reversible tamper into irreversible deletion.
    let live_before = remote.with(|st| st.live_names());
    let client = remote.client();
    let repo = repo();
    let mut ctx = a.push_ctx(&client, &repo);
    ctx.now = NOW + TimeDelta::days(3); // every asset past PRUNE_GRACE
    let pruned = prune::run_on_demand(&ctx, a.cfg.keep_snapshots as usize).await;
    assert!(pruned.is_err(), "prune ran over a rolled-back pointer");
    assert!(
        remote.with(|st| st.deleted.is_empty()),
        "prune deleted {:?} the rollback orphaned",
        remote.with(|st| st.deleted.clone())
    );
    assert_eq!(remote.with(|st| st.live_names()), live_before);

    // The escape is real, and is for an older snapshot of the *same* bundle.
    let opened = pull(
        &b,
        &remote,
        RestoreOptions {
            apply: true,
            allow_rollback: true,
            ..Default::default()
        },
    )
    .await
    .expect("--allow-rollback opens the older snapshot on purpose");
    assert_eq!(opened.plan.counter, 1);
}

/// **Criterion 3**, the tampering half — one flipped ciphertext byte in a served
/// pack refuses, writes nothing, and leaves no plaintext under machine B.
///
/// The pack is served under its own content address, so the substitution is
/// caught before its header is even read. The fixture is a bundle the push side
/// really produced and exactly one byte differs, so the only difference between
/// the passing and failing runs is the tampering.
#[tokio::test]
async fn criterion_5_a_tampered_pack_refuses_and_leaves_no_plaintext_under_machine_b() {
    let a = Machine::alice();
    let b = Machine::bob();
    seed_a_full_tree(&a);
    let remote = Remote::new().await;

    push(&a, &remote).await.expect("the push lands");
    let anchored = b.anchor_bytes();
    let a_pack = remote.with(|st| {
        st.assets
            .iter()
            .map(|asset| asset.name.clone())
            .find(|name| name.starts_with("pack-"))
            .expect("the bundle has packs")
    });
    remote.with(|st| {
        st.tampered.insert(a_pack.clone());
    });

    let err = pull(&b, &remote, applying())
        .await
        .expect_err("a tampered pack must be refused");
    let message = err.to_string();
    assert!(
        message.contains("does not hash to that name") || message.contains("cannot"),
        "the refusal does not say the bytes were substituted: {message}"
    );
    assert!(
        b.restored().is_empty(),
        "a refused restore left {:?} behind",
        b.restored()
    );
    assert!(
        plaintext_anywhere_under(b.dir.path()).is_empty(),
        "plaintext survived a refused restore at {:?}",
        plaintext_anywhere_under(b.dir.path())
    );
    assert!(
        !b.backups().exists(),
        "a refused restore took an archive it had nothing to archive"
    );
    assert_anchor_frozen(&b, anchored, "a tampered pack");

    // The same bundle, untampered, restores — so the refusal is about the byte.
    remote.with(|st| st.tampered.clear());
    pull(&b, &remote, applying())
        .await
        .expect("the untampered bundle restores");
}

/// **Criterion 3**, the missing-chunk half — a snapshot naming a pack the
/// release will not serve refuses and writes nothing.
///
/// Restoring a short file quietly is the outcome this refusal exists to prevent.
#[tokio::test]
async fn criterion_5_a_snapshot_naming_a_pack_the_release_withholds_refuses_and_writes_nothing() {
    let a = Machine::alice();
    let b = Machine::bob();
    seed_a_full_tree(&a);
    let remote = Remote::new().await;

    push(&a, &remote).await.expect("the push lands");
    let anchored = b.anchor_bytes();
    // The largest pack: the one carrying the multi-chunk transcript's data.
    let biggest = remote.with(|st| {
        st.assets
            .iter()
            .filter(|a| a.name.starts_with("pack-"))
            .max_by_key(|a| a.bytes.len())
            .map(|a| a.name.clone())
            .expect("the bundle has packs")
    });
    remote.with(|st| {
        st.withheld.insert(biggest);
    });

    let err = pull(&b, &remote, applying())
        .await
        .expect_err("a withheld pack must refuse");
    assert!(!err.to_string().is_empty());
    assert!(
        b.restored().is_empty(),
        "a refused restore left {:?} behind",
        b.restored()
    );
    assert!(plaintext_anywhere_under(b.dir.path()).is_empty());
    assert!(!b.backups().exists(), "a refused restore took an archive");
    assert_anchor_frozen(&b, anchored, "a withheld pack");
}

/// **Criterion 3**, the path half — a manifest entry that does not resolve
/// inside the sync roots is refused, **named in the report**, and produces no
/// destination.
///
/// The hostile entry here is produced by a real push rather than hand-written:
/// machine A's `config_file` is spelled with a `..` in it, which is a path a
/// user can genuinely configure and which `packer::manifest_path` renders
/// verbatim. The bundle is otherwise entirely ordinary, so the only difference
/// from the passing run is the one entry. Silently dropping it is how a user
/// concludes a restore was complete when it was not.
#[tokio::test]
async fn criterion_5_a_manifest_entry_that_escapes_its_root_is_refused_and_named_in_the_report() {
    let dir = TempDir::new().expect("a temp dir");
    let honest = alice_roots(dir.path());
    // One mutation: the same file, named through its parent.
    let sneaky = SyncRoots::at(
        honest.config_dir.join("../ai-usagebar/config.toml"),
        honest.config_dir.clone(),
        honest.desktop_data_dir.clone(),
        honest.desktop_profiles_dir.clone(),
        honest.claude_home.clone(),
    );
    let a = Machine::alice();
    let b = Machine::bob();
    let remote = Remote::new().await;

    // Machine A, rebuilt over the doctored roots but otherwise untouched.
    for root in [
        &sneaky.config_dir,
        &sneaky.desktop_data_dir,
        &sneaky.desktop_profiles_dir,
        &sneaky.claude_home,
    ] {
        std::fs::create_dir_all(root).expect("a root");
    }
    std::fs::create_dir_all(keyfile_path(&sneaky).parent().unwrap()).unwrap();
    std::fs::copy(keyfile_path(&a.roots), keyfile_path(&sneaky)).expect("the keyfile");
    pairing::write_to(
        &pairing::default_path(&sneaky),
        &pairing::Pairing {
            repo_id: 1,
            owner_id: 7,
            private: true,
            checked_at: NOW,
        },
    )
    .expect("a pairing record");
    std::fs::write(&sneaky.config_file, b"[sync]\nrepo = \"o/n\"\n").expect("a config");
    set_mtime(&sneaky.config_file, NOW);
    let honest_file = sneaky.claude_home.join("scheduled-tasks/ok.json");
    std::fs::create_dir_all(honest_file.parent().unwrap()).unwrap();
    std::fs::write(&honest_file, br#"{"cron":"@daily"}"#).expect("an honest file");
    set_mtime(&honest_file, NOW);

    let index = Index::at(&sneaky.index_file).expect("an index");
    let client = remote.client();
    let repo = repo();
    let ctx = PushCtx {
        client: &client,
        repo: &repo,
        cfg: &a.cfg,
        roots: &sneaky,
        keys: &a.keys,
        kdf: a.kdf,
        index: &index,
        repo_id: a.repo_id.clone(),
        keyfile_asset: a.keyfile_asset.clone(),
        previous: None,
        allow_rollback: false,
        now: NOW,
    };
    push::run(ctx, &mut Silent).await.expect("the push lands");

    let outcome = pull(&b, &remote, applying())
        .await
        .expect("the restore runs");
    let items = dispositions(&outcome);
    let hostile = items
        .iter()
        .find(|(wire, _)| wire.contains(".."))
        .map(|(wire, d)| (*wire, *d))
        .expect("the doctored root produced a `..` manifest entry");
    assert!(
        matches!(hostile.1, Disposition::RejectedPath(_)),
        "{}: {:?}",
        hostile.0,
        hostile.1
    );
    let item = outcome
        .plan
        .items
        .iter()
        .find(|i| i.manifest_path == hostile.0)
        .expect("the item is still in the plan");
    assert!(item.dest.is_none(), "a refused entry got a destination");

    // It is reported, not dropped — and the honest sibling still restored.
    let rendered = report::render_plan(&outcome.plan, false);
    assert!(
        rendered.contains("REFUSED") && rendered.contains(".."),
        "the refused entry is not visible in the report:\n{rendered}"
    );
    assert!(
        b.roots.claude_home.join("scheduled-tasks/ok.json").exists(),
        "one refusal stopped the rest of the restore"
    );
    for path in b.restored() {
        assert!(
            path.starts_with(&b.roots.config_dir)
                || path.starts_with(&b.roots.claude_home)
                || path.starts_with(&b.roots.desktop_data_dir)
                || path.starts_with(&b.roots.desktop_profiles_dir),
            "{path:?} landed outside every sync root"
        );
    }
}

/// A symlink planted at a destination is `RejectedPath`, and **no consent flag
/// promotes it** — not `--force`, not `--force --force-credentials`.
///
/// `SkipLocalNewer` would have been the wrong variant: it is the one disposition
/// `--force` promotes to `Overwrite`, so a symlink reported that way would be
/// written through by the user's obvious next command (T-5-22).
#[tokio::test]
#[cfg(unix)]
async fn a_symlink_at_a_destination_is_refused_and_no_flag_promotes_it() {
    const ROUTINE: &str = "claude-home/scheduled-tasks/daily routine.json";
    let a = Machine::alice();
    let b = Machine::bob();
    seed_a_full_tree(&a);
    let remote = Remote::new().await;
    push(&a, &remote).await.expect("the push lands");

    let elsewhere = b.dir.path().join("outside.txt");
    std::fs::write(&elsewhere, b"not the bundle's to touch").expect("a file outside the roots");
    let dest = b
        .roots
        .claude_home
        .join("scheduled-tasks/daily routine.json");
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &dest).expect("a planted link");

    for opts in [
        applying(),
        RestoreOptions {
            apply: true,
            force: true,
            ..Default::default()
        },
        RestoreOptions {
            apply: true,
            force: true,
            force_credentials: true,
            ..Default::default()
        },
    ] {
        let outcome = pull(&b, &remote, opts).await.expect("the restore runs");
        assert!(
            matches!(
                dispositions(&outcome)[ROUTINE],
                Disposition::RejectedPath(_)
            ),
            "opts {opts:?} promoted a symlink to a write: {:?}",
            dispositions(&outcome)[ROUTINE]
        );
        assert_eq!(
            std::fs::read(&elsewhere).unwrap(),
            b"not the bundle's to touch",
            "opts {opts:?} wrote through the link"
        );
        assert!(
            std::fs::symlink_metadata(&dest).unwrap().is_symlink(),
            "opts {opts:?} replaced the link"
        );
    }
}

// ---------------------------------------------------------------------------
// The credential gate (D2)
// ---------------------------------------------------------------------------

/// `--force` alone never overwrites a locally-newer **credential**, and
/// `--force-credentials` without `--force` grants nothing.
///
/// The failure mode this guards is silently reverting a live OAuth token to a
/// stale one: if it has since rotated, the live one is gone and everything
/// authenticated with it stops working until the user logs in again. This
/// project has already shipped one bug in that family and should not build a
/// third path into it.
#[tokio::test]
async fn criterion_3_force_alone_never_overwrites_a_live_credential() {
    const CRED: &str = "config/accounts/work/.credentials.json";
    let a = Machine::alice();
    let b = Machine::bob();
    seed_a_full_tree(&a);
    let remote = Remote::new().await;

    push(&a, &remote).await.expect("the first push lands");
    pull(&b, &remote, applying()).await.expect("the restore");

    // A pushes an older token; B's is newer, and live.
    a.edit(
        &a.roots.config_dir,
        "accounts/work/.credentials.json",
        format!(r#"{{"token":"the-stale-one","note":"{MARKER}"}}"#).as_bytes(),
    );
    push(&a, &remote).await.expect("the second push lands");

    let local = b.roots.config_dir.join("accounts/work/.credentials.json");
    let live = br#"{"token":"the-live-rotated-one"}"#;
    std::fs::write(&local, live).expect("a rotated token");
    set_mtime(&local, NOW + TimeDelta::hours(2));

    // No flag: skipped.
    let plain = pull(&b, &remote, applying()).await.expect("the pull");
    assert!(matches!(
        dispositions(&plain)[CRED],
        Disposition::SkipLocalNewer { .. }
    ));

    // `--force-credentials` without `--force` is still a skip.
    let half = pull(
        &b,
        &remote,
        RestoreOptions {
            apply: true,
            force_credentials: true,
            ..Default::default()
        },
    )
    .await
    .expect("the pull");
    assert!(
        matches!(
            dispositions(&half)[CRED],
            Disposition::SkipLocalNewer { .. }
        ),
        "--force-credentials granted itself --force"
    );
    assert_eq!(std::fs::read(&local).unwrap(), live);

    // `--force` alone stops the restore rather than replacing it.
    let forced = pull(
        &b,
        &remote,
        RestoreOptions {
            apply: true,
            force: true,
            ..Default::default()
        },
    )
    .await;
    match forced {
        Ok(outcome) => panic!(
            "--force alone replaced a live credential: {:?}",
            dispositions(&outcome)[CRED]
        ),
        Err(e) => assert!(
            e.to_string().contains("confirm") || e.to_string().contains("credential"),
            "the refusal does not say what it stopped for: {e}"
        ),
    }
    assert_eq!(
        std::fs::read(&local).unwrap(),
        live,
        "--force alone lost the live token"
    );

    // Both together, and only both, replace it.
    let both = pull(
        &b,
        &remote,
        RestoreOptions {
            apply: true,
            force: true,
            force_credentials: true,
            ..Default::default()
        },
    )
    .await
    .expect("the second consent");
    assert!(matches!(
        dispositions(&both)[CRED],
        Disposition::Overwrite { .. }
    ));
    assert!(
        std::fs::read(&local)
            .unwrap()
            .windows(MARKER.len())
            .any(|w| w == MARKER.as_bytes()),
        "the snapshot's credential did not land"
    );
    assert!(
        both.overwritten.iter().any(|p| p == CRED),
        "the replaced credential is not named: {:?}",
        both.overwritten
    );
}

// ---------------------------------------------------------------------------
// Criterion 6 — the interruption (SAFE-05)
// ---------------------------------------------------------------------------

/// **ROADMAP §Phase 5 criterion 7** — a restore that fails part way leaves the
/// items before it complete, the item at it absent under its real name, no
/// `.tmp.` file surviving anywhere under machine B, and the anchor unmoved.
///
/// The failure is injected by making one destination's parent directory
/// unwritable, which is a real per-item write failure rather than a stub. A
/// partial restore is **reported, not rolled back**: undoing the successful
/// writes would mean writing again, from an archive, on a machine that has just
/// demonstrated it cannot complete a write.
///
/// The `TMPDIR` half of SAFE-05 is asserted structurally in `write.rs` — the
/// module's own source is checked for `temp_dir`, `"/tmp"` and `into_temp_path`
/// and has none — rather than by walking a process-global directory a
/// concurrently running test also owns.
#[tokio::test]
#[cfg(unix)]
async fn criterion_7_an_interrupted_restore_leaves_no_half_written_file_and_no_anchor_move() {
    use std::os::unix::fs::PermissionsExt;

    // The premise, checked rather than assumed: root writes into a 0o500
    // directory regardless, and this test would then prove nothing. No `libc`
    // dependency — the question is answered by trying it.
    let probe = TempDir::new().expect("a temp dir");
    let closed = probe.path().join("closed");
    std::fs::create_dir(&closed).unwrap();
    std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o500)).unwrap();
    if std::fs::write(closed.join("probe"), b"x").is_ok() {
        eprintln!("skipping: a read-only directory is still writable here (running as root?)");
        return;
    }

    let a = Machine::alice();
    let b = Machine::bob();
    seed_a_full_tree(&a);
    let remote = Remote::new().await;
    push(&a, &remote).await.expect("the push lands");

    // Items are written in manifest order, so the credential (`config/…`) lands
    // before the routines. Closing the routines directory stops the run there.
    let blocked = b.roots.claude_home.join("scheduled-tasks");
    std::fs::create_dir_all(&blocked).expect("the destination directory");
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o500))
        .expect("a read-only destination directory");

    let anchored = b.anchor_bytes();
    let outcome = pull(&b, &remote, applying())
        .await
        .expect("a per-item failure is reported, never propagated as an error");
    let failed = outcome
        .failed_at
        .clone()
        .expect("the run stopped somewhere and said so");
    assert!(
        failed.starts_with("claude-home/scheduled-tasks/"),
        "it stopped somewhere unexpected: {failed}"
    );
    assert!(outcome.written > 0, "nothing at all was written");

    // Restore the mode so the walk can see inside.
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o700)).expect("chmod");

    assert!(
        !b.roots.claude_home.join(&failed[13..]).exists(),
        "the item the run stopped at exists under its real name"
    );
    for path in b.restored() {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            !name.starts_with(".tmp."),
            "a temporary file survived at {path:?}"
        );
    }
    assert_anchor_frozen(&b, anchored, "a partial restore");

    // A re-run finishes it: every write is idempotent and the completed items
    // are skipped by digest rather than rewritten.
    let resumed = pull(&b, &remote, applying())
        .await
        .expect("the restore re-runs");
    assert!(resumed.failed_at.is_none(), "{:?}", resumed.failed_at);
    assert_same_content(&a.roots, &b.roots);
}

// ---------------------------------------------------------------------------
// The stale keyfile (4-08 NEW-3)
// ---------------------------------------------------------------------------

/// A machine that missed a rekey is **refused a push**, not silently skipped —
/// and can still pull, because a restore consults no local keyfile at all.
///
/// The convenient answer was to skip the keyfile upload and let the push
/// succeed. It was rejected: the remote would be correct while the old password
/// kept opening this machine's local keyfile indefinitely, and the whole value
/// of `sync rekey` is that the old wrapper is destroyed. The two halves compose
/// here: the refusal is real, and the machine is not stranded by it.
#[tokio::test]
async fn a_machine_that_missed_a_rekey_is_refused_a_push_and_can_still_pull() {
    // FLOOR rather than CHEAP: `Keyfile::rewrap` enforces the write-path memory
    // floor, so a rekey cannot run at the cheapest parameters.
    let a = Machine::at(alice_roots, 0x11, FLOOR, PASSWORD);
    let stale = Machine::at(bob_roots, 0x11, FLOOR, PASSWORD);
    seed_a_full_tree(&a);
    let remote = Remote::new().await;
    push(&a, &remote).await.expect("the first push lands");

    // A changes the password. The old wrapper is destroyed on the remote.
    let client = remote.client();
    let repo = repo();
    let mut ctx = a.push_ctx(&client, &repo);
    ctx.previous = None;
    let new_asset = rekey::run(
        &ctx,
        &Zeroizing::new(PASSWORD.into()),
        &Zeroizing::new(NEW_PASSWORD.into()),
    )
    .await
    .expect("the rekey lands");
    assert_eq!(
        remote.with(|st| st.pointer_value().expect("published").keyfile),
        new_asset,
        "the pointer still names the old wrapper"
    );

    // The stale machine still holds the superseded wrapper. Its push refuses,
    // before a byte is packed.
    stale.seed(
        &stale.roots.claude_home,
        "scheduled-tasks/from-stale.json",
        b"{}",
    );
    let uploads_before = remote.with(|st| st.uploads);
    let err = push(&stale, &remote)
        .await
        .expect_err("a stale machine must not republish the superseded wrapper");
    let message = err.to_string();
    assert!(
        message.contains("the sync password was changed on another machine"),
        "the refusal does not say what happened: {message}"
    );
    assert!(
        message.contains("keyfile.json"),
        "the refusal does not name the catch-up: {message}"
    );
    assert_eq!(
        remote.with(|st| st.uploads),
        uploads_before,
        "the refused push uploaded something"
    );
    assert!(
        !remote
            .with(|st| st.live_names())
            .contains(&stale.keyfile_asset),
        "the destroyed wrapper is back on the release"
    );

    // But it is not stranded: a pull needs no local keyfile, only the password
    // that opens the *published* wrapper.
    let mut restorer = Machine::at(bob_roots, 0x11, FLOOR, PASSWORD);
    restorer.password = Zeroizing::new(NEW_PASSWORD.into());
    let outcome = pull(&restorer, &remote, applying())
        .await
        .expect("the new password opens the rekeyed bundle");
    assert!(outcome.failed_at.is_none());
    assert_same_content(&a.roots, &restorer.roots);
}

// ---------------------------------------------------------------------------
// 6-07 — the second machine is not read-only
// ---------------------------------------------------------------------------

/// **The gap this plan closes, as one round trip.** A second machine sets up
/// against a repository that already holds a bundle, pushes back into it, and
/// the first machine restores what it sent.
///
/// It fails without the change, and it fails at step 3 rather than at an
/// assertion: `sync setup` called `Keyfile::create` unconditionally, so B's
/// operator was shown a **generated** password for a bundle that already had
/// one — which is why [`Joining::passphrase`] panics rather than returning a
/// string. The consequence was asymmetric and easy to miss: `sync pull` worked,
/// because `restore::fetch::resolve` opens the keyfile the *pointer* names and
/// never consults a local one, while `sync push` was refused by
/// `upload::assert_keyfile_is_current` — correctly, since B's fresh wrapper is
/// not the published one. A machine that can only read is not a second machine.
///
/// Byte-identity is asserted rather than "opens under the same password"
/// because byte-identity is what the push side actually compares: two keyfiles
/// wrapping the same master key under the same password are still two different
/// assets with two different content addresses.
#[tokio::test]
async fn a_second_machine_joins_the_published_bundle_and_its_push_is_accepted() {
    let a = Machine::alice();
    seed_a_full_tree(&a);
    let remote = Remote::new().await;
    push(&a, &remote).await.expect("machine A publishes");

    let published = remote.with(|st| st.pointer_value().expect("published").keyfile);
    let asset = remote.with(|st| {
        st.assets
            .iter()
            .find(|x| x.name == published)
            .expect("the keyfile asset A uploaded")
            .bytes
            .clone()
    });

    // ---- 2. B sets up against the same remote, with A's password ----------
    let (b, prompt) = set_up_second_machine(&remote, PASSWORD)
        .await
        .expect("the published bundle is joinable with its own password");

    assert_eq!(
        std::fs::read(keyfile_path(&b.roots)).expect("setup wrote a keyfile"),
        asset,
        "B's keyfile is the published asset byte for byte, which is what makes its push \
         a continuation rather than a divergent second bundle"
    );
    assert_eq!(
        b.keyfile_asset, published,
        "and it addresses to the same name"
    );
    assert_eq!(
        prompt.asked, 1,
        "one ask, and it was the existing-password one"
    );
    assert!(
        !prompt.said.join("\n").contains(PASSWORD),
        "the password never reaches the narration"
    );

    // ---- 3. B pushes, and is accepted ------------------------------------
    b.seed(
        &b.roots.claude_home,
        "scheduled-tasks/from-the-second-machine.json",
        br#"{"cron":"@daily"}"#,
    );
    push(&b, &remote)
        .await
        .expect("the second machine's push is refused — this is the gap");
    let landed_pointer = remote.with(|st| st.pointer_value().expect("published"));
    assert_eq!(
        landed_pointer.snapshots.len(),
        2,
        "B continued A's history rather than starting one"
    );
    assert_eq!(
        landed_pointer.keyfile, published,
        "a join must not republish a second wrapper"
    );

    // ---- 4. …and A restores what B sent ----------------------------------
    let landed = pull(&a, &remote, applying())
        .await
        .expect("A opens B's snapshot with the one master key both machines hold");
    assert!(landed.failed_at.is_none(), "{landed:?}");
    assert_eq!(
        std::fs::read(
            a.roots
                .claude_home
                .join("scheduled-tasks/from-the-second-machine.json")
        )
        .expect("B's file arrived on A"),
        br#"{"cron":"@daily"}"#,
    );
}

/// The other half of the same seam: a machine that types the wrong password
/// leaves **no** keyfile behind.
///
/// `existing_keyfile_message` refuses to overwrite one, by design and for a good
/// reason — so a keyfile written before the unwrap proved anything would strand
/// the user behind a file that opens with a password nobody has, on a machine
/// setup then declines to run again.
#[tokio::test]
async fn a_wrong_password_on_the_second_machine_leaves_nothing_to_strand_it() {
    let a = Machine::alice();
    seed_a_full_tree(&a);
    let remote = Remote::new().await;
    push(&a, &remote).await.expect("machine A publishes");

    let before = remote.with(|st| st.pointer.clone());
    let dir = TempDir::new().expect("a temp dir");
    let roots = bob_roots(dir.path());
    let err = match set_up_second_machine_at(&remote, &roots, NEW_PASSWORD).await {
        Ok(()) => panic!("that password opens nothing on this bundle"),
        Err(e) => e.to_string(),
    };
    assert!(err.contains("did not open this bundle's keyfile"), "{err}");
    assert!(
        !err.contains(NEW_PASSWORD),
        "the attempt is not echoed back: {err}"
    );
    assert!(
        !keyfile_path(&roots).exists(),
        "a keyfile survived a password that never opened one — the next run would refuse it"
    );
    assert!(!pairing::default_path(&roots).exists(), "no pairing record");

    // And the remote is exactly where A left it: setup uploads nothing (D-05),
    // and a refusal at step 3 is before the one step that writes anything.
    assert_eq!(remote.with(|st| st.pointer.clone()), before);
    assert!(
        remote.with(|st| st.deleted.is_empty()),
        "nothing was deleted"
    );
}

// ---------------------------------------------------------------------------
// Criterion 5 — the anchor, and what allow_rollback never rescues
// ---------------------------------------------------------------------------

/// **ROADMAP §Phase 5 criterion 7**, the anchor half — a failed pull leaves the anchor
/// byte-identical, whatever the failure was.
///
/// A forged high counter that advanced the anchor on a *claim* rather than on a
/// verified snapshot would lock the user out of their own real bundle: a denial
/// of service anyone with repo write access could trigger at will, built out of
/// the very mechanism meant to protect them.
#[tokio::test]
async fn criterion_7_a_failed_pull_never_advances_the_anchor() {
    let a = Machine::alice();
    let b = Machine::bob();
    seed_a_full_tree(&a);
    let remote = Remote::new().await;
    push(&a, &remote).await.expect("the push lands");

    // A high-water mark this machine really has seen.
    std::fs::create_dir_all(b.anchor_path().parent().unwrap()).unwrap();
    anchor::write_to(
        &b.anchor_path(),
        &Anchor {
            repo_id: push::repo_id_for(1),
            counter: 7,
        },
    )
    .expect("an anchor");
    let anchored = b.anchor_bytes();

    // 1. A counter below the mark.
    let err = pull(&b, &remote, applying())
        .await
        .expect_err("a lower counter must refuse");
    assert!(err.to_string().contains("--allow-rollback"), "{err}");
    assert_anchor_frozen(&b, anchored.clone(), "a rolled-back counter");

    // 2. A counter borrowed from a different bundle by renaming — refused even
    //    under the flag. The escape is for an older snapshot of the *same*
    //    bundle, never for a counter taken from another one.
    anchor::write_to(
        &b.anchor_path(),
        &Anchor {
            repo_id: "github:999".into(),
            counter: 1,
        },
    )
    .expect("an anchor for another bundle");
    let borrowed = b.anchor_bytes();
    let err = pull(
        &b,
        &remote,
        RestoreOptions {
            apply: true,
            allow_rollback: true,
            ..Default::default()
        },
    )
    .await
    .expect_err("a borrowed counter must be refused under the flag too");
    assert!(
        err.to_string().contains("anchored to bundle"),
        "the refusal came from somewhere other than the anchor: {err}"
    );
    assert_anchor_frozen(&b, borrowed, "a bundle-identity mismatch");

    // 3. A malformed pointer, which fails before anything is opened at all.
    remote.with(|st| st.pointer = Some(("x".into(), b"not json".to_vec())));
    let before = b.anchor_bytes();
    pull(&b, &remote, applying())
        .await
        .expect_err("a malformed pointer must refuse");
    assert_anchor_frozen(&b, before, "a malformed pointer");
    assert!(b.restored().is_empty(), "{:?}", b.restored());
    assert!(!b.backups().exists(), "a refused restore took an archive");
}
