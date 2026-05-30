//! DeepSeek renderer — bar text + bordered Pango tooltip.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::format::{placeholders, substitute, updated_at_hm};
use crate::pacing::PaceSeverity;
use crate::pango::{color_span, escape, severity_color};
use crate::theme::Theme;
use crate::tooltip::{Line as TooltipLine, render_bordered};
use crate::usage::DeepseekSnapshot;
use crate::vendor::{RenderOpts, VendorOutcome};
use crate::waybar::{Class, WaybarOutput};

use super::fetch::FetchOutcome;

pub const DEFAULT_FORMAT: &str = "{ds_balance}";

pub fn build_placeholders(snap: &DeepseekSnapshot) -> HashMap<&'static str, String> {
    let avail = if snap.is_available { "up" } else { "down" };
    placeholders(vec![
        ("icon", "󰧑".to_string()),
        ("vendor_short", "dsk".to_string()),
        // Cross-vendor aliases — no rate-limit windows for DeepSeek.
        ("session_pct", "0".to_string()),
        ("session_reset", "—".to_string()),
        ("weekly_pct", "0".to_string()),
        ("weekly_reset", "—".to_string()),
        ("plan", "DeepSeek".to_string()),
        ("ds_balance", format_money(snap.balance, &snap.currency)),
        ("ds_granted", format_money(snap.granted, &snap.currency)),
        ("ds_topped_up", format_money(snap.topped_up, &snap.currency)),
        ("ds_available", avail.to_string()),
        ("currency", snap.currency.clone()),
    ])
}

fn format_money(v: f64, currency: &str) -> String {
    match currency {
        "USD" => format!("${v:.2}"),
        "CNY" => format!("¥{v:.2}"),
        _ => format!("{v:.2} {currency}"),
    }
}

pub fn severity(snap: &DeepseekSnapshot) -> PaceSeverity {
    if !snap.is_available {
        return PaceSeverity::Critical;
    }
    // Thresholds scaled by currency. CNY ≈ 7× USD (rough parity).
    // critical / high / mid boundaries in each currency unit.
    let (t_critical, t_high, t_mid) = match snap.currency.as_str() {
        "CNY" => (7.0_f64, 35.0, 140.0),
        _ => (1.0_f64, 5.0, 20.0), // USD and unknowns treated as USD-scale
    };
    if snap.balance < t_critical {
        PaceSeverity::Critical
    } else if snap.balance < t_high {
        PaceSeverity::High
    } else if snap.balance < t_mid {
        PaceSeverity::Mid
    } else {
        PaceSeverity::Low
    }
}

pub fn render(
    outcome: &VendorOutcome,
    snap: &DeepseekSnapshot,
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
    snap: &DeepseekSnapshot,
    theme: &Theme,
    now: DateTime<Utc>,
) -> String {
    let blue = &theme.blue;
    let dim = &theme.dim;
    let fg = &theme.fg;
    let color = severity_color(severity(snap), theme);

    let avail_label = if snap.is_available {
        "API available"
    } else {
        "API unavailable"
    };

    let mut lines: Vec<TooltipLine> = Vec::new();
    lines.push(TooltipLine::Center(format!(
        "<span font_weight='bold' foreground='{blue}'>DeepSeek</span>"
    )));
    lines.push(TooltipLine::Sep);
    lines.push(TooltipLine::Body("".into()));

    lines.push(TooltipLine::Body(format!(
        " <span foreground='{fg}'>  󰢗  Balance</span>"
    )));
    lines.push(TooltipLine::Body(format!(
        "   <span font_weight='bold' foreground='{color}'>{bal}</span>",
        bal = escape(&format_money(snap.balance, &snap.currency))
    )));
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{dim}'>     granted {granted} · topped-up {topped}</span>",
        granted = escape(&format_money(snap.granted, &snap.currency)),
        topped = escape(&format_money(snap.topped_up, &snap.currency))
    )));

    lines.push(TooltipLine::Body("".into()));
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{dim}'>  󰛴  {avail_label}</span>"
    )));

    if let Some((code, msg)) = outcome.last_error.as_ref() {
        if *code != 0 {
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
            snapshot: crate::usage::VendorSnapshot::Deepseek(o.snapshot),
            stale: o.stale,
            last_error: o.last_error,
            cache_age: o.cache_age,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::DeepseekSnapshot;

    fn sample_snap() -> DeepseekSnapshot {
        DeepseekSnapshot {
            is_available: true,
            balance: 5.50,
            granted: 5.00,
            topped_up: 0.50,
            currency: "USD".into(),
        }
    }

    fn sample_outcome(snap: DeepseekSnapshot) -> VendorOutcome {
        VendorOutcome {
            snapshot: crate::usage::VendorSnapshot::Deepseek(snap),
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
        let theme = Theme::default();
        let out = render(&outcome, &snap, &theme, &opts(), Utc::now());
        assert!(out.text.contains("$5.50"));
    }

    #[test]
    fn tooltip_includes_balance_and_availability() {
        let snap = sample_snap();
        let outcome = sample_outcome(snap.clone());
        let theme = Theme::default();
        let out = render(&outcome, &snap, &theme, &opts(), Utc::now());
        assert!(out.tooltip.contains("Balance"));
        assert!(out.tooltip.contains("$5.50"));
        assert!(out.tooltip.contains("API available"));
    }

    #[test]
    fn unavailable_api_shows_critical_severity() {
        let mut snap = sample_snap();
        snap.is_available = false;
        assert_eq!(severity(&snap), PaceSeverity::Critical);
    }

    #[test]
    fn stale_appends_pause() {
        let snap = sample_snap();
        let mut outcome = sample_outcome(snap.clone());
        outcome.stale = true;
        let theme = Theme::default();
        let out = render(&outcome, &snap, &theme, &opts(), Utc::now());
        assert!(out.text.contains("⏸"));
    }

    #[test]
    fn cny_format() {
        let snap = DeepseekSnapshot {
            is_available: true,
            balance: 20.0,
            granted: 20.0,
            topped_up: 0.0,
            currency: "CNY".into(),
        };
        assert_eq!(format_money(snap.balance, &snap.currency), "¥20.00");
    }
}
