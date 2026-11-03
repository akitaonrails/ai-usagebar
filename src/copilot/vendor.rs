//! GitHub Copilot Waybar renderer.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::countdown;
use crate::format::{placeholders, substitute, updated_at_hm};
use crate::pacing::PaceSeverity;
use crate::pango::{color_span, escape, severity_color, severity_for};
use crate::theme::Theme;
use crate::tooltip::{Line as TooltipLine, render_bordered};
use crate::vendor::{RenderOpts, VendorId, VendorOutcome};
use crate::waybar::{Class, WaybarOutput};

use super::fetch::FetchOutcome;
use super::types::Snapshot;

pub const DEFAULT_FORMAT: &str = "{copilot_premium_pct}% · {copilot_reset}";
const UNAVAILABLE: &str = "—";

/// Formats for a plan that does not include every bucket. Copilot Free has no
/// premium entitlement, so [`DEFAULT_FORMAT`] would read "—% · 29d 4h".
const CHAT_FORMAT: &str = "{copilot_chat_pct}% · {copilot_reset}";
const COMPLETIONS_FORMAT: &str = "{copilot_completions_pct}% · {copilot_reset}";

/// The bar text used when `--format` is omitted: the first bucket the plan
/// actually includes. Premium is the headline wherever it exists; Copilot Free
/// reports it as a zero entitlement, which `parse_quota` drops, and the bar
/// would otherwise show the unavailable marker instead of a figure.
pub fn default_format(snap: &Snapshot) -> &'static str {
    if snap.premium.is_some() {
        DEFAULT_FORMAT
    } else if snap.chat.is_some() {
        CHAT_FORMAT
    } else {
        COMPLETIONS_FORMAT
    }
}

impl From<FetchOutcome> for VendorOutcome {
    fn from(outcome: FetchOutcome) -> Self {
        outcome.map(crate::usage::VendorSnapshot::Copilot)
    }
}

pub fn build_placeholders(snap: &Snapshot, now: DateTime<Utc>) -> HashMap<&'static str, String> {
    let premium = quota_values(snap.premium.as_ref());
    let chat = quota_values(snap.chat.as_ref());
    let completions = quota_values(snap.completions.as_ref());
    let reset = countdown::format(snap.reset_at, now);
    placeholders([
        ("icon", "󰊤".to_string()),
        ("vendor_short", VendorId::Copilot.short_name().to_string()),
        ("plan", crate::display::sanitize_untrusted_field(&snap.plan)),
        ("session_pct", premium.percent.clone()),
        ("session_reset", reset.clone()),
        ("weekly_pct", chat.percent.clone()),
        ("weekly_reset", reset.clone()),
        (
            "copilot_plan",
            crate::display::sanitize_untrusted_field(&snap.plan),
        ),
        ("copilot_reset", reset),
        ("copilot_premium_pct", premium.percent),
        ("copilot_premium_used", premium.used),
        ("copilot_premium_limit", premium.limit),
        ("copilot_chat_pct", chat.percent),
        ("copilot_chat_used", chat.used),
        ("copilot_chat_limit", chat.limit),
        ("copilot_completions_pct", completions.percent),
        ("copilot_completions_used", completions.used),
        ("copilot_completions_limit", completions.limit),
    ])
}

struct QuotaValues {
    percent: String,
    used: String,
    limit: String,
}

fn quota_values(quota: Option<&super::types::Quota>) -> QuotaValues {
    let Some(quota) = quota else {
        return QuotaValues {
            percent: UNAVAILABLE.into(),
            used: UNAVAILABLE.into(),
            limit: UNAVAILABLE.into(),
        };
    };
    if quota.unlimited {
        return QuotaValues {
            percent: "0".into(),
            used: "0".into(),
            limit: "unlimited".into(),
        };
    }
    let (used, limit) = quota
        .used_and_entitlement()
        .map(|(used, limit)| (used.to_string(), limit.to_string()))
        .unwrap_or_else(|| (UNAVAILABLE.into(), UNAVAILABLE.into()));
    QuotaValues {
        percent: quota.used_pct().to_string(),
        used,
        limit,
    }
}

pub fn severity(snap: &Snapshot) -> PaceSeverity {
    severity_for(snap.worst_pct())
}

pub fn render(
    outcome: &VendorOutcome,
    snap: &Snapshot,
    theme: &Theme,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> WaybarOutput {
    let severity = severity(snap);
    let format = opts
        .format
        .as_deref()
        .unwrap_or_else(|| default_format(snap));
    let mut values = build_placeholders(snap, now);
    for key in ["plan", "copilot_plan"] {
        if let Some(value) = values.get_mut(key) {
            *value = escape(value);
        }
    }
    let mut text = substitute(format, &values);
    if outcome.stale {
        text.push_str(" ⏸");
    }
    let icon = opts
        .icon
        .as_deref()
        .filter(|icon| !icon.is_empty())
        .map(|icon| format!("{} ", escape(icon)))
        .unwrap_or_default();
    let tooltip = opts
        .tooltip_format
        .as_deref()
        .map(|format| substitute(format, &values))
        .unwrap_or_else(|| render_tooltip(outcome, snap, theme, now));
    WaybarOutput {
        text: color_span(severity_color(severity, theme), &format!("{icon}{text}")),
        tooltip,
        class: Class::from(severity),
    }
}

fn render_tooltip(
    outcome: &VendorOutcome,
    snap: &Snapshot,
    theme: &Theme,
    now: DateTime<Utc>,
) -> String {
    let mut lines = vec![TooltipLine::Center(format!(
        "<span font_weight='bold' foreground='{}'>GitHub Copilot {}</span>",
        theme.blue,
        escape(&crate::display::sanitize_untrusted_field(&snap.plan))
    ))];
    lines.push(TooltipLine::Sep);
    lines.push(TooltipLine::Body(String::new()));
    for (label, quota) in snap.quotas() {
        let usage = if quota.unlimited {
            "Unlimited".to_string()
        } else if let Some((used, entitlement)) = quota.used_and_entitlement() {
            format!("{}% · {used} of {entitlement} used", quota.used_pct())
        } else {
            format!(
                "{}% · {}% remaining",
                quota.used_pct(),
                quota.percent_remaining
            )
        };
        lines.push(TooltipLine::Body(format!("  {label}  {}", escape(&usage))));
    }
    lines.push(TooltipLine::Body(format!(
        "  Resets  {}",
        escape(&countdown::format(snap.reset_at, now))
    )));
    if outcome.stale {
        lines.push(TooltipLine::Body(String::new()));
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{}'>  ⏸  Showing cached data</span>",
            theme.orange
        )));
    }
    if let Some((code, message)) = outcome.last_error.as_ref()
        && *code != 0
    {
        lines.push(TooltipLine::Body(String::new()));
        lines.push(TooltipLine::Sep);
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{}'>  HTTP {code}: {}</span>",
            theme.orange,
            escape(message)
        )));
    }
    lines.push(TooltipLine::Body(String::new()));
    lines.push(TooltipLine::Sep);
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{}'>  Updated {}</span>",
        theme.dim,
        updated_at_hm(now, outcome.cache_age)
    )));
    render_bordered(&lines, theme)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::copilot::types::Quota;
    use chrono::TimeZone;

    fn sample() -> Snapshot {
        Snapshot {
            plan: "Pro".into(),
            premium: Some(Quota {
                percent_remaining: 15,
                entitlement: Some(300),
                remaining: Some(45),
                unlimited: false,
            }),
            chat: None,
            completions: None,
            reset_at: None,
        }
    }

    #[test]
    fn exposes_provider_and_generic_quota_placeholders() {
        let values = build_placeholders(&sample(), Utc::now());
        assert_eq!(values["vendor_short"], "ghc");
        assert_eq!(values["copilot_premium_pct"], "85");
        assert_eq!(values["copilot_premium_used"], "255");
        assert_eq!(values["session_pct"], "85");
        assert_eq!(values["weekly_pct"], UNAVAILABLE);
    }

    #[test]
    fn renderer_uses_the_premium_quota_and_canonical_provider_name() {
        let snap = sample();
        let outcome = VendorOutcome::fresh(crate::usage::VendorSnapshot::Copilot(snap.clone()));
        let output = render(
            &outcome,
            &snap,
            &Theme::default(),
            &RenderOpts {
                format: None,
                tooltip_format: None,
                icon: None,
                pace_tolerance: 5,
                format_pace_color: false,
                tooltip_pace_pts: false,
            },
            Utc::now(),
        );
        assert!(output.text.contains("85%"));
        assert!(output.tooltip.contains("GitHub Copilot Pro"));
    }
    /// Copilot Free has no premium entitlement, so that bucket is absent. The
    /// bar must still show a figure rather than the unavailable marker.
    #[test]
    fn a_plan_without_premium_puts_its_next_quota_on_the_bar() {
        // One fixed instant for both the reset and the render clock. Taking
        // two `Utc::now()` readings made the countdown depend on the gap
        // between them: on a coarse clock the delta stayed exactly 29 days and
        // the bar read "29d 0h" instead of "28d 23h".
        let now = Utc.with_ymd_and_hms(2026, 11, 3, 0, 0, 0).unwrap();
        let free = Snapshot {
            plan: "Free".into(),
            premium: None,
            chat: Some(Quota {
                percent_remaining: 60,
                entitlement: Some(50),
                remaining: Some(30),
                unlimited: false,
            }),
            completions: None,
            reset_at: Some(now + chrono::Duration::days(29)),
        };
        assert_eq!(default_format(&free), CHAT_FORMAT);
        assert_eq!(default_format(&sample()), DEFAULT_FORMAT);

        let outcome = VendorOutcome::fresh(crate::usage::VendorSnapshot::Copilot(free.clone()));
        let output = render(
            &outcome,
            &free,
            &Theme::default(),
            &RenderOpts {
                format: None,
                tooltip_format: None,
                icon: None,
                pace_tolerance: 5,
                format_pace_color: false,
                tooltip_pace_pts: false,
            },
            now,
        );
        // The chat figure, not the premium bucket's unavailable marker.
        assert!(output.text.contains("40% \u{b7} 29d 0h"), "{}", output.text);
        assert!(!output.text.contains(UNAVAILABLE), "{}", output.text);

        // Completions-only is the last fallback; an empty snapshot never
        // reaches the renderer (`to_snapshot` rejects it).
        let completions_only = Snapshot {
            chat: None,
            completions: free.chat.clone(),
            ..free
        };
        assert_eq!(default_format(&completions_only), COMPLETIONS_FORMAT);
    }
}
