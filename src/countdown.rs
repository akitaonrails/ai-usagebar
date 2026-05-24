//! Human-readable countdown between two instants.
//!
//! Mirrors claudebar's `countdown()` shell function (claudebar:252-268):
//!   - missing / unparseable reset → `"—"`
//!   - reset already in the past → `"now"`
//!   - ≥1 day remaining → `"{d}d {h}h"`
//!   - otherwise → `"{h}h {mm}m"` (zero-padded minutes)

use chrono::{DateTime, Utc};

/// Format `reset - now` as a short human string.
///
/// Same buckets as claudebar; `None` for `reset` returns `"—"`, matching the
/// shell behavior where `[[ -z "$ts" ]]` short-circuits.
pub fn format(reset: Option<DateTime<Utc>>, now: DateTime<Utc>) -> String {
    let Some(reset) = reset else {
        return "—".to_string();
    };

    let diff = reset.signed_duration_since(now);
    let secs = diff.num_seconds();
    if secs <= 0 {
        return "now".to_string();
    }

    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let mins = (secs % 3_600) / 60;

    if days > 0 {
        format!("{days}d {hours}h")
    } else {
        format!("{hours}h {mins:02}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(year: i32, month: u32, day: u32, h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, h, m, 0).unwrap()
    }

    #[test]
    fn missing_reset_renders_em_dash() {
        let now = at(2026, 5, 23, 12, 0);
        assert_eq!(format(None, now), "—");
    }

    #[test]
    fn past_reset_renders_now() {
        let now = at(2026, 5, 23, 12, 0);
        let reset = at(2026, 5, 23, 11, 0);
        assert_eq!(format(Some(reset), now), "now");
    }

    #[test]
    fn exact_zero_renders_now() {
        // Bash uses `<= 0`, so a zero diff is "now".
        let t = at(2026, 5, 23, 12, 0);
        assert_eq!(format(Some(t), t), "now");
    }

    #[test]
    fn hours_minutes_zero_padded() {
        let now = at(2026, 5, 23, 12, 0);
        let reset = at(2026, 5, 23, 13, 5); // 1h 5m
        assert_eq!(format(Some(reset), now), "1h 05m");
    }

    #[test]
    fn hours_minutes_no_days_under_one_day() {
        let now = at(2026, 5, 23, 12, 0);
        let reset = at(2026, 5, 24, 11, 59); // 23h 59m
        assert_eq!(format(Some(reset), now), "23h 59m");
    }

    #[test]
    fn one_day_one_hour() {
        let now = at(2026, 5, 23, 12, 0);
        let reset = at(2026, 5, 24, 13, 30); // 1d 1h (minutes dropped)
        assert_eq!(format(Some(reset), now), "1d 1h");
    }

    #[test]
    fn multiple_days_drops_minutes() {
        let now = at(2026, 5, 23, 12, 0);
        let reset = at(2026, 5, 27, 13, 45); // 4d 1h
        assert_eq!(format(Some(reset), now), "4d 1h");
    }

    #[test]
    fn one_second_remaining_renders_zero_hours() {
        // Mirrors claudebar: anything > 0 but < 1 min → "0h 00m"
        let now = at(2026, 5, 23, 12, 0, );
        let reset = now + chrono::Duration::seconds(1);
        assert_eq!(format(Some(reset), now), "0h 00m");
    }
}
