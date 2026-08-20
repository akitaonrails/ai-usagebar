//! Bounded selection of local Claude Code transcripts (D3). Owned by plan 2-04.
//!
//! Only the signature is fixed here, by plan 2-01, so [`super::scope::collect`]
//! can wire its `Transcripts` arm once and never be edited again.
//!
//! This machine holds 4.0 GB across 4110 `.jsonl` files. Unbounded, the category
//! does not fit a git remote; bounded, it fits and stays one code path. The
//! bounds select whole *files* by mtime — never an offset into one. A truncated
//! JSONL restores as a conversation that simply stops, which is worse than the
//! transcript being absent.

use chrono::{DateTime, TimeDelta, Utc};

use crate::config::{SyncCategory, SyncConfig};
use crate::sync::SyncRoots;
use crate::sync::scope::{self, CategoryScan, FileEntry};

/// Select `~/.claude/projects/**/*.jsonl` newest-first under both D3 bounds —
/// `transcript_days` and `transcript_max_bytes` — whichever binds first, whole
/// files only. Everything the bounds leave behind is counted into the scan's
/// `excluded_files` / `excluded_bytes` so the user is told what was dropped.
///
/// `now` is the age bound's reference point, passed in rather than read, so no
/// test here touches the wall clock.
pub fn collect_bounded(roots: &SyncRoots, cfg: &SyncConfig, now: DateTime<Utc>) -> CategoryScan {
    let mut scan = CategoryScan::empty(SyncCategory::Transcripts);
    // SCOPE-02/D1: off by default, and the default costs not one directory read.
    if !cfg.includes(SyncCategory::Transcripts) {
        return scan;
    }

    // The shared walker, so D2's exclusions — Cowork's `local-agent-mode-sessions`
    // among them — and the symlink refusal apply here without a second traversal.
    let mut walked = CategoryScan::empty(SyncCategory::Transcripts);
    scope::walk(&roots.claude_home.join("projects"), &mut walked);
    scan.walk_capped = walked.walk_capped;
    scan.skipped = walked.skipped;

    let mut entries: Vec<FileEntry> = walked
        .files
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|ext| ext == "jsonl"))
        .collect();
    // Newest first, path breaking the tie: a non-deterministic order would make
    // two consecutive dry-runs disagree about what the budget covered.
    entries.sort_by(|a, b| {
        b.mtime_ns
            .cmp(&a.mtime_ns)
            .then_with(|| a.path.cmp(&b.path))
    });

    let cutoff = cutoff_ns(now, cfg.transcript_days);
    // Once the budget refuses an in-window file we stop, rather than back-filling
    // with smaller older ones: the user asked for a working window, not a knapsack.
    let mut budget_bound = false;
    for entry in entries {
        let in_window = entry.mtime_ns >= cutoff;
        let fits = scan.bytes.saturating_add(entry.size) <= cfg.transcript_max_bytes;
        if in_window && fits && !budget_bound {
            scan.bytes += entry.size;
            scan.files.push(entry);
            continue;
        }
        budget_bound |= in_window && !fits;
        scan.excluded_files += 1;
        scan.excluded_bytes += entry.size;
    }
    scan
}

/// `now - days`, in the same nanoseconds-since-epoch units `FileEntry.mtime_ns`
/// carries. A `transcript_days` so large the subtraction leaves the
/// representable range means "no age bound", not a panic.
fn cutoff_ns(now: DateTime<Utc>, days: u32) -> i128 {
    TimeDelta::try_days(i64::from(days))
        .and_then(|d| now.checked_sub_signed(d))
        .map_or(i128::MIN, |t| {
            i128::from(t.timestamp()) * 1_000_000_000 + i128::from(t.timestamp_subsec_nanos())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::scope::collect;
    use chrono::TimeZone;
    use std::fs;
    use std::path::PathBuf;
    use std::time::SystemTime;
    use tempfile::TempDir;

    /// A fixed reference point. Nothing in this module reads the wall clock.
    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 19, 12, 0, 0).unwrap()
    }

    fn roots_at(dir: &TempDir) -> SyncRoots {
        SyncRoots::at(
            dir.path().join("config.toml"),
            dir.path().to_path_buf(),
            dir.path().join("desktop"),
            dir.path().join("profiles"),
            dir.path().join("claude-home"),
        )
    }

    fn enabled() -> SyncConfig {
        SyncConfig {
            categories: vec![SyncCategory::Transcripts],
            ..SyncConfig::default()
        }
    }

    /// Seed one file under `~/.claude/projects` with an explicit mtime.
    fn seed(dir: &TempDir, rel: &str, len: usize, days_old: i64) -> PathBuf {
        let path = dir.path().join("claude-home").join("projects").join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "x".repeat(len)).unwrap();
        let mtime = SystemTime::from(now() - TimeDelta::days(days_old));
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(mtime))
            .unwrap();
        path
    }

    fn names(scan: &CategoryScan) -> Vec<String> {
        let mut v: Vec<String> = scan
            .files
            .iter()
            .map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn the_default_config_leaves_transcripts_out_entirely() {
        let dir = TempDir::new().unwrap();
        seed(&dir, "proj/a.jsonl", 100, 1);

        let cfg = SyncConfig::default();
        assert!(!cfg.includes(SyncCategory::Transcripts));
        let scan = collect_bounded(&roots_at(&dir), &cfg, now());

        assert!(scan.files.is_empty());
        assert_eq!(scan.bytes, 0);
        // A walk that had happened and then been filtered would have counted the
        // file as excluded; zero here means the tree was never read.
        assert_eq!(scan.excluded_files, 0);
        assert_eq!(scan.excluded_bytes, 0);
    }

    #[test]
    fn a_transcript_inside_the_day_window_is_selected() {
        let dir = TempDir::new().unwrap();
        seed(&dir, "proj/recent.jsonl", 100, 1);

        let scan = collect_bounded(&roots_at(&dir), &enabled(), now());
        assert_eq!(names(&scan), vec!["recent.jsonl"]);
        assert_eq!(scan.bytes, 100);
        assert_eq!(scan.excluded_files, 0);
    }

    #[test]
    fn the_age_bound_binds_first_when_the_byte_budget_is_ample() {
        let dir = TempDir::new().unwrap();
        seed(&dir, "proj/recent.jsonl", 100, 29);
        seed(&dir, "proj/stale.jsonl", 100, 31);

        // Budget large enough for both; only the age bound can cut here.
        let scan = collect_bounded(&roots_at(&dir), &enabled(), now());
        assert_eq!(names(&scan), vec!["recent.jsonl"]);
        assert_eq!(scan.bytes, 100);
        assert_eq!(scan.excluded_files, 1);
        assert_eq!(scan.excluded_bytes, 100);
    }

    #[test]
    fn the_byte_bound_binds_first_when_every_file_is_inside_the_window() {
        let dir = TempDir::new().unwrap();
        seed(&dir, "proj/newest.jsonl", 100, 1);
        seed(&dir, "proj/middle.jsonl", 100, 2);
        seed(&dir, "proj/oldest.jsonl", 100, 3);

        let cfg = SyncConfig {
            transcript_max_bytes: 250,
            ..enabled()
        };
        let scan = collect_bounded(&roots_at(&dir), &cfg, now());

        assert_eq!(names(&scan), vec!["middle.jsonl", "newest.jsonl"]);
        assert_eq!(scan.bytes, 200);
        assert!(scan.bytes <= cfg.transcript_max_bytes);
        assert_eq!(scan.excluded_files, 1);
        assert_eq!(scan.excluded_bytes, 100);
    }

    #[test]
    fn a_file_bigger_than_the_whole_budget_is_dropped_not_truncated() {
        let dir = TempDir::new().unwrap();
        seed(&dir, "proj/huge.jsonl", 500, 1);

        let cfg = SyncConfig {
            transcript_max_bytes: 250,
            ..enabled()
        };
        let scan = collect_bounded(&roots_at(&dir), &cfg, now());

        assert!(scan.files.is_empty());
        assert_eq!(scan.bytes, 0);
        assert_eq!(scan.excluded_files, 1);
        assert_eq!(scan.excluded_bytes, 500);
    }

    #[test]
    fn the_budget_stops_the_selection_rather_than_back_filling_smaller_files() {
        let dir = TempDir::new().unwrap();
        seed(&dir, "proj/newest.jsonl", 200, 1);
        seed(&dir, "proj/older-small.jsonl", 10, 2);

        let cfg = SyncConfig {
            transcript_max_bytes: 100,
            ..enabled()
        };
        let scan = collect_bounded(&roots_at(&dir), &cfg, now());

        assert!(scan.files.is_empty(), "{:?}", names(&scan));
        assert_eq!(scan.excluded_files, 2);
        assert_eq!(scan.excluded_bytes, 210);
    }

    #[test]
    fn files_with_the_same_mtime_are_ordered_by_path_so_two_runs_agree() {
        let dir = TempDir::new().unwrap();
        seed(&dir, "proj/a.jsonl", 100, 1);
        seed(&dir, "proj/b.jsonl", 100, 1);

        let cfg = SyncConfig {
            transcript_max_bytes: 100,
            ..enabled()
        };
        let first = collect_bounded(&roots_at(&dir), &cfg, now());
        let second = collect_bounded(&roots_at(&dir), &cfg, now());

        assert_eq!(names(&first), vec!["a.jsonl"]);
        assert_eq!(names(&first), names(&second));
    }

    #[test]
    fn a_non_jsonl_file_under_the_projects_tree_is_not_selected() {
        let dir = TempDir::new().unwrap();
        seed(&dir, "proj/notes.md", 100, 1);
        seed(&dir, "proj/kept.jsonl", 100, 1);

        let scan = collect_bounded(&roots_at(&dir), &enabled(), now());
        assert_eq!(names(&scan), vec!["kept.jsonl"]);
        // Not a bound's doing, so it is not reported as bounded-out either.
        assert_eq!(scan.excluded_files, 0);
    }

    #[test]
    fn a_missing_projects_root_is_an_empty_scan_not_an_error() {
        let dir = TempDir::new().unwrap();
        let scan = collect_bounded(&roots_at(&dir), &enabled(), now());
        assert!(scan.files.is_empty());
        assert_eq!(scan.bytes, 0);
        assert!(!scan.walk_capped);
    }

    /// D2: Cowork transcript paths embed the owning account UUID plus an
    /// unreconstructable `ou-` suffix, so a copy renders as an empty chat. Also
    /// proves the `scope::collect` arm routes here — the only way in.
    #[test]
    fn cowork_sessions_are_never_selected_even_with_transcripts_enabled() {
        let dir = TempDir::new().unwrap();
        seed(&dir, "local-agent-mode-sessions/ou-abc/a.jsonl", 100, 1);
        seed(&dir, "proj/kept.jsonl", 100, 1);

        let scan = collect(
            SyncCategory::Transcripts,
            &roots_at(&dir),
            &enabled(),
            now(),
        );
        assert_eq!(scan.category, SyncCategory::Transcripts);
        assert_eq!(names(&scan), vec!["kept.jsonl"]);
        assert_eq!(scan.excluded_files, 0);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_transcript_is_refused_by_the_shared_walker() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let secret = outside.path().join("elsewhere.jsonl");
        fs::write(&secret, "x".repeat(100)).unwrap();
        let projects = dir.path().join("claude-home").join("projects").join("proj");
        fs::create_dir_all(&projects).unwrap();
        symlink(&secret, projects.join("linked.jsonl")).unwrap();

        let scan = collect_bounded(&roots_at(&dir), &enabled(), now());
        assert!(scan.files.is_empty(), "{:?}", names(&scan));
    }

    #[test]
    fn an_absurd_day_bound_widens_the_window_instead_of_panicking() {
        let dir = TempDir::new().unwrap();
        seed(&dir, "proj/ancient.jsonl", 100, 3650);

        let cfg = SyncConfig {
            transcript_days: u32::MAX,
            ..enabled()
        };
        let scan = collect_bounded(&roots_at(&dir), &cfg, now());
        assert_eq!(names(&scan), vec!["ancient.jsonl"]);
    }
}
