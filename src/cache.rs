//! Per-vendor on-disk cache with atomic writes, TTL checks, and inter-process
//! locking.
//!
//! Mirrors claudebar's cache layout but per-vendor:
//!   `~/.cache/ai-usagebar/<vendor>/usage.json`         payload
//!   `~/.cache/ai-usagebar/<vendor>/.stale`             marker (cache is stale)
//!   `~/.cache/ai-usagebar/<vendor>/.last_error`        HTTP code\nmessage
//!   `~/.cache/ai-usagebar/<vendor>/.fetch.lock`        flock target
//!
//! Multi-monitor safety: callers should `acquire_lock()` before the refresh+
//! fetch window, mirroring claudebar:402-407's `exec 9>"$_lockfile" / flock`.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use fs2::FileExt;

use crate::error::{AppError, Result};

/// Default TTL — claudebar's `CACHE_TTL=60`.
pub const DEFAULT_TTL: Duration = Duration::from_secs(60);

/// Maximum staleness before we refuse to serve cached data even on failure.
/// Mirrors claudebar's `WEEKLY_WINDOW` (7 days).
pub const MAX_STALE: Duration = Duration::from_secs(7 * 24 * 3600);

/// Per-vendor cache directory and helper API.
///
/// Construct with [`Cache::for_vendor`]; the directory is created lazily.
#[derive(Debug, Clone)]
pub struct Cache {
    dir: PathBuf,
}

impl Cache {
    /// Build a cache rooted at `~/.cache/ai-usagebar/<vendor>` (or under
    /// `$XDG_CACHE_HOME` when set).
    pub fn for_vendor(vendor: &str) -> Result<Self> {
        let base = xdg_cache_dir()?.join("ai-usagebar").join(vendor);
        Ok(Self { dir: base })
    }

    /// Cache for a specific named account of a vendor, rooted at
    /// `~/.cache/ai-usagebar/<vendor>/<label>`. Only *extra* accounts use
    /// this; the default account keeps [`Cache::for_vendor`] so its path never
    /// moves (issue #14, back-compat rule 2).
    pub fn for_vendor_account(vendor: &str, label: &str) -> Result<Self> {
        let base = xdg_cache_dir()?
            .join("ai-usagebar")
            .join(vendor)
            .join(label);
        Ok(Self { dir: base })
    }

    /// Cache rooted at an arbitrary directory — for tests.
    pub fn at(path: PathBuf) -> Self {
        Self { dir: path }
    }

    /// Ensure the directory exists. Safe to call repeatedly.
    pub fn ensure_dir(&self) -> Result<()> {
        fs::create_dir_all(&self.dir).map_err(|e| AppError::io_at(&self.dir, e))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn payload_path(&self) -> PathBuf {
        self.dir.join("usage.json")
    }
    pub fn stale_path(&self) -> PathBuf {
        self.dir.join(".stale")
    }
    pub fn last_error_path(&self) -> PathBuf {
        self.dir.join(".last_error")
    }
    pub fn lock_path(&self) -> PathBuf {
        self.dir.join(".fetch.lock")
    }

    /// Age of the payload (`None` if it doesn't exist). Used by the widget to
    /// decide whether the 60s cache window applies.
    pub fn payload_age(&self) -> Option<Duration> {
        let meta = fs::metadata(self.payload_path()).ok()?;
        let mtime = meta.modified().ok()?;
        SystemTime::now().duration_since(mtime).ok()
    }

    /// Returns the cached payload only if it is younger than `ttl`. Used as
    /// the fast path in `_fetch_usage` (claudebar:343-349).
    pub fn fresh_payload(&self, ttl: Duration) -> Result<Option<Vec<u8>>> {
        let Some(age) = self.payload_age() else {
            return Ok(None);
        };
        if age < ttl {
            self.read_payload().map(Some)
        } else {
            Ok(None)
        }
    }

    /// Read the payload regardless of age. `Err` if the file exists but is
    /// unreadable; `Ok(None)` if it just doesn't exist.
    pub fn maybe_payload(&self) -> Result<Option<Vec<u8>>> {
        if !self.payload_path().exists() {
            return Ok(None);
        }
        self.read_payload().map(Some)
    }

    fn read_payload(&self) -> Result<Vec<u8>> {
        let p = self.payload_path();
        let mut f = File::open(&p).map_err(|e| AppError::io_at(&p, e))?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)
            .map_err(|e| AppError::io_at(&p, e))?;
        Ok(buf)
    }

    /// Atomically write a new payload. Uses `tempfile + persist` (POSIX
    /// rename), matching claudebar's `mktemp + mv` invariant.
    pub fn write_payload(&self, bytes: &[u8]) -> Result<()> {
        self.ensure_dir()?;
        let mut tmp = tempfile::Builder::new()
            .prefix(".usage.")
            .tempfile_in(&self.dir)
            .map_err(|e| AppError::io_at(&self.dir, e))?;
        tmp.write_all(bytes)
            .map_err(|e| AppError::io_at(tmp.path(), e))?;
        tmp.as_file_mut()
            .sync_all()
            .map_err(|e| AppError::io_at(tmp.path(), e))?;
        tmp.persist(self.payload_path())
            .map_err(|e| AppError::io_at(self.payload_path(), e.error))?;
        // A successful write clears any stale marker.
        let _ = fs::remove_file(self.stale_path());
        let _ = fs::remove_file(self.last_error_path());
        Ok(())
    }

    /// Mark the cache as stale. Idempotent.
    pub fn mark_stale(&self) {
        let _ = self.ensure_dir();
        let _ = File::create(self.stale_path());
    }

    pub fn is_stale(&self) -> bool {
        self.stale_path().exists()
    }

    /// Write the `.last_error` marker — first line `code`, second line `msg`.
    /// Best-effort, never errors (matches claudebar:478-486 which silently
    /// continues if the cache dir isn't writable).
    pub fn write_last_error(&self, code: u16, msg: &str) {
        let _ = self.ensure_dir();
        let path = self.last_error_path();
        let body = format!("{code}\n{msg}");
        let _ = atomic_write(&path, body.as_bytes());
    }

    /// Best-effort removal of the `.last_error` marker.
    pub fn clear_last_error(&self) {
        let _ = fs::remove_file(self.last_error_path());
    }

    pub fn read_last_error(&self) -> Option<(u16, String)> {
        let raw = fs::read_to_string(self.last_error_path()).ok()?;
        let mut lines = raw.lines();
        let code = lines.next()?.parse::<u16>().ok()?;
        let msg = lines.next().unwrap_or_default().to_string();
        Some((code, msg))
    }
}

/// Acquire an exclusive flock on `path`, blocking up to `timeout`.
/// Returned guard releases the lock on drop.
///
/// The flock file is created if missing, but its content is unused — only
/// the lock matters.
pub fn acquire_lock(path: &Path, timeout: Duration) -> Result<LockGuard> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| AppError::io_at(parent, e))?;
    }
    let f = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
        .map_err(|e| AppError::io_at(path, e))?;

    let deadline = std::time::Instant::now() + timeout;
    loop {
        match f.try_lock_exclusive() {
            Ok(()) => return Ok(LockGuard { file: f }),
            Err(_) => {
                if std::time::Instant::now() >= deadline {
                    return Err(AppError::Other(format!(
                        "cache lock timeout after {:?}",
                        timeout
                    )));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

/// Releases the flock on drop. Holding this across an `.await` is fine as
/// long as you don't move it across tasks (we always use it in `tokio::main`
/// on a single thread).
pub struct LockGuard {
    file: File,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

/// Atomic write helper used by `write_last_error`. Public for vendors that
/// need to write small sidecar files (credentials, etc.).
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().ok_or_else(|| {
        AppError::Other(format!(
            "atomic_write: path has no parent: {}",
            path.display()
        ))
    })?;
    fs::create_dir_all(dir).map_err(|e| AppError::io_at(dir, e))?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".tmp.")
        .tempfile_in(dir)
        .map_err(|e| AppError::io_at(dir, e))?;
    tmp.write_all(bytes)
        .map_err(|e| AppError::io_at(tmp.path(), e))?;
    tmp.as_file_mut()
        .sync_all()
        .map_err(|e| AppError::io_at(tmp.path(), e))?;
    tmp.persist(path)
        .map_err(|e| AppError::io_at(path, e.error))?;
    Ok(())
}

fn xdg_cache_dir() -> Result<PathBuf> {
    directories::BaseDirs::new()
        .map(|b| b.cache_dir().to_path_buf())
        .ok_or_else(|| AppError::Other("could not resolve XDG cache dir (no HOME?)".into()))
}

/// The user's home directory, resolved cross-platform via `directories`
/// (`$HOME` on Unix/macOS, `%USERPROFILE%` / the Known Folder on Windows).
///
/// The OAuth-credential vendors (`anthropic`, `openai`) read their CLI-managed
/// files from fixed dotfiles under `$HOME`; they share this resolver the same
/// way they already share [`atomic_write`], so home resolution lives in one
/// place rather than being reimplemented per vendor.
pub fn home_dir() -> Result<PathBuf> {
    directories::BaseDirs::new()
        .map(|b| b.home_dir().to_path_buf())
        .ok_or_else(|| AppError::Other("could not resolve home directory (no HOME?)".into()))
}

/// Test-only: a named file inside a fresh `TempDir` with **no open handle** on
/// it. [`atomic_write`] replaces its destination via rename, which on Windows
/// fails while the destination is held open (as a live `NamedTempFile` handle
/// would be) — so tests that exercise a write-back must target a closed file.
/// Returns the dir (the caller keeps it alive) and the file's path; the file
/// exists only when `contents` is given.
#[cfg(test)]
pub(crate) fn closed_temp_file(name: &str, contents: Option<&str>) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join(name);
    if let Some(c) = contents {
        std::fs::write(&path, c).unwrap();
    }
    (dir, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fixture() -> (TempDir, Cache) {
        let td = TempDir::new().unwrap();
        let cache = Cache::at(td.path().join("anthropic"));
        cache.ensure_dir().unwrap();
        (td, cache)
    }

    #[test]
    fn ensure_dir_is_idempotent() {
        let (_td, cache) = fixture();
        cache.ensure_dir().unwrap();
        cache.ensure_dir().unwrap();
        assert!(cache.dir().is_dir());
    }

    #[test]
    fn write_then_read_round_trip() {
        let (_td, cache) = fixture();
        cache.write_payload(b"hello world").unwrap();
        let got = cache.maybe_payload().unwrap();
        assert_eq!(got.as_deref(), Some(&b"hello world"[..]));
    }

    #[test]
    fn maybe_payload_returns_none_when_missing() {
        let (_td, cache) = fixture();
        assert!(cache.maybe_payload().unwrap().is_none());
    }

    #[test]
    fn fresh_payload_respects_ttl() {
        let (_td, cache) = fixture();
        cache.write_payload(b"x").unwrap();
        // Fresh = within a generous TTL.
        assert!(
            cache
                .fresh_payload(Duration::from_secs(10))
                .unwrap()
                .is_some()
        );
        // Force "stale" by passing a zero TTL — payload is older than 0s.
        assert!(
            cache
                .fresh_payload(Duration::from_secs(0))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn write_clears_stale_marker_and_last_error() {
        let (_td, cache) = fixture();
        cache.mark_stale();
        cache.write_last_error(429, "rate limited");
        assert!(cache.is_stale());
        assert!(cache.read_last_error().is_some());

        cache.write_payload(b"fresh").unwrap();
        assert!(!cache.is_stale());
        assert!(cache.read_last_error().is_none());
    }

    #[test]
    fn last_error_round_trip() {
        let (_td, cache) = fixture();
        cache.write_last_error(503, "service unavailable");
        let (code, msg) = cache.read_last_error().unwrap();
        assert_eq!(code, 503);
        assert_eq!(msg, "service unavailable");
    }

    #[test]
    fn last_error_with_empty_message_round_trips() {
        let (_td, cache) = fixture();
        cache.write_last_error(429, "");
        let (code, msg) = cache.read_last_error().unwrap();
        assert_eq!(code, 429);
        assert_eq!(msg, "");
    }

    #[test]
    fn lock_serializes_concurrent_acquirers() {
        // First lock succeeds; while held, a second non-blocking attempt
        // should time out quickly.
        let (_td, cache) = fixture();
        let lock_path = cache.lock_path();
        let _guard = acquire_lock(&lock_path, Duration::from_millis(500)).unwrap();

        let res = acquire_lock(&lock_path, Duration::from_millis(100));
        assert!(matches!(res, Err(AppError::Other(_))));
    }

    #[test]
    fn atomic_write_creates_parent_dirs() {
        let td = TempDir::new().unwrap();
        let nested = td.path().join("a/b/c/file.txt");
        atomic_write(&nested, b"abc").unwrap();
        assert_eq!(fs::read(&nested).unwrap(), b"abc");
    }
}
