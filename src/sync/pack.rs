//! Pack files — the unit of transfer: many sealed blobs concatenated into one
//! remote object, followed by a sealed header describing them.
//!
//! Packing is not an optimisation. GitHub caps content creation at 80 requests
//! per minute and 500 per hour, so one request per chunk is structurally
//! impossible at 5,000 chunks; a handful of 32 MiB objects is not.
//!
//! # Layout
//!
//! ```text
//! <blob0 ciphertext><blob1 ciphertext>…<blobN ciphertext>
//! <sealed header><32-byte header id><u32 LE header length>
//! ```
//!
//! # The header id is keyed, and it lives in the trailer
//!
//! Two decisions, and both of them are the point of this layout.
//!
//! **Keyed.** The header is sealed through [`seal_chunk`], which addresses it by
//! [`Keys::chunk_id`](crate::sync::crypto::Keys::chunk_id) —
//! `blake3::keyed_hash(name_key, …)`. It is *never* addressed by
//! [`content_address`]. A pack header is a list of chunk ids that anyone holding
//! the repository can already see, plus offsets and lengths he can measure
//! against the file he is looking at: it is the single most guessable object in
//! the format. Sealing it under an unkeyed address would let him hash his guess
//! and compare — exactly the confirmation-of-plaintext oracle that keyed chunk
//! ids exist to deny. `crypto.rs` states the rule ("never seal anything under an
//! unkeyed address"); this is the object that would have broken it first.
//!
//! **In the trailer.** [`read_header`] needs that id *before* it can decrypt,
//! because the id is bound as associated data. So the id is written in the
//! clear, immediately before the length. Writing it costs
//! nothing: it is a keyed hash, so an attacker without `name_key` cannot
//! recompute it from the header he is staring at, and substituting some other id
//! simply breaks the tag. Without it the reader is not merely slower, it is
//! impossible to write.
//!
//! [`content_address`] appears here for one thing only: naming the finished
//! pack, whose bytes are already public ciphertext.
//!
//! # The header carries no names
//!
//! An entry records an id, an offset, a ciphertext length, and a plaintext
//! length. No path, no filename, no directory structure — those live in the
//! manifest, which is itself a sealed chunk (plan 1-04). Local paths leak
//! account UUIDs and session ids, so they must never sit in a pack header.
//!
//! # Packs are immutable once sealed
//!
//! A chunk already inside a pack is never re-packed; only an explicit
//! prune-repack (phase 4) ever rewrites one. That immutability is what makes a
//! crashed sync leave *orphan packs* — garbage to be collected later, never
//! corruption.
//!
//! Owned by plan 1-03.

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::error::{AppError, Result};
use crate::sync::chunk::{Blob, open_chunk, seal_chunk};
use crate::sync::crypto::{ChunkId, Keys, content_address};
use crate::sync::{MAX_SUPPORTED_PACK_HEADER, PACK_HEADER_VERSION, check_version};

/// Size a pack aims for. CAL-1's fallback, still unmeasured: private-repo
/// release assets are assumed **not** to honour `Range:`, so fetching one chunk
/// means fetching its whole pack, and 32 MiB is where that waste stays
/// tolerable. The probe was offered in phase 3 and declined — see
/// `docs/sync-format.md` §7. Raising this is an optimisation for a partial
/// restore, not a gate: a restore fetches whole packs regardless, and
/// [`PACK_MAX`] already sits under the 64 MiB asset-download cap.
pub const PACK_TARGET: usize = 32 * 1024 * 1024;

/// Hard ceiling — a writer is sealed before a blob would carry it past this.
///
/// **This bounds the writer's body, not the published asset.** For the asset,
/// see [`PACK_ASSET_MAX`].
pub const PACK_MAX: usize = 48 * 1024 * 1024;

/// The largest a finished pack **asset** can be.
///
/// [`PACK_MAX`] bounds the body: a blob is never added that would carry the
/// body past it. The published asset is that body plus the sealed header and
/// the trailer, so it is legitimately larger — and a reader that compares an
/// asset's declared size against `PACK_MAX` refuses packs this writer is
/// entitled to produce.
///
/// It did. A real 50,391,460-byte pack was refused against 50,331,648 on a
/// first restore, and the bundle could not be read at all. The producer
/// measured the body and the consumer measured the file; each was correct
/// alone.
///
/// The header is sealed as one chunk, which is what bounds it — that ceiling is
/// why a pack's entry count is capped rather than growing with tiny chunks.
pub const PACK_ASSET_MAX: usize =
    PACK_MAX + crate::sync::CHUNK_SIZE + SEAL_OVERHEAD + ID_LEN + LEN_LEN;

/// Nonce, tag and framing a sealed chunk adds over its plaintext. Measured at
/// 63 for a full chunk; 64 is the round number above it.
const SEAL_OVERHEAD: usize = 64;

/// The header id, written in the clear at the end of the pack.
const ID_LEN: usize = 32;
/// The `u32` little-endian header length, the very last bytes of the pack.
const LEN_LEN: usize = 4;
/// Everything after the sealed header.
const TRAILER_LEN: usize = ID_LEN + LEN_LEN;

/// Where one sealed blob sits inside a pack.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackEntry {
    pub id: ChunkId,
    /// Byte offset of the ciphertext from the start of the pack.
    pub offset: u64,
    /// Ciphertext length — what to slice out of the pack.
    pub clen: u32,
    /// Plaintext length the blob's frame declares — what the caller gets back.
    pub true_len: u32,
}

/// The sealed index of a pack's contents.
#[derive(Debug, Serialize, Deserialize)]
pub struct PackHeader {
    pub format: u32,
    pub entries: Vec<PackEntry>,
}

/// Accumulates sealed blobs into one pack.
#[derive(Debug, Default)]
pub struct PackWriter {
    bytes: Vec<u8>,
    entries: Vec<PackEntry>,
}

impl PackWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a sealed blob, recording where it landed.
    pub fn push(&mut self, blob: Blob) {
        self.entries.push(PackEntry {
            id: blob.id,
            offset: self.bytes.len() as u64,
            clen: blob.ciphertext.len() as u32,
            true_len: blob.true_len,
        });
        self.bytes.extend_from_slice(&blob.ciphertext);
    }

    /// Bytes of blob ciphertext so far — the number [`should_seal`] takes.
    pub fn len_bytes(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Seal the header and return the pack's **content address** together with
    /// its bytes.
    ///
    /// The returned id is [`content_address`] of the finished bytes: naming
    /// only, so a pack substituted on the remote cannot keep the name it is
    /// served under. It is emphatically not a sealing address — see the module
    /// docs for why the *header* is addressed by a keyed hash instead.
    ///
    /// An empty writer is an error: a headerless pack would be an object no
    /// reader could interpret.
    pub fn finish(self, keys: &Keys) -> Result<(ChunkId, Vec<u8>)> {
        self.finish_at_version(keys, PACK_HEADER_VERSION)
    }

    /// The seam behind [`finish`](Self::finish): the only reason the version is
    /// a parameter is that a test must be able to write a header from the future
    /// and prove the reader refuses it.
    fn finish_at_version(self, keys: &Keys, format: u32) -> Result<(ChunkId, Vec<u8>)> {
        if self.entries.is_empty() {
            return Err(AppError::Other(
                "refusing to seal a pack with no blobs in it".into(),
            ));
        }

        let header = PackHeader {
            format,
            entries: self.entries,
        };
        // JSON straight into `seal_chunk`, which frames, compresses and seals.
        // Do not zstd it here as well — that would compress twice for nothing.
        //
        // Ceiling: `seal_chunk` refuses more than CHUNK_SIZE, which bounds a
        // header at some thousands of entries. A 32 MiB pack of 256 KiB chunks
        // holds ~128, so this is slack rather than a limit; if pathologically
        // small blobs ever crowd a pack, `finish` errors cleanly and the fix is
        // a format-2 header sealed as several chunks.
        let json = serde_json::to_vec(&header)
            .map_err(|_| AppError::Other("pack header could not be serialized".into()))?;
        let sealed = seal_chunk(keys, &json)?;

        let mut out = self.bytes;
        out.extend_from_slice(&sealed.ciphertext);
        out.extend_from_slice(sealed.id.as_bytes());
        out.extend_from_slice(&(sealed.ciphertext.len() as u32).to_le_bytes());
        Ok((content_address(&out), out))
    }
}

/// Parse and open a pack's trailing header.
///
/// Every number here arrives from a remote the format treats as hostile, so all
/// of it is bounds-checked before anything is sliced, allocated, or returned
/// (T-03-01, T-03-05): the declared header length must fit inside the pack, and
/// every entry's `offset + clen` must land inside the blob region. A truncated
/// pack fails here rather than later at an out-of-bounds slice, and no entry is
/// returned from a header that failed any check.
pub fn read_header(keys: &Keys, pack: &[u8]) -> Result<PackHeader> {
    if pack.len() < TRAILER_LEN {
        return Err(AppError::Other(
            "pack is too short to hold a trailer".into(),
        ));
    }
    let len_at = pack.len() - LEN_LEN;
    let id_at = len_at - ID_LEN;

    let header_len = u32::from_le_bytes(pack[len_at..].try_into().expect("4 bytes")) as usize;
    let id = ChunkId::from_bytes(pack[id_at..len_at].try_into().expect("32 bytes"));

    // `checked_sub` is the whole DoS check: a header longer than the pack has no
    // start offset, and is refused before a single byte is allocated.
    let start = id_at
        .checked_sub(header_len)
        .ok_or_else(|| AppError::Other("pack header runs past the start of the pack".into()))?;

    // `open_chunk` binds `id` as associated data and repeats the
    // `chunk_id(plaintext) == id` recheck, so a substituted trailer id, a
    // flipped bit, or a shifted header region all fail right here.
    let json: Zeroizing<Vec<u8>> = open_chunk(keys, &id, &pack[start..id_at])?;
    let header: PackHeader = serde_json::from_slice(&json)
        .map_err(|_| AppError::Other("pack header is not a readable header".into()))?;
    check_version(header.format, MAX_SUPPORTED_PACK_HEADER, "pack header")?;

    entries_within(&header.entries, start as u64)?;
    Ok(header)
}

/// Every entry must lie inside the blob region, which ends where the sealed
/// header begins.
///
/// Split out so it can be tested against a crafted entry list directly: a header
/// whose entries point past the pack cannot be produced through [`PackWriter`],
/// only forged, and a check nothing exercises is a check nobody can trust.
fn entries_within(entries: &[PackEntry], blob_region: u64) -> Result<()> {
    for entry in entries {
        let end = entry
            .offset
            .checked_add(u64::from(entry.clen))
            .ok_or_else(|| AppError::Other("pack entry length overflows".into()))?;
        if end > blob_region {
            return Err(AppError::Other(
                "pack entry points outside the pack's blob region".into(),
            ));
        }
    }
    Ok(())
}

/// The ciphertext slice one entry names, bounds-checked against `pack`.
pub fn blob_bytes<'a>(pack: &'a [u8], entry: &PackEntry) -> Result<&'a [u8]> {
    let offset = usize::try_from(entry.offset)
        .map_err(|_| AppError::Other("pack entry offset does not fit this platform".into()))?;
    let end = offset
        .checked_add(entry.clen as usize)
        .ok_or_else(|| AppError::Other("pack entry length overflows".into()))?;
    pack.get(offset..end)
        .ok_or_else(|| AppError::Other("pack entry points outside the pack".into()))
}

/// Slice one blob out of a pack and open it.
pub fn open_blob(keys: &Keys, pack: &[u8], entry: &PackEntry) -> Result<Zeroizing<Vec<u8>>> {
    open_chunk(keys, &entry.id, blob_bytes(pack, entry)?)
}

/// `packs/<first two hex chars>/<full 64 hex>.pack`.
///
/// Two-level fanout keeps any single listing far below GitHub's 3,000-entry
/// directory width, even if the store later moves back into a git tree. It costs
/// one line, so take it.
pub fn shard_path(id: &ChunkId) -> String {
    let hex = id.to_string();
    format!("packs/{}/{hex}.pack", &hex[..2])
}

/// Would appending a blob of `next_blob_len` carry a pack of `current_len` past
/// [`PACK_MAX`]?
///
/// A pure function rather than a method so phase 2's plan builder can size packs
/// without instantiating a writer.
pub fn should_seal(current_len: usize, next_blob_len: usize) -> bool {
    current_len.saturating_add(next_blob_len) > PACK_MAX
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::crypto::{KdfParams, Keyfile};

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

    /// Deterministic xorshift: incompressible bytes with no random source and no
    /// clock anywhere near the test.
    fn incompressible(len: usize) -> Vec<u8> {
        let mut state = 0x2545_f491_4f6c_dd1du64;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 24) as u8
            })
            .collect()
    }

    /// Three distinguishable payloads and the pack they seal into.
    fn three_blob_pack(keys: &Keys) -> (Vec<Vec<u8>>, ChunkId, Vec<u8>) {
        let payloads = vec![
            b"the first payload, long enough to be recognisable".to_vec(),
            incompressible(9_000),
            vec![0x5au8; 4_096],
        ];
        let mut writer = PackWriter::new();
        for p in &payloads {
            writer.push(seal_chunk(keys, p).unwrap());
        }
        let (name, pack) = writer.finish(keys).unwrap();
        (payloads, name, pack)
    }

    #[test]
    fn three_blobs_read_back_byte_exactly_from_their_recorded_offsets() {
        let keys = keys();
        let (payloads, _, pack) = three_blob_pack(&keys);

        let header = read_header(&keys, &pack).unwrap();
        assert_eq!(header.format, PACK_HEADER_VERSION);
        assert_eq!(header.entries.len(), 3);

        for (entry, payload) in header.entries.iter().zip(&payloads) {
            assert_eq!(entry.id, keys.chunk_id(payload));
            assert_eq!(entry.true_len as usize, payload.len());
            assert_eq!(blob_bytes(&pack, entry).unwrap().len(), entry.clen as usize);
            assert_eq!(
                &*open_blob(&keys, &pack, entry).unwrap(),
                payload.as_slice()
            );
        }

        // Offsets really are consecutive rather than incidentally correct.
        let mut expected = 0u64;
        for entry in &header.entries {
            assert_eq!(entry.offset, expected);
            expected += u64::from(entry.clen);
        }
    }

    #[test]
    fn the_trailer_locates_a_header_that_opens() {
        let keys = keys();
        let (_, _, pack) = three_blob_pack(&keys);

        // Exactly the walk `read_header` performs, spelled out: last four bytes,
        // then the 32 before them, then the region they jointly describe.
        let len_at = pack.len() - LEN_LEN;
        let header_len = u32::from_le_bytes(pack[len_at..].try_into().unwrap()) as usize;
        let id_at = len_at - ID_LEN;
        let id = ChunkId::from_bytes(pack[id_at..len_at].try_into().unwrap());

        assert!(header_len > 0 && header_len <= id_at);
        // The id in the clear is the *keyed* id of the header plaintext.
        let opened = open_chunk(&keys, &id, &pack[id_at - header_len..id_at]).unwrap();
        let header: PackHeader = serde_json::from_slice(&opened).unwrap();
        assert_eq!(header.entries.len(), 3);
    }

    #[test]
    fn no_blob_plaintext_appears_anywhere_in_the_finished_pack() {
        let keys = keys();
        let needle = b"NEEDLE-plaintext-that-must-not-survive-sealing".as_slice();
        let mut writer = PackWriter::new();
        writer.push(seal_chunk(&keys, needle).unwrap());
        writer.push(seal_chunk(&keys, &incompressible(2_048)).unwrap());
        let (_, pack) = writer.finish(&keys).unwrap();

        assert!(!pack.windows(needle.len()).any(|w| w == needle));
    }

    #[test]
    fn a_flipped_bit_in_the_header_ciphertext_fails_to_open() {
        let keys = keys();
        let (_, _, mut pack) = three_blob_pack(&keys);

        let header_start = pack.len()
            - TRAILER_LEN
            - u32::from_le_bytes(pack[pack.len() - LEN_LEN..].try_into().unwrap()) as usize;
        pack[header_start] ^= 0b0000_0001;

        assert!(read_header(&keys, &pack).is_err());
    }

    #[test]
    fn a_substituted_trailer_header_id_fails_to_open() {
        let keys = keys();
        let (_, _, mut pack) = three_blob_pack(&keys);

        // Not a random 32 bytes: a genuinely valid id of a genuinely sealed
        // object in this very pack. It still fails, because the id is bound as
        // associated data.
        let other = read_header(&keys, &pack).unwrap().entries[0].id;
        let id_at = pack.len() - TRAILER_LEN;
        pack[id_at..id_at + ID_LEN].copy_from_slice(other.as_bytes());

        assert!(read_header(&keys, &pack).is_err());
    }

    #[test]
    fn the_wrong_keys_yield_an_error_rather_than_entries() {
        let mine = keys();
        let (_, _, pack) = three_blob_pack(&mine);
        // A second, unrelated key hierarchy: the header does not even locate.
        assert!(read_header(&keys(), &pack).is_err());
    }

    #[test]
    fn an_empty_writer_refuses_to_finish() {
        let keys = keys();
        let writer = PackWriter::new();
        assert!(writer.is_empty());
        assert_eq!(writer.len_bytes(), 0);
        assert!(writer.finish(&keys).is_err());
    }

    #[test]
    fn a_header_version_is_accepted_at_or_below_the_ceiling_and_refused_above() {
        let keys = keys();
        let seal_at = |format: u32| {
            let mut writer = PackWriter::new();
            writer.push(seal_chunk(&keys, b"payload").unwrap());
            writer.finish_at_version(&keys, format).unwrap().1
        };

        // Below the ceiling: an older writer's pack must still open, which is
        // the reason the check is not an equality test.
        assert!(read_header(&keys, &seal_at(MAX_SUPPORTED_PACK_HEADER - 1)).is_ok());
        assert!(read_header(&keys, &seal_at(MAX_SUPPORTED_PACK_HEADER)).is_ok());

        let err = read_header(&keys, &seal_at(MAX_SUPPORTED_PACK_HEADER + 1))
            .expect_err("a header from the future must be refused");
        assert!(err.to_string().contains("upgrade ai-usagebar"));
    }

    #[test]
    fn shard_path_shards_on_the_first_two_hex_characters() {
        let mut raw = [0u8; 32];
        raw[0] = 0xab;
        let id = ChunkId::from_bytes(raw);
        assert_eq!(
            shard_path(&id),
            format!("packs/ab/ab{}.pack", "0".repeat(62))
        );
    }

    #[test]
    fn a_pack_name_is_a_function_of_the_pack_bytes() {
        let keys = keys();
        let (_, name, pack) = three_blob_pack(&keys);

        assert_eq!(content_address(&pack), name);

        let mut tampered = pack.clone();
        tampered[0] ^= 0b0000_0001;
        assert_ne!(content_address(&tampered), name);
    }

    /// A pack filled to `PACK_MAX` still fits `PACK_ASSET_MAX` once its header
    /// and trailer are on it.
    ///
    /// This is the invariant a real restore broke: a 50,391,460-byte asset was
    /// refused against `PACK_MAX` (50,331,648) and the bundle could not be read.
    /// The body ceiling was right, the asset ceiling did not exist, and the
    /// reader used the body one.
    #[test]
    fn a_pack_filled_to_the_body_ceiling_still_fits_the_asset_ceiling() {
        // A real pack, sealed and finished, measured rather than reasoned
        // about — the defect was that nobody had measured the *asset*.
        let keys = keys();
        let mut w = PackWriter::default();
        for i in 0..8u8 {
            w.push(seal_chunk(&keys, &[i; 4096]).unwrap());
        }
        let body: usize = 8 * seal_chunk(&keys, &[0u8; 4096]).unwrap().ciphertext.len();
        let (_, asset) = w.finish(&keys).unwrap();
        let overhead = asset.len() - body;
        assert!(overhead > 0, "an asset carries a header and a trailer");
        assert!(
            PACK_MAX + overhead <= PACK_ASSET_MAX,
            "the asset ceiling must admit a full pack's own overhead: \
             {PACK_MAX} + {overhead} > {PACK_ASSET_MAX}"
        );
    }

    #[test]
    fn should_seal_fires_exactly_at_the_ceiling() {
        assert!(!should_seal(0, PACK_MAX));
        assert!(!should_seal(PACK_MAX - 10, 10));
        assert!(should_seal(PACK_MAX - 10, 11));
        assert!(should_seal(PACK_MAX, 1));
        // No overflow panic on a nonsense length from a caller.
        assert!(should_seal(usize::MAX, usize::MAX));
    }

    #[test]
    fn a_writer_filled_until_should_seal_fires_stays_within_pack_max() {
        let keys = keys();
        // One sealed full chunk, cloned rather than resealed: the sizes are what
        // this test is about, and 191 zstd passes over 256 KiB are not.
        let blob = seal_chunk(&keys, &incompressible(crate::sync::CHUNK_SIZE)).unwrap();
        let (id, ciphertext, true_len) = (blob.id, blob.ciphertext, blob.true_len);

        let mut writer = PackWriter::new();
        while !should_seal(writer.len_bytes(), ciphertext.len()) {
            writer.push(Blob {
                id,
                ciphertext: ciphertext.clone(),
                true_len,
            });
        }
        assert!(writer.len_bytes() + ciphertext.len() > PACK_MAX);

        let (_, pack) = writer.finish(&keys).unwrap();
        assert!(pack.len() <= PACK_MAX, "pack grew to {}", pack.len());
        assert!(read_header(&keys, &pack).is_ok());
    }

    #[test]
    fn truncation_fails_at_read_header_with_no_entry_returned() {
        let keys = keys();
        let (_, _, pack) = three_blob_pack(&keys);

        assert!(read_header(&keys, &pack[..pack.len() - 1]).is_err());
        assert!(read_header(&keys, &pack[..pack.len() - 1_024]).is_err());
        // And a pack too short to even hold a trailer.
        assert!(read_header(&keys, &pack[..TRAILER_LEN - 1]).is_err());
    }

    #[test]
    fn a_header_length_larger_than_the_pack_is_refused_before_any_allocation() {
        let keys = keys();
        let (_, _, mut pack) = three_blob_pack(&keys);

        // T-03-05: the trailer is attacker-controlled, so it can claim a 4 GiB
        // header inside a few-kilobyte pack.
        let len_at = pack.len() - LEN_LEN;
        pack[len_at..].copy_from_slice(&u32::MAX.to_le_bytes());
        let err = read_header(&keys, &pack).expect_err("an impossible length must be refused");
        assert!(err.to_string().contains("runs past the start"));
    }

    #[test]
    fn an_entry_reaching_past_the_blob_region_is_refused() {
        // Directly, because such a header cannot be produced by `PackWriter` —
        // only forged by whoever serves the pack.
        let entry = |offset: u64, clen: u32| PackEntry {
            id: ChunkId::from_bytes([7u8; 32]),
            offset,
            clen,
            true_len: 1,
        };

        assert!(entries_within(&[entry(0, 100), entry(100, 900)], 1_000).is_ok());
        // One byte past the end of the blob region is one byte too far.
        assert!(entries_within(&[entry(0, 100), entry(100, 901)], 1_000).is_err());
        // And the addition itself cannot be made to wrap into a passing check.
        assert!(entries_within(&[entry(u64::MAX, 1)], 1_000).is_err());
    }

    #[test]
    fn an_entry_pointing_past_the_blob_region_is_refused_before_it_is_returned() {
        let keys = keys();
        let (_, _, pack) = three_blob_pack(&keys);
        let entry = &read_header(&keys, &pack).unwrap().entries[0];

        // `read_header` rejects such an entry inside a header; `blob_bytes` is
        // the second line of defence for an entry handed in from elsewhere.
        let out_of_range = PackEntry {
            id: entry.id,
            offset: pack.len() as u64,
            clen: 64,
            true_len: entry.true_len,
        };
        assert!(blob_bytes(&pack, &out_of_range).is_err());
        assert!(open_blob(&keys, &pack, &out_of_range).is_err());

        let overflowing = PackEntry {
            offset: u64::MAX,
            ..out_of_range
        };
        assert!(blob_bytes(&pack, &overflowing).is_err());
    }
}
