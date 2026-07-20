//! OpenRouter renderer — bar text + bordered Pango tooltip.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::format::{placeholders, substitute, updated_at_hm};
use crate::pacing::PaceSeverity;
use crate::pango::{self, color_span, escape, severity_color, severity_for};
use crate::theme::Theme;
use crate::tooltip::{Line as TooltipLine, render_bordered};
use crate::usage::OpenRouterSnapshot;
use crate::vendor::{RenderOpts, VendorOutcome};
use crate::waybar::{Class, WaybarOutput};

use super::fetch::FetchOutcome;

pub const DEFAULT_FORMAT: &str = "${or_balance} · ${or_used_today}";

/// Build the placeholder map for the OpenRouter snapshot.
pub fn build_placeholders(snap: &OpenRouterSnapshot) -> HashMap<&'static str, String> {
    placeholders(vec![
        ("icon", "󱙺".to_string()),
        ("vendor_short", "opr".to_string()),
        // Cross-vendor aliases — for OpenRouter the "session" concept maps
        // to "credit consumed %", and there's no reset time so we render "—".
        ("session_pct", snap.consumed_pct().to_string()),
        ("session_reset", "—".to_string()),
        ("weekly_pct", snap.consumed_pct().to_string()),
        ("weekly_reset", "—".to_string()),
        ("plan", snap.label.clone()),
        ("or_label", snap.label.clone()),
        ("or_balance", format_money(snap.balance())),
        ("or_total", format_money(snap.total_credits)),
        ("or_used", format_money(snap.total_usage)),
        ("or_used_today", format_money(snap.usage_daily)),
        ("or_used_week", format_money(snap.usage_weekly)),
        ("or_used_month", format_money(snap.usage_monthly)),
        ("or_consumed_pct", snap.consumed_pct().to_string()),
        (
            "or_free_tier",
            (if snap.is_free_tier { "free" } else { "paid" }).into(),
        ),
        (
            "or_limit",
            snap.limit
                .map(format_money)
                .unwrap_or_else(|| "unlimited".into()),
        ),
        (
            "or_limit_remaining",
            snap.limit_remaining
                .map(format_money)
                .unwrap_or_else(|| "unlimited".into()),
        ),
    ])
}

fn format_money(v: f64) -> String {
    if v < 0.0 {
        format!("-${:.2}", -v)
    } else {
        format!("${v:.2}")
    }
}

/// Compose the full Waybar output for an OpenRouter snapshot.
pub fn render(
    outcome: &VendorOutcome,
    snap: &OpenRouterSnapshot,
    theme: &Theme,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> WaybarOutput {
    let class = Class::from(severity(snap));
    let format = opts
        .format
        .clone()
        .unwrap_or_else(|| DEFAULT_FORMAT.replace('$', ""));
    let mut values = build_placeholders(snap);
    // The default format above uses ${…} (legible in a shell context), so
    // strip the $ to match our placeholder syntax.
    values.insert("or_balance_bar", or_balance_bar(snap, theme));
    // Both sinks fed by this map (bar text and --tooltip-format) end up as
    // Pango markup, and the label is API-controlled — escape it here, its only
    // markup insertion point. The default tooltip escapes the raw snapshot
    // itself, and or_balance_bar is markup we emit, so neither is touched.
    for key in ["plan", "or_label"] {
        if let Some(value) = values.get_mut(key) {
            *value = escape(value);
        }
    }

    let format_clean = format.replace("${", "{");
    let mut text = substitute(&format_clean, &values);
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
        substitute(&fmt.replace("${", "{"), &values)
    } else {
        render_tooltip(outcome, snap, theme, now)
    };

    WaybarOutput {
        text: bar_text,
        tooltip,
        class,
    }
}

fn or_balance_bar(snap: &OpenRouterSnapshot, theme: &Theme) -> String {
    let pct = snap.consumed_pct();
    let color = severity_color(severity_for(pct), theme);
    pango::progress_bar(pct, color, theme, None)
}

/// OpenRouter severity is keyed on consumed-percentage (low credit = critical).
pub fn severity(snap: &OpenRouterSnapshot) -> PaceSeverity {
    severity_for(snap.consumed_pct())
}

fn render_tooltip(
    outcome: &VendorOutcome,
    snap: &OpenRouterSnapshot,
    theme: &Theme,
    now: DateTime<Utc>,
) -> String {
    let blue = &theme.blue;
    let dim = &theme.dim;
    let fg = &theme.fg;

    let color = severity_color(severity(snap), theme);
    let bar = or_balance_bar(snap, theme);

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
        "   {bar}  <span font_weight='bold' foreground='{color}'>{bal}</span>",
        bal = escape(&format_money(snap.balance()))
    )));
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{dim}'>  {used} of {total} used ({pct}%)</span>",
        used = escape(&format_money(snap.total_usage)),
        total = escape(&format_money(snap.total_credits)),
        pct = snap.consumed_pct()
    )));

    lines.push(TooltipLine::Body("".into()));
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{fg}'>  󰸘  Usage</span>"
    )));
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{dim}'>     today {today} · week {week} · month {month}</span>",
        today = escape(&format_money(snap.usage_daily)),
        week = escape(&format_money(snap.usage_weekly)),
        month = escape(&format_money(snap.usage_monthly))
    )));

    if let Some(limit) = snap.limit {
        let rem = snap.limit_remaining.unwrap_or(0.0);
        lines.push(TooltipLine::Body("".into()));
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{fg}'>  󱁻  Per-key limit</span>"
        )));
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{dim}'>     {rem} of {tot} remaining</span>",
            rem = escape(&format_money(rem)),
            tot = escape(&format_money(limit))
        )));
    }

    let tier_label = if snap.is_free_tier {
        "free tier"
    } else {
        "paid tier"
    };
    lines.push(TooltipLine::Body("".into()));
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{dim}'>  󰓹  {tier_label}</span>"
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
            snapshot: crate::usage::VendorSnapshot::Openrouter(o.snapshot),
            stale: o.stale,
            last_error: o.last_error,
            cache_age: o.cache_age,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::OpenRouterSnapshot;

    fn sample_snap() -> OpenRouterSnapshot {
        OpenRouterSnapshot {
            label: "OpenRouter — prod".into(),
            total_credits: 100.0,
            total_usage: 25.5,
            usage_daily: 1.0,
            usage_weekly: 7.0,
            usage_monthly: 25.5,
            is_free_tier: false,
            limit: Some(50.0),
            limit_remaining: Some(24.5),
        }
    }

    fn sample_outcome(snap: OpenRouterSnapshot) -> VendorOutcome {
        VendorOutcome {
            snapshot: crate::usage::VendorSnapshot::Openrouter(snap),
            stale: false,
            last_error: None,
            cache_age: Some(std::time::Duration::from_secs(15)),
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
    fn api_controlled_label_is_pango_escaped_in_custom_formats() {
        // The label comes from the API and both sinks fed by the placeholder
        // map are Pango markup. Unescaped, a label like `a & b <span…>` either
        // breaks the markup or lets an attacker-shaped key name paint text the
        // widget never intended.
        let mut snap = sample_snap();
        snap.label = "A & B <b>spoof</b>".into();

        let mut o = opts();
        o.format = Some("{plan}|{or_label}".into());
        let out = render(
            &sample_outcome(snap.clone()),
            &snap,
            &Theme::default(),
            &o,
            Utc::now(),
        );
        assert!(
            out.text.contains("A &amp; B &lt;b&gt;spoof&lt;/b&gt;"),
            "label was not escaped: {}",
            out.text
        );
        assert!(!out.text.contains("<b>spoof</b>"));

        // ...and the same through the tooltip sink.
        let mut o2 = opts();
        o2.tooltip_format = Some("{or_label}".into());
        let out2 = render(
            &sample_outcome(snap.clone()),
            &snap,
            &Theme::default(),
            &o2,
            Utc::now(),
        );
        assert!(out2.tooltip.contains("A &amp; B"));
        assert!(!out2.tooltip.contains("<b>spoof</b>"));
    }

    #[test]
    fn default_render_contains_balance_and_today_usage() {
        let snap = sample_snap();
        let outcome = sample_outcome(snap.clone());
        let theme = Theme::default();
        let out = render(&outcome, &snap, &theme, &opts(), Utc::now());
        assert!(out.text.contains("$74.50"));
        assert!(out.text.contains("$1.00"));
    }

    #[test]
    fn tooltip_includes_balance_usage_tier() {
        let snap = sample_snap();
        let outcome = sample_outcome(snap.clone());
        let theme = Theme::default();
        let out = render(&outcome, &snap, &theme, &opts(), Utc::now());
        assert!(out.tooltip.contains("Balance"));
        assert!(out.tooltip.contains("$25.50 of $100.00 used"));
        assert!(out.tooltip.contains("paid tier"));
        assert!(out.tooltip.contains("Per-key limit"));
        assert!(out.tooltip.contains("$24.50 of $50.00"));
    }

    #[test]
    fn free_tier_label() {
        let mut snap = sample_snap();
        snap.is_free_tier = true;
        let outcome = sample_outcome(snap.clone());
        let theme = Theme::default();
        let out = render(&outcome, &snap, &theme, &opts(), Utc::now());
        assert!(out.tooltip.contains("free tier"));
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
    fn custom_tooltip_uses_placeholders() {
        let snap = sample_snap();
        let outcome = sample_outcome(snap.clone());
        let theme = Theme::default();
        let mut o = opts();
        o.tooltip_format = Some("bal: {or_balance} | mtd: {or_used_month}".into());
        let out = render(&outcome, &snap, &theme, &o, Utc::now());
        assert_eq!(out.tooltip, "bal: $74.50 | mtd: $25.50");
    }

    #[test]
    fn severity_keys_on_consumed_pct() {
        let mut snap = sample_snap();
        snap.total_usage = 92.0; // 92% consumed → critical
        assert_eq!(severity(&snap), PaceSeverity::Critical);
        snap.total_usage = 60.0; // mid
        assert_eq!(severity(&snap), PaceSeverity::Mid);
    }
}
