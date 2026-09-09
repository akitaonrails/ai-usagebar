use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::countdown;
use crate::format::{placeholders, substitute, updated_at_hm};
use crate::pacing::{self, PaceSeverity};
use crate::pango::{color_span, escape, severity_color, severity_for};
use crate::theme::Theme;
use crate::tooltip::{Line as TooltipLine, WindowRow, push_window_with_row, render_bordered};
use crate::usage::UsageWindow;
use crate::vendor::{RenderOpts, VendorId, VendorOutcome};
use crate::waybar::{Class, WaybarOutput};

use super::fetch::FetchOutcome;
use super::types::{Usage, Window};

pub const DEFAULT_FORMAT: &str = "{ocg_rolling_pct}% · {ocg_rolling_reset}";
const DEFAULT_PLAN: &str = "OpenCode Go";
const UNAVAILABLE: &str = "—";

/// Window lengths for pacing math. The usage endpoint reports only `status`,
/// `percent`, and `resetsAt` — never a duration — so these are constants with
/// a recorded provenance:
/// - `rolling` is the 5-hour limit (`packages/console/app/src/routes/zen/go/v1/usage.ts`
///   `formatUsage` + `packages/console/app/src/routes/zen/util/handler.ts` and
///   `i18n/en.ts` "5-hour usage limit reached", validated live via
///   `GET https://opencode.ai/zen/go/v1/usage`).
/// - `weekly` resets Monday 00:00 UTC (7 days, same response capture).
///
/// There is deliberately no monthly constant: the monthly window follows the
/// subscription cycle (`getMonthlyBounds(now, timeSubscribed)` upstream —
/// 28/29/31-day months depending on the subscriber), so any fixed length
/// would pace against a wrong denominator and publish a wrong `window_secs`.
/// The monthly reset is still shown; only pacing is omitted until the API
/// reports real cycle bounds.
pub const ROLLING_WINDOW: chrono::Duration = chrono::Duration::hours(5);
pub const WEEKLY_WINDOW: chrono::Duration = chrono::Duration::days(7);

impl From<FetchOutcome> for VendorOutcome {
    fn from(outcome: FetchOutcome) -> Self {
        outcome.map(crate::usage::VendorSnapshot::OpenCodeGo)
    }
}

/// Build placeholders with the historical default pacing tolerance.
///
/// Keep this signature stable for library callers. Rendering uses the private
/// tolerance-aware helper so `--pace-tolerance` still applies.
pub fn build_placeholders(usage: &Usage, now: DateTime<Utc>) -> HashMap<&'static str, String> {
    build_placeholders_with_plan(DEFAULT_PLAN, usage, now)
}

pub fn build_placeholders_with_plan(
    plan: &str,
    usage: &Usage,
    now: DateTime<Utc>,
) -> HashMap<&'static str, String> {
    build_placeholders_with_plan_and_tolerance(plan, usage, pacing::DEFAULT_TOLERANCE, now)
}

fn build_placeholders_with_tolerance(
    usage: &Usage,
    pace_tolerance: u32,
    now: DateTime<Utc>,
) -> HashMap<&'static str, String> {
    build_placeholders_with_plan_and_tolerance(DEFAULT_PLAN, usage, pace_tolerance, now)
}

fn build_placeholders_with_plan_and_tolerance(
    plan: &str,
    usage: &Usage,
    pace_tolerance: u32,
    now: DateTime<Utc>,
) -> HashMap<&'static str, String> {
    let plan = sanitize(plan);
    let rolling = window_values(usage.rolling.as_ref(), now);
    let weekly = window_values(usage.weekly.as_ref(), now);
    let monthly = window_values(usage.monthly.as_ref(), now);
    let rolling_pace = window_pacing(usage.rolling.as_ref(), ROLLING_WINDOW, pace_tolerance, now);
    let weekly_pace = window_pacing(usage.weekly.as_ref(), WEEKLY_WINDOW, pace_tolerance, now);

    placeholders([
        (
            "vendor_short",
            VendorId::OpenCodeGo.short_name().to_string(),
        ),
        ("plan", plan.clone()),
        ("ocg_plan", plan),
        ("session_pct", rolling.percent.clone()),
        ("session_reset", rolling.reset.clone()),
        ("session_elapsed", rolling_pace.elapsed.clone()),
        ("weekly_pct", weekly.percent.clone()),
        ("weekly_reset", weekly.reset.clone()),
        ("weekly_elapsed", weekly_pace.elapsed.clone()),
        ("ocg_rolling_pct", rolling.percent),
        ("ocg_rolling_reset", rolling.reset),
        ("ocg_rolling_status", rolling.status),
        ("ocg_rolling_elapsed", rolling_pace.elapsed),
        ("ocg_rolling_pace", rolling_pace.ratio_pace),
        ("ocg_rolling_pace_indicator", rolling_pace.point_pace),
        ("ocg_weekly_pct", weekly.percent),
        ("ocg_weekly_reset", weekly.reset),
        ("ocg_weekly_status", weekly.status),
        ("ocg_weekly_elapsed", weekly_pace.elapsed),
        ("ocg_weekly_pace", weekly_pace.ratio_pace),
        ("ocg_weekly_pace_indicator", weekly_pace.point_pace),
        ("ocg_monthly_pct", monthly.percent),
        ("ocg_monthly_reset", monthly.reset),
        ("ocg_monthly_status", monthly.status),
        // No monthly pacing: the cycle length is subscriber-dependent (see
        // the `ROLLING_WINDOW` provenance note), so any elapsed/pace figure
        // would pace against a guessed denominator. Empty strings, matching
        // what an absent window yields on the Z.AI renderer.
        ("ocg_monthly_elapsed", String::new()),
        ("ocg_monthly_pace", String::new()),
        ("ocg_monthly_pace_indicator", String::new()),
    ])
}

/// Elapsed fraction and pace glyphs for one window, as ready placeholder
/// values. Empty strings when the window is absent, mirroring the Z.AI
/// renderer's convention; `pacing::calc` degrades to neutral 0/glyphs when
/// the reset is unreported (which cannot happen for this vendor — `resetsAt`
/// is required — but keeps the contract explicit).
#[derive(Default)]
struct WindowPacing {
    elapsed: String,
    ratio_pace: String,
    point_pace: String,
}

fn window_pacing(
    window: Option<&Window>,
    duration: chrono::Duration,
    pace_tolerance: u32,
    now: DateTime<Utc>,
) -> WindowPacing {
    let Some(window) = window else {
        return WindowPacing::default();
    };
    let p = pacing::calc(
        utilization_pct(window),
        Some(window.resets_at),
        now,
        duration,
        pace_tolerance,
    );
    WindowPacing {
        elapsed: p.elapsed_pct.to_string(),
        ratio_pace: p.ratio_pace.glyph().to_string(),
        point_pace: p.point_pace.glyph().to_string(),
    }
}

/// Project an OpenCode Go window onto the shared shape the tooltip helper
/// draws. Percentages arrive as floats; panels round them like every other
/// vendor's integer-percent convention.
fn as_usage_window(window: &Window, duration: chrono::Duration) -> UsageWindow {
    UsageWindow {
        utilization_pct: utilization_pct(window),
        resets_at: Some(window.resets_at),
        window_duration: duration,
    }
}

fn utilization_pct(window: &Window) -> i32 {
    window.percent.round().clamp(0.0, 100.0) as i32
}

#[derive(Debug)]
struct WindowValues {
    percent: String,
    reset: String,
    status: String,
}

fn window_values(window: Option<&Window>, now: DateTime<Utc>) -> WindowValues {
    let Some(window) = window else {
        return WindowValues {
            percent: UNAVAILABLE.to_string(),
            reset: UNAVAILABLE.to_string(),
            status: UNAVAILABLE.to_string(),
        };
    };
    WindowValues {
        percent: window.percent.to_string(),
        reset: countdown::format(Some(window.resets_at), now),
        status: sanitize(&window.status),
    }
}

fn sanitize(value: &str) -> String {
    crate::display::sanitize_untrusted_field(value)
}

pub fn severity(usage: &Usage) -> PaceSeverity {
    usage
        .rolling
        .iter()
        .chain(usage.weekly.iter())
        .chain(usage.monthly.iter())
        .map(|window| window.percent as i32)
        .max()
        .map(severity_for)
        .unwrap_or(PaceSeverity::Low)
}

/// Renderer shape matches the existing vendor adapters. `snap` remains local
/// because the shared enum does not yet have an OpenCode-Go arm.
pub fn render(
    outcome: &VendorOutcome,
    snap: &Usage,
    theme: &Theme,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> WaybarOutput {
    render_with_meta(
        snap,
        outcome.stale,
        outcome.last_error.as_ref(),
        outcome.cache_age,
        theme,
        opts,
        now,
    )
}

fn render_with_meta(
    snap: &Usage,
    stale: bool,
    last_error: Option<&(u16, String)>,
    cache_age: Option<Duration>,
    theme: &Theme,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> WaybarOutput {
    let sev = severity(snap);
    let format = opts.format.as_deref().unwrap_or(DEFAULT_FORMAT);
    let values = escaped_placeholders_with_tolerance(snap, opts.pace_tolerance, now);
    let mut text = substitute(format, &values);
    if stale {
        text.push_str(" ⏸");
    }
    let icon_prefix = match opts.icon.as_deref() {
        Some(icon) if !icon.is_empty() => format!("{} ", escape(icon)),
        _ => String::new(),
    };
    let bar_text = color_span(severity_color(sev, theme), &format!("{icon_prefix}{text}"));
    let tooltip = opts
        .tooltip_format
        .as_deref()
        .map(|format| substitute(format, &values))
        .unwrap_or_else(|| render_tooltip(snap, stale, last_error, cache_age, theme, opts, now));

    WaybarOutput {
        text: bar_text,
        tooltip,
        class: Class::from(sev),
    }
}

fn escaped_placeholders_with_tolerance(
    usage: &Usage,
    pace_tolerance: u32,
    now: DateTime<Utc>,
) -> HashMap<&'static str, String> {
    let mut values = build_placeholders_with_tolerance(usage, pace_tolerance, now);
    for key in [
        "plan",
        "ocg_plan",
        "ocg_rolling_status",
        "ocg_weekly_status",
        "ocg_monthly_status",
    ] {
        if let Some(value) = values.get_mut(key) {
            *value = escape(value);
        }
    }
    values
}

fn render_tooltip(
    snap: &Usage,
    stale: bool,
    last_error: Option<&(u16, String)>,
    cache_age: Option<Duration>,
    theme: &Theme,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> String {
    let mut lines = vec![TooltipLine::Center(format!(
        "<span font_weight='bold' foreground='{}'>{}</span>",
        theme.blue,
        escape(DEFAULT_PLAN)
    ))];
    lines.push(TooltipLine::Sep);
    lines.push(TooltipLine::Body(String::new()));

    let row =
        |w: &UsageWindow| WindowRow::paced(w, now, opts.pace_tolerance, opts.tooltip_pace_pts);
    let mut present = false;
    for (label, window, duration) in [
        ("  Rolling (5h)", snap.rolling.as_ref(), ROLLING_WINDOW),
        ("  Weekly (7d)", snap.weekly.as_ref(), WEEKLY_WINDOW),
    ] {
        let Some(window) = window else {
            continue;
        };
        present = true;
        let projected = as_usage_window(window, duration);
        push_window_with_row(&mut lines, label, &projected, theme, now, row(&projected));
    }
    // Monthly keeps its reset countdown but no pace glyph: the cycle length
    // is subscriber-dependent, so there is no exact denominator to pace
    // against (same reason the report carries no `window_secs` for it).
    if let Some(window) = snap.monthly.as_ref() {
        present = true;
        let pct = utilization_pct(window);
        let color = severity_color(severity_for(pct), theme);
        let bar = crate::pango::progress_bar(pct, color, theme, None);
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{}'>  Monthly</span>",
            theme.fg
        )));
        lines.push(TooltipLine::Body(format!(
            "   {bar}  <span font_weight='bold' foreground='{color}'>{pct}%</span>"
        )));
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{}'>  ⏱  Resets in {}</span>",
            theme.dim,
            escape(&countdown::format(Some(window.resets_at), now))
        )));
    }
    if !present {
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{}'>no usage windows reported</span>",
            theme.dim
        )));
    }
    if stale {
        lines.push(TooltipLine::Body(String::new()));
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{}'>  ⏸  Showing cached data</span>",
            theme.orange
        )));
    }
    if let Some((code, message)) = last_error
        && *code != 0
    {
        lines.push(TooltipLine::Body(String::new()));
        lines.push(TooltipLine::Sep);
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{}'>  HTTP {code}: {}</span>",
            theme.orange,
            escape(message)
        )));
    }

    lines.push(TooltipLine::Body(String::new()));
    lines.push(TooltipLine::Sep);
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{}'>  Updated {}</span>",
        theme.dim,
        updated_at_hm(now, cache_age)
    )));
    render_bordered(&lines, theme)
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};

    use super::*;
    use crate::opencode_go::types::{Usage, Window};

    fn at(value: &str) -> DateTime<Utc> {
        value.parse().expect("RFC3339 timestamp")
    }

    fn sample_usage() -> Usage {
        Usage {
            rolling: Some(Window {
                status: "ok".into(),
                percent: 12.3,
                resets_at: at("2026-08-16T20:00:00Z"),
            }),
            weekly: Some(Window {
                status: "rate-limited".into(),
                percent: 45.6,
                resets_at: at("2026-08-20T00:00:00Z"),
            }),
            monthly: Some(Window {
                status: "ok".into(),
                percent: 78.9,
                resets_at: at("2026-09-01T00:00:00Z"),
            }),
        }
    }

    #[test]
    fn exposes_exact_opencode_go_and_generic_placeholders() {
        let values = build_placeholders(&sample_usage(), at("2026-08-16T18:00:00Z"));

        assert_eq!(values["vendor_short"], "ocg");
        assert_eq!(values["session_pct"], "12.3");
        assert_eq!(values["weekly_pct"], "45.6");
        assert_eq!(values["ocg_rolling_pct"], "12.3");
        assert_eq!(values["ocg_rolling_status"], "ok");
        assert_eq!(values["ocg_weekly_pct"], "45.6");
        assert_eq!(values["ocg_weekly_status"], "rate-limited");
        assert_eq!(values["ocg_monthly_pct"], "78.9");
        assert_eq!(values["ocg_monthly_status"], "ok");
    }

    #[test]
    fn default_format_is_rolling_percentage_and_reset() {
        assert_eq!(DEFAULT_FORMAT, "{ocg_rolling_pct}% · {ocg_rolling_reset}");
    }

    #[test]
    fn absent_windows_are_unavailable_not_zero() {
        let values = build_placeholders(
            &Usage {
                rolling: None,
                weekly: None,
                monthly: None,
            },
            at("2026-08-16T18:00:00Z"),
        );

        for key in [
            "session_pct",
            "weekly_pct",
            "ocg_rolling_pct",
            "ocg_weekly_pct",
            "ocg_monthly_pct",
        ] {
            assert_eq!(values[key], "—", "{key} should be unavailable");
            assert_ne!(values[key], "0");
        }
        for key in [
            "session_reset",
            "weekly_reset",
            "ocg_rolling_reset",
            "ocg_weekly_reset",
            "ocg_monthly_reset",
            "ocg_rolling_status",
            "ocg_weekly_status",
            "ocg_monthly_status",
        ] {
            assert_eq!(values[key], "—", "{key} should be unavailable");
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

    fn outcome_for(snap: &Usage) -> VendorOutcome {
        VendorOutcome {
            snapshot: crate::usage::VendorSnapshot::OpenCodeGo(snap.clone()),
            stale: false,
            last_error: None,
            cache_age: Some(std::time::Duration::from_secs(10)),
        }
    }

    #[test]
    fn elapsed_placeholders_follow_window_progress() {
        // 2h left of a 5h window → 60% elapsed; 3d left of a 7d window → 57%.
        let now = at("2026-08-16T18:00:00Z");
        let usage = Usage {
            rolling: Some(Window {
                status: "ok".into(),
                percent: 42.0,
                resets_at: at("2026-08-16T20:00:00Z"),
            }),
            weekly: Some(Window {
                status: "ok".into(),
                percent: 15.0,
                resets_at: at("2026-08-19T18:00:00Z"),
            }),
            monthly: Some(Window {
                status: "ok".into(),
                percent: 70.0,
                resets_at: at("2026-08-31T18:00:00Z"),
            }),
        };
        let values = build_placeholders(&usage, now);
        assert_eq!(values["session_elapsed"], "60");
        assert_eq!(values["weekly_elapsed"], "57");
        assert_eq!(values["ocg_rolling_elapsed"], "60");
        assert_eq!(values["ocg_weekly_elapsed"], "57");
    }

    #[test]
    fn pace_placeholders_follow_usage_vs_elapsed() {
        // Rolling: 80% used vs 60% elapsed → ahead. Weekly: 15% vs 57% → under.
        let now = at("2026-08-16T18:00:00Z");
        let usage = Usage {
            rolling: Some(Window {
                status: "ok".into(),
                percent: 80.0,
                resets_at: at("2026-08-16T20:00:00Z"),
            }),
            weekly: Some(Window {
                status: "ok".into(),
                percent: 15.0,
                resets_at: at("2026-08-19T18:00:00Z"),
            }),
            monthly: Some(Window {
                status: "ok".into(),
                percent: 70.0,
                resets_at: at("2026-08-31T18:00:00Z"),
            }),
        };
        let values = build_placeholders(&usage, now);
        assert_eq!(values["ocg_rolling_pace"], "↑");
        assert_eq!(values["ocg_rolling_pace_indicator"], "↑");
        assert_eq!(values["ocg_weekly_pace"], "↓");
        assert_eq!(values["ocg_weekly_pace_indicator"], "↓");
    }

    #[test]
    fn monthly_keeps_reset_but_omits_pacing_until_cycle_bounds_are_known() {
        // The monthly cycle is subscriber-dependent (28/29/31 days), so no
        // fixed denominator may pace it and no `window_secs` may describe it.
        // The reset countdown and status still report.
        let now = at("2026-08-16T18:00:00Z");
        let usage = Usage {
            rolling: None,
            weekly: None,
            monthly: Some(Window {
                status: "ok".into(),
                percent: 70.0,
                resets_at: at("2026-08-31T18:00:00Z"),
            }),
        };
        let values = build_placeholders(&usage, now);
        assert_eq!(values["ocg_monthly_pct"], "70");
        assert_eq!(values["ocg_monthly_reset"], "15d 0h");
        assert_eq!(values["ocg_monthly_status"], "ok");
        assert_eq!(values["ocg_monthly_elapsed"], "");
        assert_eq!(values["ocg_monthly_pace"], "");
        assert_eq!(values["ocg_monthly_pace_indicator"], "");
    }

    #[test]
    fn public_builder_keeps_the_default_tolerance_api() {
        let now = at("2026-08-16T18:00:00Z");
        let usage = sample_usage();
        assert_eq!(
            build_placeholders(&usage, now),
            build_placeholders_with_tolerance(&usage, pacing::DEFAULT_TOLERANCE, now)
        );
    }

    #[test]
    fn custom_tolerance_changes_the_ratio_pace() {
        // 53% used vs 50% elapsed → ratio 106%: ahead at ±5, on track at ±10.
        let now = at("2026-08-16T18:00:00Z");
        let usage = Usage {
            rolling: Some(Window {
                status: "ok".into(),
                percent: 53.0,
                resets_at: at("2026-08-16T20:30:00Z"),
            }),
            weekly: None,
            monthly: None,
        };
        assert_eq!(
            build_placeholders_with_tolerance(&usage, 5, now)["ocg_rolling_pace"],
            "↑"
        );
        assert_eq!(
            build_placeholders_with_tolerance(&usage, 10, now)["ocg_rolling_pace"],
            "→"
        );
    }

    #[test]
    fn elapsed_and_pace_placeholders_are_empty_without_window() {
        let values = build_placeholders(
            &Usage {
                rolling: None,
                weekly: None,
                monthly: None,
            },
            at("2026-08-16T18:00:00Z"),
        );
        for key in [
            "session_elapsed",
            "weekly_elapsed",
            "ocg_rolling_elapsed",
            "ocg_rolling_pace",
            "ocg_rolling_pace_indicator",
            "ocg_weekly_elapsed",
            "ocg_weekly_pace",
            "ocg_weekly_pace_indicator",
            "ocg_monthly_elapsed",
            "ocg_monthly_pace",
            "ocg_monthly_pace_indicator",
        ] {
            assert_eq!(values[key], "", "{key} should be empty");
        }
    }

    #[test]
    fn tooltip_shows_the_pace_arrow_next_to_each_percentage() {
        // Rolling: 80% used with 2h left of 5h → 60% elapsed → ahead.
        let now = at("2026-08-16T18:00:00Z");
        let usage = Usage {
            rolling: Some(Window {
                status: "ok".into(),
                percent: 80.0,
                resets_at: at("2026-08-16T20:00:00Z"),
            }),
            weekly: None,
            monthly: None,
        };
        let out = render(
            &outcome_for(&usage),
            &usage,
            &Theme::default(),
            &opts(),
            now,
        );
        assert!(out.tooltip.contains("80% ↑"), "{}", out.tooltip);
    }

    #[test]
    fn tooltip_monthly_row_keeps_reset_without_a_pace_glyph() {
        let now = at("2026-08-16T18:00:00Z");
        let usage = Usage {
            rolling: None,
            weekly: None,
            monthly: Some(Window {
                status: "ok".into(),
                percent: 70.0,
                resets_at: at("2026-08-31T18:00:00Z"),
            }),
        };
        let out = render(
            &outcome_for(&usage),
            &usage,
            &Theme::default(),
            &opts(),
            now,
        );
        assert!(out.tooltip.contains("Monthly"), "{}", out.tooltip);
        assert!(out.tooltip.contains("70%"), "{}", out.tooltip);
        assert!(
            !out.tooltip.contains('↑') && !out.tooltip.contains('→') && !out.tooltip.contains('↓'),
            "monthly row must not grow a pace glyph: {}",
            out.tooltip
        );
        assert!(out.tooltip.contains("Resets in 15d 0h"), "{}", out.tooltip);
    }

    #[test]
    fn plan_and_status_are_sanitized() {
        let usage = Usage {
            rolling: Some(Window {
                status: "ok\u{1b}[31m\u{7}".into(),
                percent: 1.0,
                resets_at: at("2026-08-16T20:00:00Z"),
            }),
            weekly: None,
            monthly: None,
        };
        let values = build_placeholders_with_plan(
            "OpenCode\u{1b}[31m Go",
            &usage,
            at("2026-08-16T18:00:00Z"),
        );

        assert!(!values["plan"].contains('\u{1b}'));
        assert!(!values["ocg_rolling_status"].contains('\u{1b}'));
        assert!(!values["ocg_rolling_status"].contains('\u{7}'));
    }
}
