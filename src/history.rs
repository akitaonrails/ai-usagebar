//! Bounded local history for the compact trend sparkline in `usage --json`.
//!
//! Only an explicitly requested trend touches the file. The key is a digest
//! of the entry and metric, and each series keeps the reset of its window; no
//! account IDs or labels are stored.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAX_POINTS: usize = 20;
const SPARK_POINTS: usize = 12;
const MIN_INTERVAL_SECS: i64 = 10;
const MAX_AGE_SECS: i64 = 8 * 24 * 3600;
const MAX_SERIES: usize = 256;
const MAX_BYTES: u64 = 256 * 1024;

const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct History {
    #[serde(default)]
    series: BTreeMap<String, Series>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Series {
    /// Reset instant of the window the points belong to, in seconds.
    #[serde(default)]
    reset: Option<i64>,
    #[serde(default)]
    points: Vec<Point>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Point {
    at: i64,
    pct: u16,
}

/// Keep load, update and save under the same lock. If history is busy, the
/// caller can still render pace without reading or writing unlocked samples.
pub(crate) fn with_at(dir: &Path, now: DateTime<Utc>, update: impl FnOnce(&mut History)) -> bool {
    let lock_path = dir.join("history.lock");
    match std::fs::symlink_metadata(&lock_path) {
        Ok(meta) if meta.is_file() => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        _ => return false,
    }
    let Ok(_guard) = crate::cache::acquire_lock(&lock_path, Duration::ZERO) else {
        return false;
    };
    let path = dir.join("history.json");
    let mut history = History::load(&path);
    history.prune(now);
    update(&mut history);
    if let Ok(bytes) = serde_json::to_vec(&history)
        && bytes.len() as u64 <= MAX_BYTES
    {
        let _ = crate::cache::atomic_write(&path, &bytes);
    }
    true
}

impl History {
    fn load(path: &Path) -> Self {
        let read = || {
            if !std::fs::symlink_metadata(path).ok()?.is_file() {
                return None;
            }
            let mut bytes = Vec::new();
            std::fs::File::open(path)
                .ok()?
                .take(MAX_BYTES + 1)
                .read_to_end(&mut bytes)
                .ok()?;
            (bytes.len() as u64 <= MAX_BYTES)
                .then(|| serde_json::from_slice(&bytes).ok())
                .flatten()
        };
        read().unwrap_or_default()
    }

    /// Record one sample and return the two renderings of the same series.
    ///
    /// Samples closer together than [`MIN_INTERVAL_SECS`] replace the previous
    /// point instead of stacking, so a frontend that opens the menu twice does
    /// not turn the trend into a flat wall.
    ///
    /// A new window clears the series. The reset alone does not identify the
    /// window: vendors report it with sub-second jitter and a rolling window
    /// slides it forward, so only a move past
    /// [`crate::notify::RESET_MOVE_TOLERANCE_SECS`] from the last reading, or
    /// a reset that appears or disappears, starts over.
    pub(crate) fn record(
        &mut self,
        key: String,
        reset: Option<DateTime<Utc>>,
        pct: u16,
        now: DateTime<Utc>,
    ) -> Option<(String, Vec<u16>)> {
        if !self.series.contains_key(&key)
            && self.series.len() >= MAX_SERIES
            && let Some(oldest) = self.oldest_key()
        {
            self.series.remove(&oldest);
        }
        let series = self.series.entry(key).or_default();
        let reset = reset.map(|reset| reset.timestamp());
        let new_window = match (series.reset, reset) {
            (Some(last), Some(reset)) => {
                reset.abs_diff(last) > crate::notify::RESET_MOVE_TOLERANCE_SECS.unsigned_abs()
            }
            (last, reset) => last != reset,
        };
        if new_window {
            series.points.clear();
        }
        series.reset = reset;
        let at = now.timestamp();
        let pct = pct.min(100);
        if let Some(last) = series.points.last_mut()
            && at - last.at < MIN_INTERVAL_SECS
        {
            last.pct = pct;
        } else {
            series.points.push(Point { at, pct });
            if series.points.len() > MAX_POINTS {
                series.points.remove(0);
            }
        }
        if series.points.len() < 3 {
            return None;
        }
        let start = series.points.len().saturating_sub(SPARK_POINTS);
        let points: Vec<_> = series.points[start..]
            .iter()
            .map(|point| point.pct)
            .collect();
        Some((spark(&points), points))
    }

    fn prune(&mut self, now: DateTime<Utc>) {
        let upper = now.timestamp();
        let cutoff = upper - MAX_AGE_SECS;
        self.series.retain(|key, series| {
            series
                .points
                .retain(|point| (cutoff..=upper).contains(&point.at) && point.pct <= 100);
            series.points.sort_unstable_by_key(|point| point.at);
            series.points.dedup_by_key(|point| point.at);
            let excess = series.points.len().saturating_sub(MAX_POINTS);
            series.points.drain(..excess);
            key.len() == 64
                && key.bytes().all(|byte| byte.is_ascii_hexdigit())
                && !series.points.is_empty()
        });
        while self.series.len() > MAX_SERIES {
            let Some(oldest) = self.oldest_key() else {
                break;
            };
            self.series.remove(&oldest);
        }
    }

    fn oldest_key(&self) -> Option<String> {
        self.series
            .iter()
            .min_by_key(|(_, series)| series.points.last().map(|point| point.at))
            .map(|(key, _)| key.clone())
    }
}

fn spark(points: &[u16]) -> String {
    let min = points.iter().copied().min().unwrap_or_default();
    let max = points.iter().copied().max().unwrap_or_default();
    let span = max - min;
    points
        .iter()
        .map(|&point| {
            // A flat series is a baseline, not a half-height bar.
            if span == 0 {
                return BLOCKS[0];
            }
            let level = usize::from(point - min) * (BLOCKS.len() - 1) / usize::from(span);
            BLOCKS[level]
        })
        .collect()
}

pub(crate) fn metric_key(id: &str, group: Option<&str>, label: &str) -> String {
    use std::fmt::Write as _;

    let mut digest = Sha256::new();
    // Length-prefix textual fields so data containing separators cannot
    // produce the same key as a differently partitioned metric.
    for field in [Some(id), group, Some(label)] {
        if let Some(text) = field {
            digest.update([1]);
            digest.update((text.len() as u64).to_le_bytes());
            digest.update(text.as_bytes());
        } else {
            digest.update([0]);
        }
    }
    let mut key = String::with_capacity(64);
    for byte in digest.finalize() {
        let _ = write!(key, "{byte:02x}");
    }
    key
}

pub(crate) fn default_dir() -> Option<PathBuf> {
    crate::cache::xdg_cache_dir()
        .ok()
        .map(|dir| dir.join("ai-usagebar"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(seconds, 0).unwrap()
    }

    fn key(series: i64) -> String {
        metric_key(&format!("account {series}"), None, "Weekly")
    }

    fn reset(seconds: i64) -> Option<DateTime<Utc>> {
        Some(at(seconds))
    }

    #[test]
    fn three_samples_make_a_sparkline() {
        let mut history = History::default();
        assert!(history.record(key(1), reset(1), 10, at(0)).is_none());
        assert!(history.record(key(1), reset(1), 20, at(60)).is_none());
        let (spark, points) = history.record(key(1), reset(1), 30, at(120)).unwrap();
        assert_eq!(points, [10, 20, 30]);
        assert_eq!(spark.chars().count(), 3);
        assert_eq!(spark.chars().last(), Some('█'));
    }

    #[test]
    fn a_new_reset_starts_a_new_series() {
        let mut history = History::default();
        for (i, pct) in [10, 20, 30].into_iter().enumerate() {
            history.record(key(1), reset(1), pct, at(i as i64 * 60));
        }
        assert!(history.record(key(1), reset(18_001), 5, at(300)).is_none());
        assert_eq!(history.series[&key(1)].points.len(), 1);
        assert!(history.record(key(1), None, 6, at(360)).is_none());
        assert_eq!(history.series[&key(1)].points.len(), 1);
        assert_ne!(metric_key("a|b", None, "c"), metric_key("a", None, "b|c"));
        assert_ne!(metric_key("a", None, "c"), metric_key("a", Some(""), "c"));
    }

    #[test]
    fn a_jittered_or_sliding_reset_keeps_the_series() {
        let mut history = History::default();
        let first = at(18_000);
        history.record(key(1), Some(first), 10, at(0));
        let jittered = first + chrono::Duration::milliseconds(700);
        history.record(key(1), Some(jittered), 20, at(60));
        // A rolling window slides its reset with the refresh interval.
        let slid = first + chrono::Duration::minutes(5);
        let (_, points) = history.record(key(1), Some(slid), 30, at(120)).unwrap();
        assert_eq!(points, [10, 20, 30]);
    }

    #[test]
    fn samples_inside_the_interval_replace_the_last_point() {
        let mut history = History::default();
        history.record(key(1), reset(1), 10, at(0));
        history.record(key(1), reset(1), 80, at(5));
        history.record(key(1), reset(1), 90, at(60));
        let (spark, points) = history.record(key(1), reset(1), 100, at(120)).unwrap();
        assert_eq!(points, [80, 90, 100]);
        assert_eq!(spark.chars().count(), 3);
        assert_eq!(spark.chars().last(), Some('█'));
    }

    #[test]
    fn a_flat_series_is_a_baseline() {
        let mut history = History::default();
        let mut spark = None;
        for i in 0..3 {
            spark = history.record(key(1), reset(1), 27, at(i * 60));
        }
        assert_eq!(spark.unwrap().0, "▁▁▁");
    }

    #[test]
    fn the_series_is_capped() {
        let mut history = History::default();
        for i in 0..(MAX_POINTS + 5) {
            history.record(key(1), reset(1), 50, at(i as i64 * 60));
        }
        let series = history.series.get(&key(1)).unwrap();
        assert_eq!(series.points.len(), MAX_POINTS);
        for i in 2..(MAX_SERIES as i64 + 5) {
            history.record(key(i), reset(1), 50, at(10_000 + i));
        }
        assert_eq!(history.series.len(), MAX_SERIES);
        assert!(!history.series.contains_key(&key(1)));
    }

    #[test]
    fn contention_never_opens_or_writes_history() {
        let dir = tempfile::TempDir::new().unwrap();
        let held =
            crate::cache::acquire_lock(&dir.path().join("history.lock"), Duration::ZERO).unwrap();
        assert!(!with_at(dir.path(), at(0), |_| panic!(
            "cannot update without lock"
        )));
        assert!(!dir.path().join("history.json").exists());
        drop(held);
        assert!(with_at(dir.path(), at(0), |_| {}));
        assert!(dir.path().join("history.json").is_file());
    }

    #[test]
    fn bounded_file_and_retention() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.json");
        for invalid in [b"not json".to_vec(), vec![b' '; MAX_BYTES as usize + 1]] {
            std::fs::write(&path, invalid).unwrap();
            assert!(History::load(&path).series.is_empty());
        }
        let key = metric_key("private account", Some("private group"), "private metric");
        assert!(with_at(dir.path(), at(0), |history| {
            history.record(key.clone(), None, 10, at(0));
        }));
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.contains(&key));
        assert!(!contents.contains("private"));
        assert!(with_at(dir.path(), at(MAX_AGE_SECS + 1), |history| {
            assert!(history.series.is_empty());
        }));
    }
}
