//! Moonshot / Kimi renderer — bar text + bordered Pango tooltip. Balance-only,
//! currency-aware (USD on `.ai`, CNY on `.cn`).

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::format::{placeholders, substitute, updated_at_hm};
use crate::pacing::PaceSeverity;
use crate::pango::{color_span, escape, severity_color};
use crate::theme::Theme;
use crate::tooltip::{Line as TooltipLine, render_bordered};
use crate::usage::MoonshotSnapshot;
use crate::vendor::{RenderOpts, VendorOutcome};
use crate::waybar::{Class, WaybarOutput};

use super::fetch::FetchOutcome;

pub const DEFAULT_FORMAT: &str = "{km_balance}";

pub fn build_placeholders(snap: &MoonshotSnapshot) -> HashMap<&'static str, String> {
    placeholders(vec![
        ("icon", "󰚩".to_string()),
        ("vendor_short", "kmi".to_string()),
        // Cross-vendor aliases — Kimi has no rate-limit windows here.
        ("session_pct", "0".to_string()),
        ("session_reset", "—".to_string()),
        ("weekly_pct", "0".to_string()),
        ("weekly_reset", "—".to_string()),
        ("plan", "Kimi".to_string()),
        ("km_balance", format_money(snap.available, &snap.currency)),
        ("km_voucher", format_money(snap.voucher, &snap.currency)),
        ("km_cash", format_money(snap.cash, &snap.currency)),
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

/// `available_balance <= 0` blocks the inference API, so that's critical.
/// Otherwise scale the low/high/mid thresholds by currency (CNY ≈ 7× USD),
/// mirroring DeepSeek.
pub fn severity(snap: &MoonshotSnapshot) -> PaceSeverity {
    if snap.available <= 0.0 {
        return PaceSeverity::Critical;
    }
    let (t_critical, t_high, t_mid) = match snap.currency.as_str() {
        "CNY" => (7.0_f64, 35.0, 140.0),
        _ => (1.0_f64, 5.0, 20.0),
    };
    if snap.available < t_critical {
        PaceSeverity::Critical
    } else if snap.available < t_high {
        PaceSeverity::High
    } else if snap.available < t_mid {
        PaceSeverity::Mid
    } else {
        PaceSeverity::Low
    }
}

pub fn render(
    outcome: &VendorOutcome,
    snap: &MoonshotSnapshot,
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
    snap: &MoonshotSnapshot,
    theme: &Theme,
    now: DateTime<Utc>,
) -> String {
    let blue = &theme.blue;
    let dim = &theme.dim;
    let fg = &theme.fg;
    let color = severity_color(severity(snap), theme);

    let mut lines: Vec<TooltipLine> = Vec::new();
    lines.push(TooltipLine::Center(format!(
        "<span font_weight='bold' foreground='{blue}'>Kimi (Moonshot)</span>"
    )));
    lines.push(TooltipLine::Sep);
    lines.push(TooltipLine::Body("".into()));

    lines.push(TooltipLine::Body(format!(
        " <span foreground='{fg}'>  󰢗  Balance</span>"
    )));
    lines.push(TooltipLine::Body(format!(
        "   <span font_weight='bold' foreground='{color}'>{bal}</span>",
        bal = escape(&format_money(snap.available, &snap.currency))
    )));
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{dim}'>     cash {cash} · voucher {voucher}</span>",
        cash = escape(&format_money(snap.cash, &snap.currency)),
        voucher = escape(&format_money(snap.voucher, &snap.currency))
    )));

    if snap.available <= 0.0 {
        lines.push(TooltipLine::Body("".into()));
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{}'>  󰀪  out of credit — inference blocked</span>",
            theme.orange.as_str()
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
            snapshot: crate::usage::VendorSnapshot::Moonshot(o.snapshot),
            stale: o.stale,
            last_error: o.last_error,
            cache_age: o.cache_age,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::MoonshotSnapshot;

    fn sample_snap() -> MoonshotSnapshot {
        MoonshotSnapshot {
            available: 49.58,
            voucher: 46.58,
            cash: 3.0,
            currency: "USD".into(),
        }
    }

    fn sample_outcome(snap: MoonshotSnapshot) -> VendorOutcome {
        VendorOutcome {
            snapshot: crate::usage::VendorSnapshot::Moonshot(snap),
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
        let out = render(&sample_outcome(snap.clone()), &snap, &Theme::default(), &opts(), Utc::now());
        assert!(out.text.contains("$49.58"));
    }

    #[test]
    fn tooltip_includes_cash_and_voucher() {
        let snap = sample_snap();
        let out = render(&sample_outcome(snap.clone()), &snap, &Theme::default(), &opts(), Utc::now());
        assert!(out.tooltip.contains("cash $3.00"));
        assert!(out.tooltip.contains("voucher $46.58"));
    }

    #[test]
    fn cny_uses_yuan_symbol() {
        let mut snap = sample_snap();
        snap.currency = "CNY".into();
        let out = render(&sample_outcome(snap.clone()), &snap, &Theme::default(), &opts(), Utc::now());
        assert!(out.text.contains("¥49.58"));
    }

    #[test]
    fn zero_balance_is_critical() {
        let mut snap = sample_snap();
        snap.available = 0.0;
        assert_eq!(severity(&snap), PaceSeverity::Critical);
    }
}
