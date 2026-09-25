//! Grok Bot renderer — bar text + bordered Pango tooltip. One weekly meter,
//! plus the no-included-allowance state and the on-demand footnote.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::countdown;
use crate::format::{placeholders, substitute, updated_at_hm};
use crate::pacing::PaceSeverity;
use crate::pango::{color_span, escape, severity_color, severity_for};
use crate::theme::Theme;
use crate::tooltip::{Line as TooltipLine, WindowRow, push_window_with_row, render_bordered};
use crate::usage::{GrokbotSnapshot, UsageWindow};
use crate::vendor::{RenderOpts, VendorId, VendorOutcome};
use crate::waybar::{Class, WaybarOutput};

use super::fetch::FetchOutcome;

pub const DEFAULT_FORMAT: &str = "{gbt_weekly_pct}%";

pub fn build_placeholders(
    snap: &GrokbotSnapshot,
    now: DateTime<Utc>,
) -> HashMap<&'static str, String> {
    let reset = countdown::format(snap.reset_at, now);
    // With no included allowance the weekly placeholders resolve to empty
    // strings (the missing-placeholder convention) rather than a fabricated 0.
    let allowance = |value: String| {
        if snap.has_included_allowance {
            value
        } else {
            String::new()
        }
    };
    placeholders(vec![
        ("icon", VendorId::Grokbot.bar_icon().to_string()),
        ("vendor_short", VendorId::Grokbot.short_name().to_string()),
        // Cross-vendor aliases.
        ("plan", snap.plan.clone()),
        ("weekly_pct", allowance(snap.weekly_pct.to_string())),
        ("weekly_reset", allowance(reset.clone())),
        // Grok Bot-specific placeholders.
        ("gbt_plan", snap.plan.clone()),
        ("gbt_weekly_pct", allowance(snap.weekly_pct.to_string())),
        ("gbt_weekly_reset", allowance(reset)),
        (
            "gbt_on_demand",
            if snap.on_demand_enabled { "on" } else { "off" }.to_string(),
        ),
    ])
}

/// No included allowance reads as Low: there is no pool to be exhausting.
pub fn severity(snap: &GrokbotSnapshot) -> PaceSeverity {
    if snap.has_included_allowance {
        severity_for(snap.weekly_pct)
    } else {
        PaceSeverity::Low
    }
}

pub fn render(
    outcome: &VendorOutcome,
    snap: &GrokbotSnapshot,
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
    // User formats are Pango markup after Waybar renders them; the plan label
    // is API-controlled, so it is escaped at this projection boundary, once.
    let mut pango_values = values.clone();
    for key in ["plan", "gbt_plan"] {
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
    snap: &GrokbotSnapshot,
    theme: &Theme,
    now: DateTime<Utc>,
) -> String {
    let blue = &theme.blue;
    let dim = &theme.dim;
    let fg = &theme.fg;
    let plan_color = severity_color(severity(snap), theme);

    let mut lines: Vec<TooltipLine> = Vec::new();
    lines.push(TooltipLine::Center(format!(
        "<span font_weight='bold' foreground='{blue}'>Grok Bot</span>"
    )));
    lines.push(TooltipLine::Sep);
    lines.push(TooltipLine::Body("".into()));

    lines.push(TooltipLine::Body(format!(
        " <span foreground='{fg}'>  󰣖  Plan</span>"
    )));
    lines.push(TooltipLine::Body(format!(
        "   <span font_weight='bold' foreground='{plan_color}'>{}</span>",
        escape(&snap.plan)
    )));

    lines.push(TooltipLine::Body("".into()));
    if snap.has_included_allowance {
        // The window's length is derived from the reported instants, never
        // assumed — and only the reset carries a countdown.
        push_window_with_row(
            &mut lines,
            "  󰅄  Weekly included usage",
            &UsageWindow {
                utilization_pct: snap.weekly_pct,
                resets_at: snap.reset_at,
                window_duration: snap.window.unwrap_or_else(chrono::Duration::zero),
            },
            theme,
            now,
            WindowRow::default(),
        );
        if let Some(note) = snap.on_demand_note() {
            lines.push(TooltipLine::Body(format!(
                "     <span foreground='{dim}'>{note}</span>"
            )));
        }
    } else {
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{dim}'>  No included allowance on this account</span>"
        )));
    }

    if let Some((code, msg)) = outcome.last_error.as_ref() {
        // Code zero has never meant HTTP; the rest are real status codes.
        let (label, icon, ecolor) = match *code {
            0 => ("Grok Bot error".to_string(), "󰅚", theme.red.as_str()),
            code if code >= 500 => (format!("HTTP {code}"), "󰅚", theme.red.as_str()),
            code => (format!("HTTP {code}"), "󰀪", theme.orange.as_str()),
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
        o.map(crate::usage::VendorSnapshot::Grokbot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 14, 12, 0, 0).unwrap()
    }

    fn sample_snap() -> GrokbotSnapshot {
        GrokbotSnapshot {
            plan: "Grok Bot Plan".into(),
            billed_by: None,
            has_included_allowance: true,
            weekly_pct: 42,
            has_available_usage: true,
            on_demand_enabled: false,
            period_start: Some(now() - chrono::Duration::days(3)),
            reset_at: Some(now() + chrono::Duration::days(4)),
            window: Some(chrono::Duration::days(7)),
        }
    }

    fn sample_outcome(snap: GrokbotSnapshot) -> VendorOutcome {
        VendorOutcome {
            snapshot: crate::usage::VendorSnapshot::Grokbot(snap),
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
    fn default_render_shows_the_weekly_percent() {
        let snap = sample_snap();
        let outcome = sample_outcome(snap.clone());
        let out = render(&outcome, &snap, &Theme::default(), &opts(), now());
        assert!(out.text.contains("42%"), "text: {}", out.text);
        assert!(!out.text.contains("%%"), "text: {}", out.text);
    }

    #[test]
    fn placeholder_set_contains_all_keys() {
        let snap = sample_snap();
        let values = build_placeholders(&snap, now());
        for key in [
            "gbt_plan",
            "gbt_weekly_pct",
            "gbt_weekly_reset",
            "gbt_on_demand",
            "plan",
            "weekly_pct",
            "weekly_reset",
            "vendor_short",
        ] {
            assert!(values.contains_key(key), "missing placeholder {key}");
        }
        assert_eq!(values["gbt_weekly_pct"], "42");
        assert_eq!(values["weekly_pct"], "42");
        assert_eq!(values["gbt_on_demand"], "off");
        assert!(!values["gbt_weekly_reset"].is_empty());
    }

    #[test]
    fn no_allowance_renders_empty_weekly_placeholders_not_a_zero() {
        let snap = GrokbotSnapshot {
            has_included_allowance: false,
            weekly_pct: 0,
            ..sample_snap()
        };
        let values = build_placeholders(&snap, now());
        for key in [
            "gbt_weekly_pct",
            "gbt_weekly_reset",
            "weekly_pct",
            "weekly_reset",
        ] {
            assert_eq!(values[key], "", "{key} must render empty");
        }
        // …and the default format must not leave literal braces behind. The
        // dangling `%` is the kimi monthly-shape convention: substitution
        // tolerates the shape rather than fabricating a figure.
        let outcome = sample_outcome(snap.clone());
        let out = render(&outcome, &snap, &Theme::default(), &opts(), now());
        assert!(!out.text.contains('{'), "{}", out.text);
        assert!(!out.text.contains('}'), "{}", out.text);
        assert!(out.text.contains('%'), "{}", out.text);
        assert!(
            !out.text.contains('0'),
            "a fabricated 0 must not appear: {}",
            out.text
        );
        assert_eq!(severity(&snap), PaceSeverity::Low);
    }

    #[test]
    fn tooltip_draws_the_weekly_bar_and_a_reset_countdown() {
        let snap = sample_snap();
        let outcome = sample_outcome(snap.clone());
        let out = render(&outcome, &snap, &Theme::default(), &opts(), now());
        assert!(out.tooltip.contains("Grok Bot"), "{}", out.tooltip);
        assert!(out.tooltip.contains("Grok Bot Plan"), "{}", out.tooltip);
        assert!(
            out.tooltip.contains("Weekly included usage"),
            "{}",
            out.tooltip
        );
        assert!(out.tooltip.contains("42%"), "{}", out.tooltip);
        assert!(out.tooltip.contains("Resets in"), "{}", out.tooltip);
        // A countdown, not the raw timestamp.
        assert!(!out.tooltip.contains("2026-09-18"), "{}", out.tooltip);
    }

    #[test]
    fn the_no_allowance_state_is_a_text_line_not_a_meter() {
        let snap = GrokbotSnapshot {
            has_included_allowance: false,
            weekly_pct: 0,
            ..sample_snap()
        };
        let outcome = sample_outcome(snap.clone());
        let out = render(&outcome, &snap, &Theme::default(), &opts(), now());
        assert!(
            out.tooltip.contains("No included allowance"),
            "{}",
            out.tooltip
        );
        assert!(
            !out.tooltip.contains("Weekly included usage"),
            "{}",
            out.tooltip
        );
    }

    #[test]
    fn the_on_demand_footnote_appears_only_at_an_exhausted_pool_with_on_demand() {
        let mut snap = sample_snap();
        snap.weekly_pct = 100;
        snap.has_available_usage = true;
        snap.on_demand_enabled = true;
        let outcome = sample_outcome(snap.clone());
        let out = render(&outcome, &snap, &Theme::default(), &opts(), now());
        assert!(out.tooltip.contains("on-demand"), "{}", out.tooltip);

        snap.on_demand_enabled = false;
        let outcome = sample_outcome(snap.clone());
        let out = render(&outcome, &snap, &Theme::default(), &opts(), now());
        assert!(!out.tooltip.contains("on-demand may"), "{}", out.tooltip);
    }

    #[test]
    fn plan_is_pango_escaped() {
        let mut snap = sample_snap();
        snap.plan = "A&B <beta>".into();
        let outcome = sample_outcome(snap.clone());
        let out = render(&outcome, &snap, &Theme::default(), &opts(), now());
        assert!(
            out.tooltip.contains("A&amp;B &lt;beta&gt;"),
            "tooltip: {}",
            out.tooltip
        );
        let mut o = opts();
        o.tooltip_format = Some("{gbt_plan}".into());
        let out = render(&outcome, &snap, &Theme::default(), &o, now());
        assert_eq!(out.tooltip, "A&amp;B &lt;beta&gt;");
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
    fn fetch_outcome_conversion_preserves_metadata() {
        let snap = sample_snap();
        let fetch = FetchOutcome {
            snapshot: snap,
            stale: true,
            last_error: Some((401, "bad".into())),
            cache_age: Some(std::time::Duration::from_secs(42)),
        };
        let vendor: VendorOutcome = fetch.into();
        assert!(matches!(
            vendor.snapshot,
            crate::usage::VendorSnapshot::Grokbot(_)
        ));
        assert!(vendor.stale);
        assert_eq!(vendor.last_error, Some((401, "bad".into())));
        assert_eq!(vendor.cache_age, Some(std::time::Duration::from_secs(42)));
    }
}
