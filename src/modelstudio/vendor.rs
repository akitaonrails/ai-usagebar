//! Model Studio renderer — bar text + bordered Pango tooltip. A dual-window
//! vendor (the Codex/Kimi shape): a 5-hour and a weekly window, each with its
//! own percent and reset, either of which the account may not report. An
//! absent window is rendered absent — never as 0%, which would invent an
//! exhaustion the wire never claimed (it may even mean unlimited).

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::countdown;
use crate::format::{placeholders, substitute, updated_at_hm};
use crate::pacing::{self, PaceSeverity};
use crate::pango::{color_span, escape, severity_color, severity_for};
use crate::theme::Theme;
use crate::tooltip::{Line as TooltipLine, push_window, render_bordered};
use crate::usage::{ModelStudioSnapshot, UsageWindow};
use crate::vendor::{RenderOpts, VendorId, VendorOutcome};
use crate::waybar::{Class, WaybarOutput};

use super::fetch::FetchOutcome;

/// The API reports no plan name; the label falls back to the vendor display
/// name, the single source frontends are allowed to carry.
pub const PLAN_LABEL: &str = "Model Studio";

pub const DEFAULT_FORMAT: &str = "5h {mst_session_pct}% · 7d {mst_weekly_pct}%";
const WEEKLY_ONLY_FORMAT: &str = "7d {mst_weekly_pct}% · {mst_weekly_reset}";
const NO_WINDOWS_FORMAT: &str = "{plan}";

pub fn build_placeholders(
    snap: &ModelStudioSnapshot,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> HashMap<&'static str, String> {
    let session = window_placeholders(snap.session.as_ref(), opts, now);
    let weekly = window_placeholders(snap.weekly.as_ref(), opts, now);

    placeholders(vec![
        ("icon", VendorId::ModelStudio.bar_icon().to_string()),
        (
            "vendor_short",
            VendorId::ModelStudio.short_name().to_string(),
        ),
        // Cross-vendor aliases — same names work across all vendors so a
        // single `--format '{vendor_short} {session_pct}% · {session_reset}'`
        // renders correctly during scroll-cycle. Absent windows expand to the
        // empty string, the codebase's missing-placeholder convention.
        ("session_pct", session.pct.clone()),
        ("session_reset", session.reset.clone()),
        ("session_elapsed", session.elapsed.clone()),
        ("weekly_pct", weekly.pct.clone()),
        ("weekly_reset", weekly.reset.clone()),
        ("weekly_elapsed", weekly.elapsed.clone()),
        ("plan", PLAN_LABEL.to_string()),
        // Model Studio placeholders.
        ("mst_plan", PLAN_LABEL.to_string()),
        ("mst_session_pct", session.pct),
        ("mst_session_reset", session.reset),
        ("mst_session_elapsed", session.elapsed),
        ("mst_session_pace", session.ratio_pace),
        ("mst_session_pace_indicator", session.point_pace),
        ("mst_weekly_pct", weekly.pct),
        ("mst_weekly_reset", weekly.reset),
        ("mst_weekly_elapsed", weekly.elapsed),
        ("mst_weekly_pace", weekly.ratio_pace),
        ("mst_weekly_pace_indicator", weekly.point_pace),
    ])
}

#[derive(Default)]
struct WindowPlaceholderValues {
    pct: String,
    reset: String,
    elapsed: String,
    ratio_pace: String,
    point_pace: String,
}

fn window_placeholders(
    window: Option<&UsageWindow>,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> WindowPlaceholderValues {
    let Some(window) = window else {
        return WindowPlaceholderValues::default();
    };
    let pace = pacing::calc(
        window.utilization_pct,
        window.resets_at,
        now,
        window.window_duration,
        opts.pace_tolerance,
    );
    WindowPlaceholderValues {
        pct: window.utilization_pct.to_string(),
        reset: countdown::format(window.resets_at, now),
        elapsed: pace.elapsed_pct.to_string(),
        ratio_pace: pace.ratio_pace.glyph().to_string(),
        point_pace: pace.point_pace.glyph().to_string(),
    }
}

/// Worst of the windows the account actually reports.
pub fn severity(snap: &ModelStudioSnapshot) -> PaceSeverity {
    let max = [snap.session.as_ref(), snap.weekly.as_ref()]
        .into_iter()
        .flatten()
        .map(|window| window.utilization_pct)
        .max()
        .unwrap_or(0);
    severity_for(max)
}

pub fn render(
    outcome: &VendorOutcome,
    snap: &ModelStudioSnapshot,
    theme: &Theme,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> WaybarOutput {
    let class = Class::from(severity(snap));
    let format = opts
        .format
        .clone()
        .unwrap_or_else(|| default_format(snap).to_string());
    let values = build_placeholders(snap, opts, now);

    let mut text = substitute(&format, &values);
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
        substitute(fmt, &values)
    } else {
        render_tooltip(outcome, snap, theme, now)
    };

    WaybarOutput {
        text: bar_text,
        tooltip,
        class,
    }
}

fn default_format(snap: &ModelStudioSnapshot) -> &'static str {
    if snap.session.is_some() {
        return DEFAULT_FORMAT;
    }
    if snap.weekly.is_some() {
        return WEEKLY_ONLY_FORMAT;
    }
    NO_WINDOWS_FORMAT
}

fn render_tooltip(
    outcome: &VendorOutcome,
    snap: &ModelStudioSnapshot,
    theme: &Theme,
    now: DateTime<Utc>,
) -> String {
    let blue = &theme.blue;
    let dim = &theme.dim;

    let mut lines: Vec<TooltipLine> = Vec::new();
    lines.push(TooltipLine::Center(format!(
        "<span font_weight='bold' foreground='{blue}'>{}</span>",
        escape(PLAN_LABEL)
    )));
    lines.push(TooltipLine::Sep);
    lines.push(TooltipLine::Body("".into()));

    if let Some(session) = snap.session.as_ref() {
        push_window(&mut lines, "  󰔟  Token Plan 5h", session, theme, now, None);
    }
    if let Some(weekly) = snap.weekly.as_ref() {
        if snap.session.is_some() {
            lines.push(TooltipLine::Body("".into()));
        }
        push_window(&mut lines, "  󰃰  Token Plan 7d", weekly, theme, now, None);
    }
    if snap.session.is_none() && snap.weekly.is_none() {
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{dim}'>  no usage windows reported</span>"
        )));
    }

    if let Some((code, msg)) = outcome.last_error.as_ref()
        && *code != 0
    {
        let (icon, ecolor) = if *code >= 500 {
            ("󰅚", theme.red.as_str())
        } else {
            ("󰀪", theme.orange.as_str())
        };
        lines.push(TooltipLine::Body("".into()));
        lines.push(TooltipLine::Sep);
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{ecolor}'>  {icon}  HTTP {code}</span>"
        )));
        lines.push(TooltipLine::Body(format!(
            "     <span foreground='{dim}'>{}</span>",
            escape(msg)
        )));
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
        o.map(crate::usage::VendorSnapshot::ModelStudio)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modelstudio::types::FIVE_HOUR_WINDOW;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 23, 12, 0, 0).unwrap()
    }

    fn window(pct: i32, mins_ahead: i64) -> UsageWindow {
        UsageWindow {
            utilization_pct: pct,
            resets_at: Some(now() + chrono::Duration::minutes(mins_ahead)),
            window_duration: FIVE_HOUR_WINDOW,
        }
    }

    fn snap() -> ModelStudioSnapshot {
        ModelStudioSnapshot {
            session: Some(window(42, 90)),
            weekly: Some(UsageWindow {
                utilization_pct: 74,
                resets_at: Some(now() + chrono::Duration::days(3)),
                window_duration: chrono::Duration::days(7),
            }),
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

    fn outcome(s: &ModelStudioSnapshot) -> VendorOutcome {
        VendorOutcome {
            snapshot: crate::usage::VendorSnapshot::ModelStudio(s.clone()),
            stale: false,
            last_error: None,
            cache_age: Some(std::time::Duration::ZERO),
        }
    }

    #[test]
    fn default_render_shows_both_windows() {
        let s = snap();
        let out = render(&outcome(&s), &s, &Theme::default(), &opts(), now());
        assert!(out.text.contains("42%"), "{}", out.text);
        assert!(out.text.contains("74%"), "{}", out.text);
    }

    /// Cross-vendor aliases are what the desktop surfaces read.
    #[test]
    fn exposes_cross_vendor_aliases() {
        let values = build_placeholders(&snap(), &opts(), now());
        assert_eq!(values.get("session_pct").map(String::as_str), Some("42"));
        assert_eq!(values.get("weekly_pct").map(String::as_str), Some("74"));
        assert_eq!(values.get("vendor_short").map(String::as_str), Some("mst"));
        assert_eq!(values.get("plan").map(String::as_str), Some("Model Studio"));
        assert!(values.contains_key("mst_session_reset"));
    }

    /// An absent percentage is no-data/possibly-unlimited — the window drops
    /// out of every default surface, and never renders as 0%.
    #[test]
    fn an_absent_window_is_never_a_zero_percent() {
        let s = ModelStudioSnapshot {
            session: None,
            weekly: Some(window(74, 200)),
        };
        let out = render(&outcome(&s), &s, &Theme::default(), &opts(), now());
        assert!(out.text.contains("74%"), "{}", out.text);
        assert!(!out.text.contains("0%"), "{}", out.text);
        // No "5h " window label — "3h 20m" of reset countdown is not one.
        assert!(!out.text.contains("5h "), "{}", out.text);

        let values = build_placeholders(&s, &opts(), now());
        assert_eq!(values.get("mst_session_pct").map(String::as_str), Some(""));
        assert_eq!(
            values.get("mst_session_reset").map(String::as_str),
            Some("")
        );

        let tip = render_tooltip(&outcome(&s), &s, &Theme::default(), now());
        assert!(tip.contains("7d"), "{tip}");
        assert!(!tip.contains("0%"), "{tip}");
    }

    /// Both windows absent: an honest label, not "5h 0%".
    #[test]
    fn no_windows_at_all_renders_the_label() {
        let s = ModelStudioSnapshot {
            session: None,
            weekly: None,
        };
        let out = render(&outcome(&s), &s, &Theme::default(), &opts(), now());
        assert!(!out.text.contains("0%"), "{}", out.text);
        assert!(out.text.contains("Model Studio"), "{}", out.text);
        let tip = render_tooltip(&outcome(&s), &s, &Theme::default(), now());
        assert!(tip.contains("no usage windows reported"), "{tip}");
        assert_eq!(severity(&s), PaceSeverity::Low);
    }

    #[test]
    fn severity_is_the_worst_reported_window() {
        assert_eq!(severity(&snap()), severity_for(74));
        let s = ModelStudioSnapshot {
            session: Some(window(97, 10)),
            weekly: Some(window(12, 300)),
        };
        assert_eq!(severity(&s), severity_for(97));
    }

    #[test]
    fn tooltip_draws_a_bar_for_each_reported_window() {
        let s = snap();
        let tip = render_tooltip(&outcome(&s), &s, &Theme::default(), now());
        let cells = tip.matches('░').count() + tip.matches('█').count();
        assert_eq!(cells, 2 * crate::pango::BAR_LEN as usize, "{tip}");
        assert!(tip.contains("5h") && tip.contains("7d"), "{tip}");
    }

    #[test]
    fn custom_format_and_tooltip_use_placeholders() {
        let s = snap();
        let mut o = opts();
        o.format = Some("{vendor_short} {weekly_pct}/{session_pct}".into());
        o.tooltip_format = Some("w {mst_weekly_pct} · s {mst_session_pct}".into());
        let out = render(&outcome(&s), &s, &Theme::default(), &o, now());
        assert!(out.text.contains("mst 74/42"), "{}", out.text);
        assert_eq!(out.tooltip, "w 74 · s 42");
    }

    #[test]
    fn stale_marks_the_bar() {
        let s = snap();
        let mut o = outcome(&s);
        o.stale = true;
        let out = render(&o, &s, &Theme::default(), &opts(), now());
        assert!(out.text.contains('⏸'));
    }

    /// A recorded non-HTTP failure (code 0) carries no HTTP row; a 5xx does.
    #[test]
    fn last_error_rows_follow_the_code() {
        let s = snap();
        let mut o = outcome(&s);
        o.last_error = Some((0, "run `bl auth login --console` to re-auth".into()));
        let tip = render_tooltip(&o, &s, &Theme::default(), now());
        assert!(!tip.contains("HTTP 0"), "{tip}");

        o.last_error = Some((503, "gateway unavailable".into()));
        let tip = render_tooltip(&o, &s, &Theme::default(), now());
        assert!(tip.contains("HTTP 503"), "{tip}");
    }
}
