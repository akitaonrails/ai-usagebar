# Encrypted sync bundle format

The on-disk format ai-usagebar uses to push local state to a git remote it
treats as fully hostile. Everything is encrypted before it leaves the machine;
the remote holds ciphertext, sizes, and timings, and nothing else.

This document is the format, not a tour of the code. Someone holding only this
page should be able to write a reader. Where a number or a context string is
load-bearing it is spelled out exactly — a re-implementation that gets one of
them wrong produces a bundle that authenticates against nothing.

The implementation lives in `src/sync/`, one module per concern, and
`src/sync/crypto.rs` is the only file in it that imports a cryptographic crate.

If you are here to *use* the feature rather than re-implement it, read
[Encrypted sync](../README.md#encrypted-sync) in the README first — setup, the
daily commands, the surfaces, and the honest limits in the form a user needs
them. `sync-github.md` is the setup guide, and the measured sizes and timings
are in [`sync-calibration.md`](sync-calibration.md).

Primitives: **Argon2id** (RFC 9106), **XChaCha20-Poly1305**, **BLAKE3**
(`keyed_hash` and `derive_key`), **zstd** level 3.

---

## 1. Key hierarchy

```text
    password + salt (16 random bytes, stored in the keyfile)
        |  Argon2id  m = 1 GiB  t = 3  p = 1  ->  32 bytes
    KEK [32B]                                   ephemeral, zeroized after use
        |  XChaCha20-Poly1305 unwrap
        |  aad = canonical JSON of {format, kdf}
    master key [32B]                            random at init, never on disk
        |  BLAKE3 derive_key, one context string each
    chunk_key            name_key            root_key
```

The master key is 32 bytes straight from the OS CSPRNG, drawn once when the
bundle is initialised. It is never written down in the clear: the keyfile holds
only its wrapped form. The password never touches anything but Argon2id.

**Argon2id parameters.** Algorithm `Argon2id`, version `0x13` (19), memory
`m_kib = 1048576` (1 GiB), time `t = 3`, lanes `p = 1`, 32 bytes of output.

`p = 1` is deliberate and is not a portability compromise: the `argon2` 0.5.3
crate has no threading, so raising `p` does not make the defender's derivation
any faster — it measured about 10% *worse* — while it hands an attacker with
wide SIMD free intra-hash parallelism. One lane is the honest setting for a
single-threaded implementation.

**The three subkeys**, each `blake3::derive_key(context, master_key)`:

| Subkey | Context string | Used for |
|---|---|---|
| `chunk_key` | `ai-usagebar.sync.v1 chunk-encryption-key` | sealing and opening every chunk |
| `name_key` | `ai-usagebar.sync.v1 chunk-name-key` | the keyed hash that addresses a chunk |
| `root_key` | `ai-usagebar.sync.v1 snapshot-root-key` | sealing and opening the snapshot root |

A fourth context string, `ai-usagebar.sync.v1 chunk-nonce`, is **not** a subkey:
it is applied to `keyed_hash(name_key, <the bytes being sealed>)` to derive that
chunk's nonce (§3), and is listed here because a reader needs it. The `v1` token
in all four is load-bearing — a future v2 hierarchy must not be able to collide
with v1's.

**The KDF parameters are bound as associated data**, which is what makes a
downgrade fail rather than succeed weakly. The AAD for the master-key wrap is
the JSON serialization of a two-field struct, in declaration order:

```json
{"format":1,"kdf":{"algo":"argon2id","version":19,"m_kib":1048576,"t":3,"p":1,"salt":"<base64>"}}
```

An attacker who rewrites `m_kib` to 8 in transit gets a victim who derives a
cheap KEK — and that KEK is not the one the master key was wrapped under,
because the AAD no longer matches. The unwrap fails. It fails with the same
message a wrong password gets (`wrong password or corrupted keyfile`), because
there is nothing useful to distinguish and nothing an attacker should learn from
the difference.

A reader must use the parameters **stored in the keyfile it is opening**, never
its own compiled default. A bundle initialised at a lower `--kdf-memory` stays
openable; one initialised higher stays strong.

**`m_kib` is bounded on both sides, and the two bounds are not symmetrical.**

- **Writing** a new keyfile — at initialisation or at a password change — is
  refused below **8 MiB**. Argon2's own floor is `8 * p` KiB, which is the
  smallest input the algorithm is defined for rather than a security parameter,
  and the 12-character password rule in §9 is arithmetic against the guess rate
  the *shipped* parameters buy. The two are also coupled directly: below the
  default memory cost a user-supplied password must be at least **20
  characters** rather than 12 — the length a generated passphrase has. Lowering
  the cost therefore trades against password length instead of against security.
  It is a length rule and not an entropy one, and §9 says what that is worth: 20
  characters from the generator are 100 uniform bits, 20 characters somebody
  chose may be worth half that, and an implementation holding only the string
  cannot tell which it has.
- **Reading** is refused above **4 GiB**, and is deliberately unbounded below.
  `m_kib` reaches a reader from a keyfile a hostile remote may have edited, and
  an implementation whose Argon2 allocates infallibly turns one edited integer
  into an abort — before the AAD binding gets a chance to reject it. Refuse the
  value before allocating for it. Nothing is refused for being *too low* on
  read: a bundle written before a floor existed must stay openable, or raising
  a floor destroys data.

---

## 2. The keyfile

JSON. Holds no plaintext key material — the wrapped key is ciphertext and the
salt is public — so it is safe to store beside the bundle.

```json
{
  "format": 1,
  "kdf": {
    "algo": "argon2id",
    "version": 19,
    "m_kib": 1048576,
    "t": 3,
    "p": 1,
    "salt": "PhRZ9m0k4iCWr7YyE1x2Aw=="
  },
  "nonce": "0zP9m…24 bytes…",
  "wrapped_master_key": "Qk1…48 bytes…"
}
```

| Field | Type | Encoding |
|---|---|---|
| `format` | u32 | keyfile format version — 1 |
| `kdf.algo` | string | `"argon2id"` |
| `kdf.version` | u32 | Argon2 version, `19` (0x13) |
| `kdf.m_kib` | u32 | memory cost in KiB |
| `kdf.t` | u32 | time cost (passes) |
| `kdf.p` | u32 | lanes |
| `kdf.salt` | string | base64 (standard alphabet, padded) of 16 random bytes |
| `nonce` | string | base64 of the 24-byte XChaCha20 wrap nonce |
| `wrapped_master_key` | string | base64 of 48 bytes — 32 ciphertext + 16 Poly1305 tag |

Field order in the `kdf` object is the canonical AAD byte order. It is
serialized as a struct rather than a map for exactly that reason: an AAD whose
bytes depend on hash iteration order is an AAD that intermittently fails to
authenticate.

The version gate runs *before* any cryptographic work. Refusing a bundle from a
newer client must not cost a gibibyte and a second and a half first.

---

## 3. Chunking

Each file is split at fixed **256 KiB** boundaries from its own offset zero,
plus a shorter explicit tail when the length is not a multiple. Offsets are
aligned to the start of *each file*, never across a concatenation of files — a
change to one small file must not re-chunk anything else.

Fixed-size, not content-defined. CDC boundary positions are visible as
ciphertext lengths and fingerprint the plaintext (arXiv:2504.02095), and the
payload here is append-only JSONL and page-aligned SQLite, which fixed blocks
dedup just as well. The chunker is identified in the format as `fixed-256k`.

### The chunk id addresses the raw plaintext

```text
id = blake3::keyed_hash(name_key, plaintext)      // 32 bytes, hex-encoded when serialized
```

Computed **before** framing or compression ever touches the bytes. Two
properties, both of which a later "optimisation" would happily undo:

- **It hashes the plaintext, not the compressed frame.** Hashing the frame would
  tie every chunk id in every user's bundle to the zstd version. A routine crate
  bump, or a change to its default window size or level tuning, would re-id
  every chunk, force a full re-upload, and drop dedup to zero across the upgrade
  boundary.
- **It is keyed.** An unkeyed content hash lets anyone holding the repository
  hash a guessed plaintext and check whether that chunk is present — a
  confirmation-of-file oracle. Restic and Borg made opposite choices here; this
  format takes Borg's, because keying costs nothing.

The consequence of hashing plaintext is that two machines running different
zstd versions may produce *different ciphertext* for the same id. State that
precisely, because the sloppy version of it was a real vulnerability:

- **Harmless for dedup.** Both decrypt to identical plaintext, and the first
  upload wins.
- **Not harmless for nonce safety.** One id covering two distinct messages is
  exactly the input that reuses an AEAD nonce. That is why the nonce below is
  derived from the framed bytes actually encrypted rather than from the id, and
  is stored inline: two framings of one plaintext get two nonces, so the reuse
  cannot arise.

### Frame layout

The bytes handed to the AEAD:

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | `true_len`, u32 little-endian — the uncompressed size |
| 4 | 4 | `comp_len`, u32 little-endian — the zstd frame size |
| 8 | `comp_len` | the zstd level-3 frame |
| … | rest | zero padding |

Total length is `(8 + comp_len)` rounded up to the next power of two, capped at
256 KiB, and never below `8 + comp_len` itself — so an incompressible chunk that
zstd grows past the cap is left unpadded rather than truncated.

- **Explicit `comp_len`** makes the padding unambiguous; decoding never guesses
  where the zstd frame ends and the zeros begin.
- **Padding after compression** is what actually hides a tail's length. Padding
  first would let zstd collapse the zeros and hand the exact plaintext size
  straight back out through the ciphertext length.

### Sealing

```text
nonce  = blake3::derive_key("ai-usagebar.sync.v1 chunk-nonce",
                            blake3::keyed_hash(name_key, frame))[..24]
sealed = nonce ‖ XChaCha20-Poly1305(key = chunk_key, nonce, aad = id, msg = frame)
```

The 24-byte nonce is stored inline as the first bytes of the sealed blob, the
same framing the snapshot root uses (§5). A reader cannot re-derive it — it is a
keyed hash of the plaintext it is about to recover — so it has to travel.

**The nonce is derived from the bytes actually encrypted, never from the id.**
This is the property that keeps the AEAD safe, and it is not the same property
as determinism. A derived nonce is only sound under **nonce ↔ message**
injectivity. The id addresses the *raw plaintext*, while the message sealed is
that plaintext's compressed frame, and zstd guarantees format compatibility
across versions rather than byte-identical output. Deriving the nonce from the
id therefore let two builds seal two *distinct* messages under one
`(chunk_key, nonce, aad)` — a solvable Poly1305 one-time key, hence forgery, and
`C_A ⊕ C_B = F_A ⊕ F_B` with an identical 4-byte `true_len` prefix handing over
free known keystream. Hashing the frame instead closes it: a message that
differs by one byte gets a different nonce whether or not its id moved. A nonce
is never reused across distinct messages under `chunk_key`.

Still deterministic within a build: the same plaintext frames to the same bytes,
which derive the same nonce, which yield byte-identical output. That is what
makes dedup work and what stops a re-sync of unchanged data from creating new
remote objects forever.

**Known gap — the AAD carries no object type.** Data chunks, manifest chunks,
index chunks and pack headers are all sealed under `chunk_key` with `aad = id`
and nothing saying which kind of object they are, so the AEAD layer cannot tell
one kind served in another's place from the genuine article. It dead-ends
rather than opening anything: the id is bound, the `chunk_id(plaintext) == id`
recheck still runs, and the entry bounds checks of §4 plus the per-blob tags
refuse a confused header — the outcome is an error, not a recovery. It is
recorded here because a re-implementer should know it is a gap and not a
decision: the fix is a type byte in the AAD, which changes every sealed byte in
the format and so is a versioned format change rather than an edit. **An
implementation adding a new kind of object under `chunk_key` should introduce
the domain separator at the same time.**

Binding the id as associated data means a chunk served under the wrong name
fails its tag.

### Reading

Open, unframe, then **recheck** that `keyed_hash(name_key, plaintext) == id`.
The recheck cannot live in the AEAD layer: the id addresses the raw plaintext
while the decryption returns its framed-and-compressed form. It is belt and
braces on top of the tag, and it catches our own framing bugs as readily as an
adversary.

Both length fields are attacker-influenced right up until the tag verifies, and
are range-checked anyway. A crafted frame must produce an error, never a panic
and never an unbounded allocation: decompression allocates exactly `true_len`
and fails if the frame expands past it, so no decompression bomb can be built
out of a header that passes the bounds check.

**Ordering is not a chunk-layer property.** A chunk carries no position, so
transposing two whole `(id, ciphertext)` pairs cannot be detected here: each
pair still decrypts cleanly and still hashes to its own id. Order lives in the
manifest, and that is where it is defended — see §5.

---

## 4. Pack files

The unit of transfer. Many sealed blobs concatenated into one remote object,
followed by a sealed header describing them.

```text
<blob0 ciphertext><blob1 ciphertext>…<blobN ciphertext>
<sealed header><32-byte header id><u32 LE header length>
```

Packing is not an optimisation. GitHub caps content creation at 80 requests per
minute and 500 per hour, so one request per chunk is structurally impossible at
a few thousand chunks; a handful of 32 MiB objects is not.

**The header** is JSON, sealed through the ordinary chunk path — framed,
compressed, and sealed under its own keyed chunk id:

```json
{"format":1,"entries":[{"id":"<64 hex>","offset":0,"clen":4112,"true_len":4096}]}
```

`offset` is a byte offset from the start of the pack, `clen` the ciphertext
length to slice out, `true_len` the plaintext length the blob's frame declares.
There is no path, no filename, and no directory structure anywhere in a pack
header — local paths leak account UUIDs and session ids, and they live in the
sealed manifest instead.

**The header id is keyed, and it lives in the trailer.** Both halves matter:

- *Keyed.* A pack header is a list of chunk ids anyone holding the repository
  can already see, plus offsets and lengths he can measure against the file he
  is looking at. It is the single most guessable object in the format. Sealing
  it under an unkeyed content address would let him hash his guess and compare —
  exactly the oracle keyed chunk ids exist to deny. **Nothing in this format is
  ever sealed under an unkeyed address.**
- *In the trailer, in the clear.* A reader needs that id *before* it can
  decrypt, because the id is bound as associated data. Writing it down costs
  nothing — it is a keyed hash, so an attacker without
  `name_key` cannot recompute it from the header he is staring at, and
  substituting any other id simply breaks the tag. Without it the reader is not
  merely slower, it is impossible to write.

**Reading a pack**: take the last 4 bytes as the header length, the 32 before
them as the header id, and the `header_length` bytes before *those* as the
sealed header. Every number in that walk arrives from a hostile remote, so the
declared header length must be checked against the pack's own length before a
byte is allocated, and every entry's `offset + clen` must land inside the blob
region — which ends where the sealed header begins. A header that fails any
check returns no entries at all.

**The pack's name** is `packs/<first two hex chars>/<full 64 hex>.pack`, where
the hex is the **unkeyed** BLAKE3 hash of the finished pack bytes. This is the
one use of an unkeyed address in the format, and it is naming only, over bytes
that are already public ciphertext: it means a pack substituted on the remote
cannot keep the name it is served under. Two-level fanout keeps any single
listing far below GitHub's 3,000-entry directory width.

**Sizes**: `pack::should_seal` compares against **`PACK_MAX` = 48 MiB** and
never reads `PACK_TARGET`. So packs fill to 48 MiB, and **`PACK_TARGET` = 32 MiB
is advisory** — it names the size CAL-1's *unmeasured* fallback aims at (see §7)
and governs nothing in the sealing decision. A restore fetches a whole pack
either way, so this bounds wasted bytes, not correctness.

The distinction is not pedantry. Somebody will one day raise a pack size
constant, and the ceiling below is a function of **`PACK_MAX`**, not of the
target; a document naming the wrong one sends them to the wrong guard.

A pack header is itself a single sealed chunk, so it is bounded at 256 KiB of
JSON, some thousands of entries. A 48 MiB pack of 256 KiB chunks holds about
192, so this is slack rather than a limit; unlike the manifest and the index, it
has never been near its ceiling. **Raising `PACK_MAX` raises the entry count with
it and this ceiling must be re-checked at the same time** — it is the one place
the gap-closure that made manifests and index objects multi-chunk deliberately
did not reach. The upgrade path, if it is ever needed, is a format-2 multi-chunk
header through the same `chunk::seal_all` / `reassemble` pair the manifest uses.

**Packs are immutable once sealed.** A chunk already inside a pack is never
re-packed; only an explicit prune-repack rewrites one. That immutability is what
makes a crashed sync leave *orphan packs* — garbage to be collected later, never
corruption.

---

## 5. The object graph

```text
root  --manifest_chunks-->  manifest  --FileEntry.chunks-->  chunks
                                                              ^
                             index object  --IndexEntry-------+
                                            (chunk id -> pack, offset, clen)
```

Every hop's identifier is bound as associated data into the object it names, and
rechecked against the plaintext after opening. Substituting a manifest or a
chunk therefore fails its tag rather than quietly restoring something else.

### Root — the one mutable object

Sealed under `root_key` with a **fresh random 24-byte nonce**, stored inline as
the first 24 bytes of the framed output. The framing is `nonce ‖ ciphertext ‖
tag`, and the associated data is a fixed literal scoping the format,
concatenated with the bundle's identifier:

```text
aad = "ai-usagebar.sync.v1 root" ‖ repo_id
```

**The `repo_id` in that AAD is the reader's own, read from local configuration —
never the one the served root claims.** The literal alone would be the same
constant in every bundle in the world, so any two bundles sharing a master key —
a copied keyfile, a second remote added for the same machine — would open each
other's roots, and a cross-bundle replay would rest entirely on local anchor
state to notice. Binding the expected `repo_id` makes a repository swap fail the
Poly1305 tag instead. `repo_id` is not length-prefixed because it is the last
field: nothing follows it to be confused with.

**`repo_id` must be non-empty, on write and on read.** An empty one leaves the
AAD equal to the bare literal — the global constant this binding exists to
replace — so the scoping switches itself off with no error anywhere and two
bundles sharing a master key open each other's roots again. An implementation
must refuse to seal or open a root under an empty identifier.

This is the one place the deterministic-nonce rule is inverted, and deliberately:
every other object's nonce is derived from **the exact bytes it seals** (§3) —
never from its id — because identical plaintext *must* seal identically or dedup
dies. Deriving a nonce from an id while sealing something else is precisely the
AEAD nonce-reuse flaw this format was audited for and had removed; do not
reintroduce it for a new object kind.

The root's plaintext changes on every sync, so a message-derived nonce would
publish whether two consecutive snapshots are identical. The root is the mutable
entry point, so it takes a fresh random 24-byte nonce instead. Its AAD is
`ROOT_AAD ‖ repo_id` — the literal alone was the pre-audit form and is not
sufficient: without the identifier, two bundles sharing a master key open each
other's roots.

```json
{
  "format": 2,
  "counter": 7,
  "created_at": "2026-08-19T12:00:00Z",
  "repo_id": "usagebar-sync-abc123",
  "manifest_chunks": ["<64 hex>", "<64 hex>"],
  "chunker": "fixed-256k",
  "kdf": {"m_kib": 1048576, "t": 3, "p": 1}
}
```

- `counter` is the monotonic snapshot counter the rollback anchor compares
  against (§9).
- `repo_id` pins the repository's identity *inside* the plaintext as well as in
  the associated data above, and a reader must check the two agree. The AAD
  proves the writer meant this bundle; the recheck proves the two copies were
  not written to disagree.
- `chunker` and `kdf` are **informational duplicates**. The authoritative copy of
  the KDF parameters is the keyfile's, where they are bound as associated data
  and cannot be edited in transit. They are repeated here because the root is
  the *first* object a reader touches, so an unknown chunker or an unsupported
  KDF configuration can be refused before a single pack is fetched. A reader
  uses the keyfile's parameters and is not required to compare the two copies —
  `kdf` here sits inside the root's authenticated plaintext and the keyfile's is
  AAD-bound, so a disagreement is not something a remote can manufacture.
- `manifest_chunks` is ordered, and is the only place the manifest's chunk order
  is recorded.

### Manifest

```json
{
  "format": 2,
  "chunker": "fixed-256k",
  "files": [
    {"path": ".claude/.credentials.json", "mode": 384, "true_len": 812,
     "chunks": ["<64 hex>"]}
  ]
}
```

`mode` is the Unix mode as an integer (`384` = `0o600`). `true_len` is the
file's real length, which the sealed chunks do not carry — the last one is
padded to a power of two, so its size reveals only a bucket.

Paths, sizes, modes, and the whole directory shape are exactly the metadata a
hostile remote would like, so the manifest is sealed like any other chunk and
none of it sits in the clear beside the data it describes.

**The manifest spans as many chunks as it needs.** It is serialized to JSON and
handed to the ordinary chunker, which splits it at 256 KiB like anything else;
`Root.manifest_chunks` holds the resulting ids in order, and a reader
reassembles them through the same path — per-chunk tag, per-chunk id recheck,
then parse.

This is not a hypothetical generality. Each entry is a path, a mode, a length
and a 64-hex id, about 294 bytes once the paths are real, and the *default*
bundle measured on this milestone's target machine is 1,558 entries and 448 KiB
— already 192 KiB past a single chunk. Enabling transcript sync takes it past
5,700. Measured, at a representative 229 bytes per entry: 1,000 entries →
224 KiB → 1 chunk; 1,600 → 358 KiB → 2 chunks; 5,700 → 1.25 MiB → 5 chunks.
Compression does not rescue a single-chunk design, because the refusal is on the
plaintext length handed to the framer, long before zstd sees it.

**Ordering integrity is closed by construction, at both levels.** A file's
chunk list sits inside the manifest's sealed plaintext, and the manifest's own
chunk list sits inside the root's. Transposing either means re-sealing the
container, which needs the key. Editing one in place breaks a Poly1305 tag;
re-sealing a reordered list produces a *different* id, which the container does
not name. A reader that somehow followed a reordered list gets an error and zero
entries, never a silently scrambled restore.

A referenced chunk that is absent is reported as missing, never skipped:
concatenating whatever arrived would write a *shorter* credential file and call
it a success.

### Index object

```json
{
  "format": 1,
  "entries": [{"id": "<64 hex>", "pack": "<64 hex>", "offset": 0,
               "clen": 4112, "true_len": 4096}],
  "supersedes": ["<64 hex>"]
}
```

The chunk-id → pack-location map, sealed exactly like the manifest and
multi-chunk for the same reason — it carries one entry per chunk in the bundle,
so at scale it is the *larger* of the two.

`supersedes` names the index objects a repack replaced. Deletion follows that
order: an index must stop referencing a pack **before** the pack is deleted, or
a concurrent reader follows a pointer to nothing.

---

## 6. Versioning and evolution

Every versioned object carries two numbers: the version this build **writes**,
and the highest version it can **read**.

| Object | Written | Read ceiling |
|---|---|---|
| keyfile | 1 | 1 |
| snapshot root | 2 | 2 |
| manifest | 2 | 2 |
| index object | 1 | 1 |
| pack header | 1 | 1 |

The rule is **at or below the ceiling**, never equality:

```text
accept if found <= ceiling; refuse only found > ceiling
```

An equality check would mean a v2 client could not read a v1 bundle, which
inverts the promise the versioning exists to keep — that the KDF parameters, the
chunker, or the object shapes can be raised later without stranding bundles
already written. This project has been bitten once by a format that could not
evolve; a format that refuses its own past is the same mistake facing the other
way.

A refusal says plainly that the *client* is the old thing, not the data:
"upgrade ai-usagebar to read it".

The version is read by probing only the `format` field before deserializing the
whole object. A newer object may carry required fields this build has never
heard of, and full deserialization would fail with a confusing complaint about a
missing field instead of the true problem.

The chunker is checked the same way — **membership in a known set**, not
equality with the one this build writes. A build that introduces a second
chunker must still read the bundles it wrote with the first.

---

## 7. Calibrations

Phase 2's two — CAL-4's measured compressed bundle size and CAL-2's profile
churn — are recorded in [`sync-calibration.md`](sync-calibration.md).

### CAL-3 — Argon2id at the shipped parameters: measured

Three runs, `m = 1 GiB / t = 3 / p = 1` and the two steps down, on:

**Apple M3 Max, 36 GiB, macOS (Darwin 25.5.0), aarch64, `--release`, rustc 1.96.0.**

| Memory | t | p | Run 1 | Run 2 | Run 3 |
|---|---|---|---|---|---|
| 1024 MiB | 3 | 1 | 1503 ms | 1492 ms | 1548 ms |
| 512 MiB | 3 | 1 | 701 ms | 816 ms | 779 ms |
| 256 MiB | 3 | 1 | 336 ms | 376 ms | 380 ms |

Reproduce with the probe that produced them:

```bash
cargo test --release --test live -- --ignored --nocapture \
    cal3_argon2id_timing_at_production_parameters
```

Cost is close to linear in the memory parameter, which is the useful part: a
user who must halve `--kdf-memory` roughly halves both the wait and the
attacker's cost per guess, and can make that trade knowingly.

**No aarch64 Linux measurement was obtained.** The roadmap wanted one on a slow
aarch64 Linux box, and no such machine was reachable during this phase; a Linux
VM on this same M3 Max silicon would have answered a question nobody asked and
risked being read as a clearance for slow hardware. The documented fallback
applies unchanged: `m = 1 GiB` stays the default, the parameters travel in the
keyfile and are settable at initialisation, and a machine that cannot afford the
working set gets an actionable refusal naming `--kdf-memory` rather than an OOM
kill. Scaling the table above, a target four times slower than this one derives
in about 6 s at 1 GiB and about 1.5 s at 256 MiB.

The research figure this phase set out to check was 1582 ms on an M3 Max; the
implementation measures 1492–1548 ms on the same class of machine, so the
estimate was sound. It remains an M3 Max number, and every consumer of it should
treat it as the fast end of the range.

### CAL-1 — does a private-repo release asset honour `Range:`? Still unmeasured after four phases

**Still not measured, and nothing below is a measurement.** Phase 1 could not
run it: that phase was offline by construction and had no GitHub credential.
Phase 3 could have — it is the first phase with both an HTTP client and a live
token — and plan 3-06 offered the probe with its setup written out. It was
declined rather than run. Phases 4 and 5 did not run it either. No status code,
no `Content-Range`, no byte count.

**What stands.** The assumption is unchanged: assume ranged reads are **not**
honoured. `PACK_TARGET` stays at 32 MiB and `PACK_MAX` at 48 MiB.

**Phase 5 did not run it either, and shipped the restore path without it.** That
is now four phases with the question open. It was never a blocker and it never
sized a restore into a corner: Phase 5's reader performs a **whole-pack fetch**,
which is the correct design whichever way `Range:` goes, and `PACK_MAX`'s 48 MiB
sits under `download_asset`'s 64 MiB body cap — so no streaming verb and no
`reqwest` `stream` feature is needed either way. What CAL-1 could still buy is an
*optimisation*: if ranged reads are honoured, a restore could fetch only the
chunks it needs out of a pack rather than the whole pack, and packs could then
grow past 32 MiB without making that waste worse. That is a performance question
for whoever wants partial restore, not a gate on any shipped behaviour, and it
can be asked at any time.

**If it ever comes back positive, exactly one thing changes.** §11's `PackSource`
gains a byte-range fetch keyed on the `offset` and `clen` a reader already has
out of each pack's own sealed header. The read ceilings, the content-address
check, and the three download rounds all survive it unchanged, and the pointer's
unauthenticated `offset`/`clen`/`true_len` stay unread. It is a substitution
inside one function, not a redesign.

### CAL-5 — a torn upload's `state`, and whether `digest` is populated: also unmeasured

**Not measured either, at the end of Phase 5, and for the same reason as CAL-1:
it needs a real private repository and a real token, and it additionally *writes
and deletes* release assets.** The probe is `cal5_release_asset_state_and_digest`
in `tests/live.rs`, `#[ignore]`d and credential-gated.

Both halves of the shipped behaviour are already the conservative branch, so the
code is correct while the question is open — what is unmeasured is only how much
it could be relaxed. A release asset whose `state` is anything other than the
`"uploaded"` literal is deleted and re-uploaded by the resume scan, and the size
check runs regardless because `state` is host-supplied and not authoritative; and
because `digest` is not assumed present, the flip's verifying download happens
unconditionally. Measuring it could remove that download from a first push. It
cannot be used to drop the size check.

The probe is written and waiting in `tests/live.rs` as
`cal1_range_on_private_release_asset`. It skips with a printed message when its
token variable is absent, so it is never a hard failure:

```bash
GSD_CAL1_TOKEN=<fine-grained read-only PAT> \
GSD_CAL1_REPO=owner/throwaway-repo \
GSD_CAL1_ASSET=payload.bin \
  cargo test --test live -- --ignored --nocapture \
    cal1_range_on_private_release_asset
```

It needs a throwaway private repository carrying one release asset a little over
1 MiB, and a fine-grained PAT scoped to it with `Contents: Read`. Use a
throwaway, not the repository you paired: there is no reason to point a
hand-rolled probe at a real backup target. Delete the repository and revoke the
token afterwards.

### `permissions.admin` for a Contents-only token — open, awaiting the probe

**Not measured.** This is a question, not a decision.

D-03 wants `sync setup` to warn when the paired token carries more permission
than it needs. `sync::github::gate` parses `permissions.admin` from
`GET /repos/{owner}/{repo}` into `RepoFacts::admin_permission` and then
**deliberately warns on nothing**, because for a classic token that field
reports the *authenticated user's role on the repository*, not the token's
granted permissions — and D-01 has the user create the repository themselves,
which makes them its admin. On that reading a correctly-scoped
`Contents: read/write` PAT would still read `admin: true`, the warning would
fire on essentially every legitimate install, and a warning that always fires
teaches its reader to ignore warnings. That is a worse security outcome than no
warning, which is why plan 3-04 held it back.

**But whether a *fine-grained* PAT narrows the field is undocumented, and the
reading above is a guess.** Plan 3-06 wrote
`permissions_shape_for_a_fine_grained_contents_token` in `tests/live.rs` to
settle it and offered it at a checkpoint; it was declined, so the question is
still open:

```bash
GSD_PERM_TOKEN=<the sync PAT> \
GSD_PERM_REPO=owner/name \
  cargo test --test live -- --ignored --nocapture \
    permissions_shape_for_a_fine_grained_contents_token
```

**The token shape is part of the question.** It must be run with exactly what
[`sync-github.md`](sync-github.md) tells every user to create — fine-grained,
`Contents: Read and write`, `Metadata: Read`, no Administration, on a repository
the user owns. A classic PAT, an org-owned repository, or a token with
Administration granted each move the field for their own reasons; a reading from
one of those is an answer to a different question and must not be recorded here.

**What the answer settles.** `admin: true` means the field reflects the user's
role, cannot detect an over-permissioned token, and D-03's runtime warning is
not implementable from this endpoint — closable, with a reason. `admin: false`
means it narrows to the token's grant, and the warning becomes a one-line
addition to `assert_pushable`'s warning list.

**What stands until then.** No runtime warning ships, and the token recipe in
[`sync-github.md`](sync-github.md) is D-03's sole enforcement — which is where
its force actually lies, since the recipe is what determines the token's scope
in the first place.

---

## 8. What this format does not hide

Encryption hides contents. It does not hide the shape of the traffic, and this
design accepts that rather than pretending otherwise. Anyone who can see the
objects — the hosting service, anyone who forks the repository, anyone who later
gets into the account — learns:

- **Total bundle size**, to within the padding of its last chunks.
- **When each sync happened**, from commit or object timestamps.
- **How much changed per sync**, from the number and size of new packs. A day of
  heavy work and a day of none look different.
- **Roughly how many chunks exist**, and therefore roughly how much data.

Hiding any of that would need constant-rate cover traffic — uploading a fixed
volume on a fixed schedule whether or not anything changed. That is absurd for a
usage-monitor's state backup, so it is a decision, not an oversight.

What *is* mitigated, and worth knowing is not accidental:

- **Chunk ids are keyed**, so possession of the repository does not let anyone
  confirm that a guessed file is present.
- **Tails are padded to a power of two** after compression, so a sealed size
  names a bucket rather than an exact length.
- **The manifest is sealed and carries no paths in the clear**, and pack headers
  carry no names at all — so the directory structure, the file names, and the
  account UUIDs and session ids embedded in them stay hidden.

---

## 9. Honest limits

**There is no password recovery.** No reset link, no support address, no escrow
copy, no back door. If the password is lost the bundle is permanently unreadable
and its contents are gone. That is the only way a hosted backup can be safe to
hand to a server you do not control, and every surface that sets a password says
so before it is set.

**The password is attacked offline, without a rate limit.** A login form locks
someone out after three tries; a repository does not. Whoever holds a copy can
guess on their own hardware, forever. That is why a generated 20-character
passphrase (100 bits, straight from the OS CSPRNG) is the default path, a
user-supplied password is the exception, and a supplied one under 12 characters
is refused outright.

That 12 is not a round number: it is arithmetic against the guess rate the
shipped Argon2id parameters buy, so it means nothing unless those parameters are
held to. **Below the default memory cost the floor rises from 12 characters to
20**, the length of a generated passphrase — see §1. The two controls are one
control.

**That floor is a length, and length is a weak proxy for entropy.** Nothing in
this format measures entropy, and nothing can from the string alone: 20
characters out of the generator are 100 uniform bits, while 20 characters
somebody remembered may be worth 40, and the two arrive identically. So do not
read §1's raised floor as "only a generated passphrase is accepted" — earlier
drafts of this document said exactly that, no implementation has ever done it,
and a 20-character typed password is accepted. An implementation that refuses
one is not following this document. What the floor does is price the cheapest
mistakes out; what makes the offline attack hopeless is taking the generated
passphrase.

**Changing the password is not revocation.** A rewrap unwraps the master key
under the old password and rewraps *the same* master key under the new one — 48
bytes rewritten instead of the whole bundle. The data keys do not change. Anyone
holding an old keyfile, including one still in git history, can still unwrap it
with the old password forever. Real revocation means a new master key and a
re-encrypted bundle.

**The 1 GiB Argon2 working set is not `mlock`ed.** It cannot be under a default
memory-lock limit, and locking a fraction of it would be theatre. Key material
is held in `Zeroizing` and wiped on drop, and no key, password, nonce, or
plaintext is ever formatted into an error message, a log line, or a `Debug`
impl. But the allocating AEAD API hands back a plain `Vec` holding the unwrapped
master key and does not zeroize it; it is wiped explicitly after copying, and a
`Vec` that reallocated during construction may still leave an unreachable copy
behind. That is unavoidable with this API and is accepted knowingly.

### Residual risk: trust-on-first-use on the rollback anchor

A rollback is the one attack that produces something which authenticates
perfectly. An attacker with write access to the remote serves an *older*
snapshot root — genuinely produced by the real key, verifying flawlessly.
Nothing inside the bundle can tell the client that a newer one exists. Only
state the attacker cannot reach detects it: a monotonic counter kept on this
machine, compared against the root's `counter`, refusing anything lower unless
rollback is explicitly requested.

**On first contact there is no such counter, and the remote is believed. This is
accepted risk, not mitigated risk.** A machine that has never seen this bundle
has nothing to compare against, so an attacker who already controls the remote at
the moment of the very first fetch can serve an old snapshot and it will be
taken. Every fetch afterwards is protected. Closing it would require carrying a
counter out of band, which is a different trade than this design makes.

Three consequences a caller must respect:

- **The anchor file must be named after the remote, never after the remote's
  `repo_id`.** Whatever locates the anchor comes from local configuration — the
  remote's URL or account — because sharding it as `anchors/<repo_id>.json` is
  the obvious way to hold several bundles and it silently nullifies the whole
  mechanism. A root carrying a `repo_id` this machine has never seen would
  resolve to an absent file, which reads as first contact, which is accepted
  before any `repo_id` comparison happens. **An unrecognised `repo_id` must read
  as a mismatch, not as first contact.** First contact is a property of this
  machine and that remote; nothing the remote says may manufacture it. (The
  *repo-swap* half of this is closed independently, and cryptographically, by
  binding the expected `repo_id` into the root's associated data — §5. The
  rollback half cannot be, which is why this rule exists.)
- **The anchor lives in the config directory, never the cache.** The cache is
  documented as wipeable and users, packagers, and `rm -rf ~/.cache/*` treat it
  that way. A wiped anchor is a free rollback: it silently downgrades the next
  fetch to first contact. An attacker with local write access can delete it
  anyway — the same residual, and the reason it is durable state rather than
  disposable state. An anchor file that exists but does not parse is an *error*,
  never a reset: treating corruption as first contact would turn a damaged
  anchor into the same free rollback.
- **The rollback check decides; it does not persist.** Advancing the high-water
  mark is the caller's job and **must happen only after the snapshot verifies**.
  Advancing on a *claim* rather than on a verified snapshot means a forged high
  counter locks the user out of their own real bundle — a denial of service
  built out of the very mechanism meant to protect them.
- **Every operation that publishes a pointer must run the check, not just the
  one that obviously replays.** There are three — push, garbage collection, and
  a password change — and each of them carries the arriving snapshot records
  forward into what it writes, which launders a rollback into a
  legitimately-written pointer with a fresh valid revision. Guarding only the
  push leaves garbage collection as the executioner: it computes liveness over
  the laundered pointer and deletes every pack the rollback orphaned, and those
  packs are older than the age floor below, so nothing covers them. Reversible
  tamper becomes irreversible deletion, performed by the victim, exiting zero.
  The deliberate override belongs on the push alone (`sync push
  --allow-rollback`): neither of the other two is a command anyone reaches for
  when they mean to move the bundle backwards.

**On the restore side the same residual applies, unchanged and for the same
reason.** A second machine restoring a bundle for the first time has no anchor
either — that is the entire situation a restore exists for — so its very first
pull believes whichever snapshot the remote serves. It writes the anchor only
after that snapshot has verified, and every pull afterwards is protected. There
is no way around it that does not carry a counter out of band, which is a
different trade than this design makes. It is written here rather than left
implicit because a reader who is told "the rollback anchor catches replays"
will otherwise assume the first fetch was covered too.

### Residual: a restore is additive, and never deletes

**A file that was deleted on machine A is not deleted on machine B by pulling.**
A snapshot is a record of what one machine had, not an assertion about what
every machine should have, and a local file the manifest does not mention is
left exactly as it is — including under `--force`.

This is a decision, not an omission. Making a restore authoritative means
telling a deletion from a "never had it", which needs a per-machine baseline of
what was last synced; the account switcher's `synced.json` is the right
machinery for that and is where a future *selective* restore would start. Adding
it here would build a second reconciliation model for a case v1 does not have,
and the failure mode of getting it wrong is deleting a user's data on a machine
they only asked to receive a copy.

The practical consequence, stated plainly: pulling onto a machine that already
has files produces the **union**, with the snapshot winning every collision it
is allowed to win (§11). To get a machine that is byte-identical to the pushing
one, restore into an empty tree.

---

## 10. Remote layout

Everything above describes objects. This describes where they live on a GitHub
remote, and it is written so that someone holding only this page can implement a
reader.

### One release, one fixed tag

Every bulk object is a **release asset** on a single release, tagged
`ai-usagebar-sync-v1`, created on first push and never recreated. It is a
*published* release, not a draft: a draft has no git tag until it is published,
so `GET /repos/{owner}/{repo}/releases/tags/ai-usagebar-sync-v1` cannot find it,
and a resume scan could not locate its own crashed predecessor. Atomicity does
not come from draft state — it comes from the pointer flip below.

Release assets rather than git objects, because deleting an asset actually
removes the bytes. A git object survives in history, which would make "change
the sync password" a comforting lie rather than a real control.

### Two asset-name shapes, both content addresses

```
pack-<64 hex>.bin        content_address(pack bytes)      — a pack file, §4
keyfile-<64 hex>.json    content_address(keyfile JSON)    — the wrapped master key, §2
```

Both names are the **unkeyed** BLAKE3 hash of the object's own bytes, rendered as
64 lowercase hex characters. That is the same address §4 gives a pack on a
filesystem store; only the shape differs, because a release asset name cannot
contain a path separator, so the two-level `packs/ab/<hex>.pack` fanout collapses
to a flat prefix. Both address the same bytes.

Content addressing is what makes two questions exact rather than heuristic:

- *Is this pack already uploaded?* Its name **is** its content, so a changed pack
  gets a different name. No local record of a previous run is needed, which is
  what makes a resumed push cheap.
- *Can a rewrapped keyfile coexist with the one it replaces?* Yes, for the
  instant between the upload and the delete, because the new bytes have a new
  name. A fixed name would mean overwriting the only copy of the wrapped master
  key.

An asset whose name matches neither shape is **never** touched by this format. It
may be a future version's object or something a user attached by hand.

### The pointer, and the one compare-and-swap

```
sync/pointer.json        via the Contents API, not as a release asset
```

The pointer is an ordinary file in the repository, written through
`PUT /repos/{owner}/{repo}/contents/sync/pointer.json` with the `sha` of the blob
the writer expects to replace. **That `sha` precondition is the format's single
linearization point.** A first push omits the `sha` field entirely — which means
"create, and fail if it exists" — rather than sending it as null; those are
different requests. GitHub answers a stale `sha` with `409`, and a `PUT` that
omitted `sha` against a path that already exists with `422`; a reader-writer must
treat both as the same conflict and re-read before retrying.

Publishing a snapshot is therefore exactly one operation. Packs are immutable and
referenced by nothing until the pointer names them, so an interrupted push leaves
the previous snapshot exactly as it was — every uploaded pack is inert garbage,
collected later.

### The pointer's JSON

```json
{
  "format": 1,
  "repo_id": "github:123456789",
  "keyfile": "keyfile-<64 hex>.json",
  "snapshots": [
    {
      "root": "<base64 of the sealed snapshot root, §5>",
      "index_chunks": [
        {
          "id":       "<64 hex>",
          "pack":     "<64 hex>",
          "offset":   0,
          "clen":     4112,
          "true_len": 4096
        }
      ],
      "packs": ["<64 hex>", "…"]
    }
  ]
}
```

| Field | Type | Meaning |
|---|---|---|
| `format` | `u32` | Pointer version. Readers accept at-or-below their ceiling and refuse only what is greater, exactly as §6 requires of every other object. **Probe this field before deserializing the rest** — a newer pointer may carry required fields an older build has never heard of, and a full deserialize would complain about a missing field instead of about the version. |
| `repo_id` | string | `github:` followed by the repository's **numeric** id. A reader compares it against the identifier it holds locally and refuses a mismatch. It is a numeric id and not `owner/name` because names are transferable and re-registrable; the number is not. |
| `keyfile` | string | The asset name of the keyfile a reader must fetch to derive any key at all. |
| `snapshots` | array | **Oldest first, newest last**, at most `[sync] keep_snapshots` long (10 by default). A push appends its record at the end and truncates from the front. |
| `snapshots[].root` | base64 | The sealed snapshot root of §5, framed exactly as `Root::seal` produces it. Opening it requires `root_key` **and** the reader's own `repo_id` as associated data. |
| `snapshots[].index_chunks` | array | Where the **index object's own chunks** live — the bootstrap. See below. |
| `snapshots[].packs` | array of 64-hex | **Every** pack this snapshot needs, reused ones included. |

`packs` listing reused packs, not just new ones, is what makes garbage collection
computable from the pointer alone: the live set is the union of `packs` over the
surviving records, with no download and no key.

### The bootstrap chain a reader walks

```
sync/pointer.json
  → keyfile asset (pointer.keyfile)      → unwrap the master key with the password (§1, §2)
  → newest sealed root (snapshots[*].root, selected by the counter *inside* each root)
  → index_chunks                          → the index object's own chunks, by pack and offset
  → the index object                      → every other chunk's (pack, offset, clen, true_len)
  → root.manifest_chunks                  → the manifest, through the index object
  → manifest.files[*].chunks              → the data chunks, through the index object
```

The one link that could not be inferred is `index_chunks`. The index object maps
a chunk id to the pack and offset it lives at — but nothing describes itself, so
the index object's *own* chunks are not in it. Without those five fields in the
clear, a reader holding the pointer can locate nothing at all.

Select the newest snapshot by the `counter` **inside** each opened root, never by
position in the array. Position is attacker-controlled; the counter is inside the
authenticated plaintext.

Manifest paths are stored as a **root-prefixed relative** encoding — the name of
the collection root (`config`, `desktop-data`, `desktop-profiles`,
`claude-home`), then the path beneath it. Never an absolute path: an absolute
path is unresolvable on a second machine, leaks the pushing user's username to
anyone who obtains the repository, and is precisely what a restore's traversal
defence must reject.

### Why the container is plaintext, and what an attacker can do with it

The pointer is **not** encrypted, and that is a deliberate choice with a
specific, bounded consequence.

The reason it is safe: it introduces **no new kind of object sealed under
`chunk_key`**. Every element of value inside it is already sealed — the root under
`root_key`, with the reader's own `repo_id` bound as associated data — so the
container is a list of ciphertexts and content addresses, all of which anyone
holding the repository can already see.

What an attacker who can edit it *can* do:

- **Drop entries.** That is a rollback, and it is caught by the local monotonic
  anchor (§9), which refuses a counter below the high-water mark this machine has
  already seen.
- **Reorder the list.** Inert. A reader selects by the `counter` inside each
  sealed root, not by array position.
- **Truncate it to nothing.** A denial of service, not a disclosure, and
  indistinguishable from deleting the file — which anyone with write access could
  do anyway.

What he *cannot* do:

- **Add a fabricated snapshot.** The `root` is sealed; a forged one fails its
  Poly1305 tag.
- **Substitute another bundle's snapshot.** The root binds `repo_id` as
  associated data, so a root from a different bundle fails the tag before
  anything is parsed.
- **Learn anything new.** `counter` and `created_at` are deliberately *not*
  carried in the clear here even though they exist inside each root: they would
  be redundant leakage. A reader opens at most `keep_snapshots` small roots to
  find the newest, and garbage collection needs neither.

Accepting this is also what keeps §6's deferred AAD object-type separator
untriggered. **If a later version wants to seal this container, that is the
trigger for the separator**, and it has to be raised deliberately rather than
done quietly.

### Garbage collection

After a successful flip — and only after — a pusher may delete pack assets that
no surviving snapshot references. Two rules, and **neither is sufficient alone**:

1. The deletion set is computed against the pointer that **landed**, which is
   whatever the remote returned from the compare-and-swap, never the one the
   pusher built. If another machine won the race, the landed pointer is *its*
   pointer and its packs are consequently live.
2. **No asset younger than 24 hours is deleted, whatever the pointer says.** Rule
   1 says nothing about a machine that has uploaded a pack and has not flipped
   yet: that pack is referenced by no snapshot, the naive rule deletes it, and the
   other machine then publishes a snapshot naming data that is gone. The age floor
   is the only thing standing between garbage collection and another machine's
   in-flight push. The cost is that genuine garbage lingers a day; the alternative
   is an unrestorable backup.

The snapshot **record** is always removed before any pack: the record disappears
in the flip itself, which is strictly before the first `DELETE` is issued. The
reverse order can leave a live snapshot pointing at a deleted pack, and there is
no undo — the bytes are gone from a release asset, which is exactly the property
release assets were chosen for.

The keyfile asset named by `pointer.keyfile` is referenced by no snapshot's
`packs` and is never a deletion candidate. Deleting it makes the entire bundle
permanently unreadable.

---

## 11. Reading a bundle back

§10 says where the objects live. This says what a reader does with them, and it
is written so that someone holding §5, §10 and this section can implement a
restore that is safe against a remote assumed hostile.

Every path in a manifest is attacker-controllable — anyone with write access to
the paired repository can put an arbitrary one there, and each entry becomes a
local write on whichever machine runs `sync pull`. So the whole of this section
is biased toward refusing and explaining rather than helpfully overwriting: a
wrong push costs a re-push, a wrong restore costs the credentials and history on
the machine in front of you.

### The order is a security property

A restore is seven steps, and two of them are only correct in this order.

1. **Read the local rollback anchor.** From a path derived from *local*
   configuration, never from the remote's claimed `repo_id` (§9). A parse
   failure is an error, never "no anchor": softening it turns a damaged anchor
   into a free rollback.
2. **Resolve the chain** — pointer → keyfile → snapshot root → index object →
   manifest → packs, exactly as §10's bootstrap describes. The rollback decision
   is made *here*, against the **root's own sealed** `counter` and `repo_id`, and
   never against the plaintext pointer's copies of them.
3. **Plan.** Every manifest entry becomes exactly one decision, below.
4. **If this is a dry run, stop.** Dry run is the *absence* of the write flag,
   not a flag anything checks — the write half sits behind an early return, so
   there is no "did I remember?" below this line. It is also why step 2 fetches
   packs in three rounds: the packs holding the index object, then the packs
   holding the manifest, and only under the write flag the packs holding file
   data. A dry run downloads a bundle's metadata and not one byte of its
   content.
5. **Archive**, before the first byte, over exactly the destinations that will
   be written. Even under a force flag. Even for a partial restore.
6. **Write.**
7. **Advance the anchor** — only now, only from the **root's** sealed counter,
   and only if 2 through 6 all succeeded and nothing failed part way.

Step 7 after step 2 is the one that matters. Advancing on a *claim* would let
anyone with repo write access serve a forged high counter once and lock the user
out of their own real bundle permanently — a denial of service built out of the
rollback defence itself.

A partial restore is **reported, not rolled back**: undoing the writes that
succeeded means writing again, from an archive, on a machine that has just
demonstrated it cannot complete a write. The anchor is not advanced, so the
machine has not claimed to have seen that snapshot whole, and re-running finishes
it (below).

### The manifest path encoding

Manifest paths are **root-prefixed and relative**. There are exactly four
prefixes, one `/` separator on every platform including Windows, and the
remainder is the path beneath that root:

| prefix | root on the restoring machine |
|---|---|
| `config` | the directory holding `config.toml` |
| `desktop-data` | Claude Desktop's data directory |
| `desktop-profiles` | the claude-acc profile store |
| `claude-home` | `~/.claude` |

`config/accounts/work/.credentials.json` is a real example. `config.toml` itself
needs no fifth prefix — it is `config`'s own child. On the writing side the
**longest matching root wins**, so a root nested inside another still gets its
own prefix.

**Never an absolute path, and never a username.** An absolute path is
unresolvable on a second machine, leaks the pushing user's home directory to
anyone who obtains the repository, and is exactly what the reader below refuses —
so a bundle carrying one could be restored only by disabling its own traversal
defence.

Resolving one is the hostile-input boundary, and it refuses **before** touching
the filesystem. An entry is rejected if it is empty, contains a NUL, starts with
`/`, contains a backslash, begins with a Windows drive letter, names no root,
names a root this build does not know, names a root with nothing beneath it, or
has any component that is empty, `.`, `..`, or anything other than a plain file
name. The result is then built **one component at a time** onto the root — never
by joining an untrusted remainder, because a single absolute component handed to
a path join replaces the root wholesale, which is the classic shape of this bug.

`canonicalize` is deliberately never called. The destination usually does not
exist yet on a fresh machine, and resolving symlinks the bundle can influence is
how an escape sneaks back in after the textual checks have passed.

A rejected entry stays **in the report**, with its path and the reason it was
refused. Dropping it silently is how a user concludes a restore was complete
when it was not.

### The read ceilings, and what each bounds

Each is checked before the fetch it governs, and each refusal names both the
observed value and the ceiling, so a user who legitimately outgrows one gets a
number to raise instead of a mystery.

| Ceiling | Value | Bounds | Derived from |
|---|---|---|---|
| snapshots walked in one pointer | 256 | how many sealed roots a reader opens | `keep_snapshots` is what bounds the list a writer publishes, and it defaults to 10; a legitimate pointer is *tens* of records at most |
| manifest chunks named by a root | 128 | how many fetches the manifest costs | §5's measured sizing — 1,600 files is 2 chunks, 5,700 files is 5, at ~229 bytes per entry. 128 is ≈146,000 files |
| index-object chunks named by the pointer | 256 | the plaintext bootstrap | the byte ceiling caps a bundle at 49,152 chunks, whose ~180-byte-per-entry index is ~8.4 MiB ⇒ ~34 chunks |
| packs downloaded in one restore | 512 | **requests** | at `PACK_TARGET`, 512 packs is 16 GiB of stored data |
| bytes downloaded in one restore | 256 × `PACK_MAX` (12 GiB) | **transfer** | written as the arithmetic |

The last two are deliberately not one ceiling. A count bound does not bound
transfer — 512 one-byte packs cost 512 round trips and no bytes — and a byte
bound does not bound requests: 512 packs at `PACK_MAX` is 24 GiB, twice the byte
ceiling. Whichever binds first, binds.

The manifest-chunk ceiling is the one that matters most, and not because of the
list's size. That list is safe *inside* the root's authenticated plaintext — but
a reader consumes it to decide **how many fetches to issue**, which is a decision
made from a list before the objects it names have authenticated anything.

### What is believed, and what is not

- **Nothing about a snapshot's position.** The newest snapshot is selected by the
  `counter` **inside** each opened root. Two snapshots at one counter are no
  longer something a correct writer produces — a writer derives the counter from
  the pointer its compare-and-swap actually lands against, so the loser of a race
  re-seals one above the winner rather than reusing its own — but a hostile
  remote can still hand-write a tie, so a reader must break it deterministically
  rather than take the first match. It breaks on the root's own sealed
  `created_at` and then on the sealed root bytes: both authenticated, so the
  plaintext list's order still decides nothing.
- **Nothing about where a chunk sits.** The pointer's `offset`, `clen` and
  `true_len` are unauthenticated and are never used to slice. A reader believes
  the pointer only about *which pack asset* holds a chunk; the offsets come from
  each pack's own **sealed** header, whose every field is bounded against the
  pack's real length before use.
- **Nothing about a pack's identity.** A pack's asset name is a content address,
  so a substituted pack is refused when its bytes do not hash to the name it was
  served under — before its header is opened.
- **A root this build cannot open is skipped, not fatal.** A pointer may carry a
  record written by a newer format, and one such record must not make every older
  snapshot unrestorable.
- **A restore can never change the remote.** Four read verbs and no write verb: a
  missing release is "nothing has been pushed yet", never a reason to create one.

### The per-item decision

One decision per manifest entry, first matching row wins.

| Local file | Digest | Local mtime vs the snapshot's `created_at` | force | credential | force-credentials | Outcome |
|---|---|---|---|---|---|---|
| absent | — | — | any | any | any | **create** |
| present | equal | any | any | any | any | **skip, identical** |
| present | differs | `<=` (equal is *not* newer) | any | any | any | **update** |
| present | differs | `>` | no | any | any | **skip, local is newer** |
| present | differs | `>` | yes | no | any | **overwrite**, named in the summary |
| present | differs | `>` | yes | yes | no | **needs a second confirmation** |
| present | differs | `>` | yes | yes | yes | **overwrite**, named in the summary |

And three decided before that table is reached, none of which writes and none of
which any consent flag promotes:

- the manifest path names machine-bound or volatile state (`bridge-state.json`,
  `ant-device-registry.json`, `local-agent-mode-sessions/**`, caches, lock files)
  — **excluded by policy**, enforced on the write side rather than trusted from
  the bundle, so a future or modified client cannot talk this side into writing
  them;
- the manifest path does not resolve — **rejected**, with its reason;
- the destination exists and is **not a regular file** — a symlink, a directory,
  a socket, a device node — or cannot be stat'd — **rejected**, with its reason.

**Digest before timestamp, always.** Identity short-circuits both clocks and both
consents, which is what makes re-running an interrupted restore a no-op instead
of two hundred phantom conflicts. Identity is hashed off the disk with the push
side's own chunk addressing; the local change-detection index is **never**
consulted, because it is a cache keyed on the *push* side's stat tuple and one
stale row would declare a file the user has since edited identical and skip it.

**The conflict default is a skip with a report, not an overwrite.** Both
timestamps travel with the decision so the report can say which is which.

**A credential is the one class with a second consent.** An entry is
credential-bearing if it is in the profile store at all, or if its file name is
`.credentials.json` under any root. The general force flag never grants that
consent, and the credential flag alone — without the general one — grants
nothing. The failure mode is writing a snapshot's older OAuth token over the live
one: if it has since rotated, the live one is gone and everything authenticated
with it stops working until the user logs in again. The check applies only when
the item is *both* locally newer and already under force, because demanding a
confirmation for every credential in a fresh restore is how a gate gets
reflexively passed.

The remote side of every timestamp comparison is the snapshot root's
`created_at` — one value for the whole snapshot, because the manifest carries no
per-file mtime. The named upgrade path is a per-file `mtime_ns` under manifest
version 3, read in preference to `created_at` when present. The comparison
therefore assumes only that two machines' clocks agree to within a snapshot's
age, and it is made exact rather than merely conservative by stamping every
restored file with the snapshot's `created_at`: a restored-then-untouched file
then compares *equal* on the next pull instead of reporting a phantom conflict.

### Writing

- Every write is a tempfile created **in the destination's own directory**,
  chmodded before its first byte, then renamed into place. There is no staging
  path outside the destination directory, so decrypted plaintext never sits
  somewhere that outlives the operation, and the real name is only ever reached
  by the rename — a half-written credential cannot exist at its real name.
- **Restored files are mode 0600 and directories the restore creates are 0700**,
  unconditionally. The manifest's recorded mode is not consulted. Directories
  that already existed keep whatever their owner gave them.
- The reassembled length must equal the manifest's recorded length, or the item
  is refused rather than written short.
- Items are written in manifest order, so a restore that stops stops in the same
  place twice.
- The whole plan's paths are re-resolved before the first tempfile exists, so a
  refusal at item 40 cannot leave items 1–39 written.

What survives a process kill mid-restore: items already renamed stay complete at
0600; at most one unpersisted temporary file sits inside a destination directory;
items after the interruption are untouched; the anchor was not advanced; and a
re-run finishes it, skipping what the first run completed by digest.

### The pre-restore archive

Before the first byte, the destinations that will be written are tarred into
`sync-restore-<YYYYmmdd-HHMMSS>.tar.gz` inside the account switcher's own backups
directory (`~/.claude-acc/backups` unless the profile store was moved) — the same
directory and naming shape it already uses, so there is one place a user looks
for "undo" rather than two.

The archive holds credentials in the clear by design, so the modes are set in an
order that leaves no window: the directory is made and chmodded **0700 before
`tar` creates anything in it**, and the archive itself is chmodded **0600 the
moment it exists**.

The archive is exactly the reversal set: only the items being written, only the
ones already on disk. A restore that creates everything and overwrites nothing
therefore has no archive, and says so rather than promising one.

There is no third outcome. Either nothing existed to preserve and no archive was
created, or the archive is complete and stat'd by the time the write begins.
Every failure in between — a tar that will not run, a non-zero exit, a target
outside the archive root — aborts the restore *before* the first write. A backup
that could not be taken is never a warning.

The exact `tar -xzf … -C …` that undoes the whole restore is printed with the
summary, both on success and — a second time, as the last line — on a partial
failure, because the bottom of the output is where a user looks after one.
