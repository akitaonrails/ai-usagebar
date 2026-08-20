//! The snapshot object graph: the root (fresh random nonce, monotonic counter),
//! the manifest carried as ordinary sealed chunks, and the index object with
//! its `supersedes` link.
//!
//! # The chain, and why every hop names the next one
//!
//! ```text
//! root  --manifest_chunks-->  manifest  --chunk ids-->  chunks
//! ```
//!
//! Each hop's identifier is bound as associated data into the object it names:
//! `Keys::seal` takes the [`ChunkId`] as AAD, and
//! [`crate::sync::chunk::open_chunk`] rechecks
//! that the plaintext really hashes to the id it was served under. Substituting
//! a manifest or a chunk therefore fails its tag rather than quietly restoring
//! something else.
//!
//! # Ordering integrity lives here, not in the chunker
//!
//! A chunk carries no position. Plan 1-02 proved with a test that transposing
//! two whole `(id, ciphertext)` pairs cannot be detected at the chunk layer:
//! each pair still decrypts cleanly and still hashes to its own id, and the only
//! observable is a reordered buffer. CRYPTO-05 names reordering as something
//! that must be detected, so the detection is this module's job — the manifest's
//! ordered chunk list is the only place position exists at all.
//!
//! It is closed by construction rather than by a check: the list of ids sits
//! inside the manifest's sealed plaintext, so transposing two of them either
//! breaks the Poly1305 tag of the chunk they live in (if edited in place) or
//! changes that chunk's own id (if re-sealed), and the root names the old ids.
//! The root's own `manifest_chunks` list is closed the same way one level up:
//! it sits inside the root's sealed plaintext, which only the key holder can
//! re-seal. Either way the reader gets an error and zero entries, never a
//! reordered manifest.
//!
//! # The manifest spans as many chunks as it needs
//!
//! One chunk holds at most [`crate::sync::CHUNK_SIZE`] of plaintext, and the
//! manifest passes that long before any single payload file does. Each entry is
//! a path, a mode, a length, and a 64-hex chunk id — call it 290 bytes once the
//! paths are real ones — so the *default* bundle measured on this milestone's
//! target machine is 1,558 entries and 448 KiB, already 192 KiB past one chunk.
//! Enabling transcripts takes it past 5,700 entries. A single-chunk manifest
//! could not express the common case, let alone the large one, and compression
//! does not rescue it: the refusal is on the plaintext length handed to
//! [`crate::sync::chunk::frame`], long before zstd sees it.
//!
//! So [`Manifest::seal`] returns the ordered [`Blob`] list
//! [`crate::sync::chunk::seal_all`] produced, [`Root::manifest_chunks`] holds
//! their ids in that order, and [`Manifest::open`] goes back through
//! [`crate::sync::chunk::reassemble`], which rechecks every chunk against the id
//! it was served under before a byte of it is parsed. The index object shares
//! those helpers and is therefore unbounded in the same way — no bundle-sized
//! object in this format has to fit in one chunk.
//!
//! Owned by plan 1-04, split across chunks by plan 1-09.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::error::{AppError, Result};
use crate::sync::chunk::{Blob, reassemble, seal_all};
use crate::sync::crypto::{ChunkId, KdfParams, Keys};
use crate::sync::{
    CHUNKER_ID, INDEX_VERSION, MANIFEST_VERSION, MAX_SUPPORTED_INDEX, MAX_SUPPORTED_MANIFEST,
    MAX_SUPPORTED_ROOT, ROOT_VERSION, check_version,
};

/// Every chunker this build knows how to read.
///
/// Membership, deliberately, not equality with [`CHUNKER_ID`]: a build that
/// introduces a second chunker must still read the bundles it wrote with the
/// first. An equality check is the same unevolvable-format mistake as an
/// equality check on `format`.
const KNOWN_CHUNKERS: &[&str] = &[CHUNKER_ID];

fn check_chunker(found: &str) -> Result<()> {
    if KNOWN_CHUNKERS.contains(&found) {
        return Ok(());
    }
    // `found` comes from a remote, so it is bounded and `{:?}`-escaped before it
    // reaches a message. It is not secret — it is a chunker name, and the bytes
    // it arrived in have already authenticated.
    let shown: String = found.chars().take(32).collect();
    Err(AppError::Other(format!(
        "this bundle was split by an unknown chunker {shown:?} — \
         upgrade ai-usagebar to read it"
    )))
}

/// Just enough of any versioned object to read its `format`.
#[derive(Deserialize)]
struct VersionProbe {
    format: u32,
}

/// Check the version *before* deserializing the whole object.
///
/// A v2 object may carry fields this build has never heard of, and a required
/// one would make full deserialization fail with a confusing message about a
/// missing field. Probing first means the user is told the true problem: their
/// client is older than the bundle.
fn probe_version(json: &[u8], ceiling: u32, object: &str) -> Result<()> {
    let probe: VersionProbe = serde_json::from_slice(json)
        .map_err(|_| AppError::Other(format!("{object} is malformed")))?;
    check_version(probe.format, ceiling, object)
}

/// Serialize to JSON and seal it as ordinary chunks, in order.
///
/// No compression here: [`seal_all`] already runs zstd inside each frame, and
/// compressing twice costs CPU for nothing while adding a second determinism
/// surface to keep byte-stable.
///
/// Small objects still seal to exactly one chunk — [`seal_all`] splits only when
/// the JSON actually exceeds one — so nothing pays for the general case.
fn seal_object<T: Serialize>(keys: &Keys, value: &T, object: &str) -> Result<Vec<Blob>> {
    let json = Zeroizing::new(
        serde_json::to_vec(value)
            .map_err(|_| AppError::Other(format!("{object} serialization failed")))?,
    );
    seal_all(keys, &json)
}

/// Open a sealed object: per-chunk AEAD tag and id recheck, reassembly in the
/// order given, version ceiling, then the shape.
///
/// A failure anywhere returns no object at all rather than a partial one, which
/// is what makes a reordered or truncated chunk list read as an error instead of
/// as a shorter manifest.
fn open_object<T: DeserializeOwned>(
    keys: &Keys,
    chunks: &[(ChunkId, Vec<u8>)],
    ceiling: u32,
    object: &str,
) -> Result<T> {
    let json = reassemble(keys, chunks)?;
    probe_version(&json, ceiling, object)?;
    serde_json::from_slice(&json).map_err(|_| AppError::Other(format!("{object} is malformed")))
}

/// One file in a snapshot, and the chunks it is made of **in order**.
///
/// `true_len` is the file's real length, which the sealed chunks do not carry:
/// the last one is padded to a power of two so its size reveals only a bucket.
///
/// `Debug` is derived and prints `path`. A path is metadata, not key material,
/// and no manifest ever leaves this process unsealed — the D5 rule is about key
/// material and plaintext file *contents*, neither of which lives here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub mode: u32,
    pub true_len: u64,
    pub chunks: Vec<ChunkId>,
}

/// The map from files to their ordered chunk ids.
///
/// Sealed as an ordinary chunk, deliberately: paths, sizes, modes, and the whole
/// directory shape are exactly the metadata a hostile remote would like, so none
/// of it may sit in the clear beside the data it describes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub chunker: String,
    pub files: Vec<FileEntry>,
}

impl Manifest {
    /// Stamp the version and chunker this build writes.
    ///
    /// Both come from the constants in [`crate::sync`] rather than a re-typed
    /// literal, so there is one source of truth for each.
    pub fn new(files: Vec<FileEntry>) -> Self {
        Self {
            format: MANIFEST_VERSION,
            chunker: CHUNKER_ID.to_string(),
            files,
        }
    }

    /// Seal into one chunk or several, in order. Their ids, in that order, are
    /// what [`Root::manifest_chunks`] names.
    pub fn seal(&self, keys: &Keys) -> Result<Vec<Blob>> {
        seal_object(keys, self, "manifest")
    }

    /// Open the manifest [`Root::manifest_chunks`] names, in that order,
    /// refusing a version above this build's ceiling and a chunker it does not
    /// know.
    pub fn open(keys: &Keys, chunks: &[(ChunkId, Vec<u8>)]) -> Result<Manifest> {
        Self::open_with_ceiling(keys, chunks, MAX_SUPPORTED_MANIFEST)
    }

    /// The ceiling as a parameter, following the same seam as [`KdfParams`]:
    /// nothing below this line reads a constant it could instead be handed.
    fn open_with_ceiling(
        keys: &Keys,
        chunks: &[(ChunkId, Vec<u8>)],
        ceiling: u32,
    ) -> Result<Manifest> {
        let manifest: Manifest = open_object(keys, chunks, ceiling, "manifest")?;
        check_chunker(&manifest.chunker)?;
        Ok(manifest)
    }

    /// Every referenced chunk id that is not in `available`.
    ///
    /// Phase 5 restores through this so a dropped chunk is reported as a missing
    /// chunk. Skipping it and concatenating whatever arrived would write a
    /// *shorter* credential file and call it a success, which is the difference
    /// between a detected truncation and a silently corrupted secret.
    pub fn missing_chunks(&self, available: &HashSet<ChunkId>) -> Vec<ChunkId> {
        self.files
            .iter()
            .flat_map(|f| f.chunks.iter())
            .filter(|id| !available.contains(id))
            .copied()
            .collect()
    }
}

/// Where one sealed chunk lives inside a pack.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexEntry {
    pub id: ChunkId,
    pub pack: ChunkId,
    pub offset: u64,
    pub clen: u32,
    pub true_len: u32,
}

/// The chunk-id → pack-location map, sealed as an ordinary chunk like the
/// manifest.
///
/// `supersedes` names the index objects a repack replaced. Phase 4 deletes in
/// that order — an index must stop referencing a pack *before* the pack is
/// deleted, or a concurrent reader follows a pointer to nothing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexObject {
    pub format: u32,
    pub entries: Vec<IndexEntry>,
    pub supersedes: Vec<ChunkId>,
}

impl IndexObject {
    pub fn new(entries: Vec<IndexEntry>, supersedes: Vec<ChunkId>) -> Self {
        Self {
            format: INDEX_VERSION,
            entries,
            supersedes,
        }
    }

    /// Sealed the same way a manifest is, and for the same reason: one entry per
    /// chunk in the bundle puts a large snapshot's index past [`CHUNK_SIZE`] too.
    ///
    /// [`CHUNK_SIZE`]: crate::sync::CHUNK_SIZE
    pub fn seal(&self, keys: &Keys) -> Result<Vec<Blob>> {
        seal_object(keys, self, "index object")
    }

    pub fn open(keys: &Keys, chunks: &[(ChunkId, Vec<u8>)]) -> Result<IndexObject> {
        open_object(keys, chunks, MAX_SUPPORTED_INDEX, "index object")
    }

    /// Linear scan. `ponytail:` fine at Phase 1 sizes; if a restore ever
    /// resolves thousands of ids against one index, build a `HashMap` once at
    /// the call site rather than growing a cache in here.
    pub fn resolve(&self, id: &ChunkId) -> Option<&IndexEntry> {
        self.entries.iter().find(|e| e.id == *id)
    }
}

/// The snapshot pointer: the one mutable object in the format.
///
/// `chunker` and `kdf` are informational duplicates — the authoritative copy of
/// the KDF parameters lives in the keyfile, where they are bound as associated
/// data and cannot be edited in transit. They are repeated here because the root
/// is the *first* object a reader touches, so an unknown chunker or an
/// unsupported KDF configuration can be refused before a single pack is fetched.
///
/// **The two copies are not compared, and this build does not claim to.** An
/// earlier draft of this doc said a disagreement was "worth reporting"; nothing
/// reported it, and describing behaviour that does not exist is worse than
/// describing none. Comparing them would need a warning channel `src/sync/` does
/// not have — it is pure, and an *error* would contradict "the keyfile wins" by
/// making a cosmetic disagreement unreadable. Nor is a disagreement reachable by
/// an attacker: `kdf` sits inside the root's authenticated plaintext, and the
/// keyfile's copy is AAD-bound, so only the key holder's own writer can produce
/// one. Whichever phase gains a warning channel may add the comparison there;
/// until then a reader uses the keyfile's parameters, full stop.
///
/// `manifest_chunks` is ordered and is the only place the manifest's chunk order
/// is recorded. It is a list rather than a single id because a real bundle's
/// manifest does not fit in one chunk — and it lives here, inside the root's
/// sealed plaintext, so reordering it is not something a hostile remote can do.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Root {
    pub format: u32,
    pub counter: u64,
    pub created_at: DateTime<Utc>,
    pub repo_id: String,
    pub manifest_chunks: Vec<ChunkId>,
    pub chunker: String,
    pub kdf: KdfParams,
}

impl Root {
    /// `now` is a parameter and never a wall-clock read. Nothing in this module
    /// reads the clock, which is what lets every test pin a fixed timestamp and
    /// stay hermetic.
    pub fn new(
        counter: u64,
        now: DateTime<Utc>,
        repo_id: String,
        manifest_chunks: Vec<ChunkId>,
        kdf: KdfParams,
    ) -> Self {
        Self {
            format: ROOT_VERSION,
            counter,
            created_at: now,
            repo_id,
            manifest_chunks,
            chunker: CHUNKER_ID.to_string(),
            kdf,
        }
    }

    /// Seal under the root subkey with a **fresh random** nonce.
    ///
    /// This is the one place the format's deterministic-nonce rule is inverted,
    /// and deliberately: every other object's nonce is derived from the bytes it
    /// seals, because identical plaintext *must* seal identically or dedup dies.
    /// The root's plaintext changes on every sync, and a content-derived nonce
    /// would publish whether two consecutive snapshots are identical.
    pub fn seal(&self, keys: &Keys) -> Result<Vec<u8>> {
        let json = Zeroizing::new(
            serde_json::to_vec(self)
                .map_err(|_| AppError::Other("snapshot root serialization failed".into()))?,
        );
        keys.seal_root(&json, &self.repo_id)
    }

    /// Open a framed root, refusing a version above this build's ceiling and a
    /// chunker it does not know.
    ///
    /// **`expect_repo_id` comes from local configuration, never from the remote.**
    /// It is bound as associated data, so a root belonging to another bundle
    /// fails the tag before anything is parsed — a repo swap is caught
    /// cryptographically rather than by comparing the remote's own claim against
    /// itself. The `repo_id` *inside* the plaintext is rechecked against it
    /// afterwards, the same belt and braces
    /// [`crate::sync::chunk::open_chunk`] applies to a chunk id: the AAD proves
    /// the writer meant this bundle, the recheck proves the two copies agree.
    pub fn open(keys: &Keys, framed: &[u8], expect_repo_id: &str) -> Result<Root> {
        let json = keys.open_root(framed, expect_repo_id)?;
        probe_version(&json, MAX_SUPPORTED_ROOT, "snapshot root")?;
        let root: Root = serde_json::from_slice(&json)
            .map_err(|_| AppError::Other("snapshot root is malformed".into()))?;
        if root.repo_id != expect_repo_id {
            return Err(AppError::Other(
                "the snapshot root names a different bundle than its own associated data".into(),
            ));
        }
        check_chunker(&root.chunker)?;
        Ok(root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::crypto::Keyfile;

    /// Microseconds instead of ~1.5 s and a gibibyte. Never use production
    /// parameters in a unit test: the AUR `check()` runs these on an
    /// installer's machine.
    const CHEAP: KdfParams = KdfParams {
        m_kib: 8,
        t: 1,
        p: 1,
    };

    fn keys_from(password: &[u8]) -> Keys {
        Keyfile::create_with_floor(password, CHEAP, CHEAP.m_kib)
            .expect("keyfile creation")
            .1
    }

    fn keys() -> Keys {
        keys_from(b"correct horse battery staple")
    }

    /// Fixed, injected, never `Utc::now()`.
    fn fixed_time() -> DateTime<Utc> {
        "2026-08-19T12:00:00Z".parse().expect("a valid timestamp")
    }

    /// What a caller reads out of its own configuration and passes to
    /// [`Root::open`] — never a value taken from the remote.
    const REPO_ID: &str = "usagebar-sync-abc123";

    fn id(n: u8) -> ChunkId {
        ChunkId::from_bytes([n; 32])
    }

    /// What a reader following a root's `manifest_chunks` list would fetch and
    /// hand to `open`: the `(id, ciphertext)` pairs, in order.
    fn served(blobs: &[Blob]) -> Vec<(ChunkId, Vec<u8>)> {
        blobs.iter().map(|b| (b.id, b.ciphertext.clone())).collect()
    }

    fn three_files() -> Vec<FileEntry> {
        vec![
            FileEntry {
                path: ".claude/.credentials.json".into(),
                mode: 0o600,
                true_len: 812,
                chunks: vec![id(1)],
            },
            FileEntry {
                path: ".config/ai-usagebar/config.toml".into(),
                mode: 0o600,
                true_len: 300_000,
                chunks: vec![id(2), id(3)],
            },
            FileEntry {
                path: ".local/share/ai-usagebar/history.jsonl".into(),
                mode: 0o644,
                true_len: 0,
                chunks: vec![],
            },
        ]
    }

    // ---- manifest -------------------------------------------------------

    #[test]
    fn a_three_file_manifest_seals_and_reopens_identically() {
        let keys = keys();
        let manifest = Manifest::new(three_files());
        let blobs = manifest.seal(&keys).expect("seal");
        assert_eq!(blobs.len(), 1, "a small manifest must not be split");
        assert_eq!(
            Manifest::open(&keys, &served(&blobs)).expect("open"),
            manifest
        );
    }

    #[test]
    fn a_manifest_opened_under_the_wrong_id_is_refused() {
        let keys = keys();
        let blobs = Manifest::new(three_files()).seal(&keys).expect("seal");
        let mut chunks = served(&blobs);
        let mut wrong = *chunks[0].0.as_bytes();
        wrong[0] ^= 1;
        chunks[0].0 = ChunkId::from_bytes(wrong);
        assert!(Manifest::open(&keys, &chunks).is_err());
    }

    #[test]
    fn a_manifest_below_the_ceiling_opens_and_the_ceiling_may_be_raised() {
        let keys = keys();

        // An older client's manifest still opens under today's ceiling…
        let mut older = Manifest::new(three_files());
        older.format = MAX_SUPPORTED_MANIFEST - 1;
        let blobs = older.seal(&keys).expect("seal");
        assert_eq!(Manifest::open(&keys, &served(&blobs)).expect("open"), older);

        // …and today's manifest still opens on a future build whose ceiling is
        // higher. An equality check against the current version breaks both
        // directions, which is the whole reason the rule is at-or-below.
        let today = Manifest::new(three_files());
        let blobs = today.seal(&keys).expect("seal");
        let raised = MAX_SUPPORTED_MANIFEST + 1;
        let reopened = Manifest::open_with_ceiling(&keys, &served(&blobs), raised).expect("open");
        assert_eq!(reopened, today);
    }

    #[test]
    fn a_manifest_one_version_above_the_ceiling_is_refused() {
        let keys = keys();
        let future = Manifest {
            format: MAX_SUPPORTED_MANIFEST + 1,
            chunker: CHUNKER_ID.to_string(),
            files: three_files(),
        };
        let blobs = future.seal(&keys).expect("seal");
        let err = Manifest::open(&keys, &served(&blobs)).expect_err("must refuse");
        assert!(err.to_string().contains("upgrade ai-usagebar"));
    }

    #[test]
    fn a_manifest_naming_an_unknown_chunker_is_refused_and_the_message_names_it() {
        let keys = keys();
        let odd = Manifest {
            format: MANIFEST_VERSION,
            chunker: "rolling-cdc-v9".into(),
            files: three_files(),
        };
        let blobs = odd.seal(&keys).expect("seal");
        let err = Manifest::open(&keys, &served(&blobs)).expect_err("must refuse");
        assert!(err.to_string().contains("rolling-cdc-v9"));
    }

    #[test]
    fn no_sealed_manifest_chunk_carries_a_file_path_in_the_clear() {
        let keys = keys();
        // A multi-chunk manifest, so the check covers every chunk rather than
        // only the first one.
        let mut files = three_files();
        files.extend(many_files(4_000));
        let blobs = Manifest::new(files).seal(&keys).expect("seal");
        assert!(blobs.len() > 1, "the fixture must span several chunks");

        for needle in [
            b".credentials.json".as_slice(),
            b"config.toml".as_slice(),
            b"history.jsonl".as_slice(),
            b".claude/projects".as_slice(),
        ] {
            for blob in &blobs {
                assert!(
                    !blob.ciphertext.windows(needle.len()).any(|w| w == needle),
                    "a file path leaked into a sealed manifest chunk"
                );
            }
        }
    }

    #[test]
    fn the_chunker_reads_back_as_the_constant_this_build_writes() {
        let keys = keys();
        let blobs = Manifest::new(three_files()).seal(&keys).expect("seal");
        let reopened = Manifest::open(&keys, &served(&blobs)).expect("open");
        assert_eq!(reopened.chunker, "fixed-256k");
        assert_eq!(reopened.chunker, CHUNKER_ID);
    }

    /// Ordering integrity is this module's own requirement — the chunk layer
    /// demonstrably cannot provide it (1-02 Deviation 2). This is the shape
    /// 1-06 Attack 8 asserts against.
    #[test]
    fn transposing_two_chunk_ids_inside_a_sealed_manifest_yields_zero_entries() {
        let keys = keys();
        let ordered = Manifest::new(vec![FileEntry {
            path: "a.jsonl".into(),
            mode: 0o600,
            true_len: 900_000,
            chunks: vec![id(1), id(2), id(3), id(4)],
        }]);
        let blobs = ordered.seal(&keys).expect("seal");
        let honest = served(&blobs);

        let mut transposed = ordered.clone();
        transposed.files[0].chunks.swap(1, 2);
        let evil = served(&transposed.seal(&keys).expect("seal"));

        // Re-sealing a reordered list produces a *different* address, so the
        // root that names the honest ids never points at it...
        assert_ne!(evil[0].0, honest[0].0);
        // ...and serving its bytes under the honest id fails outright.
        let mut swapped_in = honest.clone();
        swapped_in[0].1 = evil[0].1.clone();
        assert!(Manifest::open(&keys, &swapped_in).is_err());

        // Editing the ordered list in place breaks the tag just as hard.
        let mut flipped = honest.clone();
        flipped[0].1[0] ^= 1;
        assert!(Manifest::open(&keys, &flipped).is_err());

        // And the honest manifest still opens with its order intact.
        let reopened = Manifest::open(&keys, &honest).expect("open");
        assert_eq!(reopened.files[0].chunks, vec![id(1), id(2), id(3), id(4)]);
    }

    /// The same requirement one level up: the *manifest's own* chunk order lives
    /// in `Root.manifest_chunks`, so the attack has to happen after the root is
    /// sealed — which is exactly what it cannot do. 1-06 Attack 8 asserts this
    /// shape.
    #[test]
    fn transposing_manifest_chunks_after_the_root_is_sealed_yields_zero_entries() {
        let keys = keys();
        let manifest = Manifest::new(many_files(1_600));
        let blobs = manifest.seal(&keys).expect("seal");
        assert!(blobs.len() > 1, "the fixture must span several chunks");

        let ordered: Vec<ChunkId> = blobs.iter().map(|b| b.id).collect();
        let framed = Root::new(
            7,
            fixed_time(),
            REPO_ID.to_string(),
            ordered.clone(),
            KdfParams::default(),
        )
        .seal(&keys)
        .expect("seal");

        // The list is inside the root's sealed plaintext: transposing two ids
        // means re-sealing the root, which needs the key. Served under the
        // original root ciphertext, the order comes back exactly as written.
        assert_eq!(
            Root::open(&keys, &framed, REPO_ID)
                .expect("open")
                .manifest_chunks,
            ordered
        );

        // And if a reader did follow a reordered list, reassembly hands back an
        // error rather than a manifest — `Result` carries no `Manifest`, so
        // "zero entries" is structural, not a length check.
        let mut reordered = served(&blobs);
        reordered.swap(0, 1);
        let err = Manifest::open(&keys, &reordered).expect_err("must refuse");
        assert!(err.to_string().contains("manifest is malformed"));
        assert!(!err.to_string().contains(".claude/projects"));
    }

    #[test]
    fn a_referenced_chunk_that_is_absent_is_reported_missing_not_skipped() {
        let manifest = Manifest::new(three_files());
        let available: HashSet<ChunkId> = [id(1), id(3)].into_iter().collect();
        assert_eq!(manifest.missing_chunks(&available), vec![id(2)]);

        let all: HashSet<ChunkId> = [id(1), id(2), id(3)].into_iter().collect();
        assert!(manifest.missing_chunks(&all).is_empty());
    }

    /// Entries shaped like the ones the measurement in the module doc came from:
    /// project-scoped chat-session-index paths, one chunk each. A fixture built
    /// from short synthetic paths seals far smaller than the real thing and would
    /// let a size test pass without ever exercising a split.
    fn many_files(n: u32) -> Vec<FileEntry> {
        (0..n)
            .map(|i| FileEntry {
                path: format!(
                    ".claude/projects/-Users-someone-src-a-project-with-a-fairly-long-name/\
                     0199f{i:03x}-4a1b-7c2d-9e3f-{i:012x}.jsonl"
                ),
                mode: 0o600,
                true_len: 4096,
                chunks: vec![id((i % 251) as u8)],
            })
            .collect()
    }

    /// Guards the fixture itself. The size tests below are only meaningful while
    /// one entry costs about what a real one does: the measured default bundle is
    /// 448 KiB over 1,558 entries, ~294 bytes each, and this fixture is 229. A
    /// fixture that drifts far below that stops exercising the split and starts
    /// passing vacuously, which is exactly how the first draft of these tests
    /// failed.
    #[test]
    fn the_many_files_fixture_matches_the_measured_bundles_bytes_per_entry() {
        let bytes = serde_json::to_vec(&Manifest::new(many_files(1_600)))
            .expect("serialize")
            .len();
        let per_entry = bytes / 1_600;
        assert!(
            (150..400).contains(&per_entry),
            "one entry serializes to {per_entry} bytes, far from a real bundle's ~294"
        );
    }

    /// Where the boundary actually falls: 1,000 entries is ~224 KiB, inside one
    /// 256 KiB chunk, so a small manifest is still not split. Deliberately close
    /// to the edge — if a fixture change pushes it over, this fails loudly rather
    /// than letting the split tests below quietly become the only coverage.
    #[test]
    fn a_thousand_file_manifest_still_fits_in_one_chunk_and_round_trips() {
        let keys = keys();
        let manifest = Manifest::new(many_files(1_000));
        let blobs = manifest.seal(&keys).expect("seal");
        assert_eq!(blobs.len(), 1, "this one really does fit in a single chunk");
        assert_eq!(
            Manifest::open(&keys, &served(&blobs)).expect("open"),
            manifest
        );
    }

    /// The measured default bundle on this milestone's target machine: 1,558
    /// entries, 448 KiB, against a 256 KiB chunk. Before 1-09 this could not
    /// seal at all — the *default* configuration was unshippable.
    #[test]
    fn the_default_bundles_manifest_spans_several_chunks_and_round_trips() {
        let keys = keys();
        let manifest = Manifest::new(many_files(1_600));
        let blobs = manifest.seal(&keys).expect("seal");
        assert!(
            blobs.len() > 1,
            "the fixture must actually be split, or this test passes vacuously"
        );
        assert_eq!(
            Manifest::open(&keys, &served(&blobs)).expect("open"),
            manifest
        );
    }

    /// The same bundle with transcripts enabled — the case 1-04 assumed was the
    /// only one that mattered. ~1.25 MiB of JSON, five chunks.
    #[test]
    fn a_fifty_seven_hundred_file_manifest_round_trips() {
        let keys = keys();
        let manifest = Manifest::new(many_files(5_700));
        let blobs = manifest.seal(&keys).expect("seal");
        assert!(blobs.len() > 2, "several chunks, not merely two");
        assert_eq!(
            Manifest::open(&keys, &served(&blobs)).expect("open"),
            manifest
        );
    }

    /// A truncated chunk list is a detected error, never a shorter manifest —
    /// the same reason `missing_chunks` exists one layer down.
    #[test]
    fn dropping_the_last_manifest_chunk_is_refused_rather_than_read_short() {
        let keys = keys();
        let blobs = Manifest::new(many_files(1_600)).seal(&keys).expect("seal");
        let mut truncated = served(&blobs);
        truncated.pop();
        assert!(Manifest::open(&keys, &truncated).is_err());
        assert!(Manifest::open(&keys, &[]).is_err());
    }

    // ---- snapshot root --------------------------------------------------

    fn a_root() -> Root {
        Root::new(
            7,
            fixed_time(),
            REPO_ID.to_string(),
            vec![id(9), id(10), id(11)],
            KdfParams::default(),
        )
    }

    #[test]
    fn a_root_seals_and_reopens_with_every_field_intact() {
        let keys = keys();
        let root = a_root();
        let reopened = Root::open(&keys, &root.seal(&keys).expect("seal"), REPO_ID).expect("open");
        assert_eq!(reopened, root);
        assert_eq!(reopened.counter, 7);
        assert_eq!(reopened.manifest_chunks, vec![id(9), id(10), id(11)]);
        assert_eq!(reopened.chunker, CHUNKER_ID);
        assert_eq!(reopened.kdf, KdfParams::default());
        assert_eq!(reopened.created_at, fixed_time());
        assert_eq!(reopened.repo_id, "usagebar-sync-abc123");
    }

    #[test]
    fn two_seals_of_one_root_differ_yet_both_reopen() {
        let keys = keys();
        let root = a_root();
        let first = root.seal(&keys).expect("seal");
        let second = root.seal(&keys).expect("seal");
        assert_ne!(
            first, second,
            "the root nonce must be random, unlike a chunk's"
        );
        assert_eq!(Root::open(&keys, &first, REPO_ID).expect("open"), root);
        assert_eq!(Root::open(&keys, &second, REPO_ID).expect("open"), root);
    }

    #[test]
    fn a_root_does_not_open_under_a_different_master_key() {
        let framed = a_root().seal(&keys()).expect("seal");
        let stranger = keys_from(b"a completely different passphrase");
        assert!(Root::open(&stranger, &framed, REPO_ID).is_err());
    }

    /// A whole-repository swap, under a master key the reader really does hold —
    /// the case a shared or copied keyfile creates. `repo_id` is bound as
    /// associated data, so the swap fails the tag rather than resting on the
    /// local anchor having been kept.
    #[test]
    fn a_root_from_another_bundle_is_refused_even_under_the_right_key() {
        let keys = keys();
        let mut theirs = a_root();
        theirs.repo_id = "usagebar-sync-someone-else".into();
        let framed = theirs.seal(&keys).expect("seal");

        assert!(
            Root::open(&keys, &framed, REPO_ID).is_err(),
            "a root belonging to another bundle must not open here"
        );
        // …and it does open for the bundle it was actually written for, so the
        // refusal above is the scoping rather than a broken round trip.
        assert_eq!(
            Root::open(&keys, &framed, "usagebar-sync-someone-else")
                .expect("open")
                .repo_id,
            "usagebar-sync-someone-else"
        );
    }

    #[test]
    fn one_flipped_bit_anywhere_in_a_sealed_root_fails_to_open() {
        let keys = keys();
        let framed = a_root().seal(&keys).expect("seal");
        for byte in 0..framed.len() {
            let mut tampered = framed.clone();
            tampered[byte] ^= 1;
            assert!(
                Root::open(&keys, &tampered, REPO_ID).is_err(),
                "a flipped bit at byte {byte} was accepted"
            );
        }
    }

    #[test]
    fn a_root_naming_an_unknown_chunker_is_refused_and_the_message_names_it() {
        let keys = keys();
        let mut root = a_root();
        root.chunker = "cdc-gear-64k".into();
        let err =
            Root::open(&keys, &root.seal(&keys).expect("seal"), REPO_ID).expect_err("must refuse");
        assert!(err.to_string().contains("cdc-gear-64k"));
    }

    #[test]
    fn a_root_below_the_ceiling_opens_and_one_above_it_is_refused() {
        let keys = keys();
        let mut older = a_root();
        older.format = MAX_SUPPORTED_ROOT - 1;
        assert!(Root::open(&keys, &older.seal(&keys).expect("seal"), REPO_ID).is_ok());

        let mut newer = a_root();
        newer.format = MAX_SUPPORTED_ROOT + 1;
        let err =
            Root::open(&keys, &newer.seal(&keys).expect("seal"), REPO_ID).expect_err("must refuse");
        assert!(err.to_string().contains("upgrade ai-usagebar"));
    }

    // ---- index object ---------------------------------------------------

    #[test]
    fn an_index_object_round_trips_and_resolves_a_known_chunk() {
        let keys = keys();
        let index = IndexObject::new(
            vec![
                IndexEntry {
                    id: id(1),
                    pack: id(200),
                    offset: 0,
                    clen: 4_096,
                    true_len: 3_000,
                },
                IndexEntry {
                    id: id(2),
                    pack: id(200),
                    offset: 4_096,
                    clen: 8_192,
                    true_len: 262_144,
                },
            ],
            vec![id(50), id(51)],
        );
        let blobs = index.seal(&keys).expect("seal");
        let reopened = IndexObject::open(&keys, &served(&blobs)).expect("open");
        assert_eq!(reopened, index);
        assert_eq!(reopened.supersedes, vec![id(50), id(51)]);

        let found = reopened.resolve(&id(2)).expect("a known chunk resolves");
        assert_eq!(found.pack, id(200));
        assert_eq!(found.offset, 4_096);
        assert_eq!(found.clen, 8_192);
        assert_eq!(found.true_len, 262_144);
        assert!(reopened.resolve(&id(99)).is_none());
    }

    #[test]
    fn an_index_object_above_the_ceiling_is_refused() {
        let keys = keys();
        let future = IndexObject {
            format: MAX_SUPPORTED_INDEX + 1,
            entries: vec![],
            supersedes: vec![],
        };
        let blobs = future.seal(&keys).expect("seal");
        let err = IndexObject::open(&keys, &served(&blobs)).expect_err("must refuse");
        assert!(err.to_string().contains("upgrade ai-usagebar"));
    }

    /// The index has one entry per chunk in the bundle, so it outgrows a single
    /// chunk for the same reason the manifest does. It shares the helpers, so it
    /// gets the split for free — this pins that it actually works.
    #[test]
    fn a_bundle_sized_index_object_spans_several_chunks_and_round_trips() {
        let keys = keys();
        let index = IndexObject::new(
            (0..4_000)
                .map(|i| IndexEntry {
                    id: id((i % 251) as u8),
                    pack: id(200),
                    offset: i as u64 * 4_096,
                    clen: 4_096,
                    true_len: 3_000,
                })
                .collect(),
            vec![id(50)],
        );
        let blobs = index.seal(&keys).expect("seal");
        assert!(blobs.len() > 1, "the fixture must span several chunks");
        assert_eq!(
            IndexObject::open(&keys, &served(&blobs)).expect("open"),
            index
        );
    }
}
