//! Mistral renderer — the API budget as the bar headline, the Vibe Code
//! budget beside it, and both monthly allowances in the tooltip.
//!
//! The generic `session_*` aliases carry the API budget and `weekly_*` the Vibe
//! Code budget, so a `--format` written for any other provider still shows a
//! real figure here. Both reset on the plan's calendar-month boundary.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::format::{money, placeholders, substitute, updated_at_hm, window_placeholders};
use crate::pacing::PaceSeverity;
use crate::pango::{color_span, escape, severity_color, severity_for};
use crate::theme::Theme;
use crate::tooltip::{Line, push_window_with_detail, render_bordered};
use crate::usage::MistralSnapshot;
use crate::vendor::{RenderOpts, VendorId, VendorOutcome};
use crate::waybar::{Class, WaybarOutput};

impl From<super::fetch::FetchOutcome> for VendorOutcome {
    fn from(outcome: super::fetch::FetchOutcome) -> Self {
        outcome.map(crate::usage::VendorSnapshot::Mistral)
    }
}

/// The bar's default text: the API budget, the figure the account spends first.
pub const DEFAULT_FORMAT: &str = "{mistral_api_pct}%";

/// The default when the plan reports no `api_budget`: the Vibe budget is the
/// only window left, so the bar must not render a bare `%`.
const VIBE_DEFAULT_FORMAT: &str = "{mistral_vibe_pct}%";

pub fn severity(snapshot: &MistralSnapshot) -> PaceSeverity {
    // The Vibe budget can be exhausted while the API budget is untouched, so
    // the bar follows the worst window — the same rule as the TUI headline.
    let api = snapshot.api.as_ref().map_or(0, |w| w.utilization_pct);
    severity_for(api.max(snapshot.vibe.utilization_pct))
}

pub fn build_placeholders(
    snapshot: &MistralSnapshot,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> HashMap<&'static str, String> {
    let api = window_placeholders(snapshot.api.as_ref(), opts, now);
    let vibe = window_placeholders(Some(&snapshot.vibe), opts, now);
    placeholders([
        ("icon", VendorId::Mistral.bar_icon().to_string()),
        ("vendor_short", VendorId::Mistral.short_name().to_string()),
        (
            "plan",
            snapshot
                .plan
                .clone()
                .unwrap_or_else(|| "Mistral".to_string()),
        ),
        // Generic aliases: the API budget is the plan's primary window, the
        // Vibe Code budget the second one.
        ("session_pct", api.pct.clone()),
        ("session_reset", api.reset.clone()),
        ("session_elapsed", api.elapsed.clone()),
        ("weekly_pct", vibe.pct.clone()),
        ("weekly_reset", vibe.reset.clone()),
        ("weekly_elapsed", vibe.elapsed.clone()),
        ("mistral_api_pct", api.pct.clone()),
        ("mistral_api_reset", api.reset.clone()),
        ("mistral_api_elapsed", api.elapsed.clone()),
        ("mistral_api_pace", api.ratio_pace.clone()),
        ("mistral_api_pace_indicator", api.point_pace.clone()),
        ("mistral_vibe_pct", vibe.pct.clone()),
        ("mistral_vibe_reset", vibe.reset.clone()),
        ("mistral_vibe_elapsed", vibe.elapsed.clone()),
        ("mistral_vibe_pace", vibe.ratio_pace),
        ("mistral_vibe_pace_indicator", vibe.point_pace),
        (
            "mistral_api_budget",
            snapshot
                .api_budget
                .map(|budget| money(budget, &snapshot.currency))
                .unwrap_or_default(),
        ),
        (
            "mistral_vibe_budget",
            money(snapshot.vibe_budget, &snapshot.currency),
        ),
        (
            "mistral_payg",
            if snapshot.api_payg || snapshot.vibe_payg {
                "on"
            } else {
                "off"
            }
            .to_string(),
        ),
    ])
}

pub fn render(
    outcome: &VendorOutcome,
    snapshot: &MistralSnapshot,
    theme: &Theme,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> WaybarOutput {
    let severity = severity(snapshot);
    let class = Class::from(severity);
    let format = opts.format.clone().unwrap_or_else(|| {
        if snapshot.api.is_some() {
            DEFAULT_FORMAT.to_string()
        } else {
            VIBE_DEFAULT_FORMAT.to_string()
        }
    });
    let mut values = build_placeholders(snapshot, opts, now);
    for value in values.values_mut() {
        *value = escape(value);
    }
    let mut text = substitute(&format, &values);
    if outcome.stale {
        text.push_str(" ⏸");
    }
    let icon = opts
        .icon
        .as_deref()
        .filter(|icon| !icon.is_empty())
        .map(|icon| format!("{} ", escape(icon)))
        .unwrap_or_default();
    let bar = color_span(severity_color(severity, theme), &format!("{icon}{text}"));
    let tooltip = opts
        .tooltip_format
        .as_deref()
        .map(|format| substitute(format, &values))
        .unwrap_or_else(|| render_tooltip(outcome, snapshot, theme, now));
    WaybarOutput {
        text: bar,
        tooltip,
        class,
    }
}

fn render_tooltip(
    outcome: &VendorOutcome,
    snapshot: &MistralSnapshot,
    theme: &Theme,
    now: DateTime<Utc>,
) -> String {
    let dim = &theme.dim;
    let plan = snapshot.plan.as_deref().unwrap_or("Mistral");
    let api_budget = snapshot
        .api_budget
        .map(|budget| money(budget, &snapshot.currency));
    let vibe_budget = money(snapshot.vibe_budget, &snapshot.currency);
    let mut lines = vec![
        Line::Center("Mistral".into()),
        Line::Body(format!(" <span foreground='{dim}'>{}</span>", escape(plan))),
        Line::Sep,
    ];

    if let Some(api) = &snapshot.api {
        push_window_with_detail(
            &mut lines,
            "API budget",
            api,
            theme,
            now,
            Default::default(),
            api_budget.as_deref(),
        );
        lines.push(Line::Body("".into()));
    }
    push_window_with_detail(
        &mut lines,
        "Vibe budget",
        &snapshot.vibe,
        theme,
        now,
        Default::default(),
        Some(&vibe_budget),
    );

    if snapshot.api_payg || snapshot.vibe_payg {
        lines.push(Line::Body("".into()));
        lines.push(Line::Body(format!(
            " <span foreground='{dim}'>Pay-as-you-go: API {} · Vibe {}</span>",
            if snapshot.api_payg { "on" } else { "off" },
            if snapshot.vibe_payg { "on" } else { "off" }
        )));
    }
    if outcome.stale {
        lines.push(Line::Body(
            "⏸ Previous quota observation; current allowance is unverified".into(),
        ));
    }
    if let Some((_, error)) = &outcome.last_error {
        lines.push(Line::Body(escape(error)));
    }
    lines.push(Line::Body(format!(
        "Updated {}",
        updated_at_hm(now, outcome.cache_age)
    )));
    render_bordered(&lines, theme)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::usage::UsageWindow;

    fn snapshot() -> MistralSnapshot {
        MistralSnapshot {
            plan: Some("EDU".into()),
            api: Some(UsageWindow {
                utilization_pct: 16,
                resets_at: Some("2026-12-01T00:00:00Z".parse().unwrap()),
                window_duration: chrono::Duration::zero(),
            }),
            vibe: UsageWindow {
                utilization_pct: 0,
                resets_at: Some("2026-12-01T00:00:00Z".parse().unwrap()),
                window_duration: chrono::Duration::zero(),
            },
            api_budget: Some(15.0),
            vibe_budget: 150.0,
            currency: "USD".into(),
            api_payg: false,
            vibe_payg: false,
        }
    }

    fn outcome(snapshot: MistralSnapshot) -> VendorOutcome {
        VendorOutcome {
            snapshot: crate::usage::VendorSnapshot::Mistral(snapshot),
            stale: false,
            last_error: None,
            cache_age: Some(Duration::from_secs(10)),
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
    fn placeholders_expose_both_budgets_and_the_generic_aliases() {
        let now = "2026-11-05T12:00:00Z".parse().unwrap();
        let values = build_placeholders(&snapshot(), &opts(), now);
        assert_eq!(values.get("session_pct").map(String::as_str), Some("16"));
        assert_eq!(values.get("weekly_pct").map(String::as_str), Some("0"));
        assert_eq!(
            values.get("mistral_api_budget").map(String::as_str),
            Some("$15.00")
        );
        assert_eq!(
            values.get("mistral_vibe_budget").map(String::as_str),
            Some("$150.00")
        );
        assert_eq!(values.get("mistral_payg").map(String::as_str), Some("off"));
    }

    #[test]
    fn render_shows_the_api_budget_and_both_tooltip_rows() {
        let now = "2026-11-05T12:00:00Z".parse().unwrap();
        let snapshot = snapshot();
        let rendered = render(
            &outcome(snapshot.clone()),
            &snapshot,
            &Theme::default(),
            &opts(),
            now,
        );
        assert!(rendered.text.contains("16%"), "{}", rendered.text);
        assert!(rendered.tooltip.contains("API budget"));
        assert!(rendered.tooltip.contains("Vibe budget"));
        assert!(rendered.tooltip.contains("$15.00"));
        assert!(rendered.tooltip.contains("$150.00"));
    }

    #[test]
    fn a_payg_flag_is_visible_in_the_tooltip() {
        let now = "2026-11-05T12:00:00Z".parse().unwrap();
        let mut snapshot = snapshot();
        snapshot.vibe_payg = true;
        let rendered = render(
            &outcome(snapshot.clone()),
            &snapshot,
            &Theme::default(),
            &opts(),
            now,
        );
        assert!(rendered.tooltip.contains("Pay-as-you-go"));
        assert!(rendered.tooltip.contains("Vibe on"));
    }

    #[test]
    fn severity_follows_the_worst_budget() {
        let mut snapshot = snapshot();
        snapshot.vibe.utilization_pct = 100;
        assert_eq!(severity(&snapshot), PaceSeverity::Critical);

        snapshot.vibe.utilization_pct = 0;
        snapshot.api.as_mut().unwrap().utilization_pct = 80;
        assert_eq!(severity(&snapshot), PaceSeverity::High);
    }

    #[test]
    fn a_missing_api_budget_renders_only_the_vibe_row() {
        let now = "2026-11-05T12:00:00Z".parse().unwrap();
        let mut snapshot = snapshot();
        snapshot.api = None;
        snapshot.api_budget = None;
        snapshot.vibe.utilization_pct = 40;
        let values = build_placeholders(&snapshot, &opts(), now);
        assert_eq!(values.get("session_pct").map(String::as_str), Some(""));
        assert_eq!(
            values.get("mistral_api_budget").map(String::as_str),
            Some("")
        );
        let rendered = render(
            &outcome(snapshot.clone()),
            &snapshot,
            &Theme::default(),
            &opts(),
            now,
        );
        assert!(!rendered.tooltip.contains("API budget"));
        assert!(rendered.tooltip.contains("Vibe budget"));
        assert!(rendered.tooltip.contains("$150.00"));
        // The default format falls back to the only window that exists; a
        // bare `%` would be the visible failure the empty placeholder causes.
        assert!(rendered.text.contains("40%"), "{}", rendered.text);
        assert!(!rendered.text.contains(">%"), "{}", rendered.text);
    }

    #[test]
    fn the_plan_label_is_escaped_in_the_tooltip() {
        let now = "2026-11-05T12:00:00Z".parse().unwrap();
        let mut snapshot = snapshot();
        snapshot.plan = Some("Plan <script> & co".into());
        let rendered = render(
            &outcome(snapshot.clone()),
            &snapshot,
            &Theme::default(),
            &opts(),
            now,
        );
        assert!(rendered.tooltip.contains("Plan &lt;script&gt; &amp; co"));
        assert!(!rendered.tooltip.contains("<script>"));
    }
}
