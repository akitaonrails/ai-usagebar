//! SuperGrok renderer — current subscription credit % + reset countdown.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::countdown;
use crate::format::{
    placeholders, reset_credit_lines, reset_credits, substitute, updated_at_hm, usd,
};
use crate::pacing::PaceSeverity;
use crate::pango::{self, color_span, escape, severity_color, severity_for};
use crate::theme::Theme;
use crate::tooltip::{Line as TooltipLine, render_bordered};
use crate::usage::SuperGrokSnapshot;
use crate::vendor::{RenderOpts, VendorId, VendorOutcome};
use crate::waybar::{Class, WaybarOutput};

use super::fetch::FetchOutcome;

pub const DEFAULT_FORMAT: &str = "{sgk_pct}% · {sgk_reset}";

const DEFAULT_ICON: &str = "󰚩";

pub fn build_placeholders(
    snap: &SuperGrokSnapshot,
    now: DateTime<Utc>,
) -> HashMap<&'static str, String> {
    let pct = snap.weekly_pct.to_string();
    let reset = countdown::format(snap.reset_at, now);
    let prepaid = snap.prepaid_balance.map(usd).unwrap_or_else(|| "—".into());

    placeholders(vec![
        ("icon", DEFAULT_ICON.to_string()),
        ("vendor_short", VendorId::Supergrok.short_name().to_string()),
        // Cross-vendor compatibility aliases for the single current pool.
        ("plan", snap.plan.clone()),
        ("session_pct", pct.clone()),
        ("session_reset", reset.clone()),
        ("weekly_pct", pct.clone()),
        ("weekly_reset", reset.clone()),
        // SuperGrok-specific.
        ("sgk_plan", snap.plan.clone()),
        ("sgk_pct", pct),
        ("sgk_reset", reset),
        ("sgk_period", snap.period.label().to_string()),
        ("sgk_prepaid", prepaid),
        (
            "sgk_resets_available",
            snap.reset_credits.available.to_string(),
        ),
        ("sgk_resets", reset_credits(&snap.reset_credits)),
    ])
}

pub fn severity(snap: &SuperGrokSnapshot) -> PaceSeverity {
    severity_for(snap.weekly_pct)
}

pub fn render(
    outcome: &VendorOutcome,
    snap: &SuperGrokSnapshot,
    theme: &Theme,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> WaybarOutput {
    let class = Class::from(severity(snap));
    let format = opts
        .format
        .clone()
        .unwrap_or_else(|| DEFAULT_FORMAT.to_string());
    let mut values = build_placeholders(snap, now);
    for key in ["plan", "sgk_plan"] {
        if let Some(value) = values.get_mut(key) {
            *value = escape(value);
        }
    }

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

/// One usage-percent row in the same shape as `tooltip::push_window`:
/// label, progress bar + bold %, then optional dim reset line.
fn push_pct_row(
    lines: &mut Vec<TooltipLine>,
    theme: &Theme,
    label: &str,
    pct: i32,
    reset_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) {
    let fg = &theme.fg;
    let dim = &theme.dim;
    let color = severity_color(severity_for(pct), theme);
    let bar = pango::progress_bar(pct, color, theme, None);
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{fg}'>{label}</span>"
    )));
    lines.push(TooltipLine::Body(format!(
        "   {bar}  <span font_weight='bold' foreground='{color}'>{pct}%</span>"
    )));
    if reset_at.is_some() {
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{dim}'>  ⏱  Resets in {}</span>",
            escape(&countdown::format(reset_at, now))
        )));
    }
}

/// A product slice is information about the pool, not a meter of it: one dim
/// line — icon, label padded to the widest sibling, right-aligned bold % —
/// with no gauge and no severity colour, so only the overall meter above can
/// read as the binding constraint. Percentages never exceed three digits, so
/// `{:>4}%` aligns "100%" with "3%". Labels are untrusted billing text: pad
/// by rendered width first, then escape at this sink.
fn push_product_row(
    lines: &mut Vec<TooltipLine>,
    theme: &Theme,
    product: &crate::usage::SuperGrokProduct,
    label_width: usize,
) {
    let dim = &theme.dim;
    let label = crate::display::pad_end(&product.label, label_width);
    let pct = format!("{:>4}%", product.percent);
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{dim}'>  󰚩  {}  </span><span foreground='{dim}' font_weight='bold'>{pct}</span>",
        escape(&label)
    )));
}

fn render_tooltip(
    outcome: &VendorOutcome,
    snap: &SuperGrokSnapshot,
    theme: &Theme,
    now: DateTime<Utc>,
) -> String {
    let blue = &theme.blue;
    let dim = &theme.dim;
    let fg = &theme.fg;

    let mut lines: Vec<TooltipLine> = Vec::new();
    lines.push(TooltipLine::Center(format!(
        "<span font_weight='bold' foreground='{blue}'>{}</span>",
        escape(&snap.plan)
    )));
    lines.push(TooltipLine::Sep);
    lines.push(TooltipLine::Body("".into()));

    let period_label = format!("  󰔟  {} usage", snap.period.label());
    push_pct_row(
        &mut lines,
        theme,
        &period_label,
        snap.weekly_pct,
        snap.reset_at,
        now,
    );
    if !snap.products.is_empty() {
        let label_width = snap
            .products
            .iter()
            .map(|product| crate::display::text_width(&product.label))
            .max()
            .unwrap_or(0);
        for product in &snap.products {
            push_product_row(&mut lines, theme, product, label_width);
        }
    }

    // Same rule as the panel: a $0.00 prepaid line is noise, not information
    // (unified billing accounts keep their dollars in the Management API
    // wallet, which the `[grok]` vendor reports).
    if let Some(bal) = snap.prepaid_balance.filter(|bal| *bal > 0.0) {
        let bal_s = usd(bal);
        lines.push(TooltipLine::Body("".into()));
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{dim}'>  󰢗  Prepaid API  {}</span>",
            escape(&bal_s)
        )));
    }

    if snap.reset_credits.available > 0 {
        lines.push(TooltipLine::Body("".into()));
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{fg}'>  󰁯  Reset credits</span>"
        )));
        for line in reset_credit_lines(&snap.reset_credits, now) {
            lines.push(TooltipLine::Body(format!(
                " <span foreground='{dim}'>     {}</span>",
                escape(&line)
            )));
        }
    }

    if let Some((code, msg)) = outcome.last_error.as_ref() {
        let (icon, ecolor) = if *code >= 500 {
            ("󰅚", theme.red.as_str())
        } else {
            ("󰀪", theme.orange.as_str())
        };
        let label = if *code == 0 {
            "Refresh error".to_string()
        } else {
            format!("HTTP {code}")
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
        o.map(crate::usage::VendorSnapshot::SuperGrok)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::SuperGrokPeriod;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 5, 12, 0, 0).unwrap()
    }

    fn sample_snap() -> SuperGrokSnapshot {
        SuperGrokSnapshot {
            plan: "SuperGrok".into(),
            account: "user-1".into(),
            weekly_pct: 34,
            period: SuperGrokPeriod::Weekly,
            reset_at: Some(now() + chrono::Duration::hours(20)),
            prepaid_balance: Some(0.0),
            reset_credits: Default::default(),
            products: Vec::new(),
        }
    }

    fn sample_outcome(snap: SuperGrokSnapshot) -> VendorOutcome {
        VendorOutcome {
            snapshot: crate::usage::VendorSnapshot::SuperGrok(snap),
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
    fn renders_weekly_pct_and_reset() {
        let snap = sample_snap();
        let o = sample_outcome(snap.clone());
        let out = render(&o, &snap, &Theme::default(), &opts(), now());
        assert!(out.text.contains("34%"));
        assert!(out.tooltip.contains("usage"));
        assert!(!out.tooltip.contains("Build credits"));
        assert!(out.tooltip.contains("Weekly"));
        assert!(out.tooltip.contains("SuperGrok"));
        // Usage-% vendors (Anthropic / OpenAI) draw a filled progress bar in
        // the tooltip; SuperGrok must match that shape rather than bare %.
        assert!(
            out.tooltip.contains('█') || out.tooltip.contains('░'),
            "tooltip missing progress bar cells: {}",
            out.tooltip
        );
        assert!(out.tooltip.contains("Resets in"));
    }

    #[test]
    fn tooltip_lists_product_slices() {
        let mut snap = sample_snap();
        snap.products = vec![
            crate::usage::SuperGrokProduct {
                label: "Grok Build".into(),
                percent: 20,
            },
            crate::usage::SuperGrokProduct {
                label: "Grok Chat".into(),
                percent: 14,
            },
        ];
        let o = sample_outcome(snap.clone());
        let out = render(&o, &snap, &Theme::default(), &opts(), now());
        assert!(out.tooltip.contains("Grok Build"));
        assert!(out.tooltip.contains("Grok Chat"));
        // Each slice is one dim line: no gauge, no reset of its own, and the
        // percentages right-align in one column ("  20%" / "  14%") under the
        // overall meter's bar — the only bar line in the box.
        let product_lines: Vec<&str> = out
            .tooltip
            .lines()
            .filter(|line| line.contains("Grok Build") || line.contains("Grok Chat"))
            .collect();
        assert_eq!(product_lines.len(), 2, "{}", out.tooltip);
        for line in &product_lines {
            assert!(!line.contains('█') && !line.contains('░'), "{line}");
            assert!(!line.contains("Resets"), "{line}");
        }
        assert!(product_lines[0].contains("  20%</span>"));
        assert!(product_lines[1].contains("  14%</span>"));
        assert_eq!(
            out.tooltip
                .lines()
                .filter(|line| line.contains('█'))
                .count(),
            1,
            "{}",
            out.tooltip
        );
    }

    /// A hostile product name must not inject Pango markup into the tooltip:
    /// the label is escaped at the render sink, after width-based padding.
    #[test]
    fn tooltip_escapes_product_labels() {
        let mut snap = sample_snap();
        snap.products = vec![crate::usage::SuperGrokProduct {
            label: "Grok<b>Build</b>".into(),
            percent: 7,
        }];
        let o = sample_outcome(snap.clone());
        let out = render(&o, &snap, &Theme::default(), &opts(), now());
        assert!(out.tooltip.contains("Grok&lt;b&gt;"), "{}", out.tooltip);
        assert!(!out.tooltip.contains("<b>Build"), "{}", out.tooltip);
    }

    /// The prepaid line appears only when there is credit to show — a $0.00
    /// row reads as "no money" when the billing document merely reports that
    /// nothing was purchased on top of the subscription. The `{sgk_prepaid}`
    /// placeholder keeps reporting the raw figure either way.
    #[test]
    fn tooltip_lists_prepaid_only_when_there_is_credit() {
        let mut snap = sample_snap();
        snap.prepaid_balance = Some(0.0);
        let zero = render(
            &sample_outcome(snap.clone()),
            &snap,
            &Theme::default(),
            &opts(),
            now(),
        );
        assert!(!zero.tooltip.contains("Prepaid API"), "{}", zero.tooltip);
        assert_eq!(
            build_placeholders(&snap, now())
                .get("sgk_prepaid")
                .map(String::as_str),
            Some("$0.00")
        );

        snap.prepaid_balance = Some(4.22);
        let out = render(
            &sample_outcome(snap.clone()),
            &snap,
            &Theme::default(),
            &opts(),
            now(),
        );
        assert!(out.tooltip.contains("Prepaid API"), "{}", out.tooltip);
        assert!(out.tooltip.contains("$4.22"), "{}", out.tooltip);
    }

    #[test]
    fn tooltip_reports_banked_resets_and_stays_silent_without_them() {
        let snap = sample_snap();
        let quiet = render(
            &sample_outcome(snap.clone()),
            &snap,
            &Theme::default(),
            &opts(),
            now(),
        );
        assert!(!quiet.tooltip.contains("available"), "{}", quiet.tooltip);

        let mut snap = snap;
        snap.reset_credits = crate::usage::ResetCredits {
            available: 1,
            credits: vec![crate::usage::ResetCredit {
                title: None,
                expires_at: Some(now() + chrono::Duration::days(7)),
            }],
        };
        let out = render(
            &sample_outcome(snap.clone()),
            &snap,
            &Theme::default(),
            &opts(),
            now(),
        );
        assert!(out.tooltip.contains("Reset credits"), "{}", out.tooltip);
        assert!(out.tooltip.contains("Expires"), "{}", out.tooltip);

        let ph = build_placeholders(&snap, now());
        assert_eq!(
            ph.get("sgk_resets_available").map(String::as_str),
            Some("1")
        );
        assert!(ph["sgk_resets"].starts_with("1 reset available"));
    }

    #[test]
    fn high_usage_is_critical() {
        let mut snap = sample_snap();
        snap.weekly_pct = 95;
        assert_eq!(severity(&snap), PaceSeverity::Critical);
    }

    #[test]
    fn placeholders_include_generic_aliases() {
        let snap = sample_snap();
        let ph = build_placeholders(&snap, now());
        assert_eq!(ph.get("vendor_short").map(String::as_str), Some("sgk"));
        assert_eq!(ph.get("weekly_pct").map(String::as_str), Some("34"));
        assert_eq!(ph.get("session_pct").map(String::as_str), Some("34"));
        assert_eq!(ph.get("sgk_period").map(String::as_str), Some("Weekly"));
        assert_eq!(ph.get("sgk_prepaid").map(String::as_str), Some("$0.00"));
    }
}
