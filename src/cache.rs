//! Per-vendor on-disk cache with atomic writes, TTL checks, and inter-process
//! locking.
//!
//! Mirrors claudebar's cache layout but per-vendor:
//!   `~/.cache/ai-usagebar/<vendor>/usage.json`         payload
//!   `~/.cache/ai-usagebar/<vendor>/.stale`             marker (cache is stale)
//!   `~/.cache/ai-usagebar/<vendor>/.last_error`        HTTP code\nmessage
//!   `~/.cache/ai-usagebar/<vendor>/.retry_after`       unix seconds; no network before this
//!   `~/.cache/ai-usagebar/<vendor>/.fetch.lock`        flock target
//!
//! Multi-monitor safety: callers should `acquire_lock()` before the refresh+
//! fetch window, mirroring claudebar:402-407's `exec 9>"$_lockfile" / flock`.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use fs2::FileExt;

use crate::error::{AUTH_FAILURE_MESSAGE, AppError, Result};

/// Default TTL — claudebar's `CACHE_TTL=60`.
pub const DEFAULT_TTL: Duration = Duration::from_secs(60);

/// Maximum staleness before we refuse to serve cached data even on failure.
/// Mirrors claudebar's `WEEKLY_WINDOW` (7 days).
pub const MAX_STALE: Duration = Duration::from_secs(7 * 24 * 3600);

/// How long a vendor is left alone after it answered HTTP 429. Without this
/// every 60 s poll re-hit a rate-limited endpoint, which only extends the
/// limit; five minutes is long enough for the common per-minute windows to
/// roll over and short enough that the bar recovers unattended.
pub const RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(5 * 60);

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
    /// Rate-limit backoff marker: unix epoch seconds (plain decimal text)
    /// before which no request should be made to this vendor.
    pub fn retry_after_path(&self) -> PathBuf {
        self.dir.join(".retry_after")
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
    ///
    /// This is the **one pre-network hook shared by every vendor**: each
    /// `fetch_snapshot` calls it before opening a connection and records HTTP
    /// failures through [`Cache::write_last_error`]. That makes this the single
    /// place a cross-vendor request policy can live without nineteen private
    /// copies drifting apart — which is why the rate-limit backoff is applied
    /// here rather than in each vendor.
    ///
    /// Policy, in order:
    /// 1. While a 429 backoff is armed ([`Cache::backoff_remaining_at`]), no
    ///    request is made. A payload still inside [`MAX_STALE`] is served as
    ///    the answer, TTL notwithstanding, so the bar keeps its last good figure
    ///    without touching the network. With nothing worth showing this returns
    ///    an [`AppError::Http`] with status 429 whose body names the time until
    ///    the next attempt; the vendor's `?` propagates it and the network is
    ///    never reached. `.last_error` is left as the vendor wrote it.
    /// 2. Otherwise the ordinary TTL check runs unchanged.
    pub fn fresh_payload(&self, ttl: Duration) -> Result<Option<Vec<u8>>> {
        self.fresh_payload_at(ttl, SystemTime::now())
    }

    /// [`Cache::fresh_payload`] with an injected clock for the backoff check.
    /// The TTL comparison still reads the payload's mtime against the real
    /// clock via [`Cache::payload_age`].
    pub fn fresh_payload_at(&self, ttl: Duration, now: SystemTime) -> Result<Option<Vec<u8>>> {
        if let Some(remaining) = self.backoff_remaining_at(now) {
            if self.payload_age().is_some_and(|age| age <= MAX_STALE) {
                return self.read_payload().map(Some);
            }
            return Err(AppError::Http {
                status: 429,
                body: format!("rate limited; next attempt in {}", human_backoff(remaining)),
            });
        }
        let Some(age) = self.payload_age() else {
            return Ok(None);
        };
        if age < ttl {
            self.read_payload().map(Some)
        } else {
            Ok(None)
        }
    }

    /// Arm the rate-limit backoff: no request until `now + RATE_LIMIT_BACKOFF`.
    /// Best-effort, never errors — a cache dir that cannot be written costs a
    /// retry, not a crash.
    pub fn note_rate_limit_at(&self, now: SystemTime) {
        let until = now + RATE_LIMIT_BACKOFF;
        let secs = until
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = atomic_write(&self.retry_after_path(), secs.to_string().as_bytes());
    }

    /// Best-effort removal of the backoff marker. A successful payload write
    /// and an explicit `clear_last_error` both end the backoff.
    pub fn clear_backoff(&self) {
        let _ = fs::remove_file(self.retry_after_path());
    }

    /// Time left on an armed backoff, as of `now`. `None` when the marker is
    /// missing, unparseable, or already in the past — a corrupt marker must
    /// never pin a vendor offline.
    pub fn backoff_remaining_at(&self, now: SystemTime) -> Option<Duration> {
        let raw = fs::read_to_string(self.retry_after_path()).ok()?;
        let secs = raw.trim().parse::<u64>().ok()?;
        let until = SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(secs))?;
        let remaining = until.duration_since(now).ok()?;
        if remaining.is_zero() {
            None
        } else {
            Some(remaining)
        }
    }

    /// [`Cache::backoff_remaining_at`] against the real clock.
    pub fn backoff_remaining(&self) -> Option<Duration> {
        self.backoff_remaining_at(SystemTime::now())
    }

    /// Read the payload regardless of age. `Err` if the file exists but is
    /// unreadable; `Ok(None)` if it just doesn't exist.
    ///
    /// Prefer [`Cache::fallback_payload`] on failure paths — this one imposes
    /// no age limit, so it will happily hand back a month-old figure.
    pub fn maybe_payload(&self) -> Result<Option<Vec<u8>>> {
        if !self.payload_path().exists() {
            return Ok(None);
        }
        self.read_payload().map(Some)
    }

    /// Payload for the *failure* path: the last good value, but only while it
    /// is still worth showing. Beyond `max_stale` this returns `Ok(None)` so
    /// the caller surfaces the real error instead of presenting week-old
    /// numbers as if they were current — a bar that silently freezes on
    /// history is worse than one that says it cannot reach the API.
    pub fn fallback_payload(&self, max_stale: Duration) -> Result<Option<Vec<u8>>> {
        let Some(age) = self.payload_age() else {
            return Ok(None);
        };
        if age > max_stale {
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
        // A successful write clears any stale marker, and a successful
        // response is proof the rate limit has lifted.
        let _ = fs::remove_file(self.stale_path());
        let _ = fs::remove_file(self.last_error_path());
        self.clear_backoff();
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

    /// Write the `.last_error` marker — first line `code`, everything after it
    /// `msg`. Best-effort, never errors (matches claudebar:478-486 which
    /// silently continues if the cache dir isn't writable).
    ///
    /// **Returns exactly what was written**, so a caller that also puts the
    /// failure in its [`crate::vendor::VendorOutcome`] can hand over this pair
    /// instead of deriving a second one from the raw body. The two must not be
    /// computed separately: persisting a redacted message while the in-memory
    /// copy kept the original is how a `401` body reached the widget tooltip on
    /// the one run that had a warm cache to fall back on. Callers that only
    /// persist can keep ignoring the return.
    pub fn write_last_error(&self, code: u16, msg: &str) -> (u16, String) {
        let _ = self.ensure_dir();
        let path = self.last_error_path();
        // Authentication failure bodies routinely include account identifiers or
        // partial credential details. Do not persist them; other status bodies
        // remain useful diagnostics after their usual control-char cleanup.
        let msg = if matches!(code, 401 | 403) {
            AUTH_FAILURE_MESSAGE
        } else {
            msg
        };
        let msg = crate::display::sanitize_untrusted_field(msg);
        let body = format!("{code}\n{msg}");
        let _ = atomic_write(&path, body.as_bytes());
        if code == 429 {
            self.note_rate_limit_at(SystemTime::now());
        }
        (code, msg)
    }

    /// Best-effort removal of the `.last_error` marker, and of the backoff
    /// that a 429 among those errors may have armed.
    pub fn clear_last_error(&self) {
        let _ = fs::remove_file(self.last_error_path());
        self.clear_backoff();
    }

    pub fn read_last_error(&self) -> Option<(u16, String)> {
        let raw = fs::read_to_string(self.last_error_path()).ok()?;
        // The message is *everything* past the first newline, not just the next
        // line: vendors store the raw HTTP body here and those are routinely
        // multi-line JSON, so taking one line truncated the user's diagnostic.
        // Files from before this fix parse unchanged — the writer always framed
        // them this way, only the reader threw the tail away.
        let (code, msg) = raw.split_once('\n').unwrap_or((raw.as_str(), ""));
        Some((code.parse::<u16>().ok()?, msg.to_string()))
    }
}

/// Render a backoff remainder for a tooltip: seconds below a minute, whole
/// minutes rounded *up* above it (`4m01s` reads as `5m` — promising less
/// wait than the real one would make the next poll look broken), and hours
/// once the minutes pass sixty (`1h 2m`, `1h`).
fn human_backoff(remaining: Duration) -> String {
    let secs = remaining.as_secs();
    if secs < 60 {
        return format!("{secs}s");
    }
    let minutes = secs.div_ceil(60);
    let (hours, minutes) = (minutes / 60, minutes % 60);
    match (hours, minutes) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h {m}m"),
    }
}

/// Acquire an exclusive flock on `path`, blocking up to `timeout`.
/// Returned guard releases the lock on drop.
///
/// The flock file is created if missing, but its content is unused — only
/// the lock matters.
/// Async wrapper around [`acquire_lock`].
///
/// The blocking version parks the calling thread in a sleep loop for up to
/// `timeout`. On a current-thread runtime — which is what the TUI uses — that
/// stalls *everything*: keyboard input, the refresh timer, and every other
/// vendor's in-flight request. Running the wait on the blocking pool keeps the
/// reactor free while a contended lock is waited on.
pub async fn acquire_lock_async(path: &Path, timeout: Duration) -> Result<LockGuard> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || acquire_lock(&path, timeout))
        .await
        .map_err(|e| AppError::Other(format!("cache lock task failed: {e}")))?
}

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

pub(crate) fn xdg_cache_dir() -> Result<PathBuf> {
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

    // ---- rate-limit backoff ------------------------------------------------

    /// A fixed instant well past the epoch so the arithmetic never underflows.
    fn t0() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)
    }

    fn arm_backoff_at(cache: &Cache, now: SystemTime) {
        cache.note_rate_limit_at(now);
        assert!(cache.retry_after_path().exists());
    }

    #[test]
    fn a_429_arms_the_backoff_and_other_statuses_do_not() {
        let (_td, cache) = fixture();
        cache.write_last_error(500, "upstream down");
        assert!(cache.backoff_remaining().is_none());
        assert!(!cache.retry_after_path().exists());

        cache.write_last_error(429, "slow down");
        let remaining = cache.backoff_remaining().expect("429 must arm the backoff");
        // Written against the real clock a moment ago: within a few seconds of
        // the full window, never above it.
        assert!(remaining <= RATE_LIMIT_BACKOFF, "{remaining:?}");
        assert!(
            remaining >= RATE_LIMIT_BACKOFF - Duration::from_secs(5),
            "{remaining:?}"
        );
        // The persisted `.last_error` is untouched by the backoff bookkeeping.
        assert_eq!(cache.read_last_error(), Some((429, "slow down".into())));
    }

    #[test]
    fn backoff_remaining_counts_down_from_the_injected_clock() {
        let (_td, cache) = fixture();
        arm_backoff_at(&cache, t0());

        assert_eq!(cache.backoff_remaining_at(t0()), Some(RATE_LIMIT_BACKOFF));
        assert_eq!(
            cache.backoff_remaining_at(t0() + Duration::from_secs(60)),
            Some(RATE_LIMIT_BACKOFF - Duration::from_secs(60))
        );
        // Exactly at expiry and beyond: no backoff.
        assert!(
            cache
                .backoff_remaining_at(t0() + RATE_LIMIT_BACKOFF)
                .is_none()
        );
        assert!(
            cache
                .backoff_remaining_at(t0() + RATE_LIMIT_BACKOFF + Duration::from_secs(1))
                .is_none()
        );
    }

    #[test]
    fn during_backoff_with_no_payload_fresh_payload_refuses_the_network() {
        let (_td, cache) = fixture();
        arm_backoff_at(&cache, t0());

        let err = cache
            .fresh_payload_at(DEFAULT_TTL, t0() + Duration::from_secs(19))
            .expect_err("no payload during backoff must be an error, not a fetch");
        match err {
            AppError::Http { status, body } => {
                assert_eq!(status, 429);
                assert!(body.contains("next attempt in"), "{body}");
                // 5m − 19s = 4m41s, rounded up to the next minute.
                assert!(body.ends_with("5m"), "{body}");
            }
            other => panic!("expected Http 429, got {other:?}"),
        }
    }

    /// The whole point of the backoff: a vendor that still has a figure keeps
    /// showing it instead of re-hitting the limit every poll. The payload is
    /// written now with a zero TTL, so the ordinary fast path would reject it
    /// as expired; only the backoff branch can be the one returning it.
    #[test]
    fn during_backoff_an_expired_but_not_stale_payload_is_served() {
        let (_td, cache) = fixture();
        cache.write_payload(b"last good").unwrap();
        arm_backoff_at(&cache, t0());

        // Control: without a backoff, TTL 0 means "not fresh".
        assert!(
            cache
                .fresh_payload_at(Duration::ZERO, t0() + RATE_LIMIT_BACKOFF)
                .unwrap()
                .is_none()
        );
        // Under backoff the same payload is the answer.
        assert_eq!(
            cache
                .fresh_payload_at(Duration::ZERO, t0())
                .unwrap()
                .as_deref(),
            Some(&b"last good"[..])
        );
    }

    #[test]
    fn after_the_backoff_expires_the_ttl_rule_is_back_in_charge() {
        let (_td, cache) = fixture();
        cache.write_payload(b"x").unwrap();
        arm_backoff_at(&cache, t0());
        let later = t0() + RATE_LIMIT_BACKOFF + Duration::from_secs(1);

        assert!(
            cache
                .fresh_payload_at(Duration::from_secs(10), later)
                .unwrap()
                .is_some()
        );
        assert!(
            cache
                .fresh_payload_at(Duration::ZERO, later)
                .unwrap()
                .is_none()
        );
        // And with no payload at all, expiry means a plain "go fetch".
        fs::remove_file(cache.payload_path()).unwrap();
        assert!(
            cache
                .fresh_payload_at(DEFAULT_TTL, later)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_successful_payload_write_clears_the_backoff() {
        let (_td, cache) = fixture();
        arm_backoff_at(&cache, t0());
        assert!(cache.backoff_remaining_at(t0()).is_some());

        cache.write_payload(b"fresh").unwrap();
        assert!(cache.backoff_remaining_at(t0()).is_none());
        assert!(!cache.retry_after_path().exists());
    }

    #[test]
    fn clear_last_error_also_clears_the_backoff() {
        let (_td, cache) = fixture();
        cache.write_last_error(429, "slow down");
        assert!(cache.backoff_remaining().is_some());

        cache.clear_last_error();
        assert!(cache.backoff_remaining().is_none());
        assert!(!cache.retry_after_path().exists());
    }

    #[test]
    fn a_corrupt_retry_after_marker_is_no_backoff() {
        let (_td, cache) = fixture();
        for raw in ["", "soon", "-5", "1e9", "12 34"] {
            fs::write(cache.retry_after_path(), raw).unwrap();
            assert!(
                cache.backoff_remaining_at(t0()).is_none(),
                "{raw:?} must not pin the vendor offline"
            );
            assert!(cache.fresh_payload_at(DEFAULT_TTL, t0()).unwrap().is_none());
        }
        // Surrounding whitespace is tolerated, though — a trailing newline is
        // the sort of thing a hand edit leaves behind.
        let until = t0() + Duration::from_secs(90);
        let secs = until
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        fs::write(cache.retry_after_path(), format!("{secs}\n")).unwrap();
        assert_eq!(
            cache.backoff_remaining_at(t0()),
            Some(Duration::from_secs(90))
        );
    }

    #[test]
    fn human_backoff_formats_seconds_minutes_and_hours() {
        let s = Duration::from_secs;
        assert_eq!(human_backoff(s(0)), "0s");
        assert_eq!(human_backoff(s(45)), "45s");
        assert_eq!(human_backoff(s(59)), "59s");
        assert_eq!(human_backoff(s(60)), "1m");
        assert_eq!(human_backoff(s(4 * 60)), "4m");
        assert_eq!(human_backoff(s(4 * 60 + 1)), "5m");
        assert_eq!(human_backoff(s(5 * 60)), "5m");
        assert_eq!(human_backoff(s(60 * 60)), "1h");
        assert_eq!(human_backoff(s(62 * 60)), "1h 2m");
        assert_eq!(human_backoff(s(61 * 60 + 30)), "1h 2m");
        assert_eq!(human_backoff(s(2 * 3600)), "2h");
    }

    #[test]
    fn fallback_payload_refuses_a_payload_older_than_the_limit() {
        let (_td, cache) = fixture();
        cache.write_payload(b"old").unwrap();

        // Let the payload acquire real age rather than rewriting its mtime:
        // Windows denies reopening the just-persisted file for an attribute
        // write, and the boundary being tested is the same either way. The
        // margin is ~12x the threshold so filesystem timestamp granularity
        // cannot make this flaky.
        std::thread::sleep(Duration::from_millis(60));

        // Still readable when age is not considered — `maybe_payload` is the
        // unbounded reader, which is exactly why failure paths must not use it.
        assert!(cache.maybe_payload().unwrap().is_some());

        // Past the limit, the failure path gets nothing and the caller has to
        // surface the real error. `MAX_STALE` was dead code before this:
        // every fallback served history forever.
        assert!(
            cache
                .fallback_payload(Duration::from_millis(5))
                .unwrap()
                .is_none()
        );

        // Inside the window it is still served, so the guard is a limit and
        // not a blanket refusal.
        assert_eq!(
            cache.fallback_payload(MAX_STALE).unwrap().as_deref(),
            Some(&b"old"[..])
        );
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
    fn last_error_replaces_401_body_with_credential_neutral_message() {
        let (_td, cache) = fixture();
        cache.write_last_error(401, "PANCEA user@example.test <credential>&token");

        let persisted = fs::read_to_string(cache.last_error_path()).unwrap();
        assert_eq!(persisted, format!("401\n{AUTH_FAILURE_MESSAGE}"));
        assert!(!persisted.contains("PANCEA"));
        assert!(!persisted.contains("<credential>"));
    }

    #[test]
    fn last_error_replaces_403_body_with_credential_neutral_message() {
        let (_td, cache) = fixture();
        cache.write_last_error(403, "PANCEA account@example.test <credential>&token");

        let persisted = fs::read_to_string(cache.last_error_path()).unwrap();
        assert_eq!(persisted, format!("403\n{AUTH_FAILURE_MESSAGE}"));
        assert!(!persisted.contains("PANCEA"));
        assert!(!persisted.contains("<credential>"));
    }

    /// The invariant that keeps the displayed message from drifting away from
    /// the persisted one: what comes back is what a later run would read from
    /// disk, so a caller that shows the return value cannot show anything the
    /// cache refused to keep. Asserted for the redacting arm and the ordinary
    /// one, since only the first rewrites the message.
    #[test]
    fn write_last_error_returns_exactly_what_a_later_run_would_read() {
        for (code, raw) in [
            (401u16, "PANCEA user@example.test <credential>&token"),
            (403, "PANCEA account@example.test <credential>&token"),
            (429, "rate limited, retry in 60s"),
            (500, "bad\x1b]52;c;Y2FuYXJ5\x07field"),
        ] {
            let (_td, cache) = fixture();
            let returned = cache.write_last_error(code, raw);
            assert_eq!(
                returned,
                cache.read_last_error().unwrap(),
                "returned pair diverged from the persisted one for {code}"
            );
        }
    }

    /// The defect recurred once under a second name — six vendors wrote
    /// `Some((status, body))` inline and six more built a `diag` local first —
    /// so the sweep that fixed the first six missed the rest. This forbids the
    /// shape rather than the spelling: a `last_error` pair must come from
    /// [`Cache::write_last_error`], which is the only thing that redacts.
    ///
    /// `error_to_pair` in `cursor`, `kimi` and `kiro` is untouched by this: it
    /// redacts on its own and destructures as `(*status, body)`, which is not
    /// the borrowed shape a leak takes.
    #[test]
    fn no_vendor_builds_a_last_error_pair_from_a_raw_http_body() {
        let mut sites = Vec::new();
        for file in crate::guard::rs_files_in("src") {
            if !file.ends_with("fetch.rs") {
                continue;
            }
            let source = std::fs::read_to_string(&file).expect("readable module");
            for (n, line) in crate::guard::production_code(&source).lines().enumerate() {
                if line.contains("(status, body") {
                    sites.push(format!("{}:{}", file.display(), n + 1));
                }
            }
        }
        assert!(
            sites.is_empty(),
            "a last_error pair must be the return of `write_last_error`, which \
             redacts 401/403 — building one from the raw body puts the response \
             body in the widget tooltip. Found: {sites:#?}"
        );
    }

    /// The cold-cache decision — serve a stale figure, or surface the error
    /// that caused the refresh to fail — is `outcome::fallback`'s alone. It
    /// drifted into two disagreeing generations once, when each vendor owned a
    /// copy: five replaced the original error with a generic "no usable cache"
    /// while thirteen returned it. `fallback_payload` is the entry point to
    /// that decision, so a second caller is a second copy in the making.
    #[test]
    fn only_the_shared_fallback_reads_the_stale_payload() {
        let mut sites = Vec::new();
        for file in crate::guard::rs_files_in("src") {
            if file.ends_with("outcome.rs") || file.ends_with("cache.rs") {
                continue;
            }
            let source = std::fs::read_to_string(&file).expect("readable module");
            for (n, line) in crate::guard::production_code(&source).lines().enumerate() {
                if line.contains("fallback_payload(") {
                    sites.push(format!("{}:{}", file.display(), n + 1));
                }
            }
        }
        assert!(
            sites.is_empty(),
            "reach the stale payload through `outcome::fallback`, which decides \
             what a cold cache means for every vendor at once. Found: {sites:#?}"
        );
    }

    /// The bug this closes: the pair handed to the widget was built from the
    /// raw body in parallel with the redacted one going to disk, so the run
    /// that hit the `401` showed the body and only the *next* run showed the
    /// neutral message. The returned pair carries the redaction.
    #[test]
    fn the_returned_pair_carries_the_auth_redaction() {
        for code in [401u16, 403] {
            let (_td, cache) = fixture();
            let (returned_code, msg) =
                cache.write_last_error(code, "PANCEA user@example.test <credential>&token");
            assert_eq!(returned_code, code);
            assert_eq!(msg, AUTH_FAILURE_MESSAGE);
            assert!(!msg.contains("PANCEA"), "{msg}");
            assert!(!msg.contains("<credential>"), "{msg}");
        }
    }

    /// The regression this guards: vendors write the raw HTTP body, which is
    /// usually multi-line JSON. The reader kept only line 2, so the tooltip
    /// showed `{` and dropped the actual API explanation.
    #[test]
    fn last_error_round_trips_a_multi_line_message() {
        let (_td, cache) = fixture();
        let body = "{\n  \"error\": \"quota exhausted\",\n  \"retry_after\": 3600\n}";
        cache.write_last_error(429, body);

        let (code, msg) = cache.read_last_error().unwrap();
        assert_eq!(code, 429);
        assert_eq!(msg, body);
        assert!(
            msg.contains("quota exhausted"),
            "message was truncated to its first line: {msg:?}"
        );
    }

    #[test]
    fn last_error_strips_terminal_controls_before_persisting() {
        let (_td, cache) = fixture();
        cache.write_last_error(500, "bad\x1b]52;c;Y2FuYXJ5\x07\nnext\tfield");

        let (code, msg) = cache.read_last_error().unwrap();
        assert_eq!(code, 500);
        assert_eq!(msg, "bad]52;c;Y2FuYXJ5\nnext field");
        assert!(
            msg.contains("Y2FuYXJ5"),
            "non-auth diagnostic was not preserved"
        );
        assert!(!msg.chars().any(|ch| ch.is_control() && ch != '\n'));
    }

    /// A user upgrades with a `.last_error` already on disk; it must still
    /// parse. Trailing-newline-free files (the whole marker being just a code)
    /// count too — that is the one shape the old `lines()` reader tolerated.
    #[test]
    fn last_error_reads_files_written_by_the_previous_version() {
        let (_td, cache) = fixture();

        fs::write(cache.last_error_path(), "503\nservice unavailable").unwrap();
        assert_eq!(
            cache.read_last_error(),
            Some((503, "service unavailable".into()))
        );

        fs::write(cache.last_error_path(), "429").unwrap();
        assert_eq!(cache.read_last_error(), Some((429, String::new())));

        // A non-numeric first line is still no error at all, never a fake 0.
        fs::write(cache.last_error_path(), "not-a-code\nboom").unwrap();
        assert!(cache.read_last_error().is_none());
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

    /// The regression this guards: `acquire_lock` parks the thread in a sleep
    /// loop, so on the TUI's current-thread runtime a contended lock froze
    /// keyboard input, the refresh timer and every other vendor's fetch until
    /// it timed out. `acquire_lock_async` moves the wait to the blocking pool,
    /// so unrelated timers must keep firing while the lock is held elsewhere.
    #[tokio::test(flavor = "current_thread")]
    async fn async_lock_does_not_stall_the_runtime() {
        let (_td, cache) = fixture();
        let lock_path = cache.lock_path();
        let _held = acquire_lock(&lock_path, Duration::from_millis(500)).unwrap();

        // This will wait the full timeout — it can never win the lock.
        let waiter = acquire_lock_async(&lock_path, Duration::from_millis(400));

        // Meanwhile the runtime must still be able to make progress.
        let mut ticks = 0usize;
        let ticker = async {
            let mut iv = tokio::time::interval(Duration::from_millis(20));
            iv.tick().await;
            loop {
                iv.tick().await;
                ticks += 1;
            }
        };

        tokio::select! {
            res = waiter => {
                // The lock attempt is expected to time out.
                assert!(matches!(res, Err(AppError::Other(_))));
            }
            _ = ticker => unreachable!("the ticker loops forever"),
        }
        assert!(
            ticks > 1,
            "runtime was starved while the lock was contended ({ticks} ticks)"
        );
    }

    #[test]
    fn atomic_write_creates_parent_dirs() {
        let td = TempDir::new().unwrap();
        let nested = td.path().join("a/b/c/file.txt");
        atomic_write(&nested, b"abc").unwrap();
        assert_eq!(fs::read(&nested).unwrap(), b"abc");
    }
}
