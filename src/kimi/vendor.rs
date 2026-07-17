//! Kimi renderer — bar text + bordered Pango tooltip.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::countdown;
use crate::format::{placeholders, substitute, updated_at_hm};
use crate::pacing::PaceSeverity;
use crate::pango::{color_span, escape, severity_color, severity_for};
use crate::theme::Theme;
use crate::tooltip::{Line as TooltipLine, render_bordered};
use crate::usage::KimiSnapshot;
use crate::vendor::{RenderOpts, VendorOutcome};
use crate::waybar::{Class, WaybarOutput};

use super::fetch::{FetchOutcome, SCHEMA_DRIFT_MESSAGE};

pub const DEFAULT_FORMAT: &str = "{kimi_weekly_pct}%";

pub fn build_placeholders(
    snap: &KimiSnapshot,
    now: DateTime<Utc>,
) -> HashMap<&'static str, String> {
    let plan = snap.plan.as_deref().unwrap_or("Kimi");
    let weekly_pct = snap.weekly_pct();
    let window_pct = snap.window_pct();
    placeholders(vec![
        ("icon", "󰚩".to_string()),
        ("vendor_short", "kmi".to_string()),
        // Cross-vendor aliases.
        ("plan", plan.to_string()),
        ("weekly_pct", weekly_pct.to_string()),
        ("weekly_reset", countdown::format(snap.weekly_reset_at, now)),
        ("session_pct", window_pct.to_string()),
        (
            "session_reset",
            countdown::format(snap.window_reset_at, now),
        ),
        // Kimi-specific placeholders.
        ("kimi_plan", plan.to_string()),
        ("kimi_weekly_pct", weekly_pct.to_string()),
        ("kimi_weekly_used", snap.weekly_used.to_string()),
        ("kimi_weekly_limit", snap.weekly_limit.to_string()),
        ("kimi_weekly_remaining", snap.weekly_remaining.to_string()),
        (
            "kimi_weekly_reset",
            countdown::format(snap.weekly_reset_at, now),
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
    severity_for(snap.weekly_pct().max(snap.window_pct()))
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

    let weekly_pct = snap.weekly_pct();
    let weekly_color = severity_color(severity_for(weekly_pct), theme);
    let window_pct = snap.window_pct();
    let window_color = severity_color(severity_for(window_pct), theme);

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
        "   <span font_weight='bold' foreground='{weekly_color}'>{}</span>",
        escape(plan)
    )));

    lines.push(TooltipLine::Body("".into()));
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{fg}'>  󰅄  Weekly quota</span>"
    )));
    lines.push(TooltipLine::Body(format!(
        "   <span font_weight='bold' foreground='{weekly_color}'>{used} / {limit}</span>  ({pct}%)",
        used = snap.weekly_used,
        limit = snap.weekly_limit,
        pct = weekly_pct
    )));
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{dim}'>     {remaining} remaining · reset {reset}</span>",
        remaining = snap.weekly_remaining,
        reset = escape(&countdown::format(snap.weekly_reset_at, now))
    )));

    if snap.window_limit > 0 {
        lines.push(TooltipLine::Body("".into()));
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{fg}'>  󰅁  Rolling window</span>"
        )));
        lines.push(TooltipLine::Body(format!(
            "   <span font_weight='bold' foreground='{window_color}'>{used} / {limit}</span>  ({pct}%)",
            used = snap.window_used,
            limit = snap.window_limit,
            pct = window_pct
        )));
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{dim}'>     {remaining} remaining · reset {reset}</span>",
            remaining = snap.window_remaining,
            reset = escape(&countdown::format(snap.window_reset_at, now))
        )));
    }

    if let Some((code, msg)) = outcome.last_error.as_ref() {
        let (label, icon, ecolor) = if *code == 0 && msg == SCHEMA_DRIFT_MESSAGE {
            ("Kimi API schema drift".to_string(), "󰅚", theme.red.as_str())
        } else if *code == 0 {
            ("Kimi error".to_string(), "󰅚", theme.red.as_str())
        } else if *code >= 500 {
            (format!("HTTP {code}"), "󰅚", theme.red.as_str())
        } else {
            (format!("HTTP {code}"), "󰀪", theme.orange.as_str())
        };
        lines.push(TooltipLine::Body("".into()));
        lines.push(TooltipLine::Sep);
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{ecolor}'>  {icon}  {label}</span>"
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
        Self {
            snapshot: crate::usage::VendorSnapshot::Kimi(o.snapshot),
            stale: o.stale,
            last_error: o.last_error,
            cache_age: o.cache_age,
        }
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
    fn default_render_has_exactly_one_percent() {
        let snap = sample_snap();
        let outcome = sample_outcome(snap.clone());
        let out = render(&outcome, &snap, &Theme::default(), &opts(), now());
        // "26%" should appear exactly once and there must be no double percent.
        assert!(out.text.contains("26%"), "text: {}", out.text);
        assert!(
            !out.text.contains("%%"),
            "double percent in text: {}",
            out.text
        );
        assert_eq!(out.text.matches('%').count(), 1, "text: {}", out.text);
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
    }

    #[test]
    fn generic_code_zero_error_is_not_labeled_schema_drift() {
        let snap = sample_snap();
        let mut outcome = sample_outcome(snap.clone());
        outcome.stale = true;
        outcome.last_error = Some((0, "cache lock unavailable".into()));
        let out = render(&outcome, &snap, &Theme::default(), &opts(), now());
        assert!(out.tooltip.contains("Kimi error"));
        assert!(!out.tooltip.contains("Kimi API schema drift"));
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
        assert!(out.tooltip.contains("26 / 100"));
        assert!(out.tooltip.contains("Rolling window"));
        assert!(out.tooltip.contains("15 / 100"));
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
}
