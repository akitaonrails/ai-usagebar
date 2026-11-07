//! DeepRelay renderer — the monthly allowance as the bar headline, the rolling
//! weekly caps beside it, and the dollar usage ceiling underneath.
//!
//! The generic `session_*` aliases carry the **monthly** window and
//! `weekly_*` the rolling 7-day caps, so a `--format` written for any other
//! provider still shows a real figure here. DeepRelay has no 5-hour window; the
//! alias is the plan's primary window, the same way Synthetic maps its weekly
//! budget onto `session_*`.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::format::{placeholders, substitute, tokens, updated_at_hm, usd, window_placeholders};
use crate::pacing::PaceSeverity;
use crate::pango::{color_span, escape, severity_color, severity_for};
use crate::theme::Theme;
use crate::tooltip::{Line, push_window_with_detail, render_bordered};
use crate::usage::{DeepRelayMeter, DeepRelaySnapshot, UsageWindow};
use crate::vendor::{RenderOpts, VendorId, VendorOutcome};
use crate::waybar::{Class, WaybarOutput};

impl From<super::fetch::FetchOutcome> for VendorOutcome {
    fn from(outcome: super::fetch::FetchOutcome) -> Self {
        outcome.map(crate::usage::VendorSnapshot::DeepRelay)
    }
}

/// The bar's default text: the monthly meter that will cap the account.
pub const DEFAULT_FORMAT: &str = "{deeprelay_pct}%";

pub fn headline_pct(snapshot: &DeepRelaySnapshot) -> Option<i32> {
    snapshot.monthly_worst().and_then(DeepRelayMeter::pct)
}

pub fn severity(snapshot: &DeepRelaySnapshot) -> PaceSeverity {
    headline_pct(snapshot)
        .map(severity_for)
        .unwrap_or(PaceSeverity::Low)
}

/// One meter drawn as a shared window, with the raw `used / limit` as the
/// detail so the percentage is never the only figure.
fn meter_window(
    meter: &DeepRelayMeter,
    resets_at: Option<DateTime<Utc>>,
    duration: chrono::Duration,
) -> Option<UsageWindow> {
    Some(UsageWindow {
        utilization_pct: meter.pct()?,
        resets_at,
        window_duration: duration,
    })
}

fn plan_line(snapshot: &DeepRelaySnapshot) -> String {
    let mut line = escape(&snapshot.plan);
    if let Some(cents) = snapshot.plan_price_cents {
        line.push_str(&format!(" · {}", escape(&usd(cents as f64 / 100.0))));
        if let Some(interval) = &snapshot.plan_interval {
            line.push_str(&format!("/{}", escape(interval)));
        }
    }
    if snapshot.cancel_at_period_end {
        line.push_str(" · cancels at period end");
    }
    line
}

fn meter_pct(meter: Option<&DeepRelayMeter>) -> String {
    meter
        .and_then(DeepRelayMeter::pct)
        .map(|pct| pct.to_string())
        .unwrap_or_default()
}

fn meter_detail(meter: Option<&DeepRelayMeter>) -> String {
    meter.map(DeepRelayMeter::used_of_limit).unwrap_or_default()
}

fn balance_text(snapshot: &DeepRelaySnapshot) -> String {
    snapshot.balance_usd.map(usd).unwrap_or_default()
}

fn backstop_pct(snapshot: &DeepRelaySnapshot) -> String {
    match (snapshot.backstop_used_usd, snapshot.backstop_limit_usd) {
        (Some(used), Some(limit)) if limit > 0.0 => {
            let pct = (used / limit * 100.0).round().clamp(0.0, 100.0);
            format!("{pct}")
        }
        _ => String::new(),
    }
}

fn backstop_text(snapshot: &DeepRelaySnapshot) -> String {
    match (snapshot.backstop_used_usd, snapshot.backstop_limit_usd) {
        (Some(used), Some(limit)) => format!("{} / {}", usd(used), usd(limit)),
        _ => String::new(),
    }
}

pub fn build_placeholders(
    snapshot: &DeepRelaySnapshot,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> HashMap<&'static str, String> {
    let monthly = window_placeholders(snapshot.monthly_window().as_ref(), opts, now);
    let weekly = window_placeholders(snapshot.weekly_window().as_ref(), opts, now);
    placeholders([
        ("icon", VendorId::DeepRelay.bar_icon().to_string()),
        ("vendor_short", VendorId::DeepRelay.short_name().to_string()),
        ("plan", snapshot.plan.clone()),
        // Generic aliases: the plan's primary (monthly) window and the rolling
        // weekly caps. DeepRelay has no 5-hour session.
        ("session_pct", monthly.pct.clone()),
        ("session_reset", monthly.reset.clone()),
        ("session_elapsed", monthly.elapsed.clone()),
        ("weekly_pct", weekly.pct.clone()),
        ("weekly_reset", weekly.reset.clone()),
        ("weekly_elapsed", weekly.elapsed.clone()),
        ("deeprelay_pct", monthly.pct.clone()),
        ("deeprelay_reset", monthly.reset.clone()),
        ("deeprelay_elapsed", monthly.elapsed.clone()),
        ("deeprelay_pace", monthly.ratio_pace.clone()),
        ("deeprelay_pace_indicator", monthly.point_pace.clone()),
        (
            "deeprelay_monthly_input_pct",
            meter_pct(snapshot.monthly_input.as_ref()),
        ),
        (
            "deeprelay_monthly_input",
            meter_detail(snapshot.monthly_input.as_ref()),
        ),
        (
            "deeprelay_monthly_output_pct",
            meter_pct(snapshot.monthly_output.as_ref()),
        ),
        (
            "deeprelay_monthly_output",
            meter_detail(snapshot.monthly_output.as_ref()),
        ),
        (
            "deeprelay_weekly_input_pct",
            meter_pct(snapshot.weekly_input.as_ref()),
        ),
        (
            "deeprelay_weekly_input",
            meter_detail(snapshot.weekly_input.as_ref()),
        ),
        (
            "deeprelay_weekly_output_pct",
            meter_pct(snapshot.weekly_output.as_ref()),
        ),
        (
            "deeprelay_weekly_output",
            meter_detail(snapshot.weekly_output.as_ref()),
        ),
        ("deeprelay_weekly_pace", weekly.ratio_pace),
        ("deeprelay_weekly_pace_indicator", weekly.point_pace),
        ("deeprelay_backstop_pct", backstop_pct(snapshot)),
        ("deeprelay_backstop", backstop_text(snapshot)),
        ("deeprelay_balance", balance_text(snapshot)),
        (
            "deeprelay_payg",
            snapshot
                .payg_calls_24h
                .map(|calls| calls.to_string())
                .unwrap_or_default(),
        ),
        (
            "deeprelay_searches",
            snapshot
                .searches
                .map(|(used, limit)| format!("{used} / {limit}"))
                .unwrap_or_default(),
        ),
    ])
}

pub fn render(
    outcome: &VendorOutcome,
    snapshot: &DeepRelaySnapshot,
    theme: &Theme,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> WaybarOutput {
    let severity = severity(snapshot);
    let class = Class::from(severity);
    let format = opts.format.clone().unwrap_or_else(|| {
        if headline_pct(snapshot).is_some() {
            DEFAULT_FORMAT.to_string()
        } else {
            VendorId::DeepRelay.display_name().to_string()
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

fn push_meter(
    lines: &mut Vec<Line>,
    label: &str,
    meter: Option<&DeepRelayMeter>,
    resets_at: Option<DateTime<Utc>>,
    period: chrono::Duration,
    theme: &Theme,
    now: DateTime<Utc>,
) {
    let Some(meter) = meter else { return };
    let Some(window) = meter_window(meter, resets_at, period) else {
        return;
    };
    push_window_with_detail(
        lines,
        label,
        &window,
        theme,
        now,
        Default::default(),
        Some(&meter.used_of_limit()),
    );
}

fn render_tooltip(
    outcome: &VendorOutcome,
    snapshot: &DeepRelaySnapshot,
    theme: &Theme,
    now: DateTime<Utc>,
) -> String {
    let dim = &theme.dim;
    let mut lines = vec![
        Line::Center("DeepRelay".into()),
        Line::Body(format!(
            " <span foreground='{dim}'>{}</span>",
            plan_line(snapshot)
        )),
        Line::Sep,
    ];

    let period = match (snapshot.period_start, snapshot.period_end) {
        (Some(start), Some(end)) if end > start => end - start,
        _ => chrono::Duration::zero(),
    };

    if snapshot.monthly_input.is_some() || snapshot.monthly_output.is_some() {
        lines.push(Line::Body(" <b>Monthly allowance</b>".into()));
        for (label, meter) in [
            ("Input", snapshot.monthly_input.as_ref()),
            ("Output", snapshot.monthly_output.as_ref()),
        ] {
            push_meter(
                &mut lines,
                label,
                meter,
                snapshot.period_end,
                period,
                theme,
                now,
            );
        }
    }

    if snapshot.weekly_input.is_some() || snapshot.weekly_output.is_some() {
        lines.push(Line::Body(" <b>Weekly caps (rolling)</b>".into()));
        for (label, meter) in [
            ("Input", snapshot.weekly_input.as_ref()),
            ("Output", snapshot.weekly_output.as_ref()),
        ] {
            push_meter(&mut lines, label, meter, None, period, theme, now);
        }
        lines.push(Line::Body(format!(
            " <span foreground='{dim}'>Rolling 7-day window — no reset date</span>"
        )));
    }

    if let (Some(used), Some(limit)) = (snapshot.backstop_used_usd, snapshot.backstop_limit_usd) {
        let pct = (used / limit * 100.0).round().clamp(0.0, 100.0) as i32;
        push_window_with_detail(
            &mut lines,
            "Usage ceiling",
            &UsageWindow {
                utilization_pct: pct,
                resets_at: snapshot.period_end,
                window_duration: period,
            },
            theme,
            now,
            Default::default(),
            Some(&format!("{} / {}", usd(used), usd(limit))),
        );
    }

    if let Some((used, limit)) = snapshot.searches {
        lines.push(Line::Body(format!(" Web searches {used} / {limit}")));
    }

    if snapshot.balance_usd.is_some() || snapshot.payg_calls_24h.unwrap_or(0) > 0 {
        lines.push(Line::Body(" <b>Pay as you go</b>".into()));
        if let Some(balance) = snapshot.balance_usd {
            lines.push(Line::Body(format!(" Balance {}", escape(&usd(balance)))));
        }
        if let Some(calls) = snapshot.payg_calls_24h.filter(|calls| *calls > 0) {
            lines.push(Line::Body(format!(
                " {calls} call(s) billed to the wallet in 24h"
            )));
        }
    }

    if !snapshot.models.is_empty() {
        lines.push(Line::Sep);
        lines.push(Line::Body(" <b>Top models this period</b>".into()));
        for model in &snapshot.models {
            lines.push(Line::Body(format!(
                " {} <span foreground='{dim}'>{}</span>",
                escape(&model.model),
                tokens(model.weighted_total())
            )));
        }
    }

    if outcome.stale {
        lines.push(Line::Body(
            "⏸ Previous billing observation; the current allowance is unverified".into(),
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

    fn snapshot() -> DeepRelaySnapshot {
        let raw = include_str!("../../tests/fixtures/deeprelay/subscription.json");
        serde_json::from_str::<super::super::types::Subscription>(raw)
            .unwrap()
            .snapshot()
            .unwrap()
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

    fn outcome(snapshot: &DeepRelaySnapshot) -> VendorOutcome {
        VendorOutcome {
            snapshot: crate::usage::VendorSnapshot::DeepRelay(snapshot.clone()),
            stale: false,
            last_error: None,
            cache_age: Some(Duration::from_secs(1)),
        }
    }

    #[test]
    fn bar_headline_is_the_worse_monthly_meter() {
        let snapshot = snapshot();
        let now = chrono::DateTime::from_timestamp(1_791_014_400, 0).unwrap();
        let rendered = render(
            &outcome(&snapshot),
            &snapshot,
            &Theme::default(),
            &opts(),
            now,
        );
        assert!(rendered.text.contains("32%"), "{}", rendered.text);
        assert_eq!(headline_pct(&snapshot), Some(32));
        assert!(rendered.tooltip.contains("Monthly allowance"));
        assert!(rendered.tooltip.contains("812.4M / 2.5B"));
        assert!(rendered.tooltip.contains("Weekly caps (rolling)"));
        assert!(rendered.tooltip.contains("no reset date"));
        assert!(rendered.tooltip.contains("$12.34 / $500.00"));
        assert!(rendered.tooltip.contains("deepseek-v4.1-flash"));
    }

    #[test]
    fn generic_aliases_let_a_shared_format_render() {
        let snapshot = snapshot();
        let now = chrono::DateTime::from_timestamp(1_791_014_400, 0).unwrap();
        let values = build_placeholders(&snapshot, &opts(), now);
        assert_eq!(values.get("session_pct").map(String::as_str), Some("32"));
        assert_eq!(values.get("weekly_pct").map(String::as_str), Some("50"));
        assert_eq!(values.get("vendor_short").map(String::as_str), Some("drl"));
        assert_eq!(
            values
                .get("deeprelay_monthly_input_pct")
                .map(String::as_str),
            Some("32")
        );
        assert_eq!(
            values.get("deeprelay_weekly_output").map(String::as_str),
            Some("2M / 62.5M")
        );
        assert_eq!(
            values.get("deeprelay_backstop_pct").map(String::as_str),
            Some("2")
        );
    }

    #[test]
    fn pay_as_you_go_without_meters_falls_back_to_the_name() {
        let snapshot = DeepRelaySnapshot {
            plan: "Pay as you go".into(),
            plan_price_cents: None,
            plan_interval: None,
            subscribed: false,
            cancel_at_period_end: false,
            monthly_input: None,
            monthly_output: None,
            weekly_input: None,
            weekly_output: None,
            backstop_used_usd: None,
            backstop_limit_usd: None,
            searches: None,
            payg_calls_24h: None,
            balance_usd: Some(4.5),
            period_start: None,
            period_end: None,
            models: Vec::new(),
        };
        let now = chrono::DateTime::from_timestamp(1_791_014_400, 0).unwrap();
        let rendered = render(
            &outcome(&snapshot),
            &snapshot,
            &Theme::default(),
            &opts(),
            now,
        );
        assert!(rendered.text.contains("DeepRelay"));
        assert!(rendered.tooltip.contains("$4.50"));
        assert!(!rendered.tooltip.contains("Monthly allowance"));
    }
}
