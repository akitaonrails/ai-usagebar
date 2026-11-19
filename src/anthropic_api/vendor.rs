//! Anthropic Admin API renderer — month-to-date spend, optionally against a
//! configured monthly limit (the API doesn't expose the limit or the remaining
//! prepaid balance, so the limit is a config value).

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::format::{placeholders, substitute, updated_at_hm, usd};
use crate::pacing::PaceSeverity;
use crate::pango::{color_span, escape, severity_color, severity_for};
use crate::theme::Theme;
use crate::tooltip::{Line as TooltipLine, render_bordered};
use crate::usage::AnthropicApiSnapshot;
use crate::vendor::{RenderOpts, VendorId, VendorOutcome};
use crate::waybar::{Class, WaybarOutput};

use super::fetch::FetchOutcome;

pub const DEFAULT_FORMAT: &str = "{aapi_headline}";

/// Bar headline: spend-vs-limit with a % when a limit is configured, otherwise
/// just the month-to-date spend.
fn headline(snap: &AnthropicApiSnapshot) -> String {
    match snap.limit {
        Some(l) if l > 0.0 => format!(
            "{} / ${:.0} · {}%",
            usd(snap.spent),
            l,
            snap.pct().unwrap_or(0)
        ),
        _ => format!("{}/mo", usd(snap.spent)),
    }
}

pub fn build_placeholders(snap: &AnthropicApiSnapshot) -> HashMap<&'static str, String> {
    let pct = snap.pct().unwrap_or(0);
    placeholders(vec![
        ("icon", "󰢗".to_string()),
        (
            "vendor_short",
            VendorId::AnthropicApi.short_name().to_string(),
        ),
        // Cross-vendor aliases — spend% maps to the session/weekly slots.
        ("session_pct", pct.to_string()),
        ("session_reset", "—".to_string()),
        ("weekly_pct", pct.to_string()),
        ("weekly_reset", "—".to_string()),
        ("plan", "Anthropic API".to_string()),
        ("aapi_headline", headline(snap)),
        ("aapi_spent", usd(snap.spent)),
        (
            "aapi_through",
            snap.covered_through
                .map(|date| date.to_string())
                .unwrap_or_default(),
        ),
        (
            "aapi_limit",
            snap.limit
                .map(|l| format!("${l:.0}"))
                .unwrap_or_else(|| "—".into()),
        ),
        ("aapi_pct", pct.to_string()),
    ])
}

/// Severity keys on the spend-vs-limit %. With no limit there's no signal, so
/// it stays calm (low).
pub fn severity(snap: &AnthropicApiSnapshot) -> PaceSeverity {
    match snap.pct() {
        Some(p) => severity_for(p.min(100)),
        None => PaceSeverity::Low,
    }
}

pub fn render(
    outcome: &VendorOutcome,
    snap: &AnthropicApiSnapshot,
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
    snap: &AnthropicApiSnapshot,
    theme: &Theme,
    now: DateTime<Utc>,
) -> String {
    let blue = &theme.blue;
    let dim = &theme.dim;
    let fg = &theme.fg;
    let color = severity_color(severity(snap), theme);

    let mut lines: Vec<TooltipLine> = Vec::new();
    lines.push(TooltipLine::Center(format!(
        "<span font_weight='bold' foreground='{blue}'>Anthropic API</span>"
    )));
    lines.push(TooltipLine::Sep);
    lines.push(TooltipLine::Body("".into()));

    lines.push(TooltipLine::Body(format!(
        " <span foreground='{fg}'>  󰉹  {}</span>",
        escape(&snap.coverage_label())
    )));
    lines.push(TooltipLine::Body(format!(
        "   <span font_weight='bold' foreground='{color}'>{spent}</span>",
        spent = escape(&usd(snap.spent))
    )));
    match snap.limit {
        Some(l) if l > 0.0 => {
            lines.push(TooltipLine::Body(format!(
                " <span foreground='{dim}'>     of ${l:.0} limit ({pct}%)</span>",
                pct = snap.pct().unwrap_or(0)
            )));
        }
        _ => {
            lines.push(TooltipLine::Body(format!(
                " <span foreground='{dim}'>     no monthly limit set (add `monthly_limit` under [anthropic_api])</span>"
            )));
        }
    }
    lines.push(TooltipLine::Body("".into()));
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{dim}'>  󰋼  spend consumed, not balance —</span>"
    )));
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{dim}'>     remaining credit is Console-only (no API)</span>"
    )));
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{dim}'>  󰋼  excludes Priority Tier cost, which the</span>"
    )));
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{dim}'>     cost API does not report</span>"
    )));

    if let Some((code, msg)) = outcome.last_error.as_ref() {
        // code 0 = a non-HTTP failure (schema drift, transport). Show it under a
        // "Sync error" header rather than the nonsensical "HTTP 0" — and never
        // suppress it, so the reason for a stale state is always visible.
        let (icon, ecolor, header) = if *code == 0 {
            ("󰀪", theme.orange.as_str(), "Sync error".to_string())
        } else if *code >= 500 {
            ("󰅚", theme.red.as_str(), format!("HTTP {code}"))
        } else {
            ("󰀪", theme.orange.as_str(), format!("HTTP {code}"))
        };
        lines.push(TooltipLine::Body("".into()));
        lines.push(TooltipLine::Sep);
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{ecolor}'>  {icon}  {header}</span>"
        )));
        lines.push(TooltipLine::Body(format!(
            "     <span foreground='{dim}'>{}</span>",
            escape(msg)
        )));
        let guidance: &[&str] = match *code {
            401 => &["Check that the organization report key is valid and not expired."],
            403 => &[
                "Check organization roles, report permissions and key scope.",
                "Prepaid credit does not grant access.",
            ],
            _ => &[],
        };
        if !guidance.is_empty() {
            for line in guidance {
                lines.push(TooltipLine::Body(format!(
                    "     <span foreground='{dim}'>{}</span>",
                    escape(line)
                )));
            }
            lines.push(TooltipLine::Body(format!(
                "     <span foreground='{dim}'>Setup and troubleshooting: docs/anthropic-api.md</span>"
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
        o.map(crate::usage::VendorSnapshot::AnthropicApi)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::AnthropicApiSnapshot;

    fn outcome(spent: f64, limit: Option<f64>) -> (AnthropicApiSnapshot, VendorOutcome) {
        let snap = AnthropicApiSnapshot {
            spent,
            limit,
            covered_through: Some(chrono::NaiveDate::from_ymd_opt(2026, 10, 17).unwrap()),
        };
        let o = VendorOutcome {
            snapshot: crate::usage::VendorSnapshot::AnthropicApi(snap.clone()),
            stale: false,
            last_error: None,
            cache_age: Some(std::time::Duration::from_secs(10)),
        };
        (snap, o)
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
    fn headline_with_limit_shows_spend_limit_and_pct() {
        let (snap, o) = outcome(1.34, Some(1000.0));
        let out = render(&o, &snap, &Theme::default(), &opts(), Utc::now());
        assert!(out.text.contains("$1.34 / $1000 · 0%"));
    }

    #[test]
    fn headline_without_limit_shows_monthly_spend() {
        let (snap, o) = outcome(1.34, None);
        let out = render(&o, &snap, &Theme::default(), &opts(), Utc::now());
        assert!(out.text.contains("$1.34/mo"));
        assert!(out.tooltip.contains("no monthly limit set"));
        assert!(out.tooltip.contains("Month to date (through Oct 17)"));
    }

    #[test]
    fn an_undated_report_does_not_claim_to_include_the_live_day() {
        let snap = AnthropicApiSnapshot {
            spent: 1.34,
            limit: None,
            covered_through: None,
        };
        assert_eq!(snap.coverage_label(), "Completed daily buckets this month");
    }

    #[test]
    fn severity_scales_with_spend_pct() {
        assert_eq!(
            severity(&AnthropicApiSnapshot {
                spent: 950.0,
                limit: Some(1000.0),
                covered_through: None,
            }),
            PaceSeverity::Critical
        );
        assert_eq!(
            severity(&AnthropicApiSnapshot {
                spent: 1.0,
                limit: Some(1000.0),
                covered_through: None,
            }),
            PaceSeverity::Low
        );
        assert_eq!(
            severity(&AnthropicApiSnapshot {
                spent: 500.0,
                limit: None,
                covered_through: None,
            }),
            PaceSeverity::Low
        );
    }

    #[test]
    fn rejected_report_tooltips_explain_credentials_without_requiring_an_admin_prefix() {
        for status in [401, 403] {
            let (snap, mut outcome) = outcome(7.0, None);
            outcome.stale = true;
            outcome.last_error = Some((status, crate::error::AUTH_FAILURE_MESSAGE.into()));
            let tooltip = render(
                &outcome,
                &snap,
                &Theme::default(),
                &opts(),
                chrono::NaiveDate::from_ymd_opt(2026, 7, 19)
                    .unwrap()
                    .and_hms_opt(12, 0, 0)
                    .unwrap()
                    .and_utc(),
            )
            .tooltip;
            assert!(tooltip.contains("docs/anthropic-api.md"));
            assert!(!tooltip.contains("needs an org Admin key"));
            assert!(!tooltip.contains("sk-ant-admin01-"));
            if status == 401 {
                assert!(tooltip.contains("valid and not expired"));
            } else {
                assert!(tooltip.contains("report permissions and key scope"));
                assert!(tooltip.contains("Prepaid credit does not grant access"));
            }
        }
    }
}
