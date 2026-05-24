//! Pacing math — encodes claudebar's `calc_pacing` (claudebar:279-321) and
//! `pace_color_for` (claudebar:212-219) as pure functions.
//!
//! Two parallel notions of "pacing":
//! - **Ratio** — `actual_pct / elapsed_pct`, with a tolerance band (`PACE_TOLERANCE`).
//!   Used for the `{*_pace}` and `{*_pace_pct}` placeholders. Capped at 999%.
//! - **Point delta** — `actual_pct - elapsed_pct`, a signed integer.
//!   Used for `{*_pace_indicator}`, `{*_pace_pts}`, `{*_pace_delta}`. No tolerance.
//!
//! Both are computed in one shot and returned as a `Pacing` struct so the
//! caller can pick whichever placeholder it needs without re-running the math.

use chrono::{DateTime, Utc};

/// Default tolerance band (in percentage points) for the ratio-based pacing
/// icon. Mirrors claudebar's default `PACE_TOLERANCE=5`.
pub const DEFAULT_TOLERANCE: u32 = 5;

/// A small enum captures the three visual pace states. Keeping the icon out
/// of strings lets the TUI render `Style`-colored chars and the widget render
/// raw glyphs without any string parsing on the other end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pace {
    Ahead,
    OnTrack,
    Under,
}

impl Pace {
    /// Single-char glyph used in claudebar's `{*_pace*}` placeholders.
    pub fn glyph(self) -> &'static str {
        match self {
            Pace::Ahead => "↑",
            Pace::OnTrack => "→",
            Pace::Under => "↓",
        }
    }
}

/// Result of `calc_pacing` — all fields the caller might want to render.
///
/// Field naming mirrors the placeholders so the format-substitution layer is
/// a trivial mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pacing {
    /// `{*_elapsed}` — integer percent of the window that has elapsed (0..=100).
    pub elapsed_pct: i32,
    /// `{*_pace}` — ratio-based icon, honors `tolerance`.
    pub ratio_pace: Pace,
    /// `{*_pace_indicator}` — point-based icon, no tolerance.
    pub point_pace: Pace,
    /// `{*_pace_delta}` — signed integer `usage_pct - elapsed_pct`.
    pub delta: i32,
    /// `{*_pace_pct}` — ratio-based label ("12% ahead" / "5% under" / "on track").
    pub ratio_label: String,
    /// `{*_pace_pts}` — point-based label ("12pts ahead" / "5pts under" / "on track").
    pub point_label: String,
}

impl Pacing {
    /// Neutral pacing for windows with no `resets_at` (e.g. vendors that don't
    /// expose one). Matches claudebar's early-return value.
    pub fn neutral() -> Self {
        Self {
            elapsed_pct: 0,
            ratio_pace: Pace::OnTrack,
            point_pace: Pace::OnTrack,
            delta: 0,
            ratio_label: "on track".into(),
            point_label: "on track".into(),
        }
    }
}

/// Compute pacing for a usage window.
///
/// `usage_pct` is the vendor-reported utilization (0..=100, integer to match
/// Claude's `utilization` field). `reset` is when the window rolls over;
/// `now` is the reference time (passed in for testability). `window` is the
/// window's total duration. `tolerance` is the ratio-tolerance band in
/// percentage points (e.g. `5` for ±5%).
pub fn calc(
    usage_pct: i32,
    reset: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    window: chrono::Duration,
    tolerance: u32,
) -> Pacing {
    let Some(reset) = reset else {
        return Pacing::neutral();
    };
    if window.num_seconds() <= 0 {
        return Pacing::neutral();
    }

    let remaining = reset.signed_duration_since(now).num_seconds();
    let total = window.num_seconds();
    let mut elapsed_pct = (((total - remaining) * 100) / total) as i32;
    elapsed_pct = elapsed_pct.clamp(0, 100);

    // Point-based delta and label.
    let delta = usage_pct - elapsed_pct;
    let (point_pace, point_label) = if delta > 0 {
        (Pace::Ahead, format!("{delta}pts ahead"))
    } else if delta < 0 {
        (Pace::Under, format!("{}pts under", -delta))
    } else {
        (Pace::OnTrack, "on track".to_string())
    };

    // Ratio-based icon and label (only meaningful once any time has elapsed).
    let (ratio_pace, ratio_label) = if elapsed_pct > 0 {
        let pacing_x100 = (usage_pct * 100) / elapsed_pct;
        let tol = tolerance as i32;
        if pacing_x100 > 100 + tol {
            let dev = (pacing_x100 - 100).min(999);
            (Pace::Ahead, format!("{dev}% ahead"))
        } else if pacing_x100 < 100 - tol {
            let dev = (100 - pacing_x100).min(999);
            (Pace::Under, format!("{dev}% under"))
        } else {
            (Pace::OnTrack, "on track".to_string())
        }
    } else {
        (Pace::OnTrack, "on track".to_string())
    };

    Pacing {
        elapsed_pct,
        ratio_pace,
        point_pace,
        delta,
        ratio_label,
        point_label,
    }
}

/// Color band keyed on signed point delta. Mirrors claudebar's
/// `pace_color_for` (claudebar:212-219). Returns one of the four severity
/// tiers; the caller maps to a theme color.
///
/// `delta <= -10` → low (green); `-10..=0` → mid (yellow);
/// `1..=9` → high (orange); `>= 10` → critical (red).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaceSeverity {
    Low,
    Mid,
    High,
    Critical,
}

pub fn pace_severity(delta: i32) -> PaceSeverity {
    if delta >= 10 {
        PaceSeverity::Critical
    } else if delta > 0 {
        PaceSeverity::High
    } else if delta >= -10 {
        PaceSeverity::Mid
    } else {
        PaceSeverity::Low
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, 23, h, m, 0).unwrap()
    }

    const FIVE_H: chrono::Duration = chrono::Duration::hours(5);

    #[test]
    fn missing_reset_returns_neutral() {
        let p = calc(50, None, at(12, 0), FIVE_H, DEFAULT_TOLERANCE);
        assert_eq!(p, Pacing::neutral());
    }

    #[test]
    fn zero_window_returns_neutral() {
        let p = calc(50, Some(at(12, 0)), at(12, 0), chrono::Duration::zero(), 5);
        assert_eq!(p, Pacing::neutral());
    }

    #[test]
    fn elapsed_clamps_to_zero_when_future_reset_beyond_window() {
        // Reset is 6h away but window is 5h → "remaining > total" → negative
        // elapsed → clamped to 0.
        let now = at(12, 0);
        let reset = now + chrono::Duration::hours(6);
        let p = calc(10, Some(reset), now, FIVE_H, 5);
        assert_eq!(p.elapsed_pct, 0);
    }

    #[test]
    fn elapsed_clamps_to_hundred_when_past_reset() {
        let now = at(12, 0);
        let reset = now - chrono::Duration::hours(1);
        let p = calc(50, Some(reset), now, FIVE_H, 5);
        assert_eq!(p.elapsed_pct, 100);
    }

    #[test]
    fn perfectly_even_pacing_is_on_track() {
        // 50% elapsed, 50% usage → both metrics on track.
        let now = at(12, 0);
        let reset = now + chrono::Duration::minutes(150); // 2.5h remain of 5h
        let p = calc(50, Some(reset), now, FIVE_H, DEFAULT_TOLERANCE);
        assert_eq!(p.elapsed_pct, 50);
        assert_eq!(p.delta, 0);
        assert_eq!(p.ratio_pace, Pace::OnTrack);
        assert_eq!(p.point_pace, Pace::OnTrack);
        assert_eq!(p.ratio_label, "on track");
        assert_eq!(p.point_label, "on track");
    }

    #[test]
    fn ahead_of_pace_above_tolerance() {
        // 50% elapsed, 70% usage → delta 20, ratio 140% → "40% ahead".
        let now = at(12, 0);
        let reset = now + chrono::Duration::minutes(150);
        let p = calc(70, Some(reset), now, FIVE_H, 5);
        assert_eq!(p.delta, 20);
        assert_eq!(p.point_pace, Pace::Ahead);
        assert_eq!(p.point_label, "20pts ahead");
        assert_eq!(p.ratio_pace, Pace::Ahead);
        assert_eq!(p.ratio_label, "40% ahead");
    }

    #[test]
    fn under_pace_below_tolerance() {
        // 50% elapsed, 30% usage → delta -20, ratio 60% → "40% under".
        let now = at(12, 0);
        let reset = now + chrono::Duration::minutes(150);
        let p = calc(30, Some(reset), now, FIVE_H, 5);
        assert_eq!(p.delta, -20);
        assert_eq!(p.point_pace, Pace::Under);
        assert_eq!(p.point_label, "20pts under");
        assert_eq!(p.ratio_pace, Pace::Under);
        assert_eq!(p.ratio_label, "40% under");
    }

    #[test]
    fn within_tolerance_band_is_on_track_ratio_but_point_diverges() {
        // 50% elapsed, 52% usage → ratio 104% (within ±5) → on track,
        // BUT point delta is 2 → point_pace = Ahead, point_label "2pts ahead".
        let now = at(12, 0);
        let reset = now + chrono::Duration::minutes(150);
        let p = calc(52, Some(reset), now, FIVE_H, DEFAULT_TOLERANCE);
        assert_eq!(p.ratio_pace, Pace::OnTrack);
        assert_eq!(p.ratio_label, "on track");
        assert_eq!(p.point_pace, Pace::Ahead);
        assert_eq!(p.point_label, "2pts ahead");
    }

    #[test]
    fn ratio_clamps_at_999() {
        // 1% elapsed, 60% usage → pacing_x100 = 6000, dev = 5900 → clamped to 999.
        let now = at(12, 0);
        let reset = now + chrono::Duration::minutes(297); // ~99% remaining → 1% elapsed
        let p = calc(60, Some(reset), now, FIVE_H, 5);
        assert_eq!(p.elapsed_pct, 1);
        assert_eq!(p.ratio_label, "999% ahead");
    }

    #[test]
    fn elapsed_zero_skips_ratio() {
        // 0% elapsed → ratio code is skipped; ratio defaults to on track.
        let now = at(12, 0);
        let reset = now + FIVE_H; // full window remains
        let p = calc(20, Some(reset), now, FIVE_H, 5);
        assert_eq!(p.elapsed_pct, 0);
        assert_eq!(p.ratio_pace, Pace::OnTrack);
        // But point math still runs: delta = 20.
        assert_eq!(p.delta, 20);
        assert_eq!(p.point_pace, Pace::Ahead);
    }

    #[test]
    fn severity_boundaries_match_claudebar() {
        // claudebar: <= -10 green, -10..=0 yellow, 1..9 orange, >= 10 red
        assert_eq!(pace_severity(-100), PaceSeverity::Low);
        assert_eq!(pace_severity(-10), PaceSeverity::Mid); // -10 is in -10..=0 band
        assert_eq!(pace_severity(-1), PaceSeverity::Mid);
        assert_eq!(pace_severity(0), PaceSeverity::Mid);
        assert_eq!(pace_severity(1), PaceSeverity::High);
        assert_eq!(pace_severity(9), PaceSeverity::High);
        assert_eq!(pace_severity(10), PaceSeverity::Critical);
        assert_eq!(pace_severity(100), PaceSeverity::Critical);
    }

    #[test]
    fn neutral_constructor_matches_default_state() {
        let n = Pacing::neutral();
        assert_eq!(n.elapsed_pct, 0);
        assert_eq!(n.delta, 0);
        assert_eq!(n.ratio_pace, Pace::OnTrack);
        assert_eq!(n.point_pace, Pace::OnTrack);
        assert_eq!(n.ratio_label, "on track");
        assert_eq!(n.point_label, "on track");
    }
}
