//! Unlid renderer — bar text + bordered Pango tooltip. Balance-only, like
//! Novita/Kilo/DeepSeek, with a held / month-spend breakdown in the tooltip.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::format::{placeholders, substitute, updated_at_hm, usd};
use crate::pacing::PaceSeverity;
use crate::pango::{color_span, escape, severity_color};
use crate::theme::Theme;
use crate::tooltip::{Line as TooltipLine, render_bordered};
use crate::usage::UnlidSnapshot;
use crate::vendor::{RenderOpts, VendorId, VendorOutcome};
use crate::waybar::{Class, WaybarOutput};

use super::fetch::FetchOutcome;

pub const DEFAULT_FORMAT: &str = "{ul_balance}";

pub fn build_placeholders(snap: &UnlidSnapshot) -> HashMap<&'static str, String> {
    placeholders(vec![
        ("icon", VendorId::Unlid.short_name().to_string()),
        ("vendor_short", VendorId::Unlid.short_name().to_string()),
        // Cross-vendor aliases — Unlid has no rate-limit windows.
        ("session_pct", "0".to_string()),
        ("session_reset", "—".to_string()),
        ("weekly_pct", "0".to_string()),
        ("weekly_reset", "—".to_string()),
        ("plan", VendorId::Unlid.display_name().to_string()),
        ("ul_balance", usd(snap.balance)),
        ("ul_held", usd(snap.held)),
        ("ul_month", usd(snap.month_spend)),
    ])
}

/// Severity keys on the absolute remaining USD balance (same thresholds as
/// Kilo/DeepSeek): running low = warmer color, empty = critical.
pub fn severity(snap: &UnlidSnapshot) -> PaceSeverity {
    crate::pango::balance_severity(snap.balance, "USD")
}

pub fn render(
    outcome: &VendorOutcome,
    snap: &UnlidSnapshot,
    theme: &Theme,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> WaybarOutput {
    let class = Class::from(severity(snap));
    let format = opts
        .format
        .clone()
        .unwrap_or_else(|| DEFAULT_FORMAT.to_string());
    let values = build_placeholders(snap);

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

fn render_tooltip(
    outcome: &VendorOutcome,
    snap: &UnlidSnapshot,
    theme: &Theme,
    now: DateTime<Utc>,
) -> String {
    let blue = &theme.blue;
    let dim = &theme.dim;
    let fg = &theme.fg;
    let color = severity_color(severity(snap), theme);

    let mut lines: Vec<TooltipLine> = Vec::new();
    lines.push(TooltipLine::Center(format!(
        "<span font_weight='bold' foreground='{blue}'>Unlid</span>"
    )));
    lines.push(TooltipLine::Sep);
    lines.push(TooltipLine::Body("".into()));

    lines.push(TooltipLine::Body(format!(
        " <span foreground='{fg}'>  󰢗  Balance</span>"
    )));
    lines.push(TooltipLine::Body(format!(
        "   <span font_weight='bold' foreground='{color}'>{bal}</span>",
        bal = escape(&usd(snap.balance))
    )));
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{dim}'>     held {held} · this month {month}</span>",
        held = escape(&usd(snap.held)),
        month = escape(&usd(snap.month_spend))
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
        o.map(crate::usage::VendorSnapshot::Unlid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::UnlidSnapshot;

    fn sample_snap() -> UnlidSnapshot {
        UnlidSnapshot {
            balance: 12.34,
            held: 0.5,
            month_spend: 6.78,
        }
    }

    fn sample_outcome(snap: UnlidSnapshot) -> VendorOutcome {
        VendorOutcome {
            snapshot: crate::usage::VendorSnapshot::Unlid(snap),
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
    fn default_render_shows_balance() {
        let snap = sample_snap();
        let out = render(
            &sample_outcome(snap.clone()),
            &snap,
            &Theme::default(),
            &opts(),
            Utc::now(),
        );
        assert!(out.text.contains("$12.34"));
    }

    #[test]
    fn tooltip_includes_balance_and_breakdown() {
        let snap = sample_snap();
        let out = render(
            &sample_outcome(snap.clone()),
            &snap,
            &Theme::default(),
            &opts(),
            Utc::now(),
        );
        assert!(out.tooltip.contains("Balance"));
        assert!(out.tooltip.contains("$12.34"));
        assert!(out.tooltip.contains("held $0.50"));
        assert!(out.tooltip.contains("this month $6.78"));
    }

    #[test]
    fn severity_scales_with_balance() {
        let mk = |b: f64| UnlidSnapshot {
            balance: b,
            held: 0.0,
            month_spend: 0.0,
        };
        assert_eq!(severity(&mk(0.5)), PaceSeverity::Critical);
        assert_eq!(severity(&mk(3.0)), PaceSeverity::High);
        assert_eq!(severity(&mk(12.0)), PaceSeverity::Mid);
        assert_eq!(severity(&mk(50.0)), PaceSeverity::Low);
    }

    #[test]
    fn custom_tooltip_uses_placeholder() {
        let snap = sample_snap();
        let mut o = opts();
        o.tooltip_format = Some("bal: {ul_balance} · held {ul_held} · month {ul_month}".into());
        let out = render(
            &sample_outcome(snap.clone()),
            &snap,
            &Theme::default(),
            &o,
            Utc::now(),
        );
        assert_eq!(out.tooltip, "bal: $12.34 · held $0.50 · month $6.78");
    }
}
