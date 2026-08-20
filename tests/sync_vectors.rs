//! Pinned known-answer vectors for the encrypted sync bundle format.
//!
//! `src/safe_storage.rs` already pins Chromium compatibility with
//! `key_derivation_matches_the_chromium_compatibility_vector` and
//! `encryption_matches_the_chromium_compatibility_vector`, for exactly one
//! reason: so a crypto-crate upgrade cannot silently change the on-disk format.
//! **D-03** asks for the same here, and this file is it.
//!
//! A round-trip test cannot do this job. It stays self-consistent under any
//! wrong-but-stable transform, so an `argon2` 0.6 bump or a mistyped BLAKE3
//! context string would pass every round trip in the repository while making
//! every bundle already on a user's disk unreadable.
//!
//! # Two kinds of pin, and telling them apart is the point
//!
//! A future maintainer staring at a red assertion in this file must be able to
//! tell which of these it is **without re-deriving the format**, so every pin
//! below says so in its own failure message.
//!
//! - **An id pin** is over a *keyed hash of raw plaintext* —
//!   `blake3::keyed_hash(name_key, plaintext)`, computed before framing or
//!   compression touches a byte. It is therefore **stable across a zstd
//!   upgrade**. If an id pin ever moves, every chunk in every existing bundle
//!   has been re-addressed: dedup is gone for every user, the next sync
//!   re-uploads everything, and no old bundle resolves. That is a five-alarm
//!   failure, not a test to update.
//! - **A ciphertext pin** is over sealed bytes, which cover the *compressed*
//!   frame. It is **zstd-sensitive by nature**: a `zstd` version bump or a
//!   change to its level-3 defaults may legitimately move it while every id pin
//!   stays put. A move here means the on-disk bytes changed and a compatibility
//!   decision is owed — serious, and a different kind of serious. Old bundles
//!   still open (the plaintext is unchanged and its id still matches); what is
//!   lost is byte-identical re-sealing across the upgrade boundary.
//!
//!   Say that precisely, because the sloppy version of it — "one id covering
//!   two ciphertexts is harmless" — was propagated as an invariant and was a
//!   real vulnerability. It is harmless **for dedup**. It is not harmless for
//!   nonce safety: one id covering two distinct messages is exactly the input
//!   that reuses an AEAD nonce, which is why the chunk nonce is derived from the
//!   framed bytes rather than from the id and travels inline ahead of them. Two
//!   framings of one plaintext therefore differ in their *nonce* as well as in
//!   their ciphertext.
//!
//! # Provenance, per pin
//!
//! Part 1's four vectors come from the primitives' **own specifications**, so
//! the expected bytes originate somewhere other than this codebase and the test
//! genuinely guards the crate. Each literal names its source above it, in the
//! style of `safe_storage.rs`'s "Independently reproduced with OpenSSL's
//! PBKDF2-HMAC-SHA1 implementation". This is the only file outside
//! `src/sync/crypto.rs` that reaches for `argon2` directly, and it does so
//! deliberately: calling the primitive itself is what makes it a guard on the
//! crate rather than on our wrapper. (The containment invariant
//! `only_the_crypto_module_imports_the_cryptographic_crates` walks `src/sync/`,
//! so it is unaffected.) `tests/sync_adversarial.rs` reaches for
//! `chacha20poly1305` for a different reason — it has to wrap its own keyfile
//! now that the cheap-floor seam is `pub(crate)`.
//!
//! Part 2's pins are **regression pins of current behaviour**. Nobody publishes
//! a vector for this format; their value is that they change loudly, not that
//! they are externally correct. Each was produced by running the implementation
//! once, against the tree as it stands after 1-06 and 1-09, and pasting the
//! result. If one ever fails the question is *which of the two changed and why*
//! — never "regenerate it until green".
//!
//! # Crate versions these vectors were pinned against
//!
//! `argon2` 0.5.3, `chacha20poly1305` 0.11.0, `blake3` 1.8.6, `zstd` 0.13.3,
//! `zeroize` 1.9.0, `getrandom` 0.4.2.
//!
//! Hermetic: no `$HOME`, no `$XDG`, no Keychain, no network, no clock, and no
//! randomness. Every derivation that is not deliberately pinning a published
//! parameter set uses the cheap-KDF seam, because the AUR `check()` runs this
//! file on an installer's machine.

use std::sync::OnceLock;

use argon2::{Algorithm, Argon2, AssociatedData, ParamsBuilder, Version};
use base64::Engine;
use chacha20poly1305::XChaCha20Poly1305;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use serde::Serialize;

use ai_usagebar::sync::chunk::{frame, seal_all, seal_chunk};
use ai_usagebar::sync::crypto::{ChunkId, KdfDoc, KdfParams, Keyfile, Keys, derive_kek};
use ai_usagebar::sync::model::{FileEntry, Manifest};
use ai_usagebar::sync::pack::PackWriter;
use ai_usagebar::sync::{CHUNK_SIZE, CTX_CHUNK, CTX_NAME, CTX_NONCE, CTX_ROOT, KEYFILE_VERSION};

const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Lowercase hex, so a failed assertion prints something a human can diff
/// against the literal above it rather than a wall of decimal bytes.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Decode a hex literal, ignoring whitespace, so a specification's hex block can
/// be pasted in exactly as it is printed there.
fn unhex(text: &str) -> Vec<u8> {
    let digits: Vec<u8> = text.bytes().filter(u8::is_ascii_hexdigit).collect();
    assert_eq!(digits.len() % 2, 0, "a hex literal must have even length");
    digits
        .chunks(2)
        .map(|pair| {
            let s = std::str::from_utf8(pair).expect("ascii hex");
            u8::from_str_radix(s, 16).expect("valid hex")
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Part 1 — primitive vectors, each from the primitive's own specification
// ---------------------------------------------------------------------------

/// Argon2id against RFC 9106 §5.3, the parameter set and tag the specification
/// itself publishes.
///
/// This guards the **crate**, not our format: our own parameters are elsewhere
/// (`KdfParams::default`), and this vector's `p = 4`, secret and associated data
/// are things the bundle format never uses. If `argon2` ever changes what it
/// computes, this is the test that says so — before the KEK pin below quietly
/// moves with it.
#[test]
fn argon2_crate_matches_the_rfc_9106_test_vector() {
    // RFC 9106 §5.3, "Argon2id Test Vectors": memory 32 KiB, 3 passes, 4 lanes,
    // 32-byte tag, version number 19.
    let params = ParamsBuilder::new()
        .m_cost(32)
        .t_cost(3)
        .p_cost(4)
        .output_len(32)
        .data(AssociatedData::new(&[0x04; 12]).expect("12 bytes of associated data"))
        .build()
        .expect("the RFC's parameters are valid");
    let argon = Argon2::new_with_secret(&[0x03; 8], Algorithm::Argon2id, Version::V0x13, params)
        .expect("an 8-byte secret");

    let mut tag = [0u8; 32];
    argon
        .hash_password_into(&[0x01; 32], &[0x02; 16], &mut tag)
        .expect("hashing at the RFC's parameters");

    // RFC 9106 §5.3, "Tag:". Copied from
    // https://www.rfc-editor.org/rfc/rfc9106.txt — not produced by this
    // codebase, which is the entire point of a primitive guard.
    assert_eq!(
        hex(&tag),
        "0d640df58d78766c08c037a34a8b53c9d01ef0452d75b65eb52520e96b01e659",
        "argon2 no longer computes RFC 9106's published Argon2id tag — the \
         crate's semantics changed, and every KEK it derives changed with them"
    );
}

/// The 32-byte ASCII key the official BLAKE3 test vectors use, given in their
/// `key` field.
const B3_KEY: &[u8; 32] = b"whats the Elvish word for friend";

/// The context string the official BLAKE3 test vectors use, given in their
/// `context_string` field.
const B3_CONTEXT: &str = "BLAKE3 2019-12-27 16:29:52 test vectors context";

/// The input every official BLAKE3 case uses: "a repeating sequence of 251
/// bytes: 0, 1, 2, ..., 249, 250, 0, 1, ...".
fn b3_input(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

/// BLAKE3 `keyed_hash` — the mode that addresses every chunk in the format.
#[test]
fn blake3_keyed_hash_matches_the_official_reference_test_vector() {
    // https://raw.githubusercontent.com/BLAKE3-team/BLAKE3/master/test_vectors/test_vectors.json,
    // the `input_len: 1024` case, first 32 bytes of its `keyed_hash` output.
    // 1024 is BLAKE3's own chunk size, so this case crosses the tree-hashing
    // boundary a single-block vector would never reach.
    assert_eq!(
        hex(blake3::keyed_hash(B3_KEY, &b3_input(1024)).as_bytes()),
        "75c46f6f3d9eb4f55ecaaee480db732e6c2105546f1e675003687c31719c7ba4",
        "blake3::keyed_hash no longer matches the official reference vectors — \
         every chunk id in the format is this function"
    );
}

/// BLAKE3 `derive_key` — the mode that splits the master key into three
/// subkeys. This is the one that matters most: a typo in a context string is
/// invisible to every round-trip test in the repository and silently forks the
/// key hierarchy.
#[test]
fn blake3_derive_key_matches_the_official_reference_test_vector() {
    // The same source and the same `input_len: 1024` case, first 32 bytes of its
    // `derive_key` output, using the vectors' own `context_string`.
    assert_eq!(
        hex(&blake3::derive_key(B3_CONTEXT, &b3_input(1024))),
        "7356cd7720d5b66b6d0697eb3177d9f8d73a4a5c5e968896eb6a689684302706",
        "blake3::derive_key no longer matches the official reference vectors — \
         the whole subkey hierarchy is derived through this function"
    );
}

/// XChaCha20-Poly1305 against the published AEAD vector, key, 24-byte nonce,
/// associated data, plaintext, ciphertext and tag all from the draft.
#[test]
fn xchacha20poly1305_matches_the_draft_arciszewski_xchacha_03_test_vector() {
    // draft-arciszewski-xchacha-03 §A.3.1, "AEAD_XCHACHA20_POLY1305", the
    // developer-friendly form. Fetched from
    // https://www.ietf.org/archive/id/draft-arciszewski-xchacha-03.txt
    let key = unhex("808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f");
    let nonce = unhex("404142434445464748494a4b4c4d4e4f5051525354555657");
    let aad = unhex("50515253c0c1c2c3c4c5c6c7");
    let plaintext = unhex(
        "4c616469657320616e642047656e746c656d656e206f662074686520636c6173
         73206f66202739393a204966204920636f756c64206f6666657220796f75206f
         6e6c79206f6e652074697020666f7220746865206675747572652c2073756e73
         637265656e20776f756c642062652069742e",
    );
    // The draft prints the ciphertext and the 16-byte tag separately; the
    // `chacha20poly1305` crate's `encrypt` returns them concatenated, in that
    // order, which is what the on-disk format stores.
    let expected = unhex(
        "bd6d179d3e83d43b9576579493c0e939572a1700252bfaccbed2902c21396cbb
         731c7f1b0b4aa6440bf3a82f4eda7e39ae64c6708c54c216cb96b72e1213b452
         2f8c9ba40db5d945b11b69b982c1bb9e3f3fac2bc369488f76b2383565d3fff9
         21f9664c97637da9768812f615c68b13b52e
         c0875924c1c7987947deafd8780acf49",
    );

    let key: [u8; 32] = key.try_into().expect("32-byte key");
    let nonce: [u8; 24] = nonce.try_into().expect("24-byte nonce");
    let sealed = XChaCha20Poly1305::new((&key).into())
        .encrypt(
            &nonce.into(),
            Payload {
                msg: &plaintext,
                aad: &aad,
            },
        )
        .expect("sealing the draft's plaintext");

    assert_eq!(
        hex(&sealed),
        hex(&expected),
        "chacha20poly1305 no longer matches draft-arciszewski-xchacha-03's \
         published AEAD vector — every sealed object in the format is this call"
    );
}

// ---------------------------------------------------------------------------
// Part 2 — composed-format regression pins
//
// Fixed inputs, held here so every pin below is reproducible from this file
// alone. Nothing is random and nothing is read from the environment.
// ---------------------------------------------------------------------------

/// Microseconds instead of ~1.5 s and a gibibyte. The production parameters are
/// pinned as *values* in `crypto.rs`'s own tests; what matters here is that the
/// derivation is reproducible, and the AUR `check()` runs this on somebody
/// else's machine.
const CHEAP: KdfParams = KdfParams {
    m_kib: 8,
    t: 1,
    p: 1,
};

const PASSWORD: &[u8] = b"correct horse battery staple";
const SALT: [u8; 16] = *b"ai-usagebar-salt";
const WRAP_NONCE: [u8; 24] = *b"ai-usagebar-wrap-nonce24";

/// The fixed master key the whole of part 2 hangs from. Never drawn from the
/// CSPRNG here: a pin over a random key is not a pin.
const MASTER: [u8; 32] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
    0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f,
];

/// The short fixed plaintext the id pin and the ciphertext pin both address, so
/// the two can be compared against each other directly.
const SHORT_PLAINTEXT: &[u8] = b"ai-usagebar sync vector";

/// The format-scoping half of the snapshot root's associated data, spelled out
/// rather than imported: a reader implemented from `docs/sync-format.md` §5 has
/// only this string, and pinning it here catches a change to the constant. The
/// other half is the bundle's [`REPO_ID`], appended.
const ROOT_AAD: &[u8] = b"ai-usagebar.sync.v1 root";

/// The bundle identity a caller reads from its own configuration and hands to
/// `open_root` — never a value taken from the remote.
const REPO_ID: &str = "usagebar-sync-vector";

/// A keyfile wrapping [`MASTER`], built **by hand from the documented format**
/// rather than by `Keyfile::create`, which draws a random master key and so can
/// never anchor a vector.
///
/// The AAD struct below is a re-implementation of `docs/sync-format.md` §1's
/// `{"format":…,"kdf":{…}}`, written from the document rather than shared with
/// `crypto.rs`. That is deliberate: `Keyfile::open` accepting this keyfile is
/// the only assertion in the repository that the *documented* wrap format and
/// the implemented one are the same thing.
fn fixed_keyfile() -> Keyfile {
    #[derive(Serialize)]
    struct KeyfileAad<'a> {
        format: u32,
        kdf: &'a KdfDoc,
    }

    let kdf = KdfDoc {
        algo: "argon2id".into(),
        version: 19,
        m_kib: CHEAP.m_kib,
        t: CHEAP.t,
        p: CHEAP.p,
        salt: B64.encode(SALT),
    };
    let aad = serde_json::to_vec(&KeyfileAad {
        format: KEYFILE_VERSION,
        kdf: &kdf,
    })
    .expect("the AAD serializes");

    let kek = derive_kek(PASSWORD, &SALT, CHEAP).expect("the cheap KDF seam");
    let wrapped = XChaCha20Poly1305::new((&*kek).into())
        .encrypt(
            &WRAP_NONCE.into(),
            Payload {
                msg: &MASTER,
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

/// The subkeys of [`MASTER`], reached through the production unwrap path.
///
/// Shared through a `OnceLock` for the same reason `sync_adversarial.rs` shares
/// its bundle: everything here is deterministic, so sharing changes no outcome,
/// and it is shared for time rather than for convenience.
fn fixed_keys() -> &'static Keys {
    static KEYS: OnceLock<Keys> = OnceLock::new();
    KEYS.get_or_init(|| {
        fixed_keyfile()
            .open(PASSWORD)
            .expect("a hand-wrapped keyfile must open")
    })
}

/// A hand-built keyfile opening is what makes every pin below meaningful: it is
/// the proof that `fixed_keys` really carries [`MASTER`] and not some other key.
///
/// Regression pin of current behaviour. A failure here means the keyfile's wrap
/// format changed — the AAD's field order, the nonce or salt encoding, or the
/// version gate — and every keyfile already written is now unopenable.
#[test]
fn a_keyfile_wrapped_by_hand_from_the_documented_format_opens() {
    let keyfile = fixed_keyfile();
    assert_eq!(keyfile.format, KEYFILE_VERSION);
    assert_eq!(keyfile.kdf.params(), CHEAP);

    // The wrap is deterministic given a fixed salt, nonce and master key, so the
    // whole 48-byte ciphertext-plus-tag is pinnable. **Ciphertext pin** — but a
    // zstd-independent one: nothing here is compressed. It moves only if
    // Argon2id or XChaCha20-Poly1305 changes, which part 1 would catch first.
    assert_eq!(
        keyfile.wrapped_master_key,
        "sI3J/5eBe9VNGkVOVo/DuKral+ttG7/+Zm13UNtBkOiOrCXn34D2lrhqpXfn5ZY/",
        "the master-key wrap moved: an existing keyfile no longer unwraps to \
         the key it was written with"
    );

    // And the implementation accepts it, which is the assertion that the
    // documented format and the implemented one have not drifted apart.
    assert!(
        keyfile.open(PASSWORD).is_ok(),
        "Keyfile::open rejected a keyfile built from docs/sync-format.md — the \
         document and the implementation disagree about the wrap format"
    );
}

/// The KEK, from a fixed password, a fixed 16-byte salt and fixed cheap
/// parameters.
///
/// Regression pin of current behaviour, generated by running `derive_kek` once
/// against the tree after 1-06. It is *not* an independent vector — RFC 9106's
/// is, in part 1, and that is the one that guards the crate. This one guards our
/// call into it: the algorithm, the version, the output length, and the order in
/// which the password and salt are handed over.
#[test]
fn the_key_encryption_key_is_pinned_for_a_fixed_password_and_salt() {
    let kek = derive_kek(PASSWORD, &SALT, CHEAP).expect("the cheap KDF seam");
    assert_eq!(
        hex(&*kek),
        "23b85454f240d20a8f44b9e76cdaa78f940fce2d6a43abb08573aa23ddf6822c",
        "the KEK derivation moved: no existing keyfile unwraps any more"
    );
}

/// `chunk_key` — pinned as a value, **and** proved to be the subkey the sealing
/// path actually reaches for.
///
/// Pinning the three subkeys individually is the whole point: a context string
/// wired to the wrong field yields a working system with a silently different
/// key hierarchy, which no round-trip test can catch, because the hierarchy
/// stays self-consistent under any permutation of the three.
///
/// Regression pin of current behaviour. A move means `CTX_CHUNK` changed, or the
/// field it feeds did, and every chunk in every existing bundle is now sealed
/// under a key this build does not derive.
#[test]
fn the_chunk_subkey_is_pinned_and_is_the_key_the_seal_path_uses() {
    let chunk_key = blake3::derive_key(CTX_CHUNK, &MASTER);
    assert_eq!(
        hex(&chunk_key),
        "be641f9d88f9dd21627f7c03156908874f103024d8dd4426151f510881d92d6b",
        "chunk_key moved: every sealed chunk in every existing bundle is now \
         encrypted under a key this build no longer derives"
    );

    // The wiring, reproduced from docs/sync-format.md §3 rather than from
    // `crypto.rs`: nonce = derive_key(CTX_NONCE, keyed_hash(name_key, framed))
    // [..24], aad = id, msg = the framed chunk, and the nonce stored inline
    // ahead of the ciphertext. If `seal` reached for `name` or `root` instead,
    // this would differ while every round trip in the repository kept passing.
    //
    // The nonce is a function of the **framed bytes**, not of the id. That is
    // the whole of the F-1 fix: the id addresses the raw plaintext, so deriving
    // the nonce from it would let two zstd builds seal two distinct messages
    // under one (chunk_key, nonce, aad).
    let keys = fixed_keys();
    let id = keys.chunk_id(SHORT_PLAINTEXT);
    let name_key = blake3::derive_key(CTX_NAME, &MASTER);
    let framed = frame(SHORT_PLAINTEXT).expect("framing");
    let nonce: [u8; 24] =
        blake3::derive_key(CTX_NONCE, blake3::keyed_hash(&name_key, &framed).as_bytes())[..24]
            .try_into()
            .expect("24 of 32 bytes");
    let sealed = XChaCha20Poly1305::new((&chunk_key).into())
        .encrypt(
            &nonce.into(),
            Payload {
                msg: &framed,
                aad: id.as_bytes(),
            },
        )
        .expect("sealing");
    let mut independently = nonce.to_vec();
    independently.extend_from_slice(&sealed);

    assert_eq!(
        hex(&seal_chunk(keys, SHORT_PLAINTEXT).expect("seal").ciphertext),
        hex(&independently),
        "the seal path no longer matches a sealing reconstructed from the \
         documented format under chunk_key"
    );
}

/// `name_key` — pinned as a value, and proved to be the key the chunk *address*
/// is computed under.
///
/// Regression pin of current behaviour. A move means `CTX_NAME` changed, or the
/// field it feeds did — and because the address is the id, this is also the pin
/// whose failure means dedup broke for every existing user.
#[test]
fn the_name_subkey_is_pinned_and_is_the_key_the_chunk_id_uses() {
    let name_key = blake3::derive_key(CTX_NAME, &MASTER);
    assert_eq!(
        hex(&name_key),
        "6da4903b0cc43888fd193e7fcf558220d5fe7b1aebc8ca5c2d0a846257c65278",
        "name_key moved: every chunk id in every existing bundle changes, so \
         dedup is gone and no old bundle resolves"
    );

    // The wiring: the id is `keyed_hash(name_key, raw plaintext)`. Reaching for
    // `chunk` or `root` here would leave `name_key` dead and every round trip
    // green.
    assert_eq!(
        hex(fixed_keys().chunk_id(SHORT_PLAINTEXT).as_bytes()),
        hex(blake3::keyed_hash(&name_key, SHORT_PLAINTEXT).as_bytes()),
        "chunk_id is no longer keyed_hash(name_key, plaintext)"
    );
}

/// `root_key` — pinned as a value, and proved to be the key the snapshot root is
/// sealed under.
///
/// The root's nonce is random by design, so the proof runs the other way: a root
/// framed **by hand** under `root_key` must open through `Keys::open_root`. If
/// the root path reached for `chunk` instead, `root_key` would be dead and
/// nothing else in the repository would notice.
///
/// Regression pin of current behaviour. A move means `CTX_ROOT` or the root's
/// associated-data literal changed, and no existing snapshot root opens.
#[test]
fn the_root_subkey_is_pinned_and_is_the_key_the_snapshot_root_uses() {
    let root_key = blake3::derive_key(CTX_ROOT, &MASTER);
    assert_eq!(
        hex(&root_key),
        "2e01ec59e1bc6e0a82480a8a25133b6cbfa9b2f2d76bc8118ee77a772b6dd0a6",
        "root_key moved: no existing snapshot root opens"
    );

    // `nonce ‖ ciphertext ‖ tag`, with `ROOT_AAD ‖ repo_id` as associated data —
    // docs/sync-format.md §5, reconstructed here rather than imported.
    let nonce = [0x37u8; 24];
    let mut aad = ROOT_AAD.to_vec();
    aad.extend_from_slice(REPO_ID.as_bytes());
    let sealed = XChaCha20Poly1305::new((&root_key).into())
        .encrypt(
            &nonce.into(),
            Payload {
                msg: b"counter=7".as_slice(),
                aad: &aad,
            },
        )
        .expect("sealing a root by hand");
    let mut framed = nonce.to_vec();
    framed.extend_from_slice(&sealed);

    assert_eq!(
        &*fixed_keys().open_root(&framed, REPO_ID).expect(
            "open_root rejected a root framed by hand under root_key — either \
             the subkey wiring or the root's associated data changed"
        ),
        b"counter=7"
    );

    // The `repo_id` half of the AAD is load-bearing, not decorative: the same
    // bytes offered as another bundle's root must fail the tag.
    assert!(
        fixed_keys()
            .open_root(&framed, "some-other-bundle")
            .is_err(),
        "the root's associated data is no longer scoped to the repository"
    );
}

/// **Id pin — stable across a zstd upgrade.**
///
/// The id is a keyed hash of the raw plaintext, computed before framing or
/// compression touches a byte, so no zstd change can legitimately move it.
///
/// If this pin ever fails, do not re-pin it. Every chunk in every existing
/// bundle has been re-addressed: dedup is gone for every user, the next sync
/// re-uploads everything, and no manifest resolves against what is on the
/// remote. That is a far bigger event than a failing test.
#[test]
fn the_chunk_id_of_a_fixed_plaintext_is_pinned_and_must_survive_a_zstd_upgrade() {
    let keys = fixed_keys();
    assert_eq!(
        keys.chunk_id(SHORT_PLAINTEXT).to_string(),
        "e18d31512de7c8d22508a82bd2353f9a420cfa083f56a49db08b49a0945cfe0b",
        "THE CHUNK ID MOVED. This is not a vector to regenerate: the address of \
         every chunk in every existing bundle has changed, so dedup has broken \
         for every user and no old bundle resolves. A zstd upgrade cannot cause \
         this — the id addresses raw plaintext"
    );

    // And it really is a function of the plaintext rather than of the frame: the
    // property that keeps the pin above zstd-independent.
    let framed = frame(SHORT_PLAINTEXT).expect("framing");
    assert_ne!(keys.chunk_id(SHORT_PLAINTEXT), keys.chunk_id(&framed));
}

/// **Ciphertext pin — zstd-sensitive by nature.**
///
/// The sealed bytes cover the compressed frame, so a `zstd` version bump or a
/// change to its level-3 defaults may legitimately move this while the id pin
/// above stays exactly where it is. A failure here is a prompt to find out what
/// changed and re-pin deliberately, after checking that the id pin did *not*
/// move; a failure on the id pin is not.
///
/// Re-pinned in 1-10: the first 24 bytes are now the chunk's nonce, derived from
/// the framed bytes rather than from the id, and the whole tail shifted with it.
/// The id pin above did not move — which is exactly the distinction the two
/// kinds of pin exist to make legible.
#[test]
fn the_sealed_chunk_ciphertext_is_pinned_and_is_zstd_sensitive() {
    let keys = fixed_keys();
    let blob = seal_chunk(keys, SHORT_PLAINTEXT).expect("seal");

    assert_eq!(
        hex(&blob.ciphertext),
        "008f88feaa1e98132d608fddbc1c8cb8270887352e8e14376d71b1c15f4a2c48\
         5daf5ceb8b4fec7cf17738a020975feec7da62d0b44c0b508294b849440d0521\
         9841e0853a4fcba6b5b9311f462fc3aed95e6e9cda41a7c2ed04ef9b14cc8d41\
         f0ff0de6048e1693",
        "the sealed bytes moved. Check the id pin first: if it held, this is a \
         zstd or framing change and the on-disk format now differs — a \
         compatibility decision, not a regeneration"
    );

    // The nonce travels inline, ahead of the ciphertext, and is a function of
    // the framed bytes — not of the id. Pinned as a property rather than only as
    // a prefix of the literal above, so a future change that re-derived it from
    // the id would fail *here*, naming the reason, instead of only moving bytes.
    let framed = frame(SHORT_PLAINTEXT).expect("framing");
    let name_key = blake3::derive_key(CTX_NAME, &MASTER);
    assert_eq!(
        hex(&blob.ciphertext[..24]),
        hex(
            &blake3::derive_key(CTX_NONCE, blake3::keyed_hash(&name_key, &framed).as_bytes())[..24]
        ),
        "the chunk nonce is no longer derived from the bytes actually encrypted \
         — two zstd builds can now seal two distinct messages under one nonce"
    );

    // Determinism, mirroring `safe_storage.rs`'s
    // `assert_eq!(encrypt(&k, b"same"), encrypt(&k, b"same"))`. It is a
    // security-relevant property here rather than a convenience: with a random
    // nonce, every re-upload of an unchanged chunk would be a new remote object
    // and dedup would die.
    assert_eq!(
        blob.ciphertext,
        seal_chunk(keys, SHORT_PLAINTEXT).expect("seal").ciphertext,
        "sealing is no longer deterministic, so dedup cannot work"
    );
}

// ---------------------------------------------------------------------------
// The full round trip
// ---------------------------------------------------------------------------

/// A deterministic multi-chunk payload: 700 KiB is two full 256 KiB chunks plus
/// a tail, so the manifest genuinely names an ordered list. Xorshift, so there
/// is no random source and no clock anywhere near the fixture.
fn payload() -> Vec<u8> {
    let mut state = 0x2545_f491_4f6c_dd1du64;
    (0..700 * 1024)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect()
}

/// The whole write path over [`payload`], exactly as a sync performs it: seal
/// the payload into a pack, seal the manifest naming it into the same pack, and
/// finish.
struct Bundle {
    manifest_chunks: Vec<ChunkId>,
    payload_chunks: Vec<ChunkId>,
    pack_address: ChunkId,
}

/// Built once. The pack is a megabyte of zstd work in an unoptimised `dev`
/// profile, and the AUR `check()` runs this on an installer's machine.
fn bundle() -> &'static Bundle {
    static BUNDLE: OnceLock<Bundle> = OnceLock::new();
    BUNDLE.get_or_init(|| {
        let keys = fixed_keys();
        let data = payload();

        let mut writer = PackWriter::new();
        let blobs = seal_all(keys, &data).expect("seal the payload");
        let payload_chunks: Vec<ChunkId> = blobs.iter().map(|b| b.id).collect();
        for blob in blobs {
            writer.push(blob);
        }

        let manifest = Manifest::new(vec![FileEntry {
            path: ".claude/.credentials.json".into(),
            mode: 0o600,
            true_len: data.len() as u64,
            chunks: payload_chunks.clone(),
        }]);
        let manifest_blobs = manifest.seal(keys).expect("seal the manifest");
        let manifest_chunks: Vec<ChunkId> = manifest_blobs.iter().map(|b| b.id).collect();
        for blob in manifest_blobs {
            writer.push(blob);
        }

        let (pack_address, _) = writer.finish(keys).expect("seal the pack");
        Bundle {
            manifest_chunks,
            payload_chunks,
            pack_address,
        }
    })
}

/// **Id pin — stable across a zstd upgrade.**
///
/// The manifest's own chunk id is a keyed hash of the manifest's JSON, and that
/// JSON contains the payload's chunk ids, which are keyed hashes of raw
/// plaintext. Nothing compressed enters either hash, so this id is
/// zstd-independent all the way down — and it transitively pins all three
/// payload ids, the file entry's path, mode and length, and serde's field order
/// for `Manifest` and `FileEntry`.
///
/// A move here means the same five-alarm event the chunk-id pin describes,
/// reached one level up: the root names a manifest that no longer exists at that
/// address.
#[test]
fn the_multi_chunk_bundles_manifest_id_is_pinned_and_must_survive_a_zstd_upgrade() {
    let b = bundle();
    assert_eq!(
        b.payload_chunks.len(),
        3,
        "700 KiB is two full {CHUNK_SIZE}-byte chunks and a tail — if this \
         changed, the fixture stopped exercising a multi-chunk manifest"
    );
    assert_eq!(
        b.manifest_chunks.len(),
        1,
        "a one-file manifest still fits in a single chunk"
    );

    assert_eq!(
        b.manifest_chunks[0].to_string(),
        "f3a798c501ed6e633be78ae3041afd0dbcde6b0a096257e7f64061093c5c510b",
        "THE MANIFEST ID MOVED. Not a vector to regenerate: a manifest sealed by \
         an older build is no longer at the address its root names. A zstd \
         upgrade cannot cause this — the id addresses the manifest's raw JSON"
    );

    // The payload ids the manifest carries, pinned alongside it so a failure
    // says which of the two levels moved rather than only that one of them did.
    assert_eq!(
        b.payload_chunks
            .iter()
            .map(ChunkId::to_string)
            .collect::<Vec<_>>(),
        [
            "188c032981b422a75713fc44ede125c79caa267bab13591ab7ab3264318d8447",
            "a969848d8ae7f6384c74374179232d5e504dd40b74311919623e902270d5c0e2",
            "e0eeabb67a347904d662c4d0813a88599e139aa803416d7520af7e32306aa96f",
        ],
        "the payload chunk ids moved — see the manifest-id message above"
    );
}

/// **Ciphertext pin — zstd-sensitive by nature.**
///
/// The pack's name is the unkeyed BLAKE3 of the finished pack bytes, so it
/// covers every sealed frame in it plus the sealed header and the trailer. It
/// therefore moves on a zstd change, on a frame-layout change, and on a
/// serialization-order change in the pack header — which is exactly what makes
/// it the format's broadest drift detector.
///
/// On failure: check the two id pins first. If they held, the plaintext
/// addressing is intact and only the on-disk bytes changed, which is a
/// compatibility decision (old packs still open; byte-identical re-sealing
/// across the boundary is what is lost). If an id pin moved too, that is the
/// bigger event and this pin is a symptom of it.
///
/// Re-pinned in 1-10 for the inline chunk nonce: every blob in the pack, and the
/// sealed header itself, grew the 24 bytes. Both id pins held.
#[test]
fn the_multi_chunk_bundles_pack_address_is_pinned_and_is_zstd_sensitive() {
    assert_eq!(
        bundle().pack_address.to_string(),
        "9b612506608d3304baa33ee4e32827b8923dfbdeeda9a3c2a66fa404a6f817a2",
        "the pack bytes moved. Check the two id pins first: if they held, this \
         is a zstd, framing or header-serialization change and the on-disk \
         format now differs — a compatibility decision, not a regeneration"
    );
}
