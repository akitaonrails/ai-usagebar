//! OrcaRouter renderer — bar text + bordered Pango tooltip. A balance card
//! like OpenRouter's (spend / limit / remaining), spend-only when the key is
//! unlimited, with an optional key-expiry countdown.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::countdown;
use crate::format::{placeholders, substitute, updated_at_hm, usd};
use crate::pacing::PaceSeverity;
use crate::pango::{self, color_span, escape, severity_color};
use crate::theme::Theme;
use crate::tooltip::{Line as TooltipLine, render_bordered};
use crate::usage::OrcaRouterSnapshot;
use crate::vendor::{RenderOpts, VendorId, VendorOutcome};
use crate::waybar::{Class, WaybarOutput};

use super::fetch::FetchOutcome;

pub const DEFAULT_FORMAT: &str = "{orc_remaining}";
/// Default bar format for an unlimited (sentinel) key: there is no remaining
/// figure, so the card is spend-only — the same treatment a missing monthly
/// limit gets.
pub const SPEND_ONLY_FORMAT: &str = "{orc_spend} spent";

/// Build the placeholder map for the OrcaRouter snapshot.
pub fn build_placeholders(
    snap: &OrcaRouterSnapshot,
    now: DateTime<Utc>,
) -> HashMap<&'static str, String> {
    let consumed_pct = snap.consumed_pct();
    placeholders(vec![
        ("icon", VendorId::OrcaRouter.bar_icon().to_string()),
        (
            "vendor_short",
            VendorId::OrcaRouter.short_name().to_string(),
        ),
        // Cross-vendor aliases — with a limit the session concept maps to
        // consumed %; an unlimited key has no percentage anybody can vouch
        // for, and there is no rate-limit window to reset.
        (
            "session_pct",
            consumed_pct
                .map(|p| p.to_string())
                .unwrap_or_else(|| "—".into()),
        ),
        ("session_reset", "—".to_string()),
        (
            "weekly_pct",
            consumed_pct
                .map(|p| p.to_string())
                .unwrap_or_else(|| "—".into()),
        ),
        ("weekly_reset", "—".to_string()),
        ("plan", "OrcaRouter".to_string()),
        ("orc_spend", usd(snap.spent_usd())),
        (
            "orc_limit",
            snap.limit_usd()
                .map(usd)
                .unwrap_or_else(|| "unlimited".into()),
        ),
        (
            "orc_remaining",
            snap.remaining_usd()
                .map(usd)
                .unwrap_or_else(|| "unlimited".into()),
        ),
        (
            "orc_consumed_pct",
            consumed_pct
                .map(|p| p.to_string())
                .unwrap_or_else(|| "—".into()),
        ),
        ("orc_expires", countdown::format(snap.access_until, now)),
    ])
}

/// Severity keys on the remaining credit (the actionable number, mirroring
/// the balance vendors); an unlimited key has no remaining figure, so it
/// stays calm — the same treatment a missing monthly limit gets.
pub fn severity(snap: &OrcaRouterSnapshot) -> PaceSeverity {
    match snap.remaining_usd() {
        Some(remaining) => crate::pango::balance_severity(remaining, "USD"),
        None => PaceSeverity::Low,
    }
}

/// Compose the full Waybar output for an OrcaRouter snapshot.
pub fn render(
    outcome: &VendorOutcome,
    snap: &OrcaRouterSnapshot,
    theme: &Theme,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> WaybarOutput {
    let class = Class::from(severity(snap));
    let format = opts.format.clone().unwrap_or_else(|| {
        if snap.limit_cents.is_some() {
            DEFAULT_FORMAT.to_string()
        } else {
            SPEND_ONLY_FORMAT.to_string()
        }
    });
    let mut values = build_placeholders(snap, now);
    values.insert("orc_bar", orc_bar(snap, theme));

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

fn orc_bar(snap: &OrcaRouterSnapshot, theme: &Theme) -> String {
    let color = severity_color(severity(snap), theme);
    pango::progress_bar(snap.consumed_pct().unwrap_or(0), color, theme, None)
}

fn render_tooltip(
    outcome: &VendorOutcome,
    snap: &OrcaRouterSnapshot,
    theme: &Theme,
    now: DateTime<Utc>,
) -> String {
    let blue = &theme.blue;
    let dim = &theme.dim;
    let fg = &theme.fg;

    let color = severity_color(severity(snap), theme);

    let mut lines: Vec<TooltipLine> = Vec::new();
    lines.push(TooltipLine::Center(format!(
        "<span font_weight='bold' foreground='{blue}'>OrcaRouter</span>"
    )));
    lines.push(TooltipLine::Sep);
    lines.push(TooltipLine::Body("".into()));

    lines.push(TooltipLine::Body(format!(
        " <span foreground='{fg}'>  󰢗  Credit</span>"
    )));
    match snap.remaining_usd() {
        Some(remaining) => {
            lines.push(TooltipLine::Body(format!(
                "   {bar}  <span font_weight='bold' foreground='{color}'>{rem}</span>",
                bar = orc_bar(snap, theme),
                rem = escape(&usd(remaining))
            )));
            lines.push(TooltipLine::Body(format!(
                " <span foreground='{dim}'>  {spent} of {limit} used ({pct}%)</span>",
                spent = escape(&usd(snap.spent_usd())),
                limit = escape(&usd(snap.limit_usd().unwrap_or_default())),
                pct = snap.consumed_pct().unwrap_or(0)
            )));
        }
        None => {
            // Unlimited key: no limit, no percentage — spend-only.
            lines.push(TooltipLine::Body(format!(
                "   <span font_weight='bold' foreground='{color}'>{spent} spent</span>",
                spent = escape(&usd(snap.spent_usd()))
            )));
            lines.push(TooltipLine::Body(format!(
                " <span foreground='{dim}'>  no credit limit on this key</span>"
            )));
        }
    }

    if let Some(access_until) = snap.access_until {
        lines.push(TooltipLine::Body("".into()));
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{dim}'>  󰅐  Key expires in {left}</span>",
            left = escape(&countdown::format(Some(access_until), now))
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
        o.map(crate::usage::VendorSnapshot::OrcaRouter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::OrcaRouterSnapshot;

    fn sample_snap() -> OrcaRouterSnapshot {
        OrcaRouterSnapshot {
            spent_cents: 275,
            limit_cents: Some(1250),
            access_until: None,
        }
    }

    fn sample_outcome(snap: OrcaRouterSnapshot) -> VendorOutcome {
        VendorOutcome {
            snapshot: crate::usage::VendorSnapshot::OrcaRouter(snap),
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
    fn default_render_shows_remaining() {
        let snap = sample_snap();
        let outcome = sample_outcome(snap.clone());
        let out = render(&outcome, &snap, &Theme::default(), &opts(), Utc::now());
        assert!(out.text.contains("$9.75"), "{}", out.text);
    }

    /// Cents, never dollars: wire `total_usage: 275` must render as $2.75,
    /// not $275.00.
    #[test]
    fn spend_is_rendered_in_dollars_not_cents() {
        let snap = sample_snap();
        let outcome = sample_outcome(snap.clone());
        let mut o = opts();
        o.format = Some("{orc_spend}".into());
        let out = render(&outcome, &snap, &Theme::default(), &o, Utc::now());
        assert!(out.text.contains("$2.75"), "{}", out.text);
        assert!(!out.text.contains("$275"), "{}", out.text);
    }

    /// The unlimited sentinel's whole point: no limit row, no $100M figure,
    /// spend-only — like the codebase treats a missing monthly limit.
    #[test]
    fn unlimited_key_renders_spend_only_not_a_hundred_million_wallet() {
        let snap = OrcaRouterSnapshot {
            spent_cents: 275,
            limit_cents: None,
            access_until: None,
        };
        let outcome = sample_outcome(snap.clone());
        let out = render(&outcome, &snap, &Theme::default(), &opts(), Utc::now());
        assert!(out.text.contains("$2.75"), "{}", out.text);
        assert!(!out.text.contains("100,000,000"), "{}", out.text);
        assert!(!out.text.contains("$100000000"), "{}", out.text);
        assert!(!out.text.contains("$100,000,000.00"), "{}", out.text);
        assert_eq!(severity(&snap), PaceSeverity::Low);

        let mut o = opts();
        o.format = Some("{orc_limit}|{orc_remaining}".into());
        let out = render(&outcome, &snap, &Theme::default(), &o, Utc::now());
        assert!(out.text.contains("unlimited|unlimited"), "{}", out.text);

        let tooltip = render(&outcome, &snap, &Theme::default(), &opts(), Utc::now()).tooltip;
        assert!(tooltip.contains("no credit limit"), "{tooltip}");
        assert!(!tooltip.contains("100,000,000"), "{tooltip}");
    }

    #[test]
    fn tooltip_shows_spend_limit_remaining_and_expiry() {
        let mut snap = sample_snap();
        snap.access_until = Some(Utc::now() + chrono::Duration::days(30));
        let outcome = sample_outcome(snap.clone());
        let out = render(&outcome, &snap, &Theme::default(), &opts(), Utc::now());
        assert!(out.tooltip.contains("$2.75"), "{}", out.tooltip);
        assert!(out.tooltip.contains("of $12.50 used"), "{}", out.tooltip);
        assert!(out.tooltip.contains("$9.75"), "{}", out.tooltip);
        assert!(out.tooltip.contains("expires"), "{}", out.tooltip);
    }

    /// `access_until: 0` (no expiry) must not paint an expiry row at all.
    #[test]
    fn access_until_zero_means_no_expiry_row() {
        let snap = sample_snap();
        let outcome = sample_outcome(snap.clone());
        let out = render(&outcome, &snap, &Theme::default(), &opts(), Utc::now());
        assert!(!out.tooltip.contains("expires"), "{}", out.tooltip);
        let values = build_placeholders(&snap, Utc::now());
        assert_eq!(values["orc_expires"], "—");
    }

    #[test]
    fn custom_tooltip_uses_placeholders() {
        let snap = sample_snap();
        let outcome = sample_outcome(snap.clone());
        let mut o = opts();
        o.tooltip_format = Some("left: {orc_remaining} of {orc_limit}".into());
        let out = render(&outcome, &snap, &Theme::default(), &o, Utc::now());
        assert_eq!(out.tooltip, "left: $9.75 of $12.50");
    }

    #[test]
    fn stale_appends_pause() {
        let snap = sample_snap();
        let mut outcome = sample_outcome(snap.clone());
        outcome.stale = true;
        let out = render(&outcome, &snap, &Theme::default(), &opts(), Utc::now());
        assert!(out.text.contains("⏸"));
    }

    /// Spend past the limit is a debt: signed remaining, critical severity —
    /// the OpenRouter overrun treatment.
    #[test]
    fn overrun_is_a_debt_not_zero() {
        let snap = OrcaRouterSnapshot {
            spent_cents: 1300,
            limit_cents: Some(1250),
            access_until: None,
        };
        let outcome = sample_outcome(snap.clone());
        let out = render(&outcome, &snap, &Theme::default(), &opts(), Utc::now());
        assert!(out.text.contains("-$0.50"), "{}", out.text);
        assert_eq!(severity(&snap), PaceSeverity::Critical);
    }

    #[test]
    fn severity_scales_with_remaining() {
        assert_eq!(
            severity(&OrcaRouterSnapshot {
                spent_cents: 11950,
                limit_cents: Some(12000),
                access_until: None
            }),
            PaceSeverity::Critical
        );
        assert_eq!(
            severity(&OrcaRouterSnapshot {
                spent_cents: 11600,
                limit_cents: Some(12000),
                access_until: None
            }),
            PaceSeverity::High
        );
        assert_eq!(
            severity(&OrcaRouterSnapshot {
                spent_cents: 0,
                limit_cents: Some(12000),
                access_until: None
            }),
            PaceSeverity::Low
        );
    }
}
