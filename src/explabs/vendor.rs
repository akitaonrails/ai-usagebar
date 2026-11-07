//! Experiential Labs renderer — the daily token pool as the bar headline, the
//! credit wallet beside it, and the key's cap underneath.
//!
//! The generic `session_*` aliases carry the **daily** window (the plan's
//! primary window, resetting 00:00 UTC), so a `--format` written for any other
//! provider still shows a real figure here. Experiential Labs has no rolling
//! weekly window and no 5-hour session; the daily pool is the whole account.
//! The wallet and the key's cap are dollar figures, not windows, so they ride
//! their own placeholders.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::format::{placeholders, substitute, usd, window_placeholders};
use crate::pacing::PaceSeverity;
use crate::pango::{color_span, escape, severity_color, severity_for};
use crate::theme::Theme;
use crate::tooltip::{Line, push_window_with_detail, render_bordered};
use crate::usage::{ExperientialLabsMeter, ExperientialLabsSnapshot, UsageWindow};
use crate::vendor::{RenderOpts, VendorId, VendorOutcome};
use crate::waybar::{Class, WaybarOutput};

impl From<super::fetch::FetchOutcome> for VendorOutcome {
    fn from(outcome: super::fetch::FetchOutcome) -> Self {
        outcome.map(crate::usage::VendorSnapshot::ExperientialLabs)
    }
}

/// The bar's default text: the daily meter that will cap the account.
pub const DEFAULT_FORMAT: &str = "{explabs_pct}%";

pub fn headline_pct(snapshot: &ExperientialLabsSnapshot) -> Option<i32> {
    snapshot.daily_worst().and_then(|meter| meter.pct())
}

pub fn severity(snapshot: &ExperientialLabsSnapshot) -> PaceSeverity {
    headline_pct(snapshot)
        .map(severity_for)
        .unwrap_or(PaceSeverity::Low)
}

/// The daily pool drawn as a shared window, with the raw `used / limit` as the
/// detail so the percentage is never the only figure.
fn meter_pct(meter: Option<&ExperientialLabsMeter>) -> String {
    meter
        .and_then(ExperientialLabsMeter::pct)
        .map(|pct| pct.to_string())
        .unwrap_or_default()
}

fn meter_detail(meter: Option<&ExperientialLabsMeter>) -> String {
    meter
        .map(ExperientialLabsMeter::used_of_limit)
        .unwrap_or_default()
}

fn meter_window(
    meter: &ExperientialLabsMeter,
    resets_at: Option<DateTime<Utc>>,
    duration: chrono::Duration,
) -> Option<UsageWindow> {
    Some(UsageWindow {
        utilization_pct: meter.pct()?,
        resets_at,
        window_duration: duration,
    })
}

fn balance_text(snapshot: &ExperientialLabsSnapshot) -> String {
    snapshot.balance_usd.map(usd).unwrap_or_default()
}

fn key_limit_text(snapshot: &ExperientialLabsSnapshot) -> String {
    match (snapshot.key_limit_remaining_usd, snapshot.key_limit_usd) {
        (Some(remaining), Some(limit)) => format!("{} / {}", usd(remaining), usd(limit)),
        (Some(remaining), None) => usd(remaining),
        _ => String::new(),
    }
}

pub fn build_placeholders(
    snapshot: &ExperientialLabsSnapshot,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> HashMap<&'static str, String> {
    let daily = window_placeholders(snapshot.daily_window().as_ref(), opts, now);
    placeholders([
        ("icon", VendorId::ExperientialLabs.bar_icon().to_string()),
        (
            "vendor_short",
            VendorId::ExperientialLabs.short_name().to_string(),
        ),
        ("plan", snapshot.plan.clone()),
        // Generic aliases: the daily pool is the plan's primary window, and
        // there is no rolling weekly window, so `weekly_*` stays empty.
        ("session_pct", daily.pct.clone()),
        ("session_reset", daily.reset.clone()),
        ("session_elapsed", daily.elapsed.clone()),
        ("weekly_pct", String::new()),
        ("weekly_reset", String::new()),
        ("weekly_elapsed", String::new()),
        ("explabs_pct", daily.pct.clone()),
        ("explabs_reset", daily.reset.clone()),
        ("explabs_elapsed", daily.elapsed.clone()),
        ("explabs_pace", daily.ratio_pace.clone()),
        ("explabs_pace_indicator", daily.point_pace.clone()),
        (
            "explabs_daily_input_pct",
            meter_pct(snapshot.daily_input.as_ref()),
        ),
        (
            "explabs_daily_input",
            meter_detail(snapshot.daily_input.as_ref()),
        ),
        (
            "explabs_daily_output_pct",
            meter_pct(snapshot.daily_output.as_ref()),
        ),
        (
            "explabs_daily_output",
            meter_detail(snapshot.daily_output.as_ref()),
        ),
        ("explabs_backstop_pct", String::new()),
        ("explabs_backstop", String::new()),
        ("explabs_balance", balance_text(snapshot)),
        ("explabs_key_limit", key_limit_text(snapshot)),
    ])
}

pub fn render(
    outcome: &VendorOutcome,
    snapshot: &ExperientialLabsSnapshot,
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
            VendorId::ExperientialLabs.display_name().to_string()
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
    meter: Option<&ExperientialLabsMeter>,
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
    _outcome: &VendorOutcome,
    snapshot: &ExperientialLabsSnapshot,
    theme: &Theme,
    now: DateTime<Utc>,
) -> String {
    let dim = &theme.dim;
    let mut lines = vec![
        Line::Center("Experiential Labs".into()),
        Line::Body(format!(
            " <span foreground='{dim}'>{}</span>",
            balance_text(snapshot)
        )),
        Line::Sep,
    ];

    let period = match (snapshot.period_start, snapshot.period_end) {
        (Some(start), Some(end)) if end > start => end - start,
        _ => chrono::Duration::zero(),
    };

    if snapshot.daily_input.is_some() || snapshot.daily_output.is_some() {
        lines.push(Line::Body(" <b>Daily pool</b>".into()));
        for (label, meter) in [
            ("Input", snapshot.daily_input.as_ref()),
            ("Output", snapshot.daily_output.as_ref()),
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

    if let Some(_limit) = snapshot.key_limit_usd {
        lines.push(Line::Body(format!(
            " <span foreground='{dim}'>Key cap</span> {}",
            key_limit_text(snapshot)
        )));
    }

    render_bordered(&lines, theme)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explabs::types::FREE_DAILY_TOKEN_LIMIT;
    use crate::usage::ExperientialLabsModelUsage;

    fn snapshot() -> ExperientialLabsSnapshot {
        ExperientialLabsSnapshot {
            plan: "Free".into(),
            balance_usd: Some(250.0),
            daily_input: Some(ExperientialLabsMeter {
                used: 812_400,
                limit: FREE_DAILY_TOKEN_LIMIT,
            }),
            daily_output: Some(ExperientialLabsMeter {
                used: 187_600,
                limit: FREE_DAILY_TOKEN_LIMIT,
            }),
            key_limit_usd: Some(2000.0),
            key_limit_remaining_usd: Some(1765.55),
            period_start: None,
            period_end: None,
            models: vec![ExperientialLabsModelUsage {
                model: "claude-xpl".into(),
                input_tokens: 812_400,
                output_tokens: 187_600,
                cost_usd: 1.75,
            }],
        }
    }

    #[test]
    fn daily_pct_is_the_worse_meter_against_the_shared_pool() {
        let snapshot = snapshot();
        assert_eq!(
            snapshot.daily_worst().and_then(|meter| meter.pct()),
            Some(100)
        );
    }

    #[test]
    fn balance_is_credits_minus_usage() {
        let snapshot = snapshot();
        assert_eq!(snapshot.balance(), 250.0);
    }
}
