//! The key hierarchy and every AEAD call in the encrypted-sync bundle format.
//!
//! ```text
//!     password + salt(16B, in the keyfile)
//!         |  Argon2id  m=1 GiB  t=3  p=1
//!     kek [32B]                                  ephemeral, zeroized
//!         |  XChaCha20-Poly1305 unwrap
//!         |  aad = canonical {format, kdf}       binds params, blocks downgrade
//!     master key [32B]                           random at init, never on disk
//!         |  BLAKE3 derive_key, one context string each
//!     chunk_key        name_key        root_key
//! ```
//!
//! **Containment invariant:** this is the only module under `src/sync/` that
//! imports `argon2` or `chacha20poly1305`. The test
//! `only_the_crypto_module_imports_the_cryptographic_crates` enforces it, so a
//! security review of the sync format is a review of this one file.
//!
//! Everything here is pure — no path, no env, no clock, no network. Every entry
//! point takes [`KdfParams`] by argument rather than reading a default inside
//! itself; that is the cheap-KDF test seam. Tests pass `{ m_kib: 8, t: 1, p: 1 }`
//! and run in microseconds, which keeps the AUR `check()` inside its time budget.
//! Production passes [`KdfParams::default`]. Those parameters are bounded rather
//! than trusted: [`MIN_KDF_MEMORY_KIB`] on the write path (with the floor itself
//! as an argument, so the cheap seam survives it) and [`MAX_KDF_MEMORY_KIB`]
//! inside [`derive_kek`] itself, which every derivation routes through — so a
//! caller reaching for the primitive directly with a hostile remote's `m_kib`
//! is bounded too, rather than only the two paths that remembered to ask.
//!
//! Secret hygiene is structural, not aspirational: key material lives in
//! `Zeroizing`, [`Keys`] has a hand-written `Debug` that prints `<redacted>`,
//! and no error message here interpolates a key, a password, a nonce, or a
//! plaintext.

use std::fmt;
use std::str::FromStr;

use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine;
use chacha20poly1305::XChaCha20Poly1305;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zeroize::{Zeroize, Zeroizing};

use crate::error::{AppError, Result};
use crate::sync::{
    CTX_CHUNK, CTX_NAME, CTX_NONCE, CTX_ROOT, KEYFILE_VERSION, MAX_SUPPORTED_KEYFILE, check_version,
};

/// XChaCha20-Poly1305 nonce width.
const NONCE_LEN: usize = 24;
/// Poly1305 tag width.
const TAG_LEN: usize = 16;

const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::STANDARD;

/// Argon2id parameters. Stored in cleartext in the keyfile *and* bound into the
/// wrap as associated data, so downgrading `m_kib` in transit does not produce a
/// KEK that unwraps.
///
/// Passed by argument everywhere. No derivation function may read a default
/// from inside itself — that is what makes the cheap-KDF test seam possible.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfParams {
    /// Memory cost in KiB.
    pub m_kib: u32,
    /// Time cost (passes).
    pub t: u32,
    /// Lanes. `1` because `argon2` 0.5 has no threading, so `p > 1` measures
    /// ~10% *worse* for the defender while handing a wide-SIMD attacker free
    /// intra-hash parallelism.
    pub p: u32,
}

impl Default for KdfParams {
    fn default() -> Self {
        Self {
            m_kib: 1_048_576,
            t: 3,
            p: 1,
        }
    }
}

/// The smallest Argon2id working set a *new* keyfile may be written at: 8 MiB.
///
/// Argon2's own floor is `8 * p` KiB, which is not a security parameter — it is
/// the smallest input the algorithm is defined for. Accepting it made the
/// 12-character password floor in [`crate::sync::passphrase`] arithmetic against
/// a guess rate nothing enforced: that figure is derived from what the *shipped*
/// parameters buy, and at 8 KiB a 12-character password falls in an afternoon.
///
/// 8 MiB is 1/128 of the shipped default and a thousand times argon2's floor.
/// The floor and the passphrase policy are also coupled directly — below
/// [`KdfParams::default`]'s memory, `passphrase::check` raises the accepted
/// password length from 12 characters to 20 — because a floor alone only moves
/// the line, while the coupling makes lowering the KDF cost pay for itself in
/// password length. Length, not measured entropy: 20 is what the generator
/// emits, and `passphrase` says plainly why nothing there can tell a generated
/// passphrase from a typed one of the same length.
///
/// Only new keyfiles. An existing bundle written below this stays openable
/// forever: refusing to *read* it would destroy data to enforce a policy the
/// user cannot retroactively satisfy.
pub const MIN_KDF_MEMORY_KIB: u32 = 8 * 1024;

/// The largest Argon2id working set this build will ever ask for: 4 GiB.
///
/// `m_kib` arrives from the keyfile, which arrives from a remote the format
/// treats as hostile. `argon2` 0.5.3 sets `MAX_M_COST = u32::MAX` and allocates
/// its blocks with an **infallible** `vec![]`, so one edited integer is a
/// `handle_alloc_error` abort or an OOM kill — reached *before* the AAD binding
/// gets its chance to make the unwrap fail, and in flat violation of the
/// project's hard invariant that the widget always exits 0.
///
/// Four gibibytes is four times the shipped default and far past any parameter
/// set a human would choose, so it costs a legitimate bundle nothing. This is a
/// hard ceiling, not a fit-in-RAM check; [`check_memory_budget`] is the separate,
/// actionable pre-flight a surface runs against the machine it is on.
pub const MAX_KDF_MEMORY_KIB: u32 = 4 * 1024 * 1024;

/// Refuse writing a new keyfile below `min_m_kib` — normally
/// [`MIN_KDF_MEMORY_KIB`].
///
/// Only ever applied on the *write* path. Reading is deliberately unbounded
/// below, so raising the floor later cannot strand a bundle already on a user's
/// disk.
fn check_kdf_floor(m_kib: u32, min_m_kib: u32) -> Result<()> {
    if m_kib >= min_m_kib {
        return Ok(());
    }
    Err(AppError::Other(format!(
        "refusing to write a keyfile at {m_kib} KiB of key-derivation memory: \
         the floor is {min_m_kib} KiB. Below it, guessing the password offline \
         is cheap enough that the length rules stop meaning anything — and a \
         bundle written now is attackable forever"
    )))
}

/// Refuse an Argon2id memory parameter above [`MAX_KDF_MEMORY_KIB`], before
/// anything allocates it.
///
/// Pure and clock-free, like everything else here. The parameters are public —
/// they live in cleartext in the keyfile — so naming them is safe.
///
/// Private, and it stays private because [`derive_kek`] applies it to every
/// derivation: an outside caller has nothing left to enforce by hand.
fn check_kdf_ceiling(m_kib: u32) -> Result<()> {
    if m_kib <= MAX_KDF_MEMORY_KIB {
        return Ok(());
    }
    Err(AppError::Other(format!(
        "this keyfile asks for {} MiB of key-derivation memory, above the \
         {} MiB ceiling this build will allocate — refusing before the \
         allocation rather than aborting inside it",
        m_kib / 1024,
        MAX_KDF_MEMORY_KIB / 1024
    )))
}

/// Password + salt -> key-encryption key.
///
/// **[`MAX_KDF_MEMORY_KIB`] is enforced here, not by the callers.** This is the
/// one function every derivation routes through, including the idiom
/// [`KdfDoc::params`] tells a caller to write —
/// `derive_kek(pw, &salt, keyfile.kdf.params())`, whose `m_kib` is whatever a
/// hostile remote put in the keyfile. The ceiling used to sit in `wrap` and in
/// `unwrap_master_key` instead, which bounded two call sites rather than the
/// rule: any third caller, in this crate or outside it, reached `argon2`
/// 0.5.3's infallible `vec![]` and died in `handle_alloc_error` — an abort, in
/// flat violation of the project's hard invariant that the widget always exits
/// 0. A guard in the shared function is both the smaller diff and the only one
/// a future caller cannot forget.
pub fn derive_kek(pw: &[u8], salt: &[u8; 16], k: KdfParams) -> Result<Zeroizing<[u8; 32]>> {
    check_kdf_ceiling(k.m_kib)?;
    let params = Params::new(k.m_kib, k.t, k.p, Some(32)).map_err(|_| {
        // The parameters are public (they live in cleartext in the keyfile), so
        // naming them is safe and is the only actionable thing to say.
        AppError::Other(format!(
            "invalid Argon2id parameters (m_kib={}, t={}, p={})",
            k.m_kib, k.t, k.p
        ))
    })?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(pw, salt, out.as_mut())
        .map_err(|_| AppError::Other("key derivation failed".into()))?;
    Ok(out)
}

/// A chunk's address. An address, not a secret — deriving `Debug` is fine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChunkId([u8; 32]);

impl ChunkId {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for ChunkId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl FromStr for ChunkId {
    type Err = AppError;

    fn from_str(s: &str) -> Result<Self> {
        // `is_ascii` also guarantees every 2-byte slice below lands on a char
        // boundary, so the indexing cannot panic.
        if s.len() != 64 || !s.is_ascii() {
            return Err(AppError::Other("chunk id must be 64 hex characters".into()));
        }
        let mut out = [0u8; 32];
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = u8::from_str_radix(&s[2 * i..2 * i + 2], 16)
                .map_err(|_| AppError::Other("chunk id is not valid hex".into()))?;
        }
        Ok(Self(out))
    }
}

impl Serialize for ChunkId {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ChunkId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

/// The three subkeys derived from the master key. Private fields, and a
/// hand-written `Debug`: deriving it would put key material one `{:?}` away
/// from a log line.
pub struct Keys {
    chunk: Zeroizing<[u8; 32]>,
    name: Zeroizing<[u8; 32]>,
    root: Zeroizing<[u8; 32]>,
}

impl fmt::Debug for Keys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Keys { <redacted> }")
    }
}

/// One context string per field. Getting this mapping wrong is invisible to
/// every round-trip test — it stays self-consistent under any wrong-but-stable
/// permutation — which is why 1-07 pins all three as known-answer vectors.
fn subkeys(mk: &[u8; 32]) -> Keys {
    Keys {
        chunk: Zeroizing::new(blake3::derive_key(CTX_CHUNK, mk)),
        name: Zeroizing::new(blake3::derive_key(CTX_NAME, mk)),
        root: Zeroizing::new(blake3::derive_key(CTX_ROOT, mk)),
    }
}

/// The KDF block of the keyfile, in cleartext. Field order here *is* the
/// canonical AAD byte order (see [`aad_bytes`]), so do not reorder it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfDoc {
    pub algo: String,
    pub version: u32,
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
    /// base64 of the 16 random salt bytes.
    pub salt: String,
}

impl KdfDoc {
    fn new(salt: &[u8; 16], k: KdfParams) -> Self {
        Self {
            algo: "argon2id".into(),
            version: 19,
            m_kib: k.m_kib,
            t: k.t,
            p: k.p,
            salt: B64.encode(salt),
        }
    }

    /// The parameters recorded in *this* keyfile. Callers must use these, never
    /// [`KdfParams::default`] — that is the whole point of storing them.
    pub fn params(&self) -> KdfParams {
        KdfParams {
            m_kib: self.m_kib,
            t: self.t,
            p: self.p,
        }
    }

    fn salt_bytes(&self) -> Result<[u8; 16]> {
        let raw = B64
            .decode(&self.salt)
            .map_err(|_| AppError::Other("keyfile salt is not valid base64".into()))?;
        raw.try_into()
            .map_err(|_| AppError::Other("keyfile salt is not 16 bytes".into()))
    }
}

/// Canonical AEAD associated data for the master-key wrap: `{format, kdf}`
/// serialized as a *struct*, so the byte order is the declaration order above.
/// A map would not guarantee ordering, and an AAD whose bytes depend on hash
/// iteration order is an AAD that intermittently fails to authenticate.
fn aad_bytes(format: u32, kdf: &KdfDoc) -> Result<Vec<u8>> {
    #[derive(Serialize)]
    struct KeyfileAad<'a> {
        format: u32,
        kdf: &'a KdfDoc,
    }
    serde_json::to_vec(&KeyfileAad { format, kdf })
        .map_err(|_| AppError::Other("failed to canonicalize the keyfile parameters".into()))
}

/// The on-disk keyfile: KDF parameters in cleartext plus the wrapped master
/// key. It holds no plaintext key material — the wrapped key is ciphertext and
/// the salt is public — so deriving `Debug` here is safe.
///
/// **The fields stay `pub`, and that is a considered trade rather than an
/// oversight.** They let an outside caller assemble a `Keyfile` struct without
/// going through `wrap`, so the write-path floor is not enforced by the type.
/// It is not enforced by the type in any case — a caller willing to build one by
/// hand is also willing to run Argon2id and XChaCha20-Poly1305 by hand, which is
/// what filling `wrapped_master_key` actually takes. What the fields buy is the
/// one test that proves the *documented* format and the implemented one are the
/// same thing: `tests/sync_vectors.rs` wraps a fixed master key from
/// `docs/sync-format.md` §1 and asserts [`Keyfile::open`] accepts it, which is
/// impossible against a private-field struct and irreplaceable as evidence. The
/// hole that mattered — an exported entry point that *takes* the floor as an
/// argument, so no crypto knowledge is needed to skip it — is closed above.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keyfile {
    pub format: u32,
    pub kdf: KdfDoc,
    /// base64 of the 24-byte wrap nonce.
    pub nonce: String,
    /// base64 of the 48-byte wrapped master key (32 ciphertext + 16 tag).
    pub wrapped_master_key: String,
}

impl Keyfile {
    /// Draw a fresh master key and wrap it under `pw`. Returns both the keyfile
    /// to persist and the live subkeys, so the caller never pays the KDF twice.
    ///
    /// Refuses `k.m_kib` below [`MIN_KDF_MEMORY_KIB`]. This is the **only** way
    /// to create a keyfile from outside the crate; `create_with_floor` beside it
    /// is `pub(crate)`.
    pub fn create(pw: &[u8], k: KdfParams) -> Result<(Keyfile, Keys)> {
        Self::create_with_floor(pw, k, MIN_KDF_MEMORY_KIB)
    }

    /// [`Keyfile::create`] with the memory floor as an argument — the cheap-KDF
    /// test seam, the same shape as `Cache::at` beside `Cache::for_vendor`.
    ///
    /// Tests wrap at `m_kib = 8` and run in microseconds, which is what keeps
    /// the AUR `check()` inside its budget on an installer's machine; they pass
    /// their own parameters as the floor.
    ///
    /// **`pub(crate)`, for the same reason `Keys::seal` is.** Exported, this was
    /// not a seam but a hole: `create` at 8 KiB is refused while this wrote the
    /// keyfile and it opened, so [`MIN_KDF_MEMORY_KIB`] bound `create` and
    /// `rewrap` rather than the format. Production has no reason to call it
    /// either — a caller that wants a lower floor wants [`MIN_KDF_MEMORY_KIB`]
    /// lowered, where the argument for it can be read. An integration test that
    /// needs a cheap keyfile builds one by hand from the public fields, which
    /// `tests/sync_vectors.rs` has to do anyway to pin a fixed master key.
    pub(crate) fn create_with_floor(
        pw: &[u8],
        k: KdfParams,
        min_m_kib: u32,
    ) -> Result<(Keyfile, Keys)> {
        let mut mk = Zeroizing::new([0u8; 32]);
        fill(&mut mk[..])?;
        let keyfile = Self::wrap(&mk, pw, k, min_m_kib)?;
        Ok((keyfile, subkeys(&mk)))
    }

    /// Unwrap the master key with `pw` and derive the subkeys.
    ///
    /// Uses **the parameters stored in this keyfile**, never
    /// [`KdfParams::default`]: a bundle initialised at a lower `--kdf-memory`
    /// must stay openable, and a bundle initialised higher must stay strong.
    pub fn open(&self, pw: &[u8]) -> Result<Keys> {
        Ok(subkeys(&*self.unwrap_master_key(pw)?))
    }

    /// Password change without re-encrypting any data: unwrap under the old
    /// password, rewrap the *same* master key under the new one. 48 bytes
    /// rewritten instead of the whole bundle.
    ///
    /// This is the CRYPTO-04 primitive only. The observable requirement also
    /// needs the old keyfile asset deleted from the remote, which Phase 4 owns —
    /// and even then, a keyfile already in git history stays unwrappable with
    /// the old password forever. Password change is not revocation.
    /// Refuses `k.m_kib` below [`MIN_KDF_MEMORY_KIB`], exactly as
    /// [`Keyfile::create`] does: a rewrap is the other way to choose the
    /// parameters a bundle lives at, so a floor that only guarded `create` would
    /// be a floor with a documented way around it.
    pub fn rewrap(&self, old_pw: &[u8], new_pw: &[u8], k: KdfParams) -> Result<Keyfile> {
        self.rewrap_with_floor(old_pw, new_pw, k, MIN_KDF_MEMORY_KIB)
    }

    /// [`Keyfile::rewrap`] with the memory floor as an argument. The test seam,
    /// `pub(crate)` for the reason `create_with_floor` is: a floor a caller can
    /// pass its own value for is not a floor at all.
    pub(crate) fn rewrap_with_floor(
        &self,
        old_pw: &[u8],
        new_pw: &[u8],
        k: KdfParams,
        min_m_kib: u32,
    ) -> Result<Keyfile> {
        let mk = self.unwrap_master_key(old_pw)?;
        Self::wrap(&mk, new_pw, k, min_m_kib)
    }

    fn wrap(mk: &[u8; 32], pw: &[u8], k: KdfParams, min_m_kib: u32) -> Result<Keyfile> {
        // The floor only. The ceiling is `derive_kek`'s, a few lines down, and
        // repeating it here would be a second copy of one rule — the shape that
        // let a third caller past it in the first place.
        check_kdf_floor(k.m_kib, min_m_kib)?;

        let mut salt = [0u8; 16];
        let mut nonce = [0u8; NONCE_LEN];
        fill(&mut salt)?;
        fill(&mut nonce)?;

        let kdf = KdfDoc::new(&salt, k);
        let aad = aad_bytes(KEYFILE_VERSION, &kdf)?;
        let kek = derive_kek(pw, &salt, k)?;
        let wrapped = XChaCha20Poly1305::new((&*kek).into())
            .encrypt(
                &nonce.into(),
                Payload {
                    msg: &mk[..],
                    aad: &aad,
                },
            )
            .map_err(|_| AppError::Other("failed to wrap the master key".into()))?;

        Ok(Keyfile {
            format: KEYFILE_VERSION,
            kdf,
            nonce: B64.encode(nonce),
            wrapped_master_key: B64.encode(&wrapped),
        })
    }

    fn unwrap_master_key(&self, pw: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
        // Before any cryptographic work: refusing a too-new bundle must not cost
        // 1.5 s and a gibibyte first. The absurd-working-set gate is one layer
        // down in `derive_kek`, which is still ahead of every allocation — the
        // only thing between here and there is base64 decoding.
        check_version(self.format, MAX_SUPPORTED_KEYFILE, "keyfile")?;

        let salt = self.kdf.salt_bytes()?;
        let nonce = self.nonce_bytes()?;
        let wrapped = B64
            .decode(&self.wrapped_master_key)
            .map_err(|_| AppError::Other("keyfile wrapped key is not valid base64".into()))?;
        let aad = self.aad()?;

        let kek = derive_kek(pw, &salt, self.kdf.params())?;
        let mut opened = XChaCha20Poly1305::new((&*kek).into())
            .decrypt(
                &nonce.into(),
                Payload {
                    msg: &wrapped,
                    aad: &aad,
                },
            )
            // A wrong password and a downgraded `m_kib` both land here, and both
            // get the same message: there is nothing useful to distinguish, and
            // an attacker learns nothing from the difference.
            .map_err(|_| AppError::Other("wrong password or corrupted keyfile".into()))?;

        if opened.len() != 32 {
            opened.zeroize();
            return Err(AppError::Other(
                "wrong password or corrupted keyfile".into(),
            ));
        }
        let mut mk = Zeroizing::new([0u8; 32]);
        mk.copy_from_slice(&opened);
        // The allocating AEAD API hands back a plain `Vec` holding the master
        // key and does not zeroize it for us (research §7.1). A `Vec` that
        // reallocated during construction also leaves an unreachable copy
        // behind — unavoidable with this API, and documented as accepted.
        opened.zeroize();
        Ok(mk)
    }

    fn nonce_bytes(&self) -> Result<[u8; NONCE_LEN]> {
        let raw = B64
            .decode(&self.nonce)
            .map_err(|_| AppError::Other("keyfile nonce is not valid base64".into()))?;
        raw.try_into()
            .map_err(|_| AppError::Other("keyfile nonce is not 24 bytes".into()))
    }

    fn aad(&self) -> Result<Vec<u8>> {
        aad_bytes(self.format, &self.kdf)
    }
}

impl Keys {
    /// A chunk's address: a **keyed** hash of the plaintext.
    ///
    /// Keyed, not plain BLAKE3, so read access to the repository does not let an
    /// attacker recompute the address of a guessed plaintext and confirm it is
    /// present. Restic and Borg use unkeyed content hashes for dedup and accept
    /// that confirmation-of-file oracle; keying costs nothing.
    pub fn chunk_id(&self, plaintext: &[u8]) -> ChunkId {
        ChunkId(*blake3::keyed_hash(&self.name, plaintext).as_bytes())
    }

    /// Seal `message` under `id`, with the nonce derived from **the message
    /// itself** and stored inline as the first [`NONCE_LEN`] bytes — the same
    /// framing [`Keys::seal_root`] uses.
    ///
    /// # Safety contract: the nonce binds to the message, the AAD binds to the id
    ///
    /// A derived nonce is only safe under **nonce ↔ message** injectivity. This
    /// used to derive the nonce from `id`, which addresses the *pre-image* the
    /// caller framed and compressed rather than the bytes handed to the AEAD —
    /// and zstd guarantees format compatibility across versions, not
    /// byte-identical output. Two builds compressing one plaintext differently
    /// therefore sealed two distinct messages under one
    /// `(chunk_key, nonce, aad)`: Poly1305 one-time-key recovery and keystream
    /// recovery, both reachable from an ordinary heterogeneous fleet. The nonce
    /// is now `derive_key(CTX_NONCE, keyed_hash(name_key, message))[..24]`, so a
    /// message that differs by one bit is sealed under a different nonce whether
    /// or not its id moved.
    ///
    /// **Any future caller must preserve that.** The nonce may be derived only
    /// from the exact bytes passed as `message`; the id may be anything that
    /// addresses them, because it is authenticated as associated data rather
    /// than trusted to be unique per message.
    ///
    /// # Known gap: the AAD carries no object type
    ///
    /// Every object sealed through here — a data chunk, a manifest chunk, an
    /// index chunk, a pack header — uses `chunk_key` with `aad = its own id` and
    /// **no domain separator saying which kind it is**. Serde ignores unknown
    /// fields, so an `IndexObject`'s JSON structurally deserializes as a
    /// `PackHeader`, and nothing in the AEAD layer objects to an object of one
    /// kind being served where another was expected.
    ///
    /// Traced in 1-11 and left as it is, deliberately. It dead-ends: the id is
    /// still bound, `open_chunk` still rechecks `chunk_id(plaintext) == id`, and
    /// `pack::read_header`'s bounds checks plus the per-blob tags stop the
    /// confused header before it yields anything — a read that errors, not a
    /// compromise. Closing it properly means a type byte in the AAD, which moves
    /// every sealed byte in the format and touches ~60 call sites; that is a
    /// format change and it belongs to a phase that can plan it, not to a
    /// remediation pass on a format that has just been stabilised and pinned.
    /// **Phase 2 owns it.** If you are adding a *new* kind of object to this
    /// key, add the domain separator first.
    ///
    /// `pub(crate)` because `chunk::seal_chunk` is the one caller that gets the
    /// framing right. Callers outside this crate go through
    /// [`crate::sync::chunk::seal_chunk`].
    ///
    /// Deterministic within a build: the same message yields the same nonce and
    /// therefore byte-identical output, which is what makes dedup work at all
    /// and what keeps re-syncing unchanged data from creating new git objects
    /// forever.
    pub(crate) fn seal(&self, id: &ChunkId, message: &[u8]) -> Result<Vec<u8>> {
        let nonce = self.chunk_nonce(message);
        let sealed = XChaCha20Poly1305::new((&*self.chunk).into())
            .encrypt(
                &nonce.into(),
                Payload {
                    msg: message,
                    aad: id.as_bytes(),
                },
            )
            .map_err(|_| AppError::Other("chunk encryption failed".into()))?;

        let mut framed = Vec::with_capacity(NONCE_LEN + sealed.len());
        framed.extend_from_slice(&nonce);
        framed.extend_from_slice(&sealed);
        Ok(framed)
    }

    /// A chunk's nonce: derived from the bytes actually encrypted, keyed so
    /// nobody without `name_key` can predict it for a guessed message.
    ///
    /// See [`Keys::seal`] for why this takes the message rather than the id.
    fn chunk_nonce(&self, message: &[u8]) -> [u8; NONCE_LEN] {
        let derived = blake3::derive_key(
            CTX_NONCE,
            blake3::keyed_hash(&self.name, message).as_bytes(),
        );
        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(&derived[..NONCE_LEN]);
        nonce
    }

    /// Open a chunk sealed under `id`. `framed` is `nonce ‖ ciphertext ‖ tag`,
    /// and the length is validated before the split: the bytes arrive from a
    /// remote an attacker may control, so a truncated chunk must be an error,
    /// never a panic.
    ///
    /// This performs **no** `chunk_id(plaintext) == id` identity recheck, and
    /// that is deliberate. The id addresses the caller's *raw plaintext*, while
    /// the bytes returned here are that plaintext's *framed and compressed*
    /// form — different things, so a recheck at this layer would compare apples
    /// to oranges. It would also tie every chunk id to the zstd version: a crate
    /// bump would re-id every chunk in every user's bundle, meaning a full
    /// re-upload and zero dedup.
    ///
    /// The belt-and-braces recheck therefore lives one layer up, in
    /// [`crate::sync::chunk`]'s `open_chunk` after unframing and in
    /// [`crate::sync::pack`]'s `read_header` after deserializing.
    ///
    /// **The refusal names the chunk.** Every tampering attack that reaches this
    /// line — a swapped ciphertext, a flipped bit, a truncated blob, a manifest
    /// chunk served under the id the root names — is one and the same event to
    /// Poly1305: the tag did not verify. Without the id in the message they all
    /// read identically, and an operator cannot tell which object is bad; the
    /// adversarial suite found exactly that collapse across five of its nine
    /// attacks. The id is safe to say: it is written in the clear in every pack
    /// trailer and listed in every index object, and it is a *keyed* hash, so
    /// nobody without `name_key` can invert it or confirm a guess against it. It
    /// is an address, not a secret — CRYPTO-07 is about keys, passwords, and
    /// plaintext, none of which is here.
    pub fn open(&self, id: &ChunkId, framed: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        if framed.len() < NONCE_LEN + TAG_LEN {
            return Err(AppError::Other(format!("chunk {id} is truncated")));
        }
        let (nonce, sealed) = framed.split_at(NONCE_LEN);
        let nonce: [u8; NONCE_LEN] = nonce.try_into().expect("length checked above");
        XChaCha20Poly1305::new((&*self.chunk).into())
            .decrypt(
                &nonce.into(),
                Payload {
                    msg: sealed,
                    aad: id.as_bytes(),
                },
            )
            .map(Zeroizing::new)
            .map_err(|_| AppError::Other(format!("chunk {id} failed authentication")))
    }

    /// Seal the snapshot root under the **root** subkey — the one derived with
    /// `CTX_ROOT`. Reaching for `chunk` here would pass every round-trip test,
    /// leave `root_key` dead, and silently diverge from the hierarchy 1-07 pins.
    ///
    /// The root is the one object whose plaintext changes on every sync, so its
    /// nonce cannot be content-derived without leaking equality between
    /// snapshots. It gets a fresh random 24-byte nonce, stored inline as the
    /// first `NONCE_LEN` bytes of the framed output; XChaCha's 192-bit nonce
    /// makes random generation safe with no counter accounting.
    ///
    /// `repo_id` is bound into the associated data, so a root belonging to
    /// another bundle fails the tag here rather than depending on local state to
    /// notice — see [`Keys::open_root`].
    pub fn seal_root(&self, plaintext: &[u8], repo_id: &str) -> Result<Vec<u8>> {
        let aad = root_aad(repo_id)?;
        let mut nonce = [0u8; NONCE_LEN];
        fill(&mut nonce)?;
        let sealed = XChaCha20Poly1305::new((&*self.root).into())
            .encrypt(
                &nonce.into(),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| AppError::Other("snapshot root encryption failed".into()))?;

        let mut framed = Vec::with_capacity(NONCE_LEN + sealed.len());
        framed.extend_from_slice(&nonce);
        framed.extend_from_slice(&sealed);
        Ok(framed)
    }

    /// Open a framed snapshot root belonging to `repo_id`. Validates the length
    /// before slicing: the bytes arrive from a remote an attacker may control,
    /// and a truncated root must be an error, not a panic.
    ///
    /// **`repo_id` is the caller's expectation, read from local configuration —
    /// never the value the remote claims.** It is bound as associated data, so a
    /// root belonging to a different bundle fails the Poly1305 tag here. That
    /// matters because the root is the entry point: two bundles sharing a master
    /// key (a copied keyfile, a second remote for the same machine) would
    /// otherwise open each other's roots, and a cross-bundle replay would depend
    /// entirely on local anchor state to notice. Taking the id from the remote
    /// would make the binding say nothing at all.
    pub fn open_root(&self, framed: &[u8], repo_id: &str) -> Result<Zeroizing<Vec<u8>>> {
        let aad = root_aad(repo_id)?;
        if framed.len() < NONCE_LEN + TAG_LEN {
            return Err(AppError::Other("snapshot root is truncated".into()));
        }
        let (nonce, sealed) = framed.split_at(NONCE_LEN);
        let nonce: [u8; NONCE_LEN] = nonce.try_into().expect("length checked above");
        XChaCha20Poly1305::new((&*self.root).into())
            .decrypt(
                &nonce.into(),
                Payload {
                    msg: sealed,
                    aad: &aad,
                },
            )
            .map(Zeroizing::new)
            .map_err(|_| AppError::Other("snapshot root failed authentication".into()))
    }
}

/// Associated data for the snapshot root: a fixed literal scoping the format,
/// concatenated with the repository this root belongs to.
///
/// The literal alone is not enough. The root is the mutable entry point and has
/// no content address of its own, so without the `repo_id` its AAD is the same
/// constant in every bundle in the world — and any two bundles sharing a master
/// key would open each other's roots. `repo_id` is not length-prefixed because
/// it is the last field: no other value follows it to be confused with.
///
/// **Which is why an empty one is refused here rather than at a caller.** An
/// empty `repo_id` degenerates this to the bare literal — the pre-F-3 global
/// constant, and the scoping silently gone. The check lives in the shared
/// function both [`Keys::seal_root`] and [`Keys::open_root`] route through, so
/// there is no path that forgets it; refusing symmetrically costs no reader
/// anything, because no build has ever been able to *write* such a root.
fn root_aad(repo_id: &str) -> Result<Vec<u8>> {
    if repo_id.is_empty() {
        return Err(AppError::Other(
            "a snapshot root needs a non-empty bundle identifier: an empty one \
             scopes its associated data to nothing, and two bundles sharing a \
             master key would open each other's roots"
                .into(),
        ));
    }
    let mut aad = ROOT_AAD.to_vec();
    aad.extend_from_slice(repo_id.as_bytes());
    Ok(aad)
}

/// The format-scoping half of [`root_aad`].
const ROOT_AAD: &[u8] = b"ai-usagebar.sync.v1 root";

/// Unkeyed BLAKE3, for naming an object whose bytes are already public
/// ciphertext — a pack file, say.
///
/// **Never seal anything under an unkeyed address.** An unkeyed hash of a
/// guessable plaintext is a confirmation oracle for anyone holding the
/// repository, which is exactly the property [`Keys::chunk_id`]'s keyed hash
/// exists to deny. This lives here rather than in `pack.rs` so no other module
/// needs to import a hash crate.
pub fn content_address(bytes: &[u8]) -> ChunkId {
    ChunkId(*blake3::hash(bytes).as_bytes())
}

/// Refuse an Argon2 working set that does not fit *this machine*, before
/// allocating it.
///
/// Pure, and takes `available_kib` by argument: that is the seam, exactly as
/// `Cache::at` exists so no test calls `Cache::for_vendor`. A 1 GB box must get
/// an actionable refusal naming `--kdf-memory`, never an OOM abort.
///
/// This is the *pre-flight*, for a surface that knows how much memory it has and
/// can print something a user can act on. It is not the safety net:
/// `check_kdf_ceiling` is, it runs inside [`derive_kek`] with no dependency on a
/// caller remembering anything, and it does not read the machine.
pub fn check_memory_budget(m_kib: u32, available_kib: u64) -> Result<()> {
    if u64::from(m_kib) <= available_kib {
        return Ok(());
    }
    Err(AppError::Other(format!(
        "key derivation needs {} MiB of memory but only {} MiB is available — \
         re-run with a lower --kdf-memory, which weakens every future restore \
         of this bundle",
        m_kib / 1024,
        available_kib / 1024
    )))
}

/// Best-effort available physical memory. The non-test wrapper around
/// [`check_memory_budget`]'s second argument; no test may call it.
#[cfg(target_os = "linux")]
pub fn available_memory_kib() -> Option<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    meminfo.lines().find_map(|line| {
        line.strip_prefix("MemAvailable:")?
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    })
}

/// See the Linux variant. macOS has no `MemAvailable`; `hw.memsize` (total
/// physical) is the closest honest answer.
#[cfg(target_os = "macos")]
pub fn available_memory_kib() -> Option<u64> {
    let out = std::process::Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()?;
    let bytes: u64 = String::from_utf8(out.stdout).ok()?.trim().parse().ok()?;
    Some(bytes / 1024)
}

/// See the Linux variant. Elsewhere the budget check is simply skipped.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn available_memory_kib() -> Option<u64> {
    None
}

/// The OS CSPRNG. No `rand`, no `ThreadRng`, no reseeding.
fn fill(buf: &mut [u8]) -> Result<()> {
    getrandom::fill(buf)
        .map_err(|_| AppError::Other("the operating system random source is unavailable".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Microseconds instead of ~1.5 s and a gibibyte. Never use production
    /// parameters in a unit test: the AUR `check()` runs these on an
    /// installer's machine.
    const CHEAP: KdfParams = KdfParams {
        m_kib: 8,
        t: 1,
        p: 1,
    };

    fn keys() -> Keys {
        Keyfile::create_with_floor(b"correct horse battery staple", CHEAP, CHEAP.m_kib)
            .expect("keyfile creation")
            .1
    }

    /// What a caller reads out of its own configuration and hands to
    /// [`Keys::open_root`] — never a value taken from the remote.
    const REPO_ID: &str = "usagebar-sync-abc123";

    #[test]
    fn password_round_trips_to_a_sealed_and_reopened_buffer() {
        let (keyfile, keys) =
            Keyfile::create_with_floor(b"correct horse battery staple", CHEAP, CHEAP.m_kib)
                .unwrap();

        let plaintext = b"the quick brown fox".as_slice();
        let id = keys.chunk_id(plaintext);
        let sealed = keys.seal(&id, plaintext).unwrap();
        assert_eq!(&*keys.open(&id, &sealed).unwrap(), plaintext);

        // …and the same password, through the persisted keyfile, gets there too.
        let reopened = keyfile.open(b"correct horse battery staple").unwrap();
        assert_eq!(reopened.chunk_id(plaintext), id);
        assert_eq!(&*reopened.open(&id, &sealed).unwrap(), plaintext);
    }

    #[test]
    fn identical_plaintext_under_the_same_id_seals_to_identical_bytes() {
        // Deterministic sealing is what makes dedup possible; with a random
        // nonce every re-upload of an unchanged chunk would be a new git object.
        let keys = keys();
        let plaintext = b"dedup me".as_slice();
        let id = keys.chunk_id(plaintext);
        assert_eq!(
            keys.seal(&id, plaintext).unwrap(),
            keys.seal(&id, plaintext).unwrap()
        );
    }

    /// The regression guard for the nonce-reuse blocker.
    ///
    /// `seal_chunk` addresses a chunk by its *raw plaintext* and encrypts that
    /// plaintext's compressed frame. zstd guarantees format compatibility across
    /// versions, not byte-identical output, so two builds can hand one id two
    /// different messages. If the nonce were still derived from the id, those
    /// two messages would be sealed under one `(chunk_key, nonce, aad)` — a
    /// solvable Poly1305 key and `C_A ⊕ C_B = F_A ⊕ F_B`.
    ///
    /// Two zstd versions are not installable in a unit test, so the differing
    /// framing is simulated directly: the same id, two different messages, which
    /// is exactly the shape a zstd bump produces.
    #[test]
    fn two_framings_of_one_plaintext_are_sealed_under_different_nonces() {
        let keys = keys();
        let id = keys.chunk_id(b"one plaintext, two zstd builds");

        // Same declared `true_len`, different compressed bytes — the collision a
        // heterogeneous fleet produces.
        let mut a = 23u32.to_le_bytes().to_vec();
        a.extend_from_slice(b"\x09\x00\x00\x00 frame as vN wrote it");
        let mut b = 23u32.to_le_bytes().to_vec();
        b.extend_from_slice(b"\x0a\x00\x00\x00 frame as vN+1 wrote");

        let (sealed_a, sealed_b) = (keys.seal(&id, &a).unwrap(), keys.seal(&id, &b).unwrap());
        assert_ne!(
            sealed_a[..NONCE_LEN],
            sealed_b[..NONCE_LEN],
            "two distinct messages were sealed under one nonce — the AEAD's \
             one-time-key assumption is broken and both are recoverable"
        );

        // …and both still open under the id they were sealed with, so the nonce
        // really did travel inline rather than being re-derived from the id.
        assert_eq!(&**keys.open(&id, &sealed_a).unwrap(), &a[..]);
        assert_eq!(&**keys.open(&id, &sealed_b).unwrap(), &b[..]);
    }

    #[test]
    fn a_chunk_shorter_than_a_nonce_and_a_tag_errors_instead_of_panicking() {
        let keys = keys();
        let id = keys.chunk_id(b"addressable");
        for len in [0, 1, NONCE_LEN, NONCE_LEN + TAG_LEN - 1] {
            let err = keys
                .open(&id, &vec![0u8; len])
                .expect_err("a {len}-byte chunk must be refused, not indexed into")
                .to_string();
            assert!(err.contains("is truncated"), "at {len} bytes: {err}");
        }
    }

    #[test]
    fn the_three_subkeys_are_distinct() {
        let keys = keys();
        assert_ne!(*keys.chunk, *keys.name);
        assert_ne!(*keys.chunk, *keys.root);
        assert_ne!(*keys.name, *keys.root);
    }

    #[test]
    fn a_wrong_password_yields_an_error_and_no_plaintext() {
        let (keyfile, _) =
            Keyfile::create_with_floor(b"the real password", CHEAP, CHEAP.m_kib).unwrap();
        let err = keyfile
            .open(b"not the real password")
            .expect_err("a wrong password must not open the keyfile")
            .to_string();
        assert!(err.contains("wrong password or corrupted keyfile"));
        // The message names the operation and nothing else (D5, CRYPTO-07).
        assert!(!err.contains("the real password"));
        assert!(!err.contains(&keyfile.wrapped_master_key));
    }

    #[test]
    fn a_chunk_id_round_trips_through_hex_and_serde() {
        let id = keys().chunk_id(b"addressable");
        let hex = id.to_string();
        assert_eq!(hex.len(), 64);
        assert_eq!(hex.parse::<ChunkId>().unwrap(), id);
        assert_eq!(
            serde_json::from_str::<ChunkId>(&serde_json::to_string(&id).unwrap()).unwrap(),
            id
        );
        assert!("nothex".parse::<ChunkId>().is_err());
        assert!("zz".repeat(32).parse::<ChunkId>().is_err());
    }

    #[test]
    fn debug_for_keys_redacts_the_key_material() {
        let keys = keys();
        assert_eq!(format!("{keys:?}"), "Keys { <redacted> }");
    }

    #[test]
    fn a_root_sealed_twice_differs_but_both_copies_reopen() {
        let keys = keys();
        let plaintext = b"counter=7".as_slice();
        let first = keys.seal_root(plaintext, REPO_ID).unwrap();
        let second = keys.seal_root(plaintext, REPO_ID).unwrap();

        // Fresh random nonce per seal: the root's plaintext changes every sync,
        // so deterministic sealing would leak equality between snapshots.
        assert_ne!(first, second);
        assert_eq!(&*keys.open_root(&first, REPO_ID).unwrap(), plaintext);
        assert_eq!(&*keys.open_root(&second, REPO_ID).unwrap(), plaintext);
    }

    #[test]
    fn the_root_path_uses_the_root_subkey_and_not_the_chunk_subkey() {
        let alice = keys();
        // Same chunk and name subkeys, different root subkey. If `seal_root`
        // reached for `chunk`, this would open — and `root_key` would be dead.
        let mallory = Keys {
            chunk: alice.chunk.clone(),
            name: alice.name.clone(),
            root: Zeroizing::new([0x5a; 32]),
        };
        let sealed = alice.seal_root(b"counter=7", REPO_ID).unwrap();
        assert!(mallory.open_root(&sealed, REPO_ID).is_err());
        assert!(alice.open_root(&sealed, REPO_ID).is_ok());
    }

    /// The root's AAD is scoped to the bundle, not only to the format.
    ///
    /// Two bundles can share a master key — a copied keyfile, or a second remote
    /// added for the same machine — and with a fixed AAD literal each would open
    /// the other's root. That makes a cross-bundle replay depend entirely on
    /// local anchor state to notice, which is exactly the dependency F-3 removes.
    #[test]
    fn a_root_from_another_bundle_fails_the_tag_under_the_same_master_key() {
        let keys = keys();
        let sealed = keys.seal_root(b"counter=7", "bundle-a").unwrap();

        assert!(
            keys.open_root(&sealed, "bundle-b").is_err(),
            "a root sealed for another bundle must fail the tag, not open"
        );
        // …and it is the scoping that refuses it, not a broken round trip.
        assert_eq!(&*keys.open_root(&sealed, "bundle-a").unwrap(), b"counter=7");
    }

    /// An empty `repo_id` is refused on both sides, because it is the scoping
    /// silently switching itself off.
    ///
    /// `root_aad("")` is the bare literal — the same global constant in every
    /// bundle in the world, which is exactly what F-3 replaced. Nothing else
    /// required the identifier to be non-empty, so two bundles that both left it
    /// blank and shared a master key opened each other's roots again.
    #[test]
    fn an_empty_bundle_identifier_is_refused_rather_than_scoping_the_root_to_nothing() {
        let keys = keys();

        let err = keys
            .seal_root(b"counter=7", "")
            .expect_err("an empty bundle identifier must not seal")
            .to_string();
        assert!(err.contains("non-empty bundle identifier"), "{err}");

        // …and on the read side too, so a root written by some other tool with
        // an empty identifier cannot be opened into the degenerate AAD either.
        let sealed = keys.seal_root(b"counter=7", REPO_ID).unwrap();
        assert!(keys.open_root(&sealed, "").is_err());
        assert!(keys.open_root(&sealed, REPO_ID).is_ok());
    }

    #[test]
    fn a_root_shorter_than_a_nonce_and_a_tag_errors_instead_of_panicking() {
        let keys = keys();
        for len in [0, 1, NONCE_LEN, NONCE_LEN + TAG_LEN - 1] {
            assert!(
                keys.open_root(&vec![0u8; len], REPO_ID).is_err(),
                "a {len}-byte root must be refused, not indexed into"
            );
        }
    }

    #[test]
    fn rewrap_under_a_new_password_preserves_the_subkeys() {
        let (keyfile, original) =
            Keyfile::create_with_floor(b"first password", CHEAP, CHEAP.m_kib).unwrap();
        let rewrapped = keyfile
            .rewrap_with_floor(b"first password", b"second password", CHEAP, CHEAP.m_kib)
            .unwrap();

        // A fresh salt and nonce, so the bytes differ …
        assert_ne!(keyfile.kdf.salt, rewrapped.kdf.salt);
        assert_ne!(keyfile.wrapped_master_key, rewrapped.wrapped_master_key);

        // … but the same master key is underneath, so no data needs re-encrypting.
        let opened = rewrapped.open(b"second password").unwrap();
        assert_eq!(*opened.chunk, *original.chunk);
        assert_eq!(*opened.name, *original.name);
        assert_eq!(*opened.root, *original.root);

        // The old password no longer opens the *new* keyfile.
        assert!(rewrapped.open(b"first password").is_err());
    }

    #[test]
    fn rewrap_with_the_wrong_old_password_produces_no_keyfile() {
        let (keyfile, _) =
            Keyfile::create_with_floor(b"first password", CHEAP, CHEAP.m_kib).unwrap();
        assert!(
            keyfile
                .rewrap_with_floor(b"guessed wrong", b"second password", CHEAP, CHEAP.m_kib)
                .is_err()
        );
    }

    /// The write path refuses a KDF cost the passphrase policy is not calibrated
    /// against — through both entry points, because a rewrap is the other way to
    /// choose the parameters a bundle lives at.
    ///
    /// Deliberately asserted through `create`/`rewrap`, the functions production
    /// calls, rather than through `check_kdf_floor`: a floor only guarding one
    /// of the two is a floor with a documented way around it.
    #[test]
    fn a_new_keyfile_below_the_memory_floor_is_refused_through_both_entry_points() {
        let below = KdfParams {
            m_kib: MIN_KDF_MEMORY_KIB - 1,
            t: 1,
            p: 1,
        };
        let err = Keyfile::create(b"a twelve-char", below)
            .expect_err("a keyfile below the floor must not be written")
            .to_string();
        assert!(err.contains("the floor is"), "{err}");
        assert!(
            !err.contains("a twelve-char"),
            "a refusal echoed the password"
        );

        // The floor is a real one rather than argon2's own 8 KiB, which is the
        // smallest input the algorithm is defined for and not a security
        // parameter at all. Asserted through `create` so it is the *behaviour*
        // being pinned, not the constant's value.
        assert!(
            Keyfile::create(b"a twelve-char", CHEAP).is_err(),
            "argon2's own floor must not be an acceptable place to write a bundle"
        );
        assert!(MIN_KDF_MEMORY_KIB < KdfParams::default().m_kib);

        // …and the same refusal on the rewrap path, from a keyfile that exists.
        let (keyfile, _) = Keyfile::create_with_floor(b"first password", CHEAP, CHEAP.m_kib)
            .expect("the seam still wraps cheaply");
        assert!(
            keyfile
                .rewrap(b"first password", b"second password", below)
                .is_err(),
            "rewrap must apply the same floor create does"
        );

        // An *existing* bundle below the floor still opens: refusing to read it
        // would destroy data to enforce a policy its owner cannot retroactively
        // satisfy.
        assert!(keyfile.open(b"first password").is_ok());
    }

    #[test]
    fn a_keyfile_opens_with_its_own_stored_parameters_not_the_compiled_default() {
        let stored = KdfParams {
            m_kib: 16,
            t: 2,
            p: 1,
        };
        // The point of the test: the production default is 1 GiB / t=3, so a
        // keyfile that opens here proves the stored parameters are what got used.
        assert_eq!(KdfParams::default().m_kib, 1_048_576);
        assert_ne!(stored, KdfParams::default());

        let (keyfile, _) =
            Keyfile::create_with_floor(b"stored params please", stored, stored.m_kib).unwrap();
        assert_eq!(keyfile.kdf.params(), stored);
        assert!(keyfile.open(b"stored params please").is_ok());
    }

    #[test]
    fn kdf_parameters_edited_in_transit_fail_to_unwrap() {
        let (keyfile, _) = Keyfile::create_with_floor(b"in transit", CHEAP, CHEAP.m_kib).unwrap();

        // An attacker downgrading the work factor on the wire. The parameters
        // are bound as AEAD associated data, so the KEK they now describe is not
        // the KEK the master key was wrapped under.
        let mut tampered = keyfile.clone();
        tampered.kdf.m_kib = 16;
        assert!(tampered.open(b"in transit").is_err());

        let mut retagged = keyfile.clone();
        retagged.kdf.algo = "argon2i".into();
        assert!(retagged.open(b"in transit").is_err());

        // Untouched, it still opens — so the failures above are the binding, not
        // a broken round trip.
        assert!(keyfile.open(b"in transit").is_ok());
    }

    #[test]
    fn a_keyfile_above_the_read_ceiling_is_refused_before_any_cryptographic_work() {
        let (keyfile, _) =
            Keyfile::create_with_floor(b"future bundle", CHEAP, CHEAP.m_kib).unwrap();

        // At the ceiling: accepted.
        assert_eq!(keyfile.format, MAX_SUPPORTED_KEYFILE);
        assert!(keyfile.open(b"future bundle").is_ok());

        // One above, and written by a future client at the production 1 GiB
        // parameters. If the version gate ever moved below `derive_kek`, this
        // test would allocate a gibibyte instead of returning instantly.
        let mut from_the_future = keyfile.clone();
        from_the_future.format = MAX_SUPPORTED_KEYFILE + 1;
        from_the_future.kdf.m_kib = KdfParams::default().m_kib;
        from_the_future.kdf.t = KdfParams::default().t;
        let err = from_the_future
            .open(b"future bundle")
            .expect_err("a newer format must be refused")
            .to_string();
        assert!(err.contains("upgrade ai-usagebar"));
    }

    /// The ceiling is checked inside the unwrap, not left to a caller.
    ///
    /// `m_kib` comes from a keyfile a hostile remote may have edited, and
    /// `argon2` 0.5.3 allows up to `u32::MAX` and allocates with an infallible
    /// `vec![]` — so without this the attack is an abort, not an error, and the
    /// widget stops exiting 0.
    #[test]
    fn a_keyfile_demanding_more_memory_than_the_ceiling_is_refused_before_allocating() {
        let (keyfile, _) =
            Keyfile::create_with_floor(b"a hostile remote", CHEAP, CHEAP.m_kib).unwrap();

        let mut absurd = keyfile.clone();
        absurd.kdf.m_kib = u32::MAX;
        let err = absurd
            .open(b"a hostile remote")
            .expect_err("u32::MAX KiB is 4 TiB and must never be attempted")
            .to_string();
        assert!(err.contains("ceiling this build will allocate"), "{err}");
        assert!(err.contains("4096 MiB"), "{err}");

        // Exactly at the ceiling is allowed through; one KiB past it is not.
        // Asserted on the gate itself, because letting `open` actually reach
        // `derive_kek` at 4 GiB is precisely what a unit test must not do.
        assert!(check_kdf_ceiling(MAX_KDF_MEMORY_KIB).is_ok());
        assert!(check_kdf_ceiling(MAX_KDF_MEMORY_KIB + 1).is_err());

        // Untouched, it still opens, so the refusal above is the gate rather
        // than a keyfile that never worked.
        assert!(keyfile.open(b"a hostile remote").is_ok());
    }

    /// The ceiling belongs to `derive_kek`, not to the two paths that used to
    /// remember it.
    ///
    /// `KdfDoc::params` is public and its own doc tells callers to use the
    /// keyfile's stored parameters, so this is the *recommended* idiom written
    /// out — with the `m_kib` a hostile remote chose. Guarding `wrap` and
    /// `unwrap_master_key` instead left exactly this line reaching argon2
    /// 0.5.3's infallible `vec![]`, which aborts rather than errors.
    #[test]
    fn derive_kek_refuses_the_ceiling_itself_rather_than_trusting_its_callers() {
        let (keyfile, _) =
            Keyfile::create_with_floor(b"a hostile remote", CHEAP, CHEAP.m_kib).unwrap();
        let mut absurd = keyfile.clone();
        absurd.kdf.m_kib = u32::MAX;

        let err = derive_kek(b"a hostile remote", &[0u8; 16], absurd.kdf.params())
            .expect_err("the documented idiom must not reach a 4 TiB allocation")
            .to_string();
        assert!(err.contains("ceiling this build will allocate"), "{err}");
        // …and the message names no secret: the parameters are cleartext in the
        // keyfile, the password is not in it.
        assert!(!err.contains("a hostile remote"), "{err}");

        // The honest parameters still derive, so the refusal is the gate.
        assert!(derive_kek(b"a hostile remote", &[0u8; 16], keyfile.kdf.params()).is_ok());
    }

    #[test]
    fn the_memory_budget_refuses_actionably_instead_of_letting_argon2_oom() {
        let err = check_memory_budget(1_048_576, 900_000)
            .expect_err("1 GiB must not be attempted on a 900 MiB budget")
            .to_string();
        assert!(err.contains("--kdf-memory"));
        assert!(err.contains("1024 MiB"));
        assert!(err.contains("878 MiB"));

        assert!(check_memory_budget(1_048_576, 4_000_000).is_ok());
        // Exactly enough is enough.
        assert!(check_memory_budget(1_048_576, 1_048_576).is_ok());
    }

    #[test]
    fn content_address_is_unkeyed_and_therefore_key_independent() {
        // Naming-only: it must never address a *sealed* object, or it becomes a
        // confirmation-of-plaintext oracle for anyone holding the repository.
        let public_bytes = b"already-public ciphertext".as_slice();
        assert_eq!(content_address(public_bytes), content_address(public_bytes));
        // Two unrelated key hierarchies produce the same address, which is the
        // whole difference from `chunk_id`.
        assert_ne!(keys().chunk_id(public_bytes), content_address(public_bytes));
        assert_ne!(keys().chunk_id(public_bytes), keys().chunk_id(public_bytes));
    }

    /// **Recursive.** It used to be a flat `read_dir` over `src/sync`, which
    /// left `src/sync/push/` and `src/sync/github/` — 11,500 lines, the two
    /// directories Phase 4 created — entirely invisible to the invariant whose
    /// stated value is "what lets a security auditor read one file instead of
    /// six". Phase 4's audit proved it by adding
    /// `use chacha20poly1305::XChaCha20Poly1305;` to `push/packer.rs` and
    /// watching this test pass. The walk is shared (`sync::guard`) so the next
    /// directory is covered on the day it is created.
    #[test]
    fn only_the_crypto_module_imports_the_cryptographic_crates() {
        let mut checked = 0;
        let mut skipped = 0;
        for path in crate::sync::guard::rs_files_in("src/sync") {
            if path.file_name().and_then(|n| n.to_str()) == Some("crypto.rs") {
                skipped += 1;
                continue;
            }
            checked += 1;
            let source = std::fs::read_to_string(&path).expect("readable module");
            for line in source.lines() {
                let trimmed = line.trim_start();
                // Match on the leading `use` token, so prose in a doc comment
                // naming these crates can never trip the gate.
                let Some(imported) = trimmed.strip_prefix("use ") else {
                    continue;
                };
                for crate_name in ["argon2", "chacha20poly1305"] {
                    assert!(
                        !imported.starts_with(crate_name),
                        "{} imports {crate_name} directly; every cryptographic \
                         call belongs in src/sync/crypto.rs",
                        path.display()
                    );
                }
            }
        }
        // Non-vacuity: a guard asserting an absence must prove it looked, and
        // the count is the flat walk's plus both subdirectories.
        assert_eq!(skipped, 1, "crypto.rs must be excluded exactly once");
        assert!(checked >= 20, "only {checked} files walked under src/sync");
    }
}
