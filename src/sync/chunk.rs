//! Fixed-size chunking, zstd compression, and the length-prefixed frame that
//! carries a chunk's true length so tails can be padded without ambiguity.
//!
//! # The chunk id addresses the raw plaintext
//!
//! `id = keys.chunk_id(plaintext)`, computed **before** framing or compression
//! ever touches the bytes. This is load-bearing, and it is exactly the kind of
//! thing a later "optimisation" would happily undo by hashing the sealed frame
//! instead — so: hashing the frame would tie every chunk id in every user's
//! bundle to the zstd version. A routine `zstd` crate bump, or a change to its
//! default window size or level tuning, would then re-id every chunk, force a
//! full re-upload of the entire payload, and drop dedup to zero across the
//! upgrade boundary.
//!
//! Hashing the plaintext means two machines on different zstd versions may
//! produce *different ciphertext* for one id. State that precisely, because the
//! sloppy version of it was a real vulnerability: it is harmless **for dedup**
//! — both decrypt to identical plaintext and the first upload simply wins — and
//! it is *not* harmless for nonce safety. One id covering two distinct messages
//! is exactly the input that would reuse a nonce, so
//! `crypto::Keys::seal` derives the nonce
//! from the framed bytes it encrypts rather than from the id, and stores it
//! inline. See its safety contract before changing either address.
//!
//! # Frame layout
//!
//! The bytes handed to `crypto::Keys::seal`,
//! which prepends the 24-byte nonce it derives from them:
//!
//! | Offset | Size       | Field                                            |
//! |--------|------------|--------------------------------------------------|
//! | 0      | 4          | `true_len`, u32 little-endian — uncompressed size |
//! | 4      | 4          | `comp_len`, u32 little-endian — zstd frame size   |
//! | 8      | `comp_len` | the zstd level-3 frame                            |
//! | …      | rest       | zero padding                                      |
//!
//! Total length is `(8 + comp_len)` rounded up to the next power of two, capped
//! at [`CHUNK_SIZE`] — and never below `8 + comp_len` itself, so an
//! incompressible chunk that zstd grows past `CHUNK_SIZE` is simply left
//! unpadded rather than truncated.
//!
//! Three properties make that the right layout:
//!
//! - **Explicit `comp_len`** makes the padding unambiguous. Decoding never has
//!   to guess where the zstd frame ends and the zeros begin.
//! - **Padding after compression** is what actually hides the tail length.
//!   Padding first would let zstd collapse the zeros and hand the exact
//!   plaintext size straight back out through the ciphertext length.
//! - **Every step is deterministic within a build**, so the same input yields
//!   the same frame and therefore the same ciphertext — the precondition for
//!   dedup.
//!
//! Also home to [`open_chunk`], which performs the `chunk_id(plaintext) == id`
//! identity recheck *after* unframing — the recheck cannot live in
//! [`crate::sync::crypto::Keys::open`], because the id addresses the raw
//! plaintext while that function returns the framed-and-compressed form.
//!
//! Owned by plan 1-02.

use zeroize::Zeroizing;

use crate::error::{AppError, Result};
use crate::sync::CHUNK_SIZE;
use crate::sync::crypto::{ChunkId, Keys};

/// zstd's own default level. Level 3 is the ratio/speed knee; higher levels buy
/// a few percent for several times the CPU, on data that is mostly JSONL.
const ZSTD_LEVEL: i32 = 3;

/// `true_len` + `comp_len`, both u32 little-endian.
const HEADER_LEN: usize = 8;

/// Compress `data` and wrap it in the frame documented at the module level.
///
/// Returns [`Zeroizing`] because a compressed frame is still a user's
/// credential file, only harder to read.
pub fn frame(data: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    if data.len() > CHUNK_SIZE {
        return Err(AppError::Other(format!(
            "cannot frame {} bytes: a chunk is at most {CHUNK_SIZE} bytes",
            data.len()
        )));
    }

    let compressed = Zeroizing::new(
        zstd::stream::encode_all(data, ZSTD_LEVEL)
            .map_err(|_| AppError::Other("chunk compression failed".into()))?,
    );

    let body = HEADER_LEN + compressed.len();
    // Round the *sealed* size up to a power of two so a tail's ciphertext length
    // reveals only its bucket, not its exact length (T-02-02). `.max(body)` is
    // not belt-and-braces: incompressible data can leave `body` above
    // CHUNK_SIZE, and capping alone would silently truncate the frame.
    let target = body.next_power_of_two().min(CHUNK_SIZE).max(body);

    let mut out = Zeroizing::new(vec![0u8; target]);
    out[0..4].copy_from_slice(&(data.len() as u32).to_le_bytes());
    out[4..HEADER_LEN].copy_from_slice(&(compressed.len() as u32).to_le_bytes());
    out[HEADER_LEN..body].copy_from_slice(&compressed);
    Ok(out)
}

/// Undo [`frame`].
///
/// Both length fields are attacker-influenced right up until the AEAD tag has
/// verified, and are range-checked here anyway (T-02-01): a crafted frame must
/// produce an error, never a panic and never an unbounded allocation.
pub fn unframe(frame: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    if frame.len() < HEADER_LEN {
        return Err(AppError::Other("chunk frame is truncated".into()));
    }
    let true_len = u32::from_le_bytes(frame[0..4].try_into().expect("4 bytes")) as usize;
    let comp_len = u32::from_le_bytes(frame[4..HEADER_LEN].try_into().expect("4 bytes")) as usize;

    // Bounds-check *before* slicing or allocating. `checked_add` rather than
    // `+` because `comp_len` is a full u32 and `usize` is 32 bits on some
    // targets this crate builds for.
    let end = HEADER_LEN
        .checked_add(comp_len)
        .ok_or_else(|| AppError::Other("chunk frame length overflows".into()))?;
    if end > frame.len() {
        return Err(AppError::Other(
            "chunk frame declares more compressed bytes than it carries".into(),
        ));
    }
    if true_len > CHUNK_SIZE {
        return Err(AppError::Other(
            "chunk frame declares more plaintext than a chunk can hold".into(),
        ));
    }

    // `bulk::decompress` allocates exactly `true_len` and fails if the frame
    // expands past it, so a decompression bomb cannot be built out of a valid
    // header — the bound is checked before the allocation, not after.
    let plaintext = Zeroizing::new(
        zstd::bulk::decompress(&frame[HEADER_LEN..end], true_len)
            .map_err(|_| AppError::Other("chunk decompression failed".into()))?,
    );
    if plaintext.len() != true_len {
        return Err(AppError::Other(
            "chunk frame's declared length does not match its contents".into(),
        ));
    }
    Ok(plaintext)
}

/// A sealed chunk: its address, its ciphertext, and the plaintext length the
/// frame declares.
///
/// `Debug` is derived, and may stay derived: an id is an address rather than a
/// secret, and `ciphertext` is by definition safe to print.
#[derive(Debug)]
pub struct Blob {
    pub id: ChunkId,
    pub ciphertext: Vec<u8>,
    pub true_len: u32,
}

/// Address, frame, and seal one chunk.
pub fn seal_chunk(keys: &Keys, data: &[u8]) -> Result<Blob> {
    let id = keys.chunk_id(data);
    let framed = frame(data)?;
    Ok(Blob {
        id,
        ciphertext: keys.seal(&id, &framed)?,
        true_len: data.len() as u32,
    })
}

/// Open, unframe, and **then** recheck that the plaintext really is what `id`
/// addresses.
///
/// [`Keys::open`](crate::sync::crypto::Keys::open) deliberately does not do
/// this: it sees only the framed form, while the id addresses the plaintext, so
/// this is the layer that owns the check. It is belt and braces on top of the
/// AEAD tag — it catches our own framing bugs just as readily as an adversary.
pub fn open_chunk(keys: &Keys, id: &ChunkId, ciphertext: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    let framed = keys.open(id, ciphertext)?;
    let plaintext = unframe(&framed)?;
    if keys.chunk_id(&plaintext) != *id {
        return Err(AppError::Other(
            "chunk contents do not match the id they were served under".into(),
        ));
    }
    Ok(plaintext)
}

/// Split into [`CHUNK_SIZE`] slices from offset zero, plus a shorter tail when
/// the length is not a multiple.
///
/// Offsets are aligned to the start of *each file*, never across a
/// concatenation of files: a change to one small file must not re-chunk
/// anything else.
pub fn split(data: &[u8]) -> impl Iterator<Item = &[u8]> {
    data.chunks(CHUNK_SIZE)
}

/// Seal every chunk of `data`, in order.
pub fn seal_all(keys: &Keys, data: &[u8]) -> Result<Vec<Blob>> {
    split(data).map(|slice| seal_chunk(keys, slice)).collect()
}

/// Open every chunk in the given order and concatenate.
///
/// Any failure aborts the whole reassembly and returns **no** partial buffer —
/// a half-decrypted credential file is worse than none (CRYPTO-03). The `?`
/// below drops the accumulator, and `Zeroizing` wipes whatever it had already
/// collected on the way out.
///
/// Ordering itself is the caller's to get right: a chunk carries no position,
/// so a manifest that lists chunks in the wrong order reassembles a scrambled
/// buffer without any local integrity failure. What this function does catch is
/// any chunk that is not the one its id names.
pub fn reassemble(keys: &Keys, chunks: &[(ChunkId, Vec<u8>)]) -> Result<Zeroizing<Vec<u8>>> {
    let mut out = Zeroizing::new(Vec::new());
    for (id, ciphertext) in chunks {
        out.extend_from_slice(&open_chunk(keys, id, ciphertext)?);
    }
    Ok(out)
}

/// How many full [`CHUNK_SIZE`] chunks a buffer of `len` bytes contains.
///
/// Phase 2's append fast path re-hashes exactly the last sealed chunk, and needs
/// this count without holding the data.
pub fn sealed_chunk_count(len: u64) -> u64 {
    len / CHUNK_SIZE as u64
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

    /// Poly1305's tag, appended by every seal.
    const TAG_LEN: usize = 16;

    /// The XChaCha20 nonce every seal stores inline, ahead of the ciphertext.
    const NONCE_LEN: usize = 24;

    fn keys() -> Keys {
        Keyfile::create_with_floor(b"correct horse battery staple", CHEAP, CHEAP.m_kib)
            .expect("keyfile creation")
            .1
    }

    /// Deterministic xorshift. Incompressible enough to compare sizes against a
    /// run of zeros, with no random source and no clock in sight.
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

    fn round_trip(data: &[u8]) {
        let keys = keys();
        assert_eq!(&*unframe(&frame(data).unwrap()).unwrap(), data);

        let blob = seal_chunk(&keys, data).unwrap();
        assert_eq!(blob.true_len as usize, data.len());
        assert_eq!(
            &*open_chunk(&keys, &blob.id, &blob.ciphertext).unwrap(),
            data
        );
    }

    #[test]
    fn a_hundred_bytes_round_trip_byte_exactly() {
        round_trip(&incompressible(100));
    }

    #[test]
    fn a_full_chunk_round_trips_byte_exactly() {
        round_trip(&incompressible(CHUNK_SIZE));
    }

    #[test]
    fn an_empty_chunk_round_trips_to_an_empty_chunk() {
        round_trip(b"");
    }

    #[test]
    fn the_id_is_a_function_of_the_raw_plaintext_and_of_nothing_downstream() {
        let keys = keys();
        let data = incompressible(9_000);

        let blob = seal_chunk(&keys, &data).unwrap();
        assert_eq!(blob.id, keys.chunk_id(&data));

        // …and specifically *not* of the compressed frame. If this ever flips,
        // a zstd bump re-ids every chunk in every user's bundle.
        let framed = frame(&data).unwrap();
        assert_ne!(blob.id, keys.chunk_id(&framed));
    }

    #[test]
    fn compressible_input_seals_smaller_than_incompressible_input_of_one_length() {
        let keys = keys();
        let zeros = seal_chunk(&keys, &vec![0u8; CHUNK_SIZE]).unwrap();
        let noise = seal_chunk(&keys, &incompressible(CHUNK_SIZE)).unwrap();
        assert!(
            zeros.ciphertext.len() < noise.ciphertext.len(),
            "compression must reach the sealed size: {} vs {}",
            zeros.ciphertext.len(),
            noise.ciphertext.len()
        );
    }

    #[test]
    fn a_tail_seals_to_a_power_of_two_and_not_to_its_own_length() {
        let keys = keys();
        // Two tails 100 bytes apart, both landing in the 8 KiB bucket.
        let short = seal_chunk(&keys, &incompressible(5_000)).unwrap();
        let long = seal_chunk(&keys, &incompressible(5_100)).unwrap();

        assert_eq!(
            short.ciphertext.len(),
            long.ciphertext.len(),
            "the sealed size must not track the exact tail length"
        );
        assert!((short.ciphertext.len() - NONCE_LEN - TAG_LEN).is_power_of_two());
    }

    #[test]
    fn two_seals_of_one_input_are_byte_identical() {
        let keys = keys();
        let data = incompressible(70_000);
        let first = seal_chunk(&keys, &data).unwrap();
        let second = seal_chunk(&keys, &data).unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(first.ciphertext, second.ciphertext);
    }

    #[test]
    fn a_plaintext_that_does_not_hash_to_the_supplied_id_is_refused() {
        let keys = keys();
        let (mine, theirs) = (b"mine".as_slice(), b"theirs".as_slice());

        // Seal *my* frame under *their* id: the AAD matches the id it is opened
        // under, so the tag verifies and only the identity recheck can catch it.
        let id = keys.chunk_id(theirs);
        let ciphertext = keys.seal(&id, &frame(mine).unwrap()).unwrap();
        assert!(
            keys.open(&id, &ciphertext).is_ok(),
            "the AEAD tag must verify"
        );

        let err = open_chunk(&keys, &id, &ciphertext)
            .expect_err("a mismatched plaintext must be refused")
            .to_string();
        assert!(err.contains("do not match the id"));
        assert!(!err.contains("mine"));
    }

    #[test]
    fn a_frame_declaring_more_compressed_bytes_than_it_carries_errors() {
        let mut bad = vec![0u8; HEADER_LEN + 4];
        bad[0..4].copy_from_slice(&10u32.to_le_bytes());
        bad[4..HEADER_LEN].copy_from_slice(&9_999u32.to_le_bytes());
        assert!(unframe(&bad).is_err());

        // …and a frame too short to even hold the header.
        assert!(unframe(&[0u8; 3]).is_err());

        // …and one that claims more plaintext than a chunk can hold.
        let mut huge = vec![0u8; HEADER_LEN];
        huge[0..4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(unframe(&huge).is_err());
    }

    #[test]
    fn a_full_multiple_splits_without_a_tail_and_one_more_byte_adds_one() {
        let mib = incompressible(4 * CHUNK_SIZE);
        let exact: Vec<_> = split(&mib).collect();
        assert_eq!(exact.len(), 4);
        assert!(exact.iter().all(|c| c.len() == CHUNK_SIZE));

        let mut plus_one = mib.clone();
        plus_one.push(0xab);
        let tailed: Vec<_> = split(&plus_one).collect();
        assert_eq!(tailed.len(), 5);
        assert_eq!(tailed[4], &[0xab]);
    }

    #[test]
    fn sealed_chunk_count_counts_only_whole_chunks() {
        assert_eq!(sealed_chunk_count(0), 0);
        assert_eq!(sealed_chunk_count(CHUNK_SIZE as u64 - 1), 0);
        assert_eq!(sealed_chunk_count(CHUNK_SIZE as u64), 1);
        assert_eq!(sealed_chunk_count(700 * 1024), 2);
    }

    /// The shape `reassemble` consumes.
    fn entries(blobs: Vec<Blob>) -> Vec<(ChunkId, Vec<u8>)> {
        blobs.into_iter().map(|b| (b.id, b.ciphertext)).collect()
    }

    #[test]
    fn a_seven_hundred_kib_fixture_reassembles_byte_exactly() {
        let keys = keys();
        let data = incompressible(700 * 1024);
        let blobs = seal_all(&keys, &data).unwrap();
        assert_eq!(blobs.len(), 3, "two full chunks and a tail");
        assert_eq!(&*reassemble(&keys, &entries(blobs)).unwrap(), &data[..]);
    }

    #[test]
    fn appending_leaves_every_previously_sealed_chunk_id_unchanged() {
        // The whole no-CDC decision rests on this: compare the actual id lists,
        // not merely the counts.
        let keys = keys();
        let before = incompressible(700 * 1024);
        let mut after = before.clone();
        after.extend_from_slice(&incompressible(200 * 1024));

        let ids = |data: &[u8]| -> Vec<ChunkId> {
            seal_all(&keys, data)
                .unwrap()
                .into_iter()
                .map(|b| b.id)
                .collect()
        };
        let (old, new) = (ids(&before), ids(&after));

        assert_eq!(old.len(), 3);
        assert_eq!(new.len(), 4);
        // The two fully sealed chunks are untouched…
        assert_eq!(old[..2], new[..2]);
        // …and only the old tail, now grown into a full chunk, changed.
        assert_ne!(old[2], new[2]);
    }

    #[test]
    fn a_swapped_ciphertext_aborts_reassembly_with_no_bytes_returned() {
        let keys = keys();
        let data = incompressible(700 * 1024);
        let mut chunks = entries(seal_all(&keys, &data).unwrap());
        chunks[0].1 = chunks[1].1.clone();

        // `Result` carries no buffer, so "no partial output" is structural.
        assert!(reassemble(&keys, &chunks).is_err());
    }

    #[test]
    fn transposed_ciphertexts_abort_reassembly_rather_than_reordering_it() {
        let keys = keys();
        let data = incompressible(700 * 1024);
        let mut chunks = entries(seal_all(&keys, &data).unwrap());
        let (head, tail) = chunks.split_at_mut(1);
        std::mem::swap(&mut head[0].1, &mut tail[0].1);

        // Asserting on the *error* matters: a merely different output would also
        // happen in a format with no integrity at all, so it proves nothing.
        let err = reassemble(&keys, &chunks)
            .expect_err("transposed ciphertexts must be refused")
            .to_string();
        assert!(err.contains("failed authentication") || err.contains("do not match the id"));
    }
}
