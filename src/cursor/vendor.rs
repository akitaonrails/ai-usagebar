//! Cursor renderer — bar text + bordered Pango tooltip. Two included-usage
//! pools (Cursor Models / Other Models), like Kimi's two windows but keyed on
//! model category rather than a time window.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::countdown;
use crate::format::{placeholders, substitute, updated_at_hm};
use crate::pacing::PaceSeverity;
use crate::pango::{color_span, escape, severity_color, severity_for};
use crate::theme::Theme;
use crate::tooltip::{Line as TooltipLine, render_bordered};
use crate::usage::CursorSnapshot;
use crate::vendor::{RenderOpts, VendorOutcome};
use crate::waybar::{Class, WaybarOutput};

use super::fetch::FetchOutcome;

pub const DEFAULT_FORMAT: &str = "{cursor_auto_pct}·{cursor_api_pct}%";

/// No confirmed Cursor glyph in the common Nerd Font sets, so a plain caret.
/// Override with `--icon`.
const DEFAULT_ICON: &str = "❯";

pub fn build_placeholders(
    snap: &CursorSnapshot,
    now: DateTime<Utc>,
) -> HashMap<&'static str, String> {
    let reset = countdown::format(snap.reset_at, now);
    placeholders(vec![
        ("icon", DEFAULT_ICON.to_string()),
        ("vendor_short", "cur".to_string()),
        // Cross-vendor aliases: the two pools map onto the two generic windows
        // (session = Cursor Models, weekly = Other Models) so a shared format
        // and the macOS menu bar show both. Severity still keys on the worst.
        ("plan", format!("Cursor {}", snap.plan)),
        ("session_pct", snap.auto_pct.to_string()),
        ("session_reset", reset.clone()),
        ("weekly_pct", snap.api_pct.to_string()),
        ("weekly_reset", reset.clone()),
        // Cursor-specific placeholders.
        ("cursor_plan", snap.plan.clone()),
        ("cursor_auto_pct", snap.auto_pct.to_string()),
        ("cursor_api_pct", snap.api_pct.to_string()),
        ("cursor_total_pct", snap.total_pct.to_string()),
        ("cursor_reset", reset),
        (
            "cursor_on_demand",
            if snap.on_demand_enabled { "on" } else { "off" }.to_string(),
        ),
        (
            "cursor_unlimited",
            if snap.unlimited { "yes" } else { "no" }.to_string(),
        ),
    ])
}

/// Severity keys on the binding pool. An unlimited plan has no cap, so it stays
/// calm regardless of the (meaningless) percentages.
pub fn severity(snap: &CursorSnapshot) -> PaceSeverity {
    if snap.unlimited {
        PaceSeverity::Low
    } else {
        severity_for(snap.worst_pct())
    }
}

pub fn render(
    outcome: &VendorOutcome,
    snap: &CursorSnapshot,
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

    let mut text = if snap.unlimited && opts.format.is_none() {
        "unlimited".to_string()
    } else {
        substitute(&format, &values)
    };
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

fn pool_line(lines: &mut Vec<TooltipLine>, theme: &Theme, label: &str, pct: i32) {
    let fg = &theme.fg;
    let color = severity_color(severity_for(pct), theme);
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{fg}'>  󰢻  {label}</span>"
    )));
    lines.push(TooltipLine::Body(format!(
        "   <span font_weight='bold' foreground='{color}'>{pct}%</span> used"
    )));
}

fn render_tooltip(
    outcome: &VendorOutcome,
    snap: &CursorSnapshot,
    theme: &Theme,
    now: DateTime<Utc>,
) -> String {
    let blue = &theme.blue;
    let dim = &theme.dim;
    let fg = &theme.fg;

    let mut lines: Vec<TooltipLine> = Vec::new();
    lines.push(TooltipLine::Center(format!(
        "<span font_weight='bold' foreground='{blue}'>Cursor {}</span>",
        escape(&snap.plan)
    )));
    lines.push(TooltipLine::Sep);
    lines.push(TooltipLine::Body("".into()));

    if snap.unlimited {
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{fg}'>  󰐾  Unlimited plan</span>"
        )));
    } else {
        pool_line(&mut lines, theme, "Cursor Models", snap.auto_pct);
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{dim}'>     Auto + Composer</span>"
        )));
        lines.push(TooltipLine::Body("".into()));
        pool_line(&mut lines, theme, "Other Models", snap.api_pct);
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{dim}'>     Named / API models · on-demand {}</span>",
            if snap.on_demand_enabled { "on" } else { "off" }
        )));
    }

    lines.push(TooltipLine::Body("".into()));
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{dim}'>  󰃰  Resets {}</span>",
        escape(&countdown::format(snap.reset_at, now))
    )));

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
        Self {
            snapshot: crate::usage::VendorSnapshot::Cursor(o.snapshot),
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
        Utc.with_ymd_and_hms(2026, 7, 26, 12, 0, 0).unwrap()
    }

    fn sample_snap() -> CursorSnapshot {
        CursorSnapshot {
            plan: "Ultra".into(),
            auto_pct: 98,
            api_pct: 100,
            total_pct: 99,
            unlimited: false,
            on_demand_enabled: false,
            reset_at: Some(now() + chrono::Duration::days(9)),
        }
    }

    fn sample_outcome(snap: CursorSnapshot) -> VendorOutcome {
        VendorOutcome {
            snapshot: crate::usage::VendorSnapshot::Cursor(snap),
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
    fn default_bar_shows_both_pools() {
        let snap = sample_snap();
        let out = render(
            &sample_outcome(snap.clone()),
            &snap,
            &Theme::default(),
            &opts(),
            now(),
        );
        assert!(out.text.contains("98·100%"), "text: {}", out.text);
    }

    #[test]
    fn tooltip_breaks_out_both_pools_and_reset() {
        let snap = sample_snap();
        let out = render(
            &sample_outcome(snap.clone()),
            &snap,
            &Theme::default(),
            &opts(),
            now(),
        );
        assert!(out.tooltip.contains("Cursor Ultra"));
        assert!(out.tooltip.contains("Cursor Models"));
        assert!(out.tooltip.contains("98%"));
        assert!(out.tooltip.contains("Other Models"));
        assert!(out.tooltip.contains("100%"));
        assert!(out.tooltip.contains("9d"));
    }

    #[test]
    fn severity_keys_on_the_worst_pool() {
        let mut snap = sample_snap();
        snap.auto_pct = 10;
        snap.api_pct = 95;
        // Other Models at 95% must drive severity Critical even though Cursor
        // Models is calm.
        assert_eq!(severity(&snap), PaceSeverity::Critical);
    }

    #[test]
    fn unlimited_plan_is_calm_and_labeled() {
        let mut snap = sample_snap();
        snap.unlimited = true;
        let out = render(
            &sample_outcome(snap.clone()),
            &snap,
            &Theme::default(),
            &opts(),
            now(),
        );
        assert_eq!(severity(&snap), PaceSeverity::Low);
        assert!(out.text.contains("unlimited"));
        assert!(out.tooltip.contains("Unlimited plan"));
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
    fn custom_tooltip_uses_placeholders() {
        let snap = sample_snap();
        let mut o = opts();
        o.tooltip_format = Some("auto {cursor_auto_pct} api {cursor_api_pct} {cursor_plan}".into());
        let out = render(
            &sample_outcome(snap.clone()),
            &snap,
            &Theme::default(),
            &o,
            now(),
        );
        assert_eq!(out.tooltip, "auto 98 api 100 Ultra");
    }

    #[test]
    fn generic_windows_map_to_the_two_pools() {
        let values = build_placeholders(&sample_snap(), now());
        assert_eq!(values["session_pct"], "98"); // Cursor Models (auto)
        assert_eq!(values["weekly_pct"], "100"); // Other Models (api)
        assert_eq!(values["plan"], "Cursor Ultra");
    }

    #[test]
    fn placeholder_set_contains_all_keys() {
        let values = build_placeholders(&sample_snap(), now());
        for key in [
            "icon",
            "vendor_short",
            "plan",
            "session_pct",
            "session_reset",
            "weekly_pct",
            "weekly_reset",
            "cursor_plan",
            "cursor_auto_pct",
            "cursor_api_pct",
            "cursor_total_pct",
            "cursor_reset",
            "cursor_on_demand",
            "cursor_unlimited",
        ] {
            assert!(values.contains_key(key), "missing placeholder {key}");
        }
    }

    #[test]
    fn fetch_outcome_conversion_preserves_metadata() {
        let fetch = FetchOutcome {
            snapshot: sample_snap(),
            stale: true,
            last_error: Some((401, "bad".into())),
            cache_age: Some(std::time::Duration::from_secs(42)),
        };
        let vendor: VendorOutcome = fetch.into();
        assert!(matches!(
            vendor.snapshot,
            crate::usage::VendorSnapshot::Cursor(_)
        ));
        assert!(vendor.stale);
    }
}
