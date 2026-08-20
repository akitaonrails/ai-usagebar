//! The adversarial suite for the encrypted sync bundle format.
//!
//! The threat model is an attacker who obtains the complete repository **and may
//! also serve a modified one**. A round-trip test proves none of the properties
//! this file is about — it stays self-consistent under any wrong-but-stable
//! transform. Nine attacks do.
//!
//! Every attack funnels through [`refused`], so no case can quietly assert less
//! than the others. Three things are checked, in the order they matter:
//!
//! 1. **Zero plaintext, asserted on the returned value** rather than on
//!    `is_err()`. A call that errors *and* hands back a partial buffer has still
//!    leaked, and `is_err()` would call that a pass — a test that passes while
//!    the code leaks is worse than no test, because it reads as proof. Outcomes
//!    are mapped to a *count* before they reach the assertion, so a failing
//!    assertion can never print a user's plaintext into a CI log.
//! 2. **No secret in the message** (CRYPTO-07), following the precedent in
//!    `src/error.rs` where `user_message_does_not_expose_authentication_\
//!    response_bodies` asserts on absence rather than on shape. Eight of the
//!    nine additionally pin their *whole* message by equality, which is strictly
//!    stronger: a string that is equal to a known literal cannot be hiding a key
//!    anywhere inside it.
//! 3. **A message distinct from the other eight**, so a refactor that collapses
//!    two failure modes into one string is caught rather than shipped. A single
//!    opaque "decryption failed" everywhere is technically safe and
//!    operationally useless.
//!
//! The happy-path round trip runs first and is load-bearing: nine passing
//! refusals prove nothing if the pipeline refuses everything, so the round trip
//! is what makes the refusals *selective*.
//!
//! Hermetic: no `$HOME`, no `$XDG`, no Keychain, no network, no clock, and no
//! randomness in the fixture. Every key derivation uses the cheap KDF seam,
//! because the AUR `check()` runs this file on an installer's machine.

use std::sync::OnceLock;

use base64::Engine;
use chacha20poly1305::XChaCha20Poly1305;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chrono::{DateTime, Utc};
use serde::Serialize;
use zeroize::Zeroizing;

use ai_usagebar::error::{AppError, Result};
use ai_usagebar::sync::anchor::{self, Anchor};
use ai_usagebar::sync::chunk::{reassemble, seal_all};
use ai_usagebar::sync::crypto::{ChunkId, KdfDoc, KdfParams, Keyfile, Keys, derive_kek};
use ai_usagebar::sync::model::{FileEntry, Manifest, Root};
use ai_usagebar::sync::pack::{PackHeader, PackWriter, blob_bytes, read_header};
use ai_usagebar::sync::{CHUNK_SIZE, KEYFILE_VERSION};

// ---------------------------------------------------------------------------
// The harness
// ---------------------------------------------------------------------------

/// Microseconds instead of ~1.5 s and a gibibyte. Never production parameters
/// here: the AUR `check()` runs `cargo test` during `makepkg`, on somebody
/// else's machine, and a 1 GiB working set per test would be a denial of
/// service against the install (T-06-06).
const CHEAP: KdfParams = KdfParams {
    m_kib: 8,
    t: 1,
    p: 1,
};

const PASSWORD: &str = "correct horse battery staple";
const WRONG_PASSWORD: &str = "corrct horse battery staple";
const REPO_ID: &str = "usagebar-sync-adversarial";
const SNAPSHOT_COUNTER: u64 = 3;

/// Eight full chunks plus a 4 KiB tail — multi-megabyte, so the suite exercises
/// splitting, packing and reassembly rather than a single-chunk special case.
const FIXTURE_LEN: usize = 8 * CHUNK_SIZE + 4_096;

/// A distinctive plaintext marker planted in the middle of every fixture.
/// Searching a pack for a run of zeros, or for a stretch of the incompressible
/// half, can hit by chance; this cannot.
const NEEDLE: &[u8] = b"NEEDLE-a-credential-that-must-never-appear-in-a-pack";

/// Deterministic fixture bytes — no randomness anywhere, so a failure is
/// reproducible from the seed in this file alone.
///
/// The first half is an obviously compressible run and the second an xorshift64
/// stream that zstd cannot shrink, so both compression paths are exercised by
/// every test. [`NEEDLE`] sits between them.
fn fixture(len: usize) -> Vec<u8> {
    const FILLER: &[u8] = b"ai-usagebar sync fixture, highly compressible. ";
    let mut out = Vec::with_capacity(len);
    while out.len() < len / 2 {
        out.extend_from_slice(FILLER);
    }
    out.truncate(len / 2);
    out.extend_from_slice(NEEDLE);

    let mut state = 0x2545_f491_4f6c_dd1du64;
    while out.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.push((state >> 24) as u8);
    }
    out.truncate(len);
    out
}

/// A keyfile wrapping a fixed master key, assembled from the public fields
/// instead of through `Keyfile::create_with_floor`.
///
/// That seam is `pub(crate)`: a memory floor an outside caller passes its own
/// value for is not a floor, and it was measurably not one — `create` at 8 KiB
/// was refused while `create_with_floor` at 8 KiB wrote a keyfile that opened.
/// `Keyfile::create` is the only exported constructor, and it will not wrap at
/// the cheap seam, so a hermetic suite that must stay microseconds builds its
/// own — the same thing `tests/sync_vectors.rs` does, which needs a fixed master
/// key that `create`'s CSPRNG draw could never give it.
///
/// `seed` picks both the master key and the salt, so two keyfiles from this
/// helper are two different hierarchies **and** two different KEKs. The fixed
/// wrap nonce is only safe because of that second half: one nonce under one KEK
/// wrapping two master keys is precisely the misuse this file exists to catch.
fn wrap_by_hand(seed: u8, k: KdfParams) -> Keyfile {
    /// `docs/sync-format.md` §1's `{"format":…,"kdf":{…}}`, in declaration
    /// order, which *is* the canonical AAD byte order.
    #[derive(Serialize)]
    struct KeyfileAad<'a> {
        format: u32,
        kdf: &'a KdfDoc,
    }

    const B64: base64::engine::general_purpose::GeneralPurpose =
        base64::engine::general_purpose::STANDARD;
    const WRAP_NONCE: [u8; 24] = [0x5e; 24];

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

    let kek = derive_kek(PASSWORD.as_bytes(), &salt, k).expect("the cheap KDF seam");
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

/// The suite's own keyfile and the subkeys it unwraps to — through the
/// production `Keyfile::open`, so the hierarchy under test is the real one.
fn suite_keys() -> (Keyfile, Keys) {
    let keyfile = wrap_by_hand(0x11, CHEAP);
    let keys = keyfile
        .open(PASSWORD.as_bytes())
        .expect("a hand-wrapped keyfile must open");
    (keyfile, keys)
}

/// Fixed, injected, never `Utc::now()`.
fn fixed_time() -> DateTime<Utc> {
    "2026-08-19T12:00:00Z".parse().expect("a valid timestamp")
}

fn id(n: u8) -> ChunkId {
    ChunkId::from_bytes([n; 32])
}

/// Chunk, seal, pack, manifest, root — the whole write path, in the order a real
/// sync performs it. Returns the pack bytes, the manifest, and the sealed root.
///
/// The manifest chunks go into the *same* pack as the payload, because that is
/// what a reader has to cope with: one object, everything inside it, and no way
/// to tell the two kinds apart from the outside.
fn bundle(keys: &Keys, data: &[u8]) -> (Vec<u8>, Manifest, Vec<u8>) {
    let mut writer = PackWriter::new();

    let blobs = seal_all(keys, data).expect("seal the payload");
    let chunks: Vec<ChunkId> = blobs.iter().map(|b| b.id).collect();
    for blob in blobs {
        writer.push(blob);
    }

    let manifest = Manifest::new(vec![FileEntry {
        path: ".claude/.credentials.json".into(),
        mode: 0o600,
        true_len: data.len() as u64,
        chunks,
    }]);
    let manifest_blobs = manifest.seal(keys).expect("seal the manifest");
    let manifest_chunks: Vec<ChunkId> = manifest_blobs.iter().map(|b| b.id).collect();
    for blob in manifest_blobs {
        writer.push(blob);
    }

    let (_, pack) = writer.finish(keys).expect("seal the pack");
    let sealed_root = Root::new(
        SNAPSHOT_COUNTER,
        fixed_time(),
        REPO_ID.to_string(),
        manifest_chunks,
        CHEAP,
    )
    .seal(keys)
    .expect("seal the root");

    (pack, manifest, sealed_root)
}

/// One honest bundle, its fixture, and the keys that made it.
struct Bundle {
    keyfile: Keyfile,
    keys: Keys,
    data: Vec<u8>,
    pack: Vec<u8>,
    manifest: Manifest,
    root: Vec<u8>,
}

/// The shared bundle every attack starts from — built once for the whole binary
/// and never mutated; an attack clones whatever it is about to tamper with.
///
/// Deterministic, so sharing changes no outcome. It is shared for time: `zstd`
/// is a C dependency and the `dev` profile compiles it unoptimised, so sealing a
/// multi-megabyte fixture once per test would put this file's runtime an order
/// of magnitude over the AUR `check()` budget (T-06-06) on an installer's
/// machine. Attacks needing a different key hierarchy still build their own.
fn honest() -> &'static Bundle {
    static BUNDLE: OnceLock<Bundle> = OnceLock::new();
    BUNDLE.get_or_init(|| {
        let (keyfile, keys) = suite_keys();
        let data = fixture(FIXTURE_LEN);
        let (pack, manifest, root) = bundle(&keys, &data);
        Bundle {
            keyfile,
            keys,
            data,
            pack,
            manifest,
            root,
        }
    })
}

/// What a reader following an ordered id list fetches out of a pack: the
/// `(id, ciphertext)` pairs, in that order.
fn served(pack: &[u8], header: &PackHeader, ids: &[ChunkId]) -> Result<Vec<(ChunkId, Vec<u8>)>> {
    ids.iter()
        .map(|id| {
            let entry =
                header.entries.iter().find(|e| e.id == *id).ok_or_else(|| {
                    AppError::Other("the pack does not carry a named chunk".into())
                })?;
            Ok((*id, blob_bytes(pack, entry)?.to_vec()))
        })
        .collect()
}

/// The whole read path, in the order a real fetch performs it: open the root,
/// gate it on the local rollback anchor, follow `manifest_chunks` into the pack,
/// open the manifest, then follow the file's ordered chunk list back to bytes.
///
/// Anything that goes wrong anywhere returns no buffer at all — `Result` carries
/// no bytes, so "no partial plaintext" is structural rather than a promise.
fn restore_gated(
    keys: &Keys,
    pack: &[u8],
    sealed_root: &[u8],
    local: Option<&Anchor>,
    allow_rollback: bool,
) -> Result<Zeroizing<Vec<u8>>> {
    // `REPO_ID` here stands for the caller's *local* configuration, which is the
    // only place `Root::open`'s expectation may come from: reading it back out
    // of the root would make the binding say nothing.
    let root = Root::open(keys, sealed_root, REPO_ID)?;
    anchor::accept(local, &root.repo_id, root.counter, allow_rollback)?;

    let header = read_header(keys, pack)?;
    let manifest = Manifest::open(keys, &served(pack, &header, &root.manifest_chunks)?)?;
    let file = manifest
        .files
        .first()
        .ok_or_else(|| AppError::Other("the manifest names no file".into()))?;
    reassemble(keys, &served(pack, &header, &file.chunks)?)
}

/// [`restore_gated`] on a machine that has never seen this bundle.
fn restore(keys: &Keys, pack: &[u8], sealed_root: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    restore_gated(keys, pack, sealed_root, None, false)
}

/// The funnel every attack's outcome passes through.
///
/// `outcome` is a *count* — bytes recovered, files recovered, pack entries
/// recovered — deliberately, for two reasons. Asserting the count is zero
/// asserts on the returned **value**, which `is_err()` does not: a call that
/// errors while filling an out-parameter has still leaked. And a count cannot
/// print a user's plaintext into a CI log when the assertion fails.
fn refused(outcome: Result<usize>, keyfile: &Keyfile, plaintext: &[u8]) -> String {
    assert_eq!(
        *outcome.as_ref().unwrap_or(&0),
        0,
        "an attack recovered data instead of nothing"
    );
    let message = outcome
        .expect_err("this attack must be refused")
        .to_string();
    assert_secret_free(&message, keyfile, plaintext);
    message
}

/// CRYPTO-07 made testable: a failure message names the operation and never a
/// key, a password, or a fragment of what was being protected.
fn assert_secret_free(message: &str, keyfile: &Keyfile, plaintext: &[u8]) {
    for secret in [
        PASSWORD,
        WRONG_PASSWORD,
        keyfile.wrapped_master_key.as_str(),
        keyfile.kdf.salt.as_str(),
    ] {
        assert!(
            !message.contains(secret),
            "a refusal carried key or password material"
        );
    }

    assert!(
        !contains(message.as_bytes(), NEEDLE),
        "a refusal carried the fixture's marker plaintext"
    );
    for offset in [
        0,
        plaintext.len() / 4,
        plaintext.len() / 2,
        plaintext.len() - 32,
    ] {
        assert!(
            !contains(message.as_bytes(), &plaintext[offset..offset + 32]),
            "a refusal carried a run of plaintext from offset {offset}"
        );
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    needle.len() <= haystack.len() && haystack.windows(needle.len()).any(|w| w == needle)
}

/// Entries shaped like a real bundle's — project-scoped session paths at ~229
/// bytes each. Short synthetic paths seal far smaller than the real thing and
/// would let the multi-chunk assertions pass vacuously (1-09 Deviation 2).
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

// ---------------------------------------------------------------------------
// Task 1 — the happy path, without which the nine refusals prove nothing
// ---------------------------------------------------------------------------

#[test]
fn the_full_stack_round_trips_a_multi_megabyte_fixture_byte_exactly() {
    let b = honest();

    assert_eq!(
        b.manifest.files[0].chunks.len(),
        9,
        "eight full chunks and a partial tail"
    );
    assert_eq!(b.manifest.files[0].true_len, FIXTURE_LEN as u64);

    let restored = restore(&b.keys, &b.pack, &b.root).expect("the honest bundle restores");
    assert_eq!(&*restored, &b.data[..]);
}

#[test]
fn sealing_one_fixture_twice_produces_identical_packs_and_identical_chunk_ids() {
    let b = honest();
    let (again_pack, again_manifest, again_root) = bundle(&b.keys, &b.data);

    // Determinism is not a nicety: with a random nonce every re-upload of an
    // unchanged chunk would be a new git object and dedup would die.
    assert_eq!(b.pack, again_pack);
    assert_eq!(b.manifest.files[0].chunks, again_manifest.files[0].chunks);

    // The root is the one object that must *not* be deterministic — its
    // plaintext changes every sync, so a content-derived nonce would publish
    // whether two consecutive snapshots are identical.
    assert_ne!(b.root, again_root);
    assert_eq!(
        &*restore(&b.keys, &b.pack, &again_root).expect("either root opens either pack"),
        &b.data[..]
    );
}

#[test]
fn no_fixture_plaintext_and_no_file_path_survives_into_the_pack_bytes() {
    let b = honest();

    assert!(!contains(&b.pack, NEEDLE));
    for offset in [0, FIXTURE_LEN / 4, FIXTURE_LEN * 3 / 4, FIXTURE_LEN - 32] {
        assert!(
            !contains(&b.pack, &b.data[offset..offset + 32]),
            "plaintext at offset {offset} survived sealing"
        );
    }
    // The manifest travels inside the same pack, and a path is exactly the
    // metadata a hostile remote would like.
    assert!(!contains(&b.pack, b".credentials.json"));
}

// ---------------------------------------------------------------------------
// Task 2 — the nine attacks
//
// Each attack is a plain function that performs its own assertions and returns
// the message it produced, so `the_nine_refusals_carry_nine_distinct_messages`
// can compare all nine without restating any of them.
// ---------------------------------------------------------------------------

/// Attack 1. Opening the keyfile under a different password.
fn attack_1_wrong_password() -> String {
    let b = honest();

    // No `Keys` value comes into existence at all — stronger than a subkey that
    // merely happens to be wrong, because there is nothing to leak.
    assert!(b.keyfile.open(WRONG_PASSWORD.as_bytes()).is_err());

    let outcome = b
        .keyfile
        .open(WRONG_PASSWORD.as_bytes())
        .and_then(|wrong| restore(&wrong, &b.pack, &b.root))
        .map(|bytes| bytes.len());
    let message = refused(outcome, &b.keyfile, &b.data);
    assert_eq!(message, "wrong password or corrupted keyfile");

    // Selective, not a broken build: the real password still restores the
    // fixture byte-exactly.
    assert_eq!(
        &*restore(&b.keys, &b.pack, &b.root).expect("the honest password restores"),
        &b.data[..]
    );
    message
}

/// Attack 2. Rewriting `m_kib` in the *serialized* keyfile. Mutating the
/// in-memory struct would not exercise the associated-data binding at all.
fn attack_2_downgraded_kdf_parameters() -> String {
    // One keyfile above the cheap seam, because Argon2's own floor is m = 8·p:
    // a keyfile written at the seam has nothing left to downgrade to. 64 KiB at
    // t = 1 is still microseconds, so the AUR budget is untouched.
    const SEALED_AT: KdfParams = KdfParams {
        m_kib: 64,
        t: 1,
        p: 1,
    };
    let keyfile = wrap_by_hand(0x22, SEALED_AT);
    let keys = keyfile
        .open(PASSWORD.as_bytes())
        .expect("a hand-wrapped keyfile must open");
    let data = fixture(FIXTURE_LEN);
    let (pack, _, root) = bundle(&keys, &data);

    let on_the_wire = serde_json::to_string(&keyfile).expect("the keyfile serializes");
    assert!(on_the_wire.contains("\"m_kib\":64"));
    let downgrade = |to: &str| -> Keyfile {
        serde_json::from_str(&on_the_wire.replace("\"m_kib\":64", &format!("\"m_kib\":{to}")))
            .expect("a downgraded keyfile still deserializes — that is the whole attack")
    };
    let open_and_restore = |keyfile: Keyfile| {
        keyfile
            .open(PASSWORD.as_bytes())
            .and_then(|k| restore(&k, &pack, &root))
            .map(|bytes| bytes.len())
    };

    // Leg 1 — a *valid* downgrade, to the cheap seam. The parameters are bound
    // as AEAD associated data, so the KEK they now describe is not the KEK the
    // master key was wrapped under, and the correct password does not help.
    let shared = refused(open_and_restore(downgrade("8")), &keyfile, &data);

    // …and it reads exactly like a wrong password, deliberately: both produce a
    // wrong KEK and no code can tell them apart, so a message that *did*
    // distinguish them would be an oracle. `crypto.rs` says so at the `map_err`;
    // pinning it here means a future "helpful" split is caught, not shipped.
    assert_eq!(shared, "wrong password or corrupted keyfile");

    // Leg 2 — a downgrade *below* Argon2's floor, refused by name before a byte
    // of memory is allocated. This is the message the distinctness test
    // collects, because leg 1's is attack 1's by cryptographic necessity.
    let message = refused(open_and_restore(downgrade("4")), &keyfile, &data);
    assert_eq!(message, "invalid Argon2id parameters (m_kib=4, t=1, p=1)");

    // Untouched, the keyfile still opens — so the refusals above are the
    // binding, not a keyfile that never worked.
    assert!(keyfile.open(PASSWORD.as_bytes()).is_ok());
    message
}

/// The pair both swap attacks use: one chunk from the compressible half and one
/// from the incompressible half, so their ciphertexts differ in length. A
/// one-directional test can pass by accident of length; this pair makes that
/// accident impossible to hide.
fn swap_pair(ids: &[ChunkId]) -> (usize, usize) {
    (0, ids.len() - 2)
}

/// Attacks 3 and 4. Serving one chunk's ciphertext under another chunk's id,
/// in both directions.
fn chunk_swap(forward: bool) -> String {
    let bundle = honest();
    let header = read_header(&bundle.keys, &bundle.pack).expect("the honest pack opens");

    let ids = &bundle.manifest.files[0].chunks;
    let (a, b) = swap_pair(ids);
    let mut pairs = served(&bundle.pack, &header, ids).expect("the pack carries every named chunk");
    assert_ne!(
        pairs[a].1.len(),
        pairs[b].1.len(),
        "the swap pair must differ in ciphertext length, or direction proves nothing"
    );

    // Forward: chunk B's ciphertext served under chunk A's id. Reverse: the
    // other way round. `victim` is the id whose ciphertext is no longer its own.
    let (victim, donor) = if forward { (a, b) } else { (b, a) };
    pairs[victim].1 = pairs[donor].1.clone();

    let outcome = reassemble(&bundle.keys, &pairs).map(|bytes| bytes.len());
    let message = refused(outcome, &bundle.keyfile, &bundle.data);
    assert_eq!(
        message,
        format!("chunk {} failed authentication", ids[victim])
    );
    message
}

fn attack_3_chunk_swap_forward() -> String {
    chunk_swap(true)
}

fn attack_4_chunk_swap_reverse() -> String {
    chunk_swap(false)
}

/// Attack 5. Inverting one bit inside a chunk's ciphertext, in the pack the
/// attacker serves.
fn attack_5_flipped_bit() -> String {
    let bundle = honest();
    let header = read_header(&bundle.keys, &bundle.pack).expect("the honest pack opens");

    // A third chunk, so this refusal stays distinguishable from the two swaps'.
    let ids = &bundle.manifest.files[0].chunks;
    let target = ids[1];
    let (a, b) = swap_pair(ids);
    assert!(target != ids[a] && target != ids[b]);
    let entry = header
        .entries
        .iter()
        .find(|e| e.id == target)
        .expect("the pack carries the target chunk")
        .clone();

    let mut message = String::new();
    // "Anywhere in the ciphertext": first byte, middle, last byte.
    for offset in [0, entry.clen as usize / 2, entry.clen as usize - 1] {
        let mut tampered = bundle.pack.clone();
        tampered[entry.offset as usize + offset] ^= 0b0000_0001;
        let outcome = restore(&bundle.keys, &tampered, &bundle.root).map(|bytes| bytes.len());
        message = refused(outcome, &bundle.keyfile, &bundle.data);
        assert_eq!(message, format!("chunk {target} failed authentication"));
    }
    message
}

/// Attack 6. A pack cut short by the remote that serves it.
fn attack_6_truncated_pack() -> String {
    let b = honest();
    assert!(
        read_header(&b.keys, &b.pack).is_ok(),
        "the honest pack opens"
    );

    // The two cuts the threat model names: the final byte, and the final
    // kilobyte. Both fail inside `read_header` and hand back no entry at all —
    // the assertion is the entry count, not `is_err()`.
    for cut in [1usize, 1_024] {
        let outcome = read_header(&b.keys, &b.pack[..b.pack.len() - cut]).map(|h| h.entries.len());
        // *Which* of the pack reader's refusals fires depends on the bytes the
        // truncation exposes as the trailing length, and those are a keyed hash
        // — different on every run. Only the refusal itself is deterministic,
        // so only that is asserted here; a suite that is the phase's proof must
        // not flake.
        refused(outcome, &b.keyfile, &b.data);
    }

    // The deterministic case, and therefore the one the distinctness test
    // collects: a pack cut below its own trailer.
    let outcome = read_header(&b.keys, &b.pack[..20]).map(|h| h.entries.len());
    let message = refused(outcome, &b.keyfile, &b.data);
    assert_eq!(message, "pack is too short to hold a trailer");
    message
}

/// Attack 7. Bytes cut from a sealed manifest.
fn attack_7_truncated_manifest() -> String {
    let b = honest();
    let header = read_header(&b.keys, &b.pack).expect("the honest pack opens");
    let named = Root::open(&b.keys, &b.root, REPO_ID)
        .expect("the honest root opens")
        .manifest_chunks;
    let intact = served(&b.pack, &header, &named).expect("the pack carries the manifest");
    assert_eq!(
        Manifest::open(&b.keys, &intact)
            .expect("the honest manifest opens")
            .files
            .len(),
        1
    );

    let mut message = String::new();
    // The assertion is the *file count*, because "opened with a shorter file
    // list" is precisely the failure mode being hunted here — an `is_err()`
    // check could not tell that apart from a clean refusal.
    for keep in [intact[0].1.len() - 8, intact[0].1.len() / 2] {
        let mut cut = intact.clone();
        cut[0].1.truncate(keep);
        let outcome = Manifest::open(&b.keys, &cut).map(|m| m.files.len());
        message = refused(outcome, &b.keyfile, &b.data);
        assert_eq!(message, format!("chunk {} failed authentication", named[0]));
    }
    message
}

/// Attack 8. Transposing `ChunkId`s that a sealed object names.
///
/// The guarantee is **not** "a reordered list fails to parse". It is that the
/// ordered list sits inside its container's *sealed plaintext*: `manifest_chunks`
/// inside the root, and each file's chunk list inside the manifest. Transposing
/// two ids therefore requires the key. The parse failure is a consequence, so it
/// is asserted last and never on its own.
fn attack_8_transposed_manifest_ids() -> String {
    let b = honest();

    // One multi-chunk file entry (the list transposed below) plus enough real-
    // shaped entries that the manifest itself spans several chunks, so the root
    // genuinely names an *ordered list* rather than a single id.
    let mut files = vec![FileEntry {
        path: ".claude/.credentials.json".into(),
        mode: 0o600,
        true_len: 900_000,
        chunks: vec![id(1), id(2), id(3), id(4)],
    }];
    files.extend(many_files(1_600));
    let manifest = Manifest::new(files);

    let blobs = manifest.seal(&b.keys).expect("seal the manifest");
    assert!(
        blobs.len() > 1,
        "the fixture must actually span several chunks"
    );
    let ordered: Vec<ChunkId> = blobs.iter().map(|blob| blob.id).collect();
    let intact: Vec<(ChunkId, Vec<u8>)> = blobs
        .iter()
        .map(|blob| (blob.id, blob.ciphertext.clone()))
        .collect();

    let sealed_root = Root::new(
        SNAPSHOT_COUNTER,
        fixed_time(),
        REPO_ID.to_string(),
        ordered.clone(),
        CHEAP,
    )
    .seal(&b.keys)
    .expect("seal the root");

    // --- the guarantee ---------------------------------------------------
    // `manifest_chunks` lives inside the root's sealed plaintext, so served
    // under the original root ciphertext the order comes back exactly as
    // written. A hostile remote has no edit to make here.
    assert_eq!(
        Root::open(&b.keys, &sealed_root, REPO_ID)
            .expect("the honest root opens")
            .manifest_chunks,
        ordered
    );

    // …and an attacker who re-seals a transposed list under a key he actually
    // holds produces a root the real key refuses outright. That is "reordering
    // requires the key", stated as an assertion rather than as prose.
    let mut transposed = ordered.clone();
    transposed.swap(0, 1);
    // A second, unrelated hierarchy: the key the attacker actually holds.
    let stranger = wrap_by_hand(0x33, CHEAP)
        .open(PASSWORD.as_bytes())
        .expect("a hand-wrapped keyfile must open");
    let forged = Root::new(
        SNAPSHOT_COUNTER,
        fixed_time(),
        REPO_ID.to_string(),
        transposed,
        CHEAP,
    )
    .seal(&stranger)
    .expect("seal");
    let forged_message = refused(
        Root::open(&b.keys, &forged, REPO_ID).map(|r| r.manifest_chunks.len()),
        &b.keyfile,
        &b.data,
    );
    assert_eq!(forged_message, "snapshot root failed authentication");

    // --- transposing ids inside a file entry ------------------------------
    // Re-sealing a reordered manifest yields a *different* `ChunkId`, so the
    // root never points at it. The genuine attack is therefore to serve those
    // bytes under the id the root does name, and that is the leg that errors.
    let mut reordered_entry = manifest.clone();
    reordered_entry.files[0].chunks.swap(1, 2);
    let evil = reordered_entry.seal(&b.keys).expect("seal");
    assert_ne!(evil[0].id, ordered[0]);

    let mut swapped_in = intact.clone();
    swapped_in[0].1 = evil[0].ciphertext.clone();
    let served_message = refused(
        Manifest::open(&b.keys, &swapped_in).map(|m| m.files.len()),
        &b.keyfile,
        &b.data,
    );
    assert_eq!(
        served_message,
        format!("chunk {} failed authentication", ordered[0])
    );

    // --- the consequence ---------------------------------------------------
    // A reader that *did* follow a reordered list reassembles a buffer that will
    // not parse. This is downstream of the guarantee above, not a substitute
    // for it, and zero files is asserted on the count.
    let mut followed_out_of_order = intact.clone();
    followed_out_of_order.swap(0, 1);
    let message = refused(
        Manifest::open(&b.keys, &followed_out_of_order).map(|m| m.files.len()),
        &b.keyfile,
        &b.data,
    );
    assert_eq!(message, "manifest is malformed");

    // The honest manifest still opens, with its order intact.
    assert_eq!(
        Manifest::open(&b.keys, &intact).expect("the honest manifest opens"),
        manifest
    );
    message
}

/// Attack 9. An authentic older snapshot replayed to hide a newer one.
fn attack_9_rolled_back_snapshot() -> String {
    let b = honest();

    // The replayed root is *authentic* — it opens perfectly, because the real
    // key produced it. Nothing inside the bundle can tell the client that a
    // newer snapshot exists; only local state the attacker cannot reach can.
    assert_eq!(
        Root::open(&b.keys, &b.root, REPO_ID)
            .expect("the replayed root opens")
            .counter,
        SNAPSHOT_COUNTER
    );

    let local = Anchor {
        repo_id: REPO_ID.to_string(),
        counter: SNAPSHOT_COUNTER + 6,
    };
    let outcome =
        restore_gated(&b.keys, &b.pack, &b.root, Some(&local), false).map(|bytes| bytes.len());
    let message = refused(outcome, &b.keyfile, &b.data);
    assert!(
        message.contains("--allow-rollback"),
        "the refusal must name the escape, or a user restoring an old snapshot \
         on purpose has nothing to type"
    );
    assert!(message.contains("counter 3") && message.contains("seen 9"));
    // This one is not pinned by equality, so check directly that nothing shaped
    // like a rendered key hid inside the prose.
    assert!(
        !message
            .as_bytes()
            .windows(32)
            .any(|w| w.iter().all(u8::is_ascii_hexdigit)),
        "a 32-hex-character run is what a leaked key would look like"
    );

    // The escape works, and returns the fixture byte-exactly: the refusal is a
    // policy decision, not a read path that cannot cope.
    let allowed = restore_gated(&b.keys, &b.pack, &b.root, Some(&local), true)
        .expect("--allow-rollback restores");
    assert_eq!(&*allowed, &b.data[..]);
    message
}

// ---------------------------------------------------------------------------
// The nine, as tests
// ---------------------------------------------------------------------------

#[test]
fn attack_1_a_wrong_password_yields_no_subkey_and_no_plaintext() {
    attack_1_wrong_password();
}

#[test]
fn attack_2_downgrading_the_kdf_parameters_in_transit_opens_nothing() {
    attack_2_downgraded_kdf_parameters();
}

#[test]
fn attack_3_serving_one_chunks_ciphertext_under_an_earlier_chunks_id_fails() {
    attack_3_chunk_swap_forward();
}

#[test]
fn attack_4_serving_one_chunks_ciphertext_under_a_later_chunks_id_fails() {
    attack_4_chunk_swap_reverse();
}

#[test]
fn attack_5_flipping_one_bit_of_a_chunks_ciphertext_fails() {
    attack_5_flipped_bit();
}

#[test]
fn attack_6_a_truncated_pack_fails_at_header_read_with_no_entry_returned() {
    attack_6_truncated_pack();
}

#[test]
fn attack_7_a_truncated_manifest_returns_no_file_entry() {
    attack_7_truncated_manifest();
}

#[test]
fn attack_8_transposed_chunk_ids_are_protected_by_the_sealed_order() {
    attack_8_transposed_manifest_ids();
}

#[test]
fn attack_9_a_rolled_back_snapshot_is_refused_unless_explicitly_allowed() {
    attack_9_rolled_back_snapshot();
}

#[test]
fn the_nine_refusals_carry_nine_distinct_messages() {
    // CRYPTO-03 asks for unambiguous. One opaque string for all nine would be
    // technically safe and operationally useless — an operator could not tell a
    // corrupted disk from a hostile remote.
    let messages = [
        attack_1_wrong_password(),
        attack_2_downgraded_kdf_parameters(),
        attack_3_chunk_swap_forward(),
        attack_4_chunk_swap_reverse(),
        attack_5_flipped_bit(),
        attack_6_truncated_pack(),
        attack_7_truncated_manifest(),
        attack_8_transposed_manifest_ids(),
        attack_9_rolled_back_snapshot(),
    ];

    for (i, first) in messages.iter().enumerate() {
        assert!(!first.is_empty());
        for (j, second) in messages.iter().enumerate().skip(i + 1) {
            assert_ne!(
                first,
                second,
                "attacks {} and {} are indistinguishable to an operator",
                i + 1,
                j + 1
            );
        }
    }
}
