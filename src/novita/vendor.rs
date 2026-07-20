//! Novita AI renderer — bar text + bordered Pango tooltip. Balance-only, like
//! Kilo/DeepSeek, with a top-up / credit-limit / owed breakdown in the tooltip.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::format::{placeholders, substitute, updated_at_hm};
use crate::pacing::PaceSeverity;
use crate::pango::{color_span, escape, severity_color};
use crate::theme::Theme;
use crate::tooltip::{Line as TooltipLine, render_bordered};
use crate::usage::NovitaSnapshot;
use crate::vendor::{RenderOpts, VendorOutcome};
use crate::waybar::{Class, WaybarOutput};

use super::fetch::FetchOutcome;

pub const DEFAULT_FORMAT: &str = "{nv_balance}";

pub fn build_placeholders(snap: &NovitaSnapshot) -> HashMap<&'static str, String> {
    placeholders(vec![
        ("icon", "󰄔".to_string()),
        ("vendor_short", "nvt".to_string()),
        // Cross-vendor aliases — Novita has no rate-limit windows.
        ("session_pct", "0".to_string()),
        ("session_reset", "—".to_string()),
        ("weekly_pct", "0".to_string()),
        ("weekly_reset", "—".to_string()),
        ("plan", "Novita".to_string()),
        ("nv_balance", format_money(snap.available)),
        ("nv_cash", format_money(snap.cash)),
        ("nv_credit_limit", format_money(snap.credit_limit)),
        ("nv_owed", format_money(snap.outstanding)),
    ])
}

fn format_money(v: f64) -> String {
    if v < 0.0 {
        format!("-${:.2}", -v)
    } else {
        format!("${v:.2}")
    }
}

/// Severity keys on the absolute remaining USD balance (same thresholds as
/// Kilo/DeepSeek): running low = warmer color, empty = critical.
pub fn severity(snap: &NovitaSnapshot) -> PaceSeverity {
    if snap.available < 1.0 {
        PaceSeverity::Critical
    } else if snap.available < 5.0 {
        PaceSeverity::High
    } else if snap.available < 20.0 {
        PaceSeverity::Mid
    } else {
        PaceSeverity::Low
    }
}

pub fn render(
    outcome: &VendorOutcome,
    snap: &NovitaSnapshot,
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
    snap: &NovitaSnapshot,
    theme: &Theme,
    now: DateTime<Utc>,
) -> String {
    let blue = &theme.blue;
    let dim = &theme.dim;
    let fg = &theme.fg;
    let color = severity_color(severity(snap), theme);

    let mut lines: Vec<TooltipLine> = Vec::new();
    lines.push(TooltipLine::Center(format!(
        "<span font_weight='bold' foreground='{blue}'>Novita</span>"
    )));
    lines.push(TooltipLine::Sep);
    lines.push(TooltipLine::Body("".into()));

    lines.push(TooltipLine::Body(format!(
        " <span foreground='{fg}'>  󰢗  Balance</span>"
    )));
    lines.push(TooltipLine::Body(format!(
        "   <span font_weight='bold' foreground='{color}'>{bal}</span>",
        bal = escape(&format_money(snap.available))
    )));
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{dim}'>     top-up {cash} · credit limit {lim}</span>",
        cash = escape(&format_money(snap.cash)),
        lim = escape(&format_money(snap.credit_limit))
    )));

    if snap.outstanding > 0.0 {
        lines.push(TooltipLine::Body("".into()));
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{dim}'>  󰆑  owed {owed}</span>",
            owed = escape(&format_money(snap.outstanding))
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
        Self {
            snapshot: crate::usage::VendorSnapshot::Novita(o.snapshot),
            stale: o.stale,
            last_error: o.last_error,
            cache_age: o.cache_age,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::NovitaSnapshot;

    fn sample_snap() -> NovitaSnapshot {
        NovitaSnapshot {
            available: 100.0,
            cash: 80.0,
            credit_limit: 20.0,
            outstanding: 0.0,
        }
    }

    fn sample_outcome(snap: NovitaSnapshot) -> VendorOutcome {
        VendorOutcome {
            snapshot: crate::usage::VendorSnapshot::Novita(snap),
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
    fn default_render_shows_available_balance() {
        let snap = sample_snap();
        let out = render(
            &sample_outcome(snap.clone()),
            &snap,
            &Theme::default(),
            &opts(),
            Utc::now(),
        );
        assert!(out.text.contains("$100.00"));
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
        assert!(out.tooltip.contains("$100.00"));
        assert!(out.tooltip.contains("top-up $80.00"));
        assert!(out.tooltip.contains("credit limit $20.00"));
    }

    #[test]
    fn owed_line_only_when_positive() {
        let mut snap = sample_snap();
        snap.outstanding = 3.0;
        let out = render(
            &sample_outcome(snap.clone()),
            &snap,
            &Theme::default(),
            &opts(),
            Utc::now(),
        );
        assert!(out.tooltip.contains("owed $3.00"));
    }

    #[test]
    fn severity_scales_with_balance() {
        let mk = |b: f64| NovitaSnapshot {
            available: b,
            cash: 0.0,
            credit_limit: 0.0,
            outstanding: 0.0,
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
        o.tooltip_format = Some("bal: {nv_balance} · owed {nv_owed}".into());
        let out = render(
            &sample_outcome(snap.clone()),
            &snap,
            &Theme::default(),
            &o,
            Utc::now(),
        );
        assert_eq!(out.tooltip, "bal: $100.00 · owed $0.00");
    }
}
