//! Kilo Code renderer — bar text + bordered Pango tooltip. Balance-only, like
//! DeepSeek (a remaining-credit number with no rate-limit windows).

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::format::{placeholders, substitute, updated_at_hm};
use crate::pacing::PaceSeverity;
use crate::pango::{color_span, escape, severity_color};
use crate::theme::Theme;
use crate::tooltip::{Line as TooltipLine, render_bordered};
use crate::usage::KiloSnapshot;
use crate::vendor::{RenderOpts, VendorOutcome};
use crate::waybar::{Class, WaybarOutput};

use super::fetch::FetchOutcome;

pub const DEFAULT_FORMAT: &str = "{kilo_balance}";

pub fn build_placeholders(snap: &KiloSnapshot) -> HashMap<&'static str, String> {
    placeholders(vec![
        ("icon", "󰭟".to_string()),
        ("vendor_short", "klo".to_string()),
        // Cross-vendor aliases — Kilo has no rate-limit windows.
        ("session_pct", "0".to_string()),
        ("session_reset", "—".to_string()),
        ("weekly_pct", "0".to_string()),
        ("weekly_reset", "—".to_string()),
        ("plan", snap.label.clone()),
        ("kilo_balance", format_money(snap.balance)),
    ])
}

fn format_money(v: f64) -> String {
    if v < 0.0 {
        format!("-${:.2}", -v)
    } else {
        format!("${v:.2}")
    }
}

/// Kilo has no purchased-total on this endpoint, so severity keys on the
/// absolute remaining USD balance (mirrors DeepSeek's USD thresholds): running
/// low = warmer color, empty = critical (the `402` boundary).
pub fn severity(snap: &KiloSnapshot) -> PaceSeverity {
    if snap.balance < 1.0 {
        PaceSeverity::Critical
    } else if snap.balance < 5.0 {
        PaceSeverity::High
    } else if snap.balance < 20.0 {
        PaceSeverity::Mid
    } else {
        PaceSeverity::Low
    }
}

pub fn render(
    outcome: &VendorOutcome,
    snap: &KiloSnapshot,
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
    snap: &KiloSnapshot,
    theme: &Theme,
    now: DateTime<Utc>,
) -> String {
    let blue = &theme.blue;
    let dim = &theme.dim;
    let fg = &theme.fg;
    let color = severity_color(severity(snap), theme);

    let mut lines: Vec<TooltipLine> = Vec::new();
    lines.push(TooltipLine::Center(format!(
        "<span font_weight='bold' foreground='{blue}'>{label}</span>",
        label = escape(&snap.label)
    )));
    lines.push(TooltipLine::Sep);
    lines.push(TooltipLine::Body("".into()));

    lines.push(TooltipLine::Body(format!(
        " <span foreground='{fg}'>  󰢗  Balance</span>"
    )));
    lines.push(TooltipLine::Body(format!(
        "   <span font_weight='bold' foreground='{color}'>{bal}</span>",
        bal = escape(&format_money(snap.balance))
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
            snapshot: crate::usage::VendorSnapshot::Kilo(o.snapshot),
            stale: o.stale,
            last_error: o.last_error,
            cache_age: o.cache_age,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::KiloSnapshot;

    fn sample_snap() -> KiloSnapshot {
        KiloSnapshot {
            label: "Kilo".into(),
            balance: 8.42,
        }
    }

    fn sample_outcome(snap: KiloSnapshot) -> VendorOutcome {
        VendorOutcome {
            snapshot: crate::usage::VendorSnapshot::Kilo(snap),
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
        let outcome = sample_outcome(snap.clone());
        let out = render(&outcome, &snap, &Theme::default(), &opts(), Utc::now());
        assert!(out.text.contains("$8.42"));
    }

    #[test]
    fn tooltip_includes_balance() {
        let snap = sample_snap();
        let outcome = sample_outcome(snap.clone());
        let out = render(&outcome, &snap, &Theme::default(), &opts(), Utc::now());
        assert!(out.tooltip.contains("Balance"));
        assert!(out.tooltip.contains("$8.42"));
    }

    #[test]
    fn severity_scales_with_balance() {
        assert_eq!(
            severity(&KiloSnapshot {
                label: "".into(),
                balance: 0.5
            }),
            PaceSeverity::Critical
        );
        assert_eq!(
            severity(&KiloSnapshot {
                label: "".into(),
                balance: 3.0
            }),
            PaceSeverity::High
        );
        assert_eq!(
            severity(&KiloSnapshot {
                label: "".into(),
                balance: 12.0
            }),
            PaceSeverity::Mid
        );
        assert_eq!(
            severity(&KiloSnapshot {
                label: "".into(),
                balance: 50.0
            }),
            PaceSeverity::Low
        );
    }

    #[test]
    fn stale_appends_pause() {
        let snap = sample_snap();
        let mut outcome = sample_outcome(snap.clone());
        outcome.stale = true;
        let out = render(&outcome, &snap, &Theme::default(), &opts(), Utc::now());
        assert!(out.text.contains("⏸"));
    }

    #[test]
    fn custom_tooltip_uses_placeholder() {
        let snap = sample_snap();
        let outcome = sample_outcome(snap.clone());
        let mut o = opts();
        o.tooltip_format = Some("bal: {kilo_balance}".into());
        let out = render(&outcome, &snap, &Theme::default(), &o, Utc::now());
        assert_eq!(out.tooltip, "bal: $8.42");
    }
}
