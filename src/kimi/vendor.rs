//! Kimi renderer — bar text + bordered Pango tooltip.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::countdown;
use crate::format::{placeholders, substitute, updated_at_hm};
use crate::pacing::PaceSeverity;
use crate::pango::{color_span, escape, severity_color, severity_for};
use crate::theme::Theme;
use crate::tooltip::{Line as TooltipLine, WindowRow, push_window_with_row, render_bordered};
use crate::usage::{KimiSnapshot, UsageWindow};
use crate::vendor::{RenderOpts, VendorId, VendorOutcome};
use crate::waybar::{Class, WaybarOutput};

use super::fetch::{FetchOutcome, SCHEMA_DRIFT_MESSAGE};

/// Presentation classification for Kimi's legacy `(u16, String)` cached
/// diagnostic. Code zero has never meant HTTP; the stable schema marker lets
/// renderers distinguish an upstream response-shape change from other local
/// failures without changing the on-disk cache format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningKind {
    Http(u16),
    SchemaDrift,
    Other,
}

pub fn warning_kind(code: u16, message: &str) -> WarningKind {
    if code != 0 {
        WarningKind::Http(code)
    } else if message == SCHEMA_DRIFT_MESSAGE {
        WarningKind::SchemaDrift
    } else {
        WarningKind::Other
    }
}

/// Kimi has two independent quota percentages. Keep both on the individual
/// widget in the same order as the detail panel: current 5h window, then 7d.
pub const DEFAULT_FORMAT: &str = "5h {kimi_window_pct}% · 7d {kimi_weekly_pct}%";

/// Kimi reports the weekly quota's reset instant but never its length; the
/// subscription bucket rolls every 7 days.
pub const WEEKLY_WINDOW: chrono::Duration = chrono::Duration::days(7);
/// The rolling bucket's length *is* advertised — 300 minutes — and only that
/// spelling is accepted on the way in (`types::is_five_hour_window`).
pub const ROLLING_WINDOW: chrono::Duration = chrono::Duration::hours(5);

/// Project a quota pair onto the shared window shape the tooltip helper draws.
fn window(pct: i32, resets_at: Option<DateTime<Utc>>, duration: chrono::Duration) -> UsageWindow {
    UsageWindow {
        utilization_pct: pct,
        resets_at,
        window_duration: duration,
    }
}

pub fn build_placeholders(
    snap: &KimiSnapshot,
    now: DateTime<Utc>,
) -> HashMap<&'static str, String> {
    let plan = snap.plan.as_deref().unwrap_or("Kimi");
    let weekly_pct = snap.weekly_pct();
    let window_pct = snap.window_pct();
    // The newer response shape has no weekly bucket: the weekly placeholders
    // resolve to empty strings (the codebase's missing-placeholder
    // convention) rather than a fabricated 0.
    let weekly = |value: String| {
        if snap.has_weekly {
            value
        } else {
            String::new()
        }
    };
    let monthly = |value: String| {
        if snap.monthly_pct.is_some() {
            value
        } else {
            String::new()
        }
    };
    placeholders(vec![
        ("icon", "󰚩".to_string()),
        ("vendor_short", VendorId::Kimi.short_name().to_string()),
        // Cross-vendor aliases.
        ("plan", plan.to_string()),
        ("weekly_pct", weekly(weekly_pct.to_string())),
        (
            "weekly_reset",
            weekly(countdown::format(snap.weekly_reset_at, now)),
        ),
        ("session_pct", window_pct.to_string()),
        (
            "session_reset",
            countdown::format(snap.window_reset_at, now),
        ),
        // Kimi-specific placeholders.
        ("kimi_plan", plan.to_string()),
        ("kimi_weekly_pct", weekly(weekly_pct.to_string())),
        ("kimi_weekly_used", weekly(snap.weekly_used.to_string())),
        ("kimi_weekly_limit", weekly(snap.weekly_limit.to_string())),
        (
            "kimi_weekly_remaining",
            weekly(snap.weekly_remaining.to_string()),
        ),
        (
            "kimi_weekly_reset",
            weekly(countdown::format(snap.weekly_reset_at, now)),
        ),
        (
            "kimi_monthly_pct",
            monthly(snap.monthly_pct.unwrap_or(0).to_string()),
        ),
        (
            "kimi_monthly_reset",
            monthly(countdown::format(snap.monthly_reset_at, now)),
        ),
        ("kimi_window_pct", window_pct.to_string()),
        ("kimi_window_used", snap.window_used.to_string()),
        ("kimi_window_limit", snap.window_limit.to_string()),
        ("kimi_window_remaining", snap.window_remaining.to_string()),
        (
            "kimi_window_reset",
            countdown::format(snap.window_reset_at, now),
        ),
    ])
}

pub fn severity(snap: &KimiSnapshot) -> PaceSeverity {
    severity_for(snap.worst_pct())
}

pub fn render(
    outcome: &VendorOutcome,
    snap: &KimiSnapshot,
    theme: &Theme,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> WaybarOutput {
    let class = Class::from(severity(snap));
    let format = opts
        .format
        .clone()
        .unwrap_or_else(|| DEFAULT_FORMAT.to_string());
    let values = build_placeholders(snap, now);
    // User formats are Pango markup after Waybar renders them. Escape API
    // strings there, while retaining raw values for the default tooltip (which
    // escapes exactly once at its markup insertion point).
    let mut pango_values = values.clone();
    for key in ["plan", "kimi_plan"] {
        if let Some(value) = pango_values.get_mut(key) {
            *value = escape(value);
        }
    }

    let mut text = substitute(&format, &pango_values);
    if outcome.stale {
        text.push_str(" ⏸");
    }

    let wrapper_color = severity_color(severity(snap), theme).to_string();
    let icon_prefix = match opts.icon.as_deref() {
        Some(ic) if !ic.is_empty() => format!("{ic} "),
        _ => String::new(),
    };
    let bar_text = color_span(&wrapper_color, &format!("{icon_prefix}{text}"));

    let tooltip = if let Some(fmt) = opts.tooltip_format.as_deref() {
        substitute(fmt, &pango_values)
    } else {
        render_tooltip(outcome, snap, theme, now)
    };

    WaybarOutput {
        text: bar_text,
        tooltip,
        class,
    }
}

fn render_tooltip(
    outcome: &VendorOutcome,
    snap: &KimiSnapshot,
    theme: &Theme,
    now: DateTime<Utc>,
) -> String {
    let blue = &theme.blue;
    let dim = &theme.dim;
    let fg = &theme.fg;

    let plan_color = severity_color(severity(snap), theme);

    let mut lines: Vec<TooltipLine> = Vec::new();
    lines.push(TooltipLine::Center(format!(
        "<span font_weight='bold' foreground='{blue}'>Kimi</span>"
    )));
    lines.push(TooltipLine::Sep);
    lines.push(TooltipLine::Body("".into()));

    let plan = snap.plan.as_deref().unwrap_or("Kimi");
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{fg}'>  󰣖  Plan</span>"
    )));
    lines.push(TooltipLine::Body(format!(
        "   <span font_weight='bold' foreground='{plan_color}'>{}</span>",
        escape(plan)
    )));

    // Kimi reports each quota as used/limit against a limit of 100, so the
    // pair is the percentage in longhand: project it onto a window and the
    // bar carries it, like every other vendor's.
    if snap.window_limit > 0 {
        lines.push(TooltipLine::Body("".into()));
        push_window_with_row(
            &mut lines,
            "  󰅁  Rolling window (5h)",
            &window(snap.window_pct(), snap.window_reset_at, ROLLING_WINDOW),
            theme,
            now,
            WindowRow::default(),
        );
    }

    if snap.has_weekly {
        lines.push(TooltipLine::Body("".into()));
        push_window_with_row(
            &mut lines,
            "  󰅄  Weekly quota",
            &window(snap.weekly_pct(), snap.weekly_reset_at, WEEKLY_WINDOW),
            theme,
            now,
            WindowRow::default(),
        );
    }

    if let Some(monthly_pct) = snap.monthly_pct {
        lines.push(TooltipLine::Body("".into()));
        push_window_with_row(
            &mut lines,
            "  󰃰  Monthly",
            // The monthly pool resets from the order date, so there is no
            // fixed window length to pace against; with a default row the
            // tooltip helper never reads the duration.
            &UsageWindow {
                utilization_pct: monthly_pct,
                resets_at: snap.monthly_reset_at,
                window_duration: chrono::Duration::zero(),
            },
            theme,
            now,
            WindowRow::default(),
        );
    }

    if let Some((code, msg)) = outcome.last_error.as_ref() {
        let (label, icon, ecolor) = match warning_kind(*code, msg) {
            WarningKind::SchemaDrift => {
                ("Kimi API schema drift".to_string(), "󰅚", theme.red.as_str())
            }
            WarningKind::Other => ("Kimi error".to_string(), "󰅚", theme.red.as_str()),
            WarningKind::Http(code) if code >= 500 => {
                (format!("HTTP {code}"), "󰅚", theme.red.as_str())
            }
            WarningKind::Http(code) => (format!("HTTP {code}"), "󰀪", theme.orange.as_str()),
        };
        lines.push(TooltipLine::Body("".into()));
        lines.push(TooltipLine::Sep);
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{ecolor}'>  {icon}  {label}</span>"
        )));
        if msg != &label {
            lines.push(TooltipLine::Body(format!(
                "     <span foreground='{dim}'>{}</span>",
                escape(msg)
            )));
        }
    }

    let updated = updated_at_hm(now, outcome.cache_age);
    lines.push(TooltipLine::Body("".into()));
    lines.push(TooltipLine::Sep);
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{dim}'>  󰅐  Updated {updated}</span>"
    )));

    render_bordered(&lines, theme)
}

impl From<FetchOutcome> for VendorOutcome {
    fn from(o: FetchOutcome) -> Self {
        o.map(crate::usage::VendorSnapshot::Kimi)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 2, 7, 12, 0, 0).unwrap()
    }

    fn sample_snap() -> KimiSnapshot {
        KimiSnapshot {
            plan: Some("LEVEL_INTERMEDIATE".into()),
            weekly_limit: 100,
            weekly_used: 26,
            weekly_remaining: 74,
            weekly_reset_at: Some(now() + chrono::Duration::days(4)),
            has_weekly: true,
            monthly_pct: None,
            monthly_reset_at: None,
            window_limit: 100,
            window_used: 15,
            window_remaining: 85,
            window_reset_at: Some(now() + chrono::Duration::hours(2)),
        }
    }

    /// An account on the newer `usages`-map shape: no weekly bucket, a
    /// monthly pool instead.
    fn monthly_snap() -> KimiSnapshot {
        KimiSnapshot {
            plan: Some("Allegretto".into()),
            weekly_limit: 0,
            weekly_used: 0,
            weekly_remaining: 0,
            weekly_reset_at: None,
            has_weekly: false,
            monthly_pct: Some(42),
            monthly_reset_at: Some(now() + chrono::Duration::days(30)),
            window_limit: 100,
            window_used: 15,
            window_remaining: 85,
            window_reset_at: Some(now() + chrono::Duration::hours(2)),
        }
    }

    fn sample_outcome(snap: KimiSnapshot) -> VendorOutcome {
        VendorOutcome {
            snapshot: crate::usage::VendorSnapshot::Kimi(snap),
            stale: false,
            last_error: None,
            cache_age: Some(std::time::Duration::from_secs(10)),
        }
    }

    fn opts() -> RenderOpts {
        RenderOpts {
            format: None,
            tooltip_format: None,
            icon: None,
            pace_tolerance: 5,
            format_pace_color: false,
            tooltip_pace_pts: false,
        }
    }

    #[test]
    fn default_render_has_one_percent_for_each_quota() {
        let snap = sample_snap();
        let outcome = sample_outcome(snap.clone());
        let out = render(&outcome, &snap, &Theme::default(), &opts(), now());
        assert!(out.text.contains("15%"), "text: {}", out.text);
        assert!(out.text.contains("26%"), "text: {}", out.text);
        assert!(!out.text.contains("%%"), "text: {}", out.text);
        assert_eq!(out.text.matches('%').count(), 2, "text: {}", out.text);
    }

    #[test]
    fn pct_placeholders_are_bare_integers() {
        let snap = sample_snap();
        let values = build_placeholders(&snap, now());
        assert_eq!(values["kimi_weekly_pct"], "26");
        assert_eq!(values["weekly_pct"], "26");
        assert_eq!(values["kimi_window_pct"], "15");
        assert_eq!(values["session_pct"], "15");
    }

    #[test]
    fn severity_worst_of_windows() {
        let mut snap = sample_snap();
        snap.weekly_used = 10;
        snap.weekly_remaining = 90;
        snap.window_used = 95;
        snap.window_remaining = 5;
        // 95% window should drive severity to Critical even though weekly is Low.
        assert_eq!(severity(&snap), PaceSeverity::Critical);
    }

    #[test]
    fn zero_limits_are_low() {
        let snap = KimiSnapshot {
            weekly_limit: 0,
            weekly_used: 0,
            weekly_remaining: 0,
            window_limit: 0,
            window_used: 0,
            window_remaining: 0,
            ..sample_snap()
        };
        assert_eq!(severity(&snap), PaceSeverity::Low);
    }

    #[test]
    fn missing_window_omitted_from_tooltip() {
        let mut snap = sample_snap();
        snap.window_limit = 0;
        let outcome = sample_outcome(snap.clone());
        let out = render(&outcome, &snap, &Theme::default(), &opts(), now());
        assert!(out.tooltip.contains("Weekly quota"));
        assert!(!out.tooltip.contains("Rolling window"));
    }

    #[test]
    fn custom_tooltip_format_substitutes_exactly() {
        let snap = sample_snap();
        let outcome = sample_outcome(snap.clone());
        let mut o = opts();
        o.tooltip_format = Some("W:{kimi_weekly_pct} R:{kimi_window_pct}".into());
        let out = render(&outcome, &snap, &Theme::default(), &o, now());
        assert_eq!(out.tooltip, "W:26 R:15");
    }

    #[test]
    fn plan_is_pango_escaped() {
        let mut snap = sample_snap();
        snap.plan = Some("A&B <beta>".into());
        let outcome = sample_outcome(snap.clone());
        let out = render(&outcome, &snap, &Theme::default(), &opts(), now());
        assert!(
            out.tooltip.contains("A&amp;B &lt;beta&gt;"),
            "tooltip: {}",
            out.tooltip
        );
    }

    #[test]
    fn custom_plan_placeholder_is_pango_escaped_once() {
        let mut snap = sample_snap();
        snap.plan = Some("A&B <beta>".into());
        let outcome = sample_outcome(snap.clone());
        let mut o = opts();
        o.tooltip_format = Some("{kimi_plan}".into());
        let out = render(&outcome, &snap, &Theme::default(), &o, now());
        assert_eq!(out.tooltip, "A&amp;B &lt;beta&gt;");
    }

    #[test]
    fn schema_error_has_schema_label_not_http_422() {
        let snap = sample_snap();
        let mut outcome = sample_outcome(snap.clone());
        outcome.stale = true;
        outcome.last_error = Some((0, SCHEMA_DRIFT_MESSAGE.into()));
        let out = render(&outcome, &snap, &Theme::default(), &opts(), now());
        assert!(out.tooltip.contains("Kimi API schema drift"));
        assert!(!out.tooltip.contains("HTTP 422"));
        assert_eq!(out.tooltip.matches("Kimi API schema drift").count(), 1);
    }

    #[test]
    fn generic_code_zero_error_is_not_labeled_schema_drift() {
        let snap = sample_snap();
        let mut outcome = sample_outcome(snap.clone());
        outcome.stale = true;
        outcome.last_error = Some((0, "cache lock unavailable".into()));
        let out = render(&outcome, &snap, &Theme::default(), &opts(), now());
        assert!(out.tooltip.contains("Kimi error"));
        assert!(out.tooltip.contains("cache lock unavailable"));
        assert!(!out.tooltip.contains("Kimi API schema drift"));
    }

    #[test]
    fn warning_kind_uses_schema_marker_without_treating_code_zero_as_http() {
        assert_eq!(
            warning_kind(0, SCHEMA_DRIFT_MESSAGE),
            WarningKind::SchemaDrift
        );
        assert_eq!(
            warning_kind(0, "cache lock unavailable"),
            WarningKind::Other
        );
        assert_eq!(warning_kind(503, "unavailable"), WarningKind::Http(503));
    }

    #[test]
    fn fetch_outcome_conversion_preserves_metadata() {
        let snap = sample_snap();
        let fetch = FetchOutcome {
            snapshot: snap.clone(),
            stale: true,
            last_error: Some((401, "bad".into())),
            cache_age: Some(std::time::Duration::from_secs(42)),
        };
        let vendor: VendorOutcome = fetch.into();
        assert!(matches!(
            vendor.snapshot,
            crate::usage::VendorSnapshot::Kimi(_)
        ));
        assert!(vendor.stale);
        assert_eq!(vendor.last_error, Some((401, "bad".into())));
        assert_eq!(vendor.cache_age, Some(std::time::Duration::from_secs(42)));
    }

    #[test]
    fn tooltip_includes_plan_and_usage_and_countdowns() {
        let snap = sample_snap();
        let outcome = sample_outcome(snap.clone());
        let out = render(&outcome, &snap, &Theme::default(), &opts(), now());
        assert!(out.tooltip.contains("Kimi"));
        assert!(out.tooltip.contains("LEVEL_INTERMEDIATE"));
        assert!(out.tooltip.contains("Weekly quota"));
        assert!(out.tooltip.contains("Rolling window"));
        assert!(out.tooltip.contains("26%"));
        assert!(out.tooltip.contains("15%"));
        // Reset should be a countdown, not raw RFC3339.
        assert!(!out.tooltip.contains("2026-02-11T17:32:50"));
        assert!(!out.tooltip.contains("2026-02-07T12:32:50"));
    }

    #[test]
    fn stale_appends_pause() {
        let snap = sample_snap();
        let mut outcome = sample_outcome(snap.clone());
        outcome.stale = true;
        let out = render(&outcome, &snap, &Theme::default(), &opts(), now());
        assert!(out.text.contains("⏸"));
    }

    #[test]
    fn placeholder_set_contains_all_keys() {
        let snap = sample_snap();
        let values = build_placeholders(&snap, now());
        for key in [
            "kimi_plan",
            "kimi_weekly_pct",
            "kimi_weekly_used",
            "kimi_weekly_limit",
            "kimi_weekly_remaining",
            "kimi_weekly_reset",
            "kimi_monthly_pct",
            "kimi_monthly_reset",
            "kimi_window_pct",
            "kimi_window_used",
            "kimi_window_limit",
            "kimi_window_remaining",
            "kimi_window_reset",
            "plan",
            "weekly_pct",
            "session_pct",
        ] {
            assert!(values.contains_key(key), "missing placeholder {key}");
        }
    }

    /// The whole point of the rework: Kimi's quotas are percentages behind a
    /// pair of counters, so they draw like every other vendor's window.
    #[test]
    fn tooltip_draws_a_progress_bar_for_both_quotas() {
        let snap = sample_snap();
        let outcome = sample_outcome(snap.clone());
        let out = render(&outcome, &snap, &Theme::default(), &opts(), now());
        assert_eq!(
            out.tooltip.matches('░').count() + out.tooltip.matches('█').count(),
            2 * crate::pango::BAR_LEN as usize,
            "expected one full-width bar per quota: {}",
            out.tooltip
        );
        assert!(out.tooltip.contains("Resets in"), "{}", out.tooltip);
    }

    /// Kimi reports each quota as used/limit against a limit of 100, so the
    /// counters are the percentage in longhand — the bar above already shows
    /// it. The reset line carries the countdown alone, like every other
    /// vendor's row.
    #[test]
    fn tooltip_reset_line_carries_the_countdown_alone() {
        let snap = sample_snap();
        let outcome = sample_outcome(snap.clone());
        let out = render(&outcome, &snap, &Theme::default(), &opts(), now());
        for line in out.tooltip.lines().filter(|l| l.contains("Resets in")) {
            assert_eq!(
                line.matches('·').count(),
                0,
                "reset line carries more than the countdown: {line}"
            );
        }
        assert!(!out.tooltip.contains("100"), "{}", out.tooltip);
    }

    /// Kimi opts out of pacing, like Codex; the rows must not sprout a glyph
    /// on their own.
    #[test]
    fn tooltip_rows_carry_no_pace_glyph() {
        let snap = sample_snap();
        let outcome = sample_outcome(snap.clone());
        let out = render(&outcome, &snap, &Theme::default(), &opts(), now());
        for glyph in ['↑', '→', '↓'] {
            assert!(!out.tooltip.contains(glyph), "{}", out.tooltip);
        }
    }

    /// The tooltip runs shortest window first, the way Claude's and Codex's
    /// both open on their 5h row. Kimi's rolling bucket is that window, so it
    /// sits above the weekly quota — the same order `DEFAULT_FORMAT` and the
    /// detail panel use.
    #[test]
    fn tooltip_puts_the_rolling_window_above_the_weekly_quota() {
        let snap = sample_snap();
        let outcome = sample_outcome(snap.clone());
        let out = render(&outcome, &snap, &Theme::default(), &opts(), now());
        let rolling = out
            .tooltip
            .find("Rolling window (5h)")
            .unwrap_or_else(|| panic!("no rolling row: {}", out.tooltip));
        let weekly = out
            .tooltip
            .find("Weekly quota")
            .unwrap_or_else(|| panic!("no weekly row: {}", out.tooltip));
        assert!(rolling < weekly, "{}", out.tooltip);
    }

    /// Kimi's compact surface must not discard either independent quota.
    #[test]
    fn default_bar_text_shows_the_rolling_and_weekly_quotas() {
        let snap = sample_snap();
        let outcome = sample_outcome(snap.clone());
        let out = render(&outcome, &snap, &Theme::default(), &opts(), now());
        assert!(out.text.contains("5h 15% · 7d 26%"), "{}", out.text);
    }

    #[test]
    fn monthly_shape_toolbar_omits_the_weekly_row_and_adds_a_monthly_row() {
        let snap = monthly_snap();
        let outcome = sample_outcome(snap.clone());
        let out = render(&outcome, &snap, &Theme::default(), &opts(), now());
        assert!(
            out.tooltip.contains("Rolling window (5h)"),
            "{}",
            out.tooltip
        );
        assert!(!out.tooltip.contains("Weekly quota"), "{}", out.tooltip);
        assert!(out.tooltip.contains("Monthly"), "{}", out.tooltip);
        assert!(out.tooltip.contains("42%"), "{}", out.tooltip);
        // The code slice inside the pool never gets a row of its own.
        assert!(!out.tooltip.contains("Code"), "{}", out.tooltip);
        assert_eq!(
            out.tooltip.matches('░').count() + out.tooltip.matches('█').count(),
            2 * crate::pango::BAR_LEN as usize,
            "one bar per present window: {}",
            out.tooltip
        );
    }

    #[test]
    fn monthly_shape_weekly_placeholders_render_empty_and_monthly_ones_resolve() {
        let snap = monthly_snap();
        let values = build_placeholders(&snap, now());
        for key in [
            "kimi_weekly_pct",
            "kimi_weekly_used",
            "kimi_weekly_limit",
            "kimi_weekly_remaining",
            "kimi_weekly_reset",
            "weekly_pct",
            "weekly_reset",
        ] {
            assert_eq!(values[key], "", "{key} must render empty");
        }
        assert_eq!(values["kimi_monthly_pct"], "42");
        assert!(!values["kimi_monthly_reset"].is_empty());
        // No placeholder exposes the code slice inside the monthly pool.
        assert!(
            !values.keys().any(|k| k.contains("code")),
            "a code-slice placeholder must not exist"
        );
    }

    /// DEFAULT_FORMAT stays `5h {kimi_window_pct}% · 7d {kimi_weekly_pct}%`:
    /// with the weekly placeholder resolving to empty, substitution must
    /// tolerate the shape — no literal braces left behind.
    #[test]
    fn monthly_shape_default_format_tolerates_the_empty_weekly_placeholder() {
        let snap = monthly_snap();
        let outcome = sample_outcome(snap.clone());
        let out = render(&outcome, &snap, &Theme::default(), &opts(), now());
        assert!(!out.text.contains('{'), "{}", out.text);
        assert!(!out.text.contains('}'), "{}", out.text);
        assert!(out.text.contains("5h 15%"), "{}", out.text);
        assert!(out.text.contains("7d %"), "{}", out.text);
    }

    #[test]
    fn severity_is_the_max_over_the_present_windows() {
        let mut snap = monthly_snap();
        snap.window_used = 10;
        snap.window_remaining = 90;
        snap.monthly_pct = Some(95);
        // The monthly pool alone can drive severity.
        assert_eq!(severity(&snap), PaceSeverity::Critical);
        snap.monthly_pct = None;
        assert_eq!(severity(&snap), PaceSeverity::Low);
    }
}
