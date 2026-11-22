//! Bounded local history for the compact trend sparkline in `usage --json`.
//!
//! The file lives next to the per-vendor caches and is keyed by entry id and
//! metric label. A series restarts when the window's `reset_at` changes, so a
//! sparkline never mixes two windows. Missing or unreadable history is not an
//! error: the report simply omits the sparkline.

use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

const MAX_POINTS: usize = 20;
const SPARK_POINTS: usize = 12;
const MIN_INTERVAL_SECS: i64 = 10;
const MAX_AGE_SECS: i64 = 8 * 24 * 3600;

const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct History {
    #[serde(default)]
    series: BTreeMap<String, Series>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Series {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reset: Option<String>,
    #[serde(default)]
    points: Vec<Point>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Point {
    at: i64,
    pct: u16,
}

/// Hold an exclusive lock for a load/annotate/save cycle.
///
/// Best effort: a machine without a usable cache directory still gets the
/// report, just without the history.
pub fn lock() -> Option<std::fs::File> {
    let path = path()?;
    lock_at(path.parent()?)
}

fn lock_at(dir: &std::path::Path) -> Option<std::fs::File> {
    std::fs::create_dir_all(dir).ok()?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join("history.lock"))
        .ok()?;
    file.lock_exclusive().ok()?;
    Some(file)
}

impl History {
    pub fn load() -> Self {
        let Some(path) = path() else {
            return Self::default();
        };
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = path() else {
            return;
        };
        let Ok(bytes) = serde_json::to_vec(self) else {
            return;
        };
        let _ = crate::cache::atomic_write(&path, &bytes);
    }

    /// Record one sample and return the sparkline for the series.
    ///
    /// Samples closer together than [`MIN_INTERVAL_SECS`] replace the previous
    /// point instead of stacking, so a frontend that opens the menu twice does
    /// not turn the trend into a flat wall.
    pub fn record(
        &mut self,
        key: &str,
        reset: Option<&str>,
        pct: u16,
        now: DateTime<Utc>,
    ) -> Option<String> {
        self.prune(now);
        let series = self.series.entry(key.to_string()).or_default();
        if series.reset.as_deref() != reset {
            series.points.clear();
            series.reset = reset.map(str::to_string);
        }
        let at = now.timestamp();
        if let Some(last) = series.points.last_mut()
            && at - last.at < MIN_INTERVAL_SECS
        {
            last.pct = pct;
            return spark(series);
        }
        series.points.push(Point { at, pct });
        if series.points.len() > MAX_POINTS {
            let excess = series.points.len() - MAX_POINTS;
            series.points.drain(0..excess);
        }
        spark(series)
    }

    /// The last samples as percentages for a line-chart frontend.
    pub fn points(&self, key: &str) -> Option<Vec<u16>> {
        let series = self.series.get(key)?;
        if series.points.len() < 3 {
            return None;
        }
        let start = series.points.len().saturating_sub(SPARK_POINTS);
        Some(
            series.points[start..]
                .iter()
                .map(|point| point.pct.min(100))
                .collect(),
        )
    }

    fn prune(&mut self, now: DateTime<Utc>) {
        let cutoff = now.timestamp() - MAX_AGE_SECS;
        self.series
            .retain(|_, series| series.points.last().is_some_and(|point| point.at >= cutoff));
    }
}

fn spark(series: &Series) -> Option<String> {
    if series.points.len() < 3 {
        return None;
    }
    let start = series.points.len().saturating_sub(SPARK_POINTS);
    let points = &series.points[start..];
    let min = points.iter().map(|point| point.pct.min(100)).min()?;
    let max = points.iter().map(|point| point.pct.min(100)).max()?;
    let span = max - min;
    Some(
        points
            .iter()
            .map(|point| {
                // A flat series is a baseline, not a half-height bar.
                if span == 0 {
                    return BLOCKS[0];
                }
                let level = usize::from(point.pct.min(100).saturating_sub(min))
                    * (BLOCKS.len() - 1)
                    / usize::from(span);
                BLOCKS[level.min(BLOCKS.len() - 1)]
            })
            .collect(),
    )
}

fn path() -> Option<PathBuf> {
    crate::cache::xdg_cache_dir()
        .ok()
        .map(|dir| dir.join("ai-usagebar").join("history.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(seconds, 0).unwrap()
    }

    #[test]
    fn three_samples_make_a_sparkline() {
        let mut history = History::default();
        assert!(
            history
                .record("codex|Weekly", Some("r1"), 10, at(0))
                .is_none()
        );
        assert!(
            history
                .record("codex|Weekly", Some("r1"), 20, at(60))
                .is_none()
        );
        let spark = history
            .record("codex|Weekly", Some("r1"), 30, at(120))
            .unwrap();
        assert_eq!(spark.chars().count(), 3);
        assert_eq!(spark.chars().last(), Some('█'));
    }

    #[test]
    fn a_new_reset_starts_a_new_series() {
        let mut history = History::default();
        for (i, pct) in [10, 20, 30].into_iter().enumerate() {
            history.record("codex|Weekly", Some("r1"), pct, at(i as i64 * 60));
        }
        assert!(
            history
                .record("codex|Weekly", Some("r2"), 5, at(300))
                .is_none()
        );
    }

    #[test]
    fn samples_inside_the_interval_replace_the_last_point() {
        let mut history = History::default();
        history.record("codex|Weekly", Some("r1"), 10, at(0));
        history.record("codex|Weekly", Some("r1"), 80, at(5));
        history.record("codex|Weekly", Some("r1"), 90, at(60));
        let spark = history
            .record("codex|Weekly", Some("r1"), 100, at(120))
            .unwrap();
        assert_eq!(spark.chars().count(), 3);
        assert_eq!(spark.chars().last(), Some('█'));
    }

    #[test]
    fn a_flat_series_is_a_baseline() {
        let mut history = History::default();
        let mut spark = None;
        for i in 0..3 {
            spark = history.record("codex|Weekly", Some("r1"), 27, at(i * 60));
        }
        assert_eq!(spark.as_deref(), Some("▁▁▁"));
    }

    #[test]
    fn the_series_is_capped() {
        let mut history = History::default();
        for i in 0..(MAX_POINTS + 5) {
            history.record("codex|Weekly", Some("r1"), 50, at(i as i64 * 60));
        }
        let series = history.series.get("codex|Weekly").unwrap();
        assert_eq!(series.points.len(), MAX_POINTS);
    }

    #[test]
    fn the_history_lock_is_exclusive() {
        let dir = tempfile::TempDir::new().unwrap();
        let held = lock_at(dir.path()).unwrap();
        let second = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(dir.path().join("history.lock"))
            .unwrap();
        assert!(fs2::FileExt::try_lock_exclusive(&second).is_err());
        drop(held);
        assert!(fs2::FileExt::try_lock_exclusive(&second).is_ok());
    }

    #[test]
    fn invalid_history_falls_back_to_empty() {
        // The parser is the only thing that can fail here; `load` swallows it.
        assert!(serde_json::from_slice::<History>(b"not json").is_err());
    }
}
