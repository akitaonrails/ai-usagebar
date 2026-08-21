//! The local change-detection index (D5).
//!
//! It lives under `~/.cache`, **not** `~/.config`, precisely so it is never
//! itself synced. It is a *hint*: deleting it must degrade to a full re-scan,
//! never to a wrong answer. Every read path here is written so that the only
//! way to be wrong is to be slow — a miss re-chunks the file, which costs I/O;
//! a false hit would silently omit a changed file from the snapshot, which is
//! the one failure this module exists to prevent.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, params, params_from_iter};

use crate::error::{AppError, Result};
use crate::sync::crypto::ChunkId;
use crate::sync::scope::FileEntry;

/// Schema version this build writes and is the only one it reads. Unlike the
/// bundle format's `check_version` ceiling, an index at any other version is
/// simply thrown away and rebuilt — it is a cache, so there is nothing to
/// migrate and nothing to lose.
pub const SCHEMA_VERSION: i64 = 1;

/// Ceiling on a stored `chunk_ids` blob, enforced inside the SQL so an absurd
/// row is never materialised. The local index is not authenticated — any local
/// process can write to it — and Phase 1 left the rule that an id list read
/// before its container authenticates needs its own bound. 32 MiB is a million
/// ids, roughly a 256 GiB file: unreachable legitimately.
const MAX_CHUNK_IDS_BYTES: i64 = 32 * 1024 * 1024;

/// How many ids one `IN (…)` list carries.
///
/// SQLite's `SQLITE_MAX_VARIABLE_NUMBER` was 999 before 3.32 and is 32766 after,
/// and this crate does not choose which SQLite a packager links. 900 is under
/// both, so the query never depends on that.
const SQL_BATCH: usize = 900;

/// `meta` key holding the fingerprint of the master key this index was built
/// under. See [`Index::bind_to`].
const KEY_BINDING_KEY: &str = "key_binding";

/// Hashed under the name subkey to produce that fingerprint. Fixed, and never
/// a chunk of anyone's data.
const KEY_BINDING_LABEL: &[u8] = b"ai-usagebar sync index binding v1";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS file (
  path          TEXT PRIMARY KEY,
  size          INTEGER NOT NULL,
  mtime_ns      INTEGER NOT NULL,
  inode         INTEGER NOT NULL,
  sealed_chunks INTEGER NOT NULL,
  chunk_ids     BLOB    NOT NULL,
  seen_gen      INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS chunk (
  id        BLOB PRIMARY KEY,
  pack      BLOB    NOT NULL,
  \"offset\" INTEGER NOT NULL,
  clen      INTEGER NOT NULL,
  plen      INTEGER NOT NULL,
  seen_gen  INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v BLOB);
";

/// What a hit on the change-detection tuple gives back: enough to reuse the
/// file's chunks without opening it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRecord {
    /// How many full [`crate::sync::CHUNK_SIZE`] chunks the file had. The tail
    /// chunk is the one that grows, so an append can reuse everything below it.
    pub sealed_chunks: u64,
    /// The file's chunk ids, in file order.
    pub chunk_ids: Vec<[u8; 32]>,
}

/// Where the local index believes one already-packed chunk lives.
///
/// The same five numbers a [`PackEntry`](crate::sync::pack::PackEntry) carries,
/// which is the point: a chunk the last push packed can be named in the next
/// snapshot's index object without re-reading, re-sealing or re-uploading it.
///
/// It is a **hint**, exactly as the rest of this module is. Every field arrives
/// from an unauthenticated local database, so a caller must treat absence as the
/// safe answer — and [`Index::chunk_locations`] guarantees that direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkLocation {
    pub pack: ChunkId,
    pub offset: u64,
    pub clen: u32,
    pub plen: u32,
}

/// `~/.cache/ai-usagebar/sync/index.sqlite3`, via the same resolver the vendor
/// caches use.
pub fn default_path() -> Result<PathBuf> {
    Ok(crate::cache::xdg_cache_dir()?
        .join("ai-usagebar")
        .join("sync")
        .join("index.sqlite3"))
}

/// The index database. Constructed with [`Index::at`] everywhere, including
/// production — [`default_path`] is the only thing that knows about `$HOME`.
pub struct Index {
    conn: Connection,
    path: PathBuf,
    rebuilt: bool,
    rehash: bool,
}

impl Index {
    /// Open (creating if absent) the index at `path`.
    ///
    /// The file is created and set to mode 0600 **before** the connection is
    /// opened: its rows carry account UUIDs inside full paths, so the mode is
    /// never briefly wrong on a shared machine.
    ///
    /// A file that is not a database, fails SQLite's integrity check, carries a
    /// schema version this build does not write, or has our table names with
    /// the wrong columns is **discarded and recreated**, not repaired. Repair
    /// could keep a stale row, and a stale row is a changed file reported as
    /// unchanged. The cost of discarding is one slow sync; see
    /// [`Index::was_rebuilt`].
    pub fn at(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| AppError::io_at(dir, e))?;
        }
        create_private(path)?;
        match open_checked(path) {
            Ok(conn) => Ok(Self {
                conn,
                path: path.to_path_buf(),
                rebuilt: false,
                rehash: false,
            }),
            Err(why) => {
                // Path and reason only — the rows themselves are account UUIDs.
                eprintln!(
                    "ai-usagebar sync: rebuilding the local index at {} ({why})",
                    path.display()
                );
                discard(path)?;
                create_private(path)?;
                let conn = open_checked(path)?;
                Ok(Self {
                    conn,
                    path: path.to_path_buf(),
                    rebuilt: true,
                    rehash: false,
                })
            }
        }
    }

    /// Where this index lives — so a report can name it without resolving
    /// `$HOME` itself, which is what keeps the report builder hermetic.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// True when this open threw away a damaged index and started over, so the
    /// dry-run can tell the user why the run is slow instead of leaving them
    /// guessing.
    pub fn was_rebuilt(&self) -> bool {
        self.rebuilt
    }

    /// `--force-rehash`: make every cached-record read miss for the life of this
    /// handle, so the planner opens and hashes every file.
    ///
    /// **It suppresses reads and clears nothing.** Every row survives, and the
    /// run rewrites them with what it actually found — a flag that also
    /// destroyed the cache would be more destructive than its name (T-5-65).
    /// The escape that *does* discard is [`reset_at`], which is a different
    /// question with a different flag.
    ///
    /// It sits on the handle rather than on [`crate::sync::plan::build`]'s
    /// argument list because `lookup` and `cached` are the only two reads there
    /// are: putting the switch on them covers every planner — `sync push`,
    /// `push --dry-run` and `status` — instead of only the ones a caller
    /// remembered to thread a bool through.
    #[must_use]
    pub fn rehashing(mut self) -> Self {
        self.rehash = true;
        self
    }

    /// When the last successful sync completed, if the index knows. `None` is
    /// a normal first-run answer, not an error.
    pub fn last_sync(&self) -> Option<DateTime<Utc>> {
        let raw: String = self.meta("last_sync")?;
        DateTime::parse_from_rfc3339(&raw)
            .ok()
            .map(|t| t.with_timezone(&Utc))
    }

    /// Record the completion time of a successful sync.
    pub fn set_last_sync(&self, at: DateTime<Utc>) -> Result<()> {
        self.set_meta("last_sync", at.to_rfc3339())
    }

    /// The current sync generation, `0` before the first bump.
    pub fn generation(&self) -> u64 {
        self.meta::<i64>("generation")
            .and_then(|v| u64::try_from(v).ok())
            .unwrap_or(0)
    }

    /// Start a new generation and return it. Every row touched during the run
    /// is stamped with this, which is what [`Index::evict_unseen`] then ages.
    pub fn bump_generation(&self) -> Result<u64> {
        let next = self.generation().saturating_add(1);
        self.set_meta("generation", clamp_i64(next))?;
        Ok(next)
    }

    /// D5's change detection, and the only question the planner asks per file:
    /// does `(path, size, mtime_ns, inode)` match what we stored?
    ///
    /// A hit means the caller never opens the file — the short-circuit SYNC-02
    /// rests on — so this performs no I/O beyond the SQLite read. Anything
    /// unexpected (no row, a field that does not fit, a `chunk_ids` blob that
    /// is not a whole number of 32-byte ids, a SQL error) is a miss, never a
    /// partial answer.
    pub fn lookup(&self, entry: &FileEntry) -> Option<FileRecord> {
        if self.rehash {
            return None;
        }
        let path = entry.path.to_str()?;
        let (size, mtime_ns, inode) = key_of(entry)?;
        self.read_row(
            "SELECT sealed_chunks, chunk_ids FROM file \
             WHERE path = ?1 AND size = ?2 AND mtime_ns = ?3 AND inode = ?4 \
               AND length(chunk_ids) <= ?5",
            params![path, size, mtime_ns, inode, MAX_CHUNK_IDS_BYTES],
        )
    }

    /// The stored row for `path` whatever its D5 tuple now says — what we
    /// chunked *last* time, which is the append fast path's starting guess.
    ///
    /// [`Index::lookup`] answers "is this file unchanged?" and its `Some` may
    /// be trusted. This answers "what did this file used to be?" and its `Some`
    /// is a **hypothesis**: the caller must re-hash the last sealed chunk
    /// before reusing any id, because a rewrite that happens to grow a file is
    /// indistinguishable from an append by `(size, mtime_ns, inode)` alone.
    pub fn cached(&self, path: &Path) -> Option<FileRecord> {
        if self.rehash {
            return None;
        }
        let path = path.to_str()?;
        self.read_row(
            "SELECT sealed_chunks, chunk_ids FROM file \
             WHERE path = ?1 AND length(chunk_ids) <= ?2",
            params![path, MAX_CHUNK_IDS_BYTES],
        )
    }

    /// Decode one `file` row. Every failure — no row, SQL error, a field that
    /// does not fit, a `chunk_ids` blob that is not a whole number of ids —
    /// reads as `None`, never as a partial answer.
    fn read_row(&self, sql: &str, params: impl rusqlite::Params) -> Option<FileRecord> {
        let (sealed, blob): (i64, Vec<u8>) = self
            .conn
            .query_row(sql, params, |row| Ok((row.get(0)?, row.get(1)?)))
            .ok()?;
        if !blob.len().is_multiple_of(32) {
            // A truncated row would otherwise yield a short chunk list, i.e. a
            // file uploaded with its tail missing. Re-chunk instead.
            return None;
        }
        Some(FileRecord {
            sealed_chunks: u64::try_from(sealed).ok()?,
            // `as_chunks`, not `chunks_exact(32)`: the const generic gives back
            // `&[u8; 32]` already, so nothing copies into a scratch array and
            // nothing can disagree about the length.
            chunk_ids: blob.as_chunks::<32>().0.to_vec(),
        })
    }

    /// Store what chunking a file produced, stamped with the current
    /// generation.
    pub fn record(
        &self,
        entry: &FileEntry,
        sealed_chunks: u64,
        chunk_ids: &[[u8; 32]],
    ) -> Result<()> {
        // An entry that cannot be represented simply gets no row, which reads
        // back as "changed": a re-chunk next run, not a failed sync.
        let (Some(path), Some((size, mtime_ns, inode)), Ok(sealed)) = (
            entry.path.to_str(),
            key_of(entry),
            i64::try_from(sealed_chunks),
        ) else {
            return Ok(());
        };
        // One statement, so SQLite commits it whole or not at all: a process
        // killed mid-write leaves the previous row or none, and both are
        // correct answers for a hint.
        self.conn
            .execute(
                "INSERT OR REPLACE INTO file \
                 (path, size, mtime_ns, inode, sealed_chunks, chunk_ids, seen_gen) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    path,
                    size,
                    mtime_ns,
                    inode,
                    sealed,
                    chunk_ids.concat(),
                    clamp_i64(self.generation()),
                ],
            )
            .map_err(|e| self.err(e))?;
        Ok(())
    }

    /// Stamp the current generation on a row confirmed unchanged, so eviction
    /// does not age out a file just because it never needed re-chunking.
    pub fn touch(&self, path: &Path) -> Result<()> {
        let Some(path) = path.to_str() else {
            return Ok(());
        };
        self.conn
            .execute(
                "UPDATE file SET seen_gen = ?1 WHERE path = ?2",
                params![clamp_i64(self.generation()), path],
            )
            .map_err(|e| self.err(e))?;
        Ok(())
    }

    /// Drop every row not seen within the last `keep_generations` generations
    /// and return how many went — borg's cache-age mechanic, so a deleted
    /// file's row does not live forever.
    pub fn evict_unseen(&self, keep_generations: u64) -> Result<usize> {
        // Signed on purpose: on a fresh index `generation - keep` is negative,
        // and a saturating unsigned zero would evict the rows just written.
        let horizon = clamp_i64(self.generation()).saturating_sub(clamp_i64(keep_generations));
        let mut removed = self
            .conn
            .execute("DELETE FROM file WHERE seen_gen <= ?1", [horizon])
            .map_err(|e| self.err(e))?;
        removed += self
            .conn
            .execute("DELETE FROM chunk WHERE seen_gen <= ?1", [horizon])
            .map_err(|e| self.err(e))?;
        Ok(removed)
    }

    // ---- the chunk table ----------------------------------------------
    //
    // The table has existed since plan 2-03 and had no writer until 4-02, which
    // is why "already uploaded" could only ever be inferred from the `file`
    // table: a chunk shared with a file that failed its append check was
    // re-sealed and re-uploaded even though the bytes were already on the
    // remote. These four accessors close that.

    /// Record where a batch of chunks landed: `(chunk id, pack id, offset,
    /// ciphertext length, plaintext length)`.
    ///
    /// One transaction for the whole batch, and the generation stamped inside
    /// the same statement — a first push records thousands of rows, and a
    /// transaction per row is the difference between a second and a minute.
    ///
    /// **Call it after the pack is finished**, never before: a pack's content
    /// address does not exist until its header is sealed, so a row written
    /// earlier names nothing.
    pub fn record_chunks(&self, rows: &[(ChunkId, ChunkId, u64, u32, u32)]) -> Result<usize> {
        if rows.is_empty() {
            return Ok(0);
        }
        let seen_gen = clamp_i64(self.generation());
        let tx = self.conn.unchecked_transaction().map_err(|e| self.err(e))?;
        let mut written = 0usize;
        {
            let mut stmt = tx
                .prepare(
                    "INSERT OR REPLACE INTO chunk \
                     (id, pack, \"offset\", clen, plen, seen_gen) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )
                .map_err(|e| self.err(e))?;
            for (id, pack, offset, clen, plen) in rows {
                written += stmt
                    .execute(params![
                        id.as_bytes().as_slice(),
                        pack.as_bytes().as_slice(),
                        clamp_i64(*offset),
                        *clen,
                        *plen,
                        seen_gen,
                    ])
                    .map_err(|e| self.err(e))?;
            }
        }
        tx.commit().map_err(|e| self.err(e))?;
        Ok(written)
    }

    /// Where the index believes each of `ids` lives, for the subset it knows.
    ///
    /// One query per [`SQL_BATCH`] ids rather than one per id, and it **fails
    /// towards not-known**: a SQL error, a missing table, a blob that is not 32
    /// bytes, a negative offset or length — every one of them yields absence.
    /// The cost of absence is re-uploading bytes that are already there; the
    /// cost of a wrong hit is a snapshot naming a pack that holds nothing, so
    /// there is only one safe direction and this is it.
    pub fn chunk_locations(&self, ids: &[ChunkId]) -> HashMap<ChunkId, ChunkLocation> {
        let mut found = HashMap::new();
        for batch in ids.chunks(SQL_BATCH) {
            let sql = format!(
                "SELECT id, pack, \"offset\", clen, plen FROM chunk WHERE id IN ({})",
                vec!["?"; batch.len()].join(",")
            );
            let Ok(mut stmt) = self.conn.prepare(&sql) else {
                return HashMap::new();
            };
            let rows = stmt.query_map(
                params_from_iter(batch.iter().map(|id| id.as_bytes().as_slice())),
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            );
            let Ok(rows) = rows else {
                return HashMap::new();
            };
            for row in rows.flatten() {
                if let Some((id, at)) = located(row) {
                    found.insert(id, at);
                }
            }
        }
        found
    }

    /// Drop every row naming one of `packs`, after those packs are actually gone
    /// from the remote, so the index never claims a chunk lives somewhere it
    /// does not.
    ///
    /// Deleting rows is safe by construction: the worst outcome is a re-upload.
    pub fn forget_chunks(&self, packs: &[ChunkId]) -> Result<usize> {
        let mut removed = 0usize;
        for batch in packs.chunks(SQL_BATCH) {
            let sql = format!(
                "DELETE FROM chunk WHERE pack IN ({})",
                vec!["?"; batch.len()].join(",")
            );
            removed += self
                .conn
                .execute(
                    &sql,
                    params_from_iter(batch.iter().map(|id| id.as_bytes().as_slice())),
                )
                .map_err(|e| self.err(e))?;
        }
        Ok(removed)
    }

    /// Tie this index to the master key whose chunk ids it caches, clearing it
    /// if the key has changed.
    ///
    /// A chunk id is `keyed_hash(name_key, plaintext)`, so **every id in here
    /// belongs to exactly one master key**. Write a new keyfile — which
    /// `sync setup` does whenever it is re-run — and every cached id becomes an
    /// address that nothing will ever seal again.
    ///
    /// That used to be a corrupt path and is now merely a slow one.
    /// `plan::build` reuses a cached file's ids without reopening the file, and
    /// the manifest was built from that list, so after a key change the plan
    /// named the old ids, the packer computed new ones, nothing matched,
    /// nothing was sealed — and the manifest shipped referencing chunks that
    /// were never uploaded. `packer::build` now names only what it actually
    /// read and sealed (6-08), so the worst a stale binding costs is re-reading
    /// every file. Which is exactly what this binding exists to avoid.
    ///
    /// The stored value is `chunk_id` over a fixed label: a keyed hash, so it
    /// identifies the key without being invertible to it, and it reuses the
    /// primitive whose behaviour this is about rather than inventing a second.
    ///
    /// Returns `true` when the index was cleared.
    pub fn bind_to(&self, keys: &crate::sync::crypto::Keys) -> Result<bool> {
        let current = keys.chunk_id(KEY_BINDING_LABEL);
        let current = current.as_bytes().to_vec();
        match self.meta::<Vec<u8>>(KEY_BINDING_KEY) {
            Some(stored) if stored == current => Ok(false),
            Some(_) => {
                // Not `reset_at`: the file is open, and `last_sync` is about the
                // remote rather than about any key.
                self.conn
                    .execute_batch("DELETE FROM file; DELETE FROM chunk;")
                    .map_err(|e| self.err(e))?;
                self.set_meta(KEY_BINDING_KEY, current)?;
                Ok(true)
            }
            None => {
                self.set_meta(KEY_BINDING_KEY, current)?;
                Ok(false)
            }
        }
    }

    fn meta<T: rusqlite::types::FromSql>(&self, key: &str) -> Option<T> {
        self.conn
            .query_row("SELECT v FROM meta WHERE k = ?1", [key], |row| row.get(0))
            .ok()
    }

    fn set_meta(&self, key: &str, value: impl rusqlite::ToSql) -> Result<()> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO meta (k, v) VALUES (?1, ?2)",
                params![key, value],
            )
            .map_err(|e| self.err(e))?;
        Ok(())
    }

    fn err(&self, e: rusqlite::Error) -> AppError {
        sql_err(&self.path, e)
    }
}

/// D5's three numeric fields as SQLite integers.
///
/// D5 locks the tuple as `(path, size, mtime_ns, inode)`. The research argued
/// for borg's `ctime` as a fifth field, since ctime cannot be forged from user
/// space; D5 decided against it. That is a decision, not an omission — an
/// attacker who can set mtime on the user's own files can already edit them.
///
/// `None` — a value that does not fit an i64 — reads as "no match", so the file
/// is re-chunked. Always the safe direction.
fn key_of(entry: &FileEntry) -> Option<(i64, i64, i64)> {
    Some((
        i64::try_from(entry.size).ok()?,
        i64::try_from(entry.mtime_ns).ok()?,
        i64::try_from(entry.inode).ok()?,
    ))
}

/// Decode one `chunk` row, or `None`.
///
/// Rejects rather than casts: a negative `offset`, `clen` or `plen` is a row no
/// writer of ours produced, and `as u64` would turn it into an enormous
/// plausible-looking number pointing into the middle of somebody's pack.
fn located(row: (Vec<u8>, Vec<u8>, i64, i64, i64)) -> Option<(ChunkId, ChunkLocation)> {
    let (id, pack, offset, clen, plen) = row;
    let id: [u8; 32] = id.try_into().ok()?;
    let pack: [u8; 32] = pack.try_into().ok()?;
    Some((
        ChunkId::from_bytes(id),
        ChunkLocation {
            pack: ChunkId::from_bytes(pack),
            offset: u64::try_from(offset).ok()?,
            clen: u32::try_from(clen).ok()?,
            plen: u32::try_from(plen).ok()?,
        },
    ))
}

fn clamp_i64(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

fn sql_err(path: &Path, e: rusqlite::Error) -> AppError {
    AppError::Other(format!("local sync index at {}: {e}", path.display()))
}

/// Open `path` and bring it to [`SCHEMA_VERSION`], or fail so [`Index::at`] can
/// discard it. Every check here is a reason to throw the file away.
fn open_checked(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path).map_err(|e| sql_err(path, e))?;
    // Catches both "this is not a SQLite file at all" and a genuinely damaged
    // one; a zero-length file is a valid empty database and passes.
    let verdict: String = conn
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|e| sql_err(path, e))?;
    if verdict != "ok" {
        return Err(AppError::Other(format!(
            "local sync index at {} failed SQLite's integrity check",
            path.display()
        )));
    }
    let found: Option<i64> = conn
        .query_row("SELECT v FROM meta WHERE k = 'schema_version'", [], |row| {
            row.get(0)
        })
        .ok();
    if let Some(v) = found
        && v != SCHEMA_VERSION
    {
        return Err(AppError::Other(format!(
            "local sync index at {} is schema version {v}, this build writes {SCHEMA_VERSION}",
            path.display()
        )));
    }
    conn.execute_batch(SCHEMA).map_err(|e| sql_err(path, e))?;
    // Prepare-only probe: a file carrying our table names with different
    // columns survives `CREATE TABLE IF NOT EXISTS`, and would otherwise fail
    // at the first lookup — mid-sync, where the answer cannot be "discard".
    conn.prepare(
        "SELECT path, size, mtime_ns, inode, sealed_chunks, chunk_ids, seen_gen FROM file",
    )
    .map_err(|e| sql_err(path, e))?;
    // The same probe for the chunk table. Its *reads* degrade to "nothing
    // known" on their own, but `record_chunks` is a write in the middle of a
    // push, where the answer can no longer be "discard the index" — so the
    // shape is checked here, where it still can be.
    conn.prepare("SELECT id, pack, \"offset\", clen, plen, seen_gen FROM chunk")
        .map_err(|e| sql_err(path, e))?;
    conn.execute(
        "INSERT OR REPLACE INTO meta (k, v) VALUES ('schema_version', ?1)",
        [SCHEMA_VERSION],
    )
    .map_err(|e| sql_err(path, e))?;
    Ok(conn)
}

/// Remove a damaged index, sidecars included: SQLite would happily replay a
/// stale `-journal` or `-wal` over the fresh database and reinstate exactly the
/// corruption we just removed.
/// `--rebuild-index`: throw the index away and start it empty at the same path.
///
/// The **explicit** version of what [`Index::at`] already does on its own when
/// it meets a file it cannot trust — same discard, same fresh mode-0600
/// database, same "one slow sync" cost. This one is user-invoked, for the case
/// where the automatic check passes and the user has some other reason to
/// distrust the cache.
///
/// It **removes the file** rather than issuing `DROP TABLE` against it: an index
/// corrupt enough to need this escape is one SQLite may refuse to open at all,
/// and a recovery path must not depend on the thing it is recovering from
/// (T-5-64). Absent is not an error — it is the same outcome by a shorter road.
/// A directory at `path` is refused by name rather than removed, because
/// nothing here should ever recurse over a user's tree.
pub fn reset_at(path: &Path) -> Result<Index> {
    if path.is_dir() {
        return Err(AppError::Other(format!(
            "the local sync index path {} is a directory, not a database — \
             refusing to remove it",
            path.display()
        )));
    }
    discard(path)?;
    Index::at(path)
}

fn discard(path: &Path) -> Result<()> {
    for suffix in ["", "-journal", "-wal", "-shm"] {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        let victim = PathBuf::from(name);
        match std::fs::remove_file(&victim) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(AppError::io_at(&victim, e)),
        }
    }
    Ok(())
}

/// Create the file mode-0600 if it does not exist. SQLite is happy to adopt a
/// zero-length file, which is what lets the mode be right from byte zero — on
/// the rebuild path just as much as on a fresh one.
fn create_private(path: &Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    match opts.open(path) {
        Ok(_) => Ok(()),
        // Lost a race with another process that just created it — fine.
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(AppError::io_at(path, e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use tempfile::TempDir;

    fn entry(path: &Path, size: u64, mtime_ns: i128, inode: u64) -> FileEntry {
        FileEntry {
            path: path.to_path_buf(),
            size,
            mtime_ns,
            inode,
        }
    }

    fn sample(dir: &TempDir) -> FileEntry {
        entry(
            &dir.path().join("a.jsonl"),
            4096,
            1_700_000_000_123_456_789,
            42,
        )
    }

    fn ids(n: usize) -> Vec<[u8; 32]> {
        (0..n)
            .map(|i| {
                let mut id = [0u8; 32];
                id[0] = u8::try_from(i).unwrap();
                id
            })
            .collect()
    }

    #[cfg(unix)]
    fn assert_private(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "{mode:o}");
    }

    // ---- schema and open ----------------------------------------------

    #[test]
    fn opening_a_fresh_index_creates_the_file_and_reports_no_last_sync() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested").join("index.sqlite3");
        let index = Index::at(&path).unwrap();
        assert!(path.exists(), "a missing parent directory must be created");
        assert!(index.last_sync().is_none());
        assert!(!index.was_rebuilt());
        assert_eq!(index.meta::<i64>("schema_version"), Some(SCHEMA_VERSION));
    }

    #[cfg(unix)]
    #[test]
    fn the_index_file_is_created_mode_0600() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index.sqlite3");
        let _index = Index::at(&path).unwrap();
        assert_private(&path);
    }

    #[test]
    fn reopening_an_index_of_the_current_version_keeps_what_was_written() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index.sqlite3");
        let e = sample(&dir);
        {
            let index = Index::at(&path).unwrap();
            index.record(&e, 2, &ids(3)).unwrap();
        }
        let index = Index::at(&path).unwrap();
        assert!(
            !index.was_rebuilt(),
            "a healthy index must not be discarded"
        );
        assert_eq!(index.lookup(&e).unwrap().sealed_chunks, 2);
    }

    #[test]
    fn last_sync_round_trips_and_a_garbage_value_reads_as_unknown() {
        let dir = TempDir::new().unwrap();
        let index = Index::at(&dir.path().join("index.sqlite3")).unwrap();
        let at = DateTime::parse_from_rfc3339("2026-08-19T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        index.set_last_sync(at).unwrap();
        assert_eq!(index.last_sync(), Some(at));

        index.set_meta("last_sync", "not-a-date").unwrap();
        assert!(index.last_sync().is_none());
    }

    // ---- the change-detection tuple -----------------------------------

    #[test]
    fn lookup_of_an_unknown_path_is_none() {
        let dir = TempDir::new().unwrap();
        let index = Index::at(&dir.path().join("index.sqlite3")).unwrap();
        assert!(index.lookup(&sample(&dir)).is_none());
    }

    #[test]
    fn lookup_hits_when_the_whole_tuple_matches() {
        let dir = TempDir::new().unwrap();
        let index = Index::at(&dir.path().join("index.sqlite3")).unwrap();
        let e = sample(&dir);
        index.record(&e, 7, &ids(2)).unwrap();
        let hit = index.lookup(&e).expect("an identical tuple must hit");
        assert_eq!(hit.sealed_chunks, 7);
        assert_eq!(hit.chunk_ids, ids(2));
    }

    #[test]
    fn lookup_misses_when_size_differs() {
        let dir = TempDir::new().unwrap();
        let index = Index::at(&dir.path().join("index.sqlite3")).unwrap();
        let e = sample(&dir);
        index.record(&e, 1, &ids(1)).unwrap();
        let mut changed = e.clone();
        changed.size += 1;
        assert!(index.lookup(&changed).is_none());
    }

    #[test]
    fn lookup_misses_when_mtime_ns_differs() {
        let dir = TempDir::new().unwrap();
        let index = Index::at(&dir.path().join("index.sqlite3")).unwrap();
        let e = sample(&dir);
        index.record(&e, 1, &ids(1)).unwrap();
        let mut changed = e.clone();
        changed.mtime_ns += 1;
        assert!(index.lookup(&changed).is_none());
    }

    #[test]
    fn lookup_misses_when_inode_differs() {
        let dir = TempDir::new().unwrap();
        let index = Index::at(&dir.path().join("index.sqlite3")).unwrap();
        let e = sample(&dir);
        index.record(&e, 1, &ids(1)).unwrap();
        let mut changed = e.clone();
        changed.inode += 1;
        assert!(index.lookup(&changed).is_none());
    }

    #[test]
    fn chunk_id_lists_round_trip_in_order_at_length_0_1_and_many() {
        let dir = TempDir::new().unwrap();
        let index = Index::at(&dir.path().join("index.sqlite3")).unwrap();
        let e = sample(&dir);
        for n in [0usize, 1, 200] {
            let expected = ids(n);
            index.record(&e, n as u64, &expected).unwrap();
            assert_eq!(index.lookup(&e).unwrap().chunk_ids, expected, "n = {n}");
        }
    }

    #[test]
    fn a_chunk_ids_blob_that_is_not_a_whole_number_of_ids_reads_as_no_match() {
        let dir = TempDir::new().unwrap();
        let index = Index::at(&dir.path().join("index.sqlite3")).unwrap();
        let e = sample(&dir);
        index.record(&e, 2, &ids(2)).unwrap();
        // Truncate the stored blob mid-id, as a partial write would.
        index
            .conn
            .execute("UPDATE file SET chunk_ids = substr(chunk_ids, 1, 33)", [])
            .unwrap();
        assert!(
            index.lookup(&e).is_none(),
            "a truncated row must force a re-chunk, not yield a short list"
        );
    }

    // ---- generations and eviction -------------------------------------

    #[test]
    fn eviction_drops_an_untouched_row_and_keeps_a_touched_one() {
        let dir = TempDir::new().unwrap();
        let index = Index::at(&dir.path().join("index.sqlite3")).unwrap();
        let stale = entry(&dir.path().join("stale"), 1, 1, 1);
        let fresh = entry(&dir.path().join("fresh"), 2, 2, 2);
        assert_eq!(index.generation(), 0);
        index.record(&stale, 0, &[]).unwrap();
        index.record(&fresh, 0, &[]).unwrap();

        // Nothing is old enough yet on a first run.
        assert_eq!(index.evict_unseen(1).unwrap(), 0);

        assert_eq!(index.bump_generation().unwrap(), 1);
        index.touch(&fresh.path).unwrap();
        assert_eq!(index.evict_unseen(1).unwrap(), 1);
        assert!(index.lookup(&stale).is_none());
        assert!(index.lookup(&fresh).is_some());
    }

    // ---- the index is a hint ------------------------------------------

    #[test]
    fn deleting_the_index_reports_every_file_as_changed() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index.sqlite3");
        let e = sample(&dir);
        {
            let index = Index::at(&path).unwrap();
            index.record(&e, 5, &ids(5)).unwrap();
            assert!(index.lookup(&e).is_some());
        }
        std::fs::remove_file(&path).unwrap();

        let index = Index::at(&path).unwrap();
        assert!(
            index.lookup(&e).is_none(),
            "a deleted index must degrade to a full re-scan"
        );
        index.record(&e, 5, &ids(5)).unwrap();
        assert!(index.lookup(&e).is_some());
    }

    #[test]
    fn garbage_bytes_are_discarded_and_the_index_rebuilds_itself() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index.sqlite3");
        std::fs::write(&path, b"this is not a database, it is a pipe").unwrap();

        let index = Index::at(&path).unwrap();
        assert!(index.was_rebuilt());
        assert!(index.lookup(&sample(&dir)).is_none());
        #[cfg(unix)]
        assert_private(&path);

        let e = sample(&dir);
        index.record(&e, 3, &ids(3)).unwrap();
        assert_eq!(index.lookup(&e).unwrap().chunk_ids, ids(3));
    }

    #[test]
    fn a_future_schema_version_is_discarded_rather_than_read() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index.sqlite3");
        let e = sample(&dir);
        {
            let index = Index::at(&path).unwrap();
            index.record(&e, 9, &ids(9)).unwrap();
            index
                .set_meta("schema_version", SCHEMA_VERSION + 1)
                .unwrap();
        }

        let index = Index::at(&path).unwrap();
        assert!(index.was_rebuilt());
        assert!(
            index.lookup(&e).is_none(),
            "rows written by an unknown schema must not be interpreted"
        );
        assert_eq!(index.meta::<i64>("schema_version"), Some(SCHEMA_VERSION));
        #[cfg(unix)]
        assert_private(&path);

        index.record(&e, 3, &ids(3)).unwrap();
        assert_eq!(index.lookup(&e).unwrap().sealed_chunks, 3);
    }

    #[test]
    fn our_table_names_with_the_wrong_columns_are_discarded_too() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index.sqlite3");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("CREATE TABLE file (path TEXT PRIMARY KEY, junk TEXT)")
                .unwrap();
        }
        let index = Index::at(&path).unwrap();
        assert!(index.was_rebuilt());
        let e = sample(&dir);
        index.record(&e, 1, &ids(1)).unwrap();
        assert!(index.lookup(&e).is_some());
    }

    // ---- the chunk table ----------------------------------------------

    fn cid(byte: u8) -> ChunkId {
        ChunkId::from_bytes([byte; 32])
    }

    fn open(dir: &TempDir) -> Index {
        Index::at(&dir.path().join("index.sqlite3")).unwrap()
    }

    #[test]
    fn a_recorded_chunk_is_known_and_locatable() {
        let dir = TempDir::new().unwrap();
        let index = open(&dir);
        assert_eq!(
            index
                .record_chunks(&[(cid(1), cid(9), 0, 4112, 4096)])
                .unwrap(),
            1
        );

        assert_eq!(
            index.chunk_locations(&[cid(1)]).get(&cid(1)),
            Some(&ChunkLocation {
                pack: cid(9),
                offset: 0,
                clen: 4112,
                plen: 4096,
            })
        );
    }

    #[test]
    fn re_recording_an_id_moves_it_rather_than_duplicating_it() {
        let dir = TempDir::new().unwrap();
        let index = open(&dir);
        index.record_chunks(&[(cid(1), cid(9), 0, 10, 8)]).unwrap();
        index.record_chunks(&[(cid(1), cid(8), 64, 10, 8)]).unwrap();

        let rows: i64 = index
            .conn
            .query_row("SELECT count(*) FROM chunk", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1, "a repack moves a chunk, it does not clone it");
        assert_eq!(index.chunk_locations(&[cid(1)])[&cid(1)].pack, cid(8));
    }

    /// One query, batched under SQLite's variable limit — a first push asks
    /// about thousands of ids at once and a statement per id is the difference
    /// between a second and a minute.
    #[test]
    fn membership_is_answered_for_more_ids_than_sqlite_takes_parameters() {
        let dir = TempDir::new().unwrap();
        let index = open(&dir);
        let many: Vec<ChunkId> = (0..2_500u32)
            .map(|i| {
                let mut raw = [0u8; 32];
                raw[..4].copy_from_slice(&i.to_le_bytes());
                ChunkId::from_bytes(raw)
            })
            .collect();
        let rows: Vec<_> = many
            .iter()
            .take(2_000)
            .map(|id| (*id, cid(9), 0u64, 10u32, 8u32))
            .collect();
        assert_eq!(index.record_chunks(&rows).unwrap(), 2_000);

        let known = index.chunk_locations(&many);
        assert_eq!(known.len(), 2_000);
        assert!(known.contains_key(&many[1_999]));
        assert!(!known.contains_key(&many[2_000]));
    }

    #[test]
    fn forgetting_a_pack_leaves_no_row_pointing_at_it() {
        let dir = TempDir::new().unwrap();
        let index = open(&dir);
        index
            .record_chunks(&[
                (cid(1), cid(9), 0, 10, 8),
                (cid(2), cid(9), 16, 10, 8),
                (cid(3), cid(8), 0, 10, 8),
            ])
            .unwrap();

        assert_eq!(index.forget_chunks(&[cid(9)]).unwrap(), 2);
        assert_eq!(
            index
                .chunk_locations(&[cid(1), cid(2), cid(3)])
                .into_keys()
                .collect::<HashSet<_>>(),
            [cid(3)].into()
        );
    }

    /// The module's rule, applied to the new table: every malformed shape is
    /// **absence**, which costs a re-upload of bytes that are already there and
    /// never a wrong "already present".
    #[test]
    fn a_malformed_row_reads_as_absent_rather_than_being_materialised() {
        for damage in [
            "UPDATE chunk SET \"offset\" = -1",
            "UPDATE chunk SET clen = -1",
            "UPDATE chunk SET plen = -1",
            "UPDATE chunk SET pack = substr(pack, 1, 31)",
            "UPDATE chunk SET id = substr(id, 1, 31)",
        ] {
            let dir = TempDir::new().unwrap();
            let index = open(&dir);
            index.record_chunks(&[(cid(1), cid(9), 0, 10, 8)]).unwrap();
            index.conn.execute(damage, []).unwrap();
            assert!(
                index.chunk_locations(&[cid(1)]).is_empty(),
                "{damage} must read as absent"
            );
        }
    }

    /// A chunk table this build cannot read must be thrown away at **open**,
    /// where the answer can still be "discard" — never mid-push, where it
    /// cannot.
    #[test]
    fn a_chunk_table_with_the_wrong_columns_is_discarded_at_open() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index.sqlite3");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("CREATE TABLE chunk (id BLOB PRIMARY KEY, junk TEXT)")
                .unwrap();
        }
        let index = Index::at(&path).unwrap();
        assert!(index.was_rebuilt());
        // …and both halves work on the rebuilt file, rather than erroring.
        index.record_chunks(&[(cid(1), cid(9), 0, 10, 8)]).unwrap();
        assert!(index.chunk_locations(&[cid(1)]).contains_key(&cid(1)));
    }

    #[test]
    fn a_missing_chunk_table_reads_as_nothing_known() {
        let dir = TempDir::new().unwrap();
        let index = open(&dir);
        index.record_chunks(&[(cid(1), cid(9), 0, 10, 8)]).unwrap();
        index.conn.execute("DROP TABLE chunk", []).unwrap();
        assert!(index.chunk_locations(&[cid(1)]).is_empty());
    }

    // ---- 5-07: the two index-recovery escapes -------------------------

    /// `--rebuild-index` on a warm index: the rows are gone, the file is back,
    /// and it is private again. A recovery that left the database group- or
    /// world-readable would be a leak created by the repair.
    #[test]
    fn reset_at_replaces_a_populated_index_with_an_empty_private_one() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index.sqlite3");
        let index = Index::at(&path).unwrap();
        let file = sample(&dir);
        index.record(&file, 1, &ids(2)).unwrap();
        index
            .set_last_sync(
                DateTime::parse_from_rfc3339("2026-08-19T12:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
            )
            .unwrap();
        assert!(index.lookup(&file).is_some(), "the fixture is warm");
        drop(index);

        let fresh = reset_at(&path).unwrap();
        assert!(
            fresh.lookup(&file).is_none(),
            "every row went with the file"
        );
        assert!(fresh.last_sync().is_none());
        assert_eq!(fresh.generation(), 0);
        assert_eq!(fresh.meta::<i64>("schema_version"), Some(SCHEMA_VERSION));
        #[cfg(unix)]
        assert_private(&path);
    }

    /// Absent is not an error — a user who deleted the file by hand and then
    /// passed the flag has asked for the state they are already in.
    #[test]
    fn reset_at_on_a_path_that_does_not_exist_is_the_same_outcome() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("never").join("index.sqlite3");
        let fresh = reset_at(&path).unwrap();
        assert!(path.exists());
        assert!(fresh.last_sync().is_none());
    }

    /// T-5-64's sibling: the escape removes a *file*. A directory is named and
    /// refused, never walked — this function must not be a route to `rm -r`.
    #[test]
    fn reset_at_refuses_a_directory_and_removes_nothing() {
        let dir = TempDir::new().unwrap();
        let victim = dir.path().join("index.sqlite3");
        std::fs::create_dir_all(victim.join("something-precious")).unwrap();

        let Err(err) = reset_at(&victim) else {
            panic!("a directory is not a database");
        };
        assert!(err.to_string().contains("is a directory"), "{err}");
        assert!(
            victim.join("something-precious").is_dir(),
            "the tree survived the refusal"
        );
    }

    /// `--force-rehash`: every cached read misses, and **nothing is cleared**.
    /// A flag that also destroyed the cache would be more destructive than its
    /// name says (T-5-65), so the rows are asserted to survive by reopening.
    #[test]
    fn a_rehashing_index_misses_every_cached_read_and_keeps_every_row() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("index.sqlite3");
        let index = Index::at(&path).unwrap();
        let file = sample(&dir);
        index.record(&file, 1, &ids(2)).unwrap();
        assert!(index.lookup(&file).is_some());
        assert!(index.cached(&file.path).is_some());

        let rehashing = index.rehashing();
        assert!(rehashing.lookup(&file).is_none(), "the D5 tuple misses");
        assert!(
            rehashing.cached(&file.path).is_none(),
            "the append hint too"
        );
        drop(rehashing);

        let reopened = Index::at(&path).unwrap();
        assert!(
            reopened.lookup(&file).is_some(),
            "--force-rehash suppressed reads; it must not have deleted rows"
        );
    }

    /// Stamped with the run's generation in the same statement, exactly as
    /// `record` and `touch` are, so `evict_unseen` ages a chunk row on the same
    /// clock as a file row.
    #[test]
    fn chunk_rows_are_stamped_with_the_current_generation() {
        let dir = TempDir::new().unwrap();
        let index = open(&dir);
        index.bump_generation().unwrap();
        index.record_chunks(&[(cid(1), cid(9), 0, 10, 8)]).unwrap();

        assert_eq!(index.evict_unseen(1).unwrap(), 0, "written this generation");
        index.bump_generation().unwrap();
        index.bump_generation().unwrap();
        assert_eq!(index.evict_unseen(1).unwrap(), 1);
        assert!(index.chunk_locations(&[cid(1)]).is_empty());
    }
}
