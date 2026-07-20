//! Active-vendor state file. Set by `--cycle-next` / `--cycle-prev` (which
//! Waybar's `on-scroll-up`/`on-scroll-down` invoke), read by the widget on
//! every tick. The TUI does NOT consult this — it has its own tab state.
//!
//! On-disk shape: a single line with the vendor slug (e.g. `openai`). Located
//! at `<cache-dir>/active_vendor`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::cache::{acquire_lock, atomic_write};
use crate::error::{AppError, Result};
use crate::vendor::VendorId;

/// Much shorter than the vendors' fetch locks (15–45s): the critical section
/// here is one small read plus a rename, so a wait this long already means a
/// wedged holder — and a scroll event that blocks for seconds is worse than a
/// dropped one.
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);

fn state_dir() -> Result<PathBuf> {
    let base = directories::BaseDirs::new()
        .ok_or_else(|| AppError::Other("could not resolve XDG cache dir".into()))?;
    Ok(base.cache_dir().join("ai-usagebar"))
}

fn state_path() -> Result<PathBuf> {
    Ok(state_dir()?.join("active_vendor"))
}

/// Read the persisted active vendor, if any. `None` means "no override —
/// callers fall back to [ui] primary or anthropic".
pub fn read() -> Option<VendorId> {
    read_from(&state_path().ok()?)
}

/// Read the persisted active vendor from an explicit path. The real-path
/// [`read`] is a thin wrapper over this; tests use this directly with a
/// `TempDir`-backed path so they never touch `~/.cache/ai-usagebar`.
pub fn read_from(path: &Path) -> Option<VendorId> {
    let raw = fs::read_to_string(path).ok()?;
    parse_slug(raw.trim())
}

/// Persist `vendor` as the active one. Atomic.
pub fn write(vendor: VendorId) -> Result<()> {
    write_to(&state_path()?, vendor)
}

/// Persist `vendor` to an explicit path. Atomic. Test-friendly counterpart
/// to [`write`] (mirrors [`crate::cache::Cache::at`] vs `for_vendor`).
pub fn write_to(path: &Path, vendor: VendorId) -> Result<()> {
    atomic_write(path, vendor.slug().as_bytes())
}

/// Cycle the active vendor by `delta` positions through `enabled` (which
/// preserves canonical order). Wraps. If no state exists, starts at `start`
/// (usually `[ui] primary` or anthropic).
pub fn cycle(enabled: &[VendorId], start: VendorId, delta: i32) -> Result<VendorId> {
    cycle_at(&state_path()?, enabled, start, delta)
}

/// The flock guarding a state file's read-modify-write, as a sibling of the
/// state file itself (mirroring `Cache::lock_path`'s `.fetch.lock`).
fn lock_path_for(state: &Path) -> PathBuf {
    let mut p = state.as_os_str().to_os_string();
    p.push(".lock");
    PathBuf::from(p)
}

/// Cycle using an explicit state-file path. The real-path [`cycle`] is a thin
/// wrapper over this; tests drive this with a `TempDir` path so the cycle +
/// persistence logic is covered without reading or writing the real cache.
pub fn cycle_at(
    path: &Path,
    enabled: &[VendorId],
    start: VendorId,
    delta: i32,
) -> Result<VendorId> {
    if enabled.is_empty() {
        return Err(AppError::Other("no enabled vendors to cycle".into()));
    }
    // One flick of a scroll wheel fires several `--cycle-next` processes at
    // once. An atomic *write* only guarantees no torn file — it does not stop
    // two of them reading the same current vendor and both persisting the same
    // next one, silently eating a step. The lock has to span read→compute→write.
    let _lock = acquire_lock(&lock_path_for(path), LOCK_TIMEOUT)?;
    let current = read_from(path)
        .filter(|v| enabled.contains(v))
        .unwrap_or(start);
    let cur_idx = enabled.iter().position(|v| *v == current).unwrap_or(0);
    let n = enabled.len() as i32;
    let next_idx = ((cur_idx as i32 + delta).rem_euclid(n)) as usize;
    let next = enabled[next_idx];
    write_to(path, next)?;
    Ok(next)
}

fn parse_slug(s: &str) -> Option<VendorId> {
    match s {
        "anthropic" => Some(VendorId::Anthropic),
        "openai" => Some(VendorId::Openai),
        "zai" => Some(VendorId::Zai),
        "openrouter" => Some(VendorId::Openrouter),
        "deepseek" => Some(VendorId::Deepseek),
        "kimi" => Some(VendorId::Kimi),
        "kilo" => Some(VendorId::Kilo),
        "novita" => Some(VendorId::Novita),
        "moonshot" => Some(VendorId::Moonshot),
        "grok" => Some(VendorId::Grok),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // Deliberately excludes Deepseek so the not-in-enabled-set fallback test
    // below has a real vendor to persist that is outside this cycle set.
    const CYCLE_SET: [VendorId; 4] = [
        VendorId::Anthropic,
        VendorId::Openai,
        VendorId::Zai,
        VendorId::Openrouter,
    ];

    /// One flick of a scroll wheel fires several `--cycle-next` processes at
    /// once. Before the lock spanned the read-modify-write, two of them could
    /// read the same current vendor and both persist the same next one, so N
    /// events advanced fewer than N steps. With the lock held across the whole
    /// operation, every cycle observes its predecessor's write.
    #[test]
    fn concurrent_cycles_do_not_lose_a_step() {
        let td = TempDir::new().unwrap();
        let path = td.path().join("active_vendor");
        let start = VendorId::Anthropic;

        // Four threads, each doing one +1 step, over a 4-vendor set: if no step
        // is lost the value returns exactly to `start`.
        const THREADS: usize = 4;
        std::thread::scope(|s| {
            for _ in 0..THREADS {
                s.spawn(|| {
                    let _ = cycle_at(&path, &CYCLE_SET, start, 1);
                });
            }
        });

        let landed = read_from(&path).expect("a vendor must have been persisted");
        assert_eq!(
            landed,
            start,
            "{THREADS} single steps over {} vendors must return to the start; \
             landing on {landed:?} means a step was lost to a race",
            CYCLE_SET.len()
        );
    }

    #[test]
    fn parse_slug_round_trip() {
        for id in VendorId::all() {
            assert_eq!(parse_slug(id.slug()), Some(*id));
        }
    }

    #[test]
    fn parse_slug_unknown_returns_none() {
        assert!(parse_slug("not-a-vendor").is_none());
        assert!(parse_slug("").is_none());
    }

    #[test]
    fn read_from_missing_or_garbage_returns_none() {
        let td = TempDir::new().unwrap();
        // Missing file → None.
        assert!(read_from(&td.path().join("active_vendor")).is_none());
        // Round-trip a real slug.
        let path = td.path().join("active_vendor");
        write_to(&path, VendorId::Zai).unwrap();
        assert_eq!(read_from(&path), Some(VendorId::Zai));
        // Garbage content → None (not a known slug).
        fs::write(&path, "not-a-vendor").unwrap();
        assert!(read_from(&path).is_none());
    }

    #[test]
    fn cycle_at_persists_state_across_calls() {
        let td = TempDir::new().unwrap();
        let path = td.path().join("active_vendor");

        // No state yet → starts at `start`, steps forward to Openai.
        let v = cycle_at(&path, &CYCLE_SET, VendorId::Anthropic, 1).unwrap();
        assert_eq!(v, VendorId::Openai);
        assert_eq!(read_from(&path), Some(VendorId::Openai));

        // Next forward step reads the persisted Openai → Zai.
        let v = cycle_at(&path, &CYCLE_SET, VendorId::Anthropic, 1).unwrap();
        assert_eq!(v, VendorId::Zai);
        assert_eq!(read_from(&path), Some(VendorId::Zai));
    }

    #[test]
    fn cycle_at_wraps_forward_and_backward() {
        let td = TempDir::new().unwrap();
        let path = td.path().join("active_vendor");
        write_to(&path, VendorId::Anthropic).unwrap();

        // backward from Anthropic wraps to Openrouter
        assert_eq!(
            cycle_at(&path, &CYCLE_SET, VendorId::Anthropic, -1).unwrap(),
            VendorId::Openrouter
        );
        // forward from Openrouter wraps back to Anthropic
        assert_eq!(
            cycle_at(&path, &CYCLE_SET, VendorId::Anthropic, 1).unwrap(),
            VendorId::Anthropic
        );
    }

    #[test]
    fn cycle_at_ignores_persisted_vendor_not_in_enabled_set() {
        let td = TempDir::new().unwrap();
        let path = td.path().join("active_vendor");
        // Persist a vendor that isn't in the enabled set → fall back to `start`.
        write_to(&path, VendorId::Deepseek).unwrap();
        let enabled = [VendorId::Anthropic, VendorId::Openai];
        // start=Openai (idx 1), +1 wraps to idx 0 = Anthropic.
        let v = cycle_at(&path, &enabled, VendorId::Openai, 1).unwrap();
        assert_eq!(v, VendorId::Anthropic);
    }

    #[test]
    fn cycle_at_errors_on_empty_enabled() {
        let td = TempDir::new().unwrap();
        let path = td.path().join("active_vendor");
        let res = cycle_at(&path, &[], VendorId::Anthropic, 1);
        assert!(matches!(res, Err(AppError::Other(_))));
    }
}
