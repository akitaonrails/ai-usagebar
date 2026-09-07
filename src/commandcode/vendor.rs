use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::countdown;
use crate::format::{placeholders, substitute, updated_at_hm, usd};
use crate::pacing::PaceSeverity;
use crate::pango::{color_span, escape, severity_color, severity_for};
use crate::theme::Theme;
use crate::tooltip::{Line as TooltipLine, render_bordered};
use crate::vendor::{RenderOpts, VendorId, VendorOutcome};
use crate::waybar::{Class, WaybarOutput};

use super::fetch::FetchOutcome;
use super::types::{Snapshot, SpendWindow};

pub const DEFAULT_FORMAT: &str = "{cc_session_pct}% · {cc_session_reset}";
const DEFAULT_PLAN: &str = "Command Code";
const UNAVAILABLE: &str = "—";

impl From<FetchOutcome> for VendorOutcome {
    fn from(outcome: FetchOutcome) -> Self {
        outcome.map(crate::usage::VendorSnapshot::CommandCode)
    }
}

pub fn build_placeholders(snap: &Snapshot, now: DateTime<Utc>) -> HashMap<&'static str, String> {
    let plan = sanitize(snap.plan.as_deref().unwrap_or(DEFAULT_PLAN));
    let session = window_values(snap.five_hour.as_ref(), now);
    let weekly = window_values(snap.weekly.as_ref(), now);
    // The monthly allowance rendered as a window of its own: dollars drawn
    // from the plan pool, refilling at the billing period end.
    let monthly = snap.monthly_window();
    let monthly_values = window_values(monthly.as_ref(), now);
    let remaining = snap
        .credits
        .as_ref()
        .map(|credits| usd(credits.remaining()))
        .unwrap_or_else(|| UNAVAILABLE.to_string());
    let pool = snap
        .credit_pool
        .map(usd)
        .unwrap_or_else(|| UNAVAILABLE.to_string());
    let spent = snap
        .credits_spent()
        .map(usd)
        .unwrap_or_else(|| UNAVAILABLE.to_string());
    // When the monthly ledger refills (billing period end). Absent until the
    // subscription supplies it.
    let credits_reset = snap
        .period_end
        .map(|at| countdown::format(Some(at), now))
        .unwrap_or_else(|| UNAVAILABLE.to_string());

    placeholders([
        (
            "vendor_short",
            VendorId::CommandCode.short_name().to_string(),
        ),
        ("plan", plan.clone()),
        ("cc_plan", plan),
        // Generic names so a shared format string works across vendors.
        ("session_pct", session.percent.clone()),
        ("session_reset", session.reset.clone()),
        ("weekly_pct", weekly.percent.clone()),
        ("weekly_reset", weekly.reset.clone()),
        ("cc_session_pct", session.percent),
        ("cc_session_reset", session.reset),
        ("cc_session_used", session.used),
        ("cc_session_cap", session.cap),
        ("cc_weekly_pct", weekly.percent),
        ("cc_weekly_reset", weekly.reset),
        ("cc_weekly_used", weekly.used),
        ("cc_weekly_cap", weekly.cap),
        ("cc_monthly_pct", monthly_values.percent.clone()),
        ("cc_monthly_reset", monthly_values.reset.clone()),
        ("cc_monthly_used", monthly_values.used.clone()),
        ("cc_monthly_cap", monthly_values.cap.clone()),
        ("cc_credits", remaining),
        ("cc_credits_pool", pool),
        ("cc_credits_spent", spent),
        ("cc_credits_reset", credits_reset),
    ])
}

#[derive(Debug)]
struct WindowValues {
    percent: String,
    reset: String,
    used: String,
    cap: String,
}

fn window_values(window: Option<&SpendWindow>, now: DateTime<Utc>) -> WindowValues {
    let Some(window) = window else {
        return WindowValues {
            percent: UNAVAILABLE.to_string(),
            reset: UNAVAILABLE.to_string(),
            used: UNAVAILABLE.to_string(),
            cap: UNAVAILABLE.to_string(),
        };
    };
    WindowValues {
        percent: window.pct().to_string(),
        reset: countdown::format(window.resets_at, now),
        used: usd(window.used),
        cap: usd(window.cap),
    }
}

fn sanitize(value: &str) -> String {
    crate::display::sanitize_untrusted_field(value)
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
    render_with_meta(
        snap,
        outcome.stale,
        outcome.last_error.as_ref(),
        outcome.cache_age,
        theme,
        opts,
        now,
    )
}

fn render_with_meta(
    snap: &Snapshot,
    stale: bool,
    last_error: Option<&(u16, String)>,
    cache_age: Option<Duration>,
    theme: &Theme,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> WaybarOutput {
    let sev = severity(snap);
    let format = opts.format.as_deref().unwrap_or(DEFAULT_FORMAT);
    let values = escaped_placeholders(snap, now);
    let mut text = substitute(format, &values);
    if stale {
        text.push_str(" ⏸");
    }
    let icon_prefix = match opts.icon.as_deref() {
        Some(icon) if !icon.is_empty() => format!("{} ", escape(icon)),
        _ => String::new(),
    };
    let bar_text = color_span(severity_color(sev, theme), &format!("{icon_prefix}{text}"));
    let tooltip = opts
        .tooltip_format
        .as_deref()
        .map(|format| substitute(format, &values))
        .unwrap_or_else(|| render_tooltip(snap, stale, last_error, cache_age, theme, now));

    WaybarOutput {
        text: bar_text,
        tooltip,
        class: Class::from(sev),
    }
}

fn escaped_placeholders(snap: &Snapshot, now: DateTime<Utc>) -> HashMap<&'static str, String> {
    let mut values = build_placeholders(snap, now);
    for key in ["plan", "cc_plan"] {
        if let Some(value) = values.get_mut(key) {
            *value = escape(value);
        }
    }
    values
}

fn render_tooltip(
    snap: &Snapshot,
    stale: bool,
    last_error: Option<&(u16, String)>,
    cache_age: Option<Duration>,
    theme: &Theme,
    now: DateTime<Utc>,
) -> String {
    let plan = snap.plan.as_deref().unwrap_or(DEFAULT_PLAN);
    let mut lines = vec![TooltipLine::Center(format!(
        "<span font_weight='bold' foreground='{}'>{}</span>",
        theme.blue,
        escape(&sanitize(plan))
    ))];
    lines.push(TooltipLine::Sep);
    lines.push(TooltipLine::Body(String::new()));

    let mut present = false;
    for (label, window) in [
        ("Session (5h)", snap.five_hour.as_ref()),
        ("Weekly", snap.weekly.as_ref()),
        ("Monthly", snap.monthly_window().as_ref()),
    ] {
        let Some(window) = window else {
            continue;
        };
        present = true;
        let values = window_values(Some(window), now);
        lines.push(TooltipLine::Body(format!(
            "  {}  {}% · {} of {} · {}",
            label,
            escape(&values.percent),
            escape(&values.used),
            escape(&values.cap),
            escape(&values.reset)
        )));
    }
    if !present {
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{}'>no usage windows reported</span>",
            theme.dim
        )));
    }

    if let Some(credits) = snap.credits.as_ref() {
        lines.push(TooltipLine::Body(String::new()));
        // The Monthly row above already carries the spend-of-pool detail, so
        // the ledger line only adds the raw remaining balance and the refill.
        let reset = match snap.period_end {
            Some(at) => format!(" · resets in {}", countdown::format(Some(at), now)),
            None => String::new(),
        };
        lines.push(TooltipLine::Body(format!(
            "  Credits  {}{}",
            escape(&usd(credits.remaining())),
            escape(&reset)
        )));
    }

    if stale {
        lines.push(TooltipLine::Body(String::new()));
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{}'>  ⏸  Showing cached data</span>",
            theme.orange
        )));
    }
    if let Some((code, message)) = last_error
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
        updated_at_hm(now, cache_age)
    )));
    render_bordered(&lines, theme)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commandcode::types::{Credits, Snapshot, SpendWindow};

    fn at(value: &str) -> DateTime<Utc> {
        value.parse().expect("RFC3339 timestamp")
    }

    fn sample() -> Snapshot {
        Snapshot {
            plan: Some("GOAT".into()),
            five_hour: Some(SpendWindow {
                used: 1.23,
                cap: 14.0,
                resets_at: Some(at("2026-08-27T04:40:19Z")),
            }),
            weekly: Some(SpendWindow {
                used: 5.24,
                cap: 35.0,
                resets_at: Some(at("2026-09-02T18:36:12Z")),
            }),
            credits: Some(Credits {
                monthly: 49.28,
                purchased: 0.0,
                free: 0.0,
            }),
            credit_pool: Some(70.0),
            period_end: Some(at("2026-09-17T14:28:52Z")),
        }
    }

    #[test]
    fn exposes_exact_and_generic_placeholders() {
        let values = build_placeholders(&sample(), at("2026-08-27T02:30:00Z"));

        assert_eq!(values["vendor_short"], "cmc");
        assert_eq!(values["plan"], "GOAT");
        assert_eq!(values["cc_session_pct"], "9");
        assert_eq!(values["cc_weekly_pct"], "15");
        // The monthly allowance is a derived window: $20.72 of the $70 pool.
        assert_eq!(values["cc_monthly_pct"], "30");
        assert_eq!(values["cc_monthly_used"], "$20.72");
        assert_eq!(values["cc_monthly_cap"], "$70.00");
        assert_eq!(values["cc_monthly_reset"], "21d 11h");
        // Generic aliases keep a shared format string working across vendors.
        assert_eq!(values["session_pct"], "9");
        assert_eq!(values["weekly_pct"], "15");
    }

    #[test]
    fn spend_figures_use_the_shared_money_formatter() {
        let values = build_placeholders(&sample(), at("2026-08-27T02:30:00Z"));

        assert_eq!(values["cc_session_used"], "$1.23");
        assert_eq!(values["cc_session_cap"], "$14.00");
        assert_eq!(values["cc_credits"], "$49.28");
        assert_eq!(values["cc_credits_pool"], "$70.00");
        assert_eq!(values["cc_credits_spent"], "$20.72");
        // 2026-09-17 minus 2026-08-27 → 21 days and change.
        assert_eq!(values["cc_credits_reset"], "21d 11h");
    }

    #[test]
    fn monthly_window_needs_ledger_and_a_recognised_plan() {
        // No ledger: nothing to derive the spend from.
        let no_ledger = Snapshot {
            credits: None,
            credit_pool: Some(70.0),
            period_end: Some(at("2026-09-17T14:28:52Z")),
            ..sample()
        };
        assert!(no_ledger.monthly_window().is_none());
        assert_eq!(no_ledger.worst_pct(), 15);

        // No pool: no denominator.
        let no_pool = Snapshot {
            credit_pool: None,
            ..sample()
        };
        assert!(no_pool.monthly_window().is_none());
    }

    #[test]
    fn default_format_leads_with_the_session_window() {
        assert_eq!(DEFAULT_FORMAT, "{cc_session_pct}% · {cc_session_reset}");
    }

    #[test]
    fn absent_windows_and_ledger_are_unavailable_not_zero() {
        let values = build_placeholders(&Snapshot::default(), at("2026-08-27T02:30:00Z"));

        for key in [
            "session_pct",
            "weekly_pct",
            "cc_session_pct",
            "cc_session_reset",
            "cc_weekly_pct",
            "cc_session_used",
            "cc_credits",
            "cc_credits_pool",
            "cc_credits_spent",
        ] {
            assert_eq!(values[key], UNAVAILABLE, "{key} should be unavailable");
            assert_ne!(values[key], "0");
        }
        // With no plan, the vendor name stands in.
        assert_eq!(values["plan"], DEFAULT_PLAN);
    }

    #[test]
    fn severity_follows_the_window_closest_to_its_cap() {
        let mut snapshot = sample();
        assert_eq!(severity(&snapshot), severity_for(15));

        snapshot.weekly = Some(SpendWindow {
            used: 34.0,
            cap: 35.0,
            resets_at: None,
        });
        assert_eq!(severity(&snapshot), severity_for(97));
    }

    #[test]
    fn plan_is_sanitized_before_it_reaches_the_bar() {
        let snapshot = Snapshot {
            plan: Some("GO\u{1b}[31mAT\u{7}".into()),
            ..sample()
        };

        let values = build_placeholders(&snapshot, at("2026-08-27T02:30:00Z"));

        assert!(!values["plan"].contains('\u{1b}'));
        assert!(!values["plan"].contains('\u{7}'));
        assert!(!values["cc_plan"].contains('\u{1b}'));
    }

    #[test]
    fn tooltip_shows_both_windows_and_the_credit_ledger() {
        let theme = Theme::default();
        let tooltip = render_tooltip(
            &sample(),
            false,
            None,
            None,
            &theme,
            at("2026-08-27T02:30:00Z"),
        );

        assert!(tooltip.contains("GOAT"), "{tooltip}");
        assert!(tooltip.contains("Session (5h)"), "{tooltip}");
        assert!(tooltip.contains("$1.23 of $14.00"), "{tooltip}");
        assert!(tooltip.contains("Weekly"), "{tooltip}");
        // The monthly allowance renders as a third window row.
        assert!(tooltip.contains("Monthly"), "{tooltip}");
        assert!(tooltip.contains("$20.72 of $70.00"), "{tooltip}");
        assert!(tooltip.contains("$49.28"), "{tooltip}");
        assert!(tooltip.contains("resets in 21d 11h"), "{tooltip}");
    }

    #[test]
    fn tooltip_says_so_when_the_vendor_reports_no_windows() {
        let theme = Theme::default();
        let tooltip = render_tooltip(
            &Snapshot::default(),
            false,
            None,
            None,
            &theme,
            at("2026-08-27T02:30:00Z"),
        );

        assert!(tooltip.contains("no usage windows reported"), "{tooltip}");
    }

    #[test]
    fn stale_and_http_errors_surface_in_the_tooltip() {
        let theme = Theme::default();
        let tooltip = render_tooltip(
            &sample(),
            true,
            Some(&(503, "service unavailable".to_string())),
            None,
            &theme,
            at("2026-08-27T02:30:00Z"),
        );

        assert!(tooltip.contains("Showing cached data"), "{tooltip}");
        assert!(tooltip.contains("HTTP 503"), "{tooltip}");
    }

    #[test]
    fn an_unknown_plan_still_renders_without_an_allowance_line() {
        let snapshot = Snapshot {
            plan: Some("individual-future".into()),
            credit_pool: None,
            ..sample()
        };
        let theme = Theme::default();

        let tooltip = render_tooltip(
            &snapshot,
            false,
            None,
            None,
            &theme,
            at("2026-08-27T02:30:00Z"),
        );

        assert!(tooltip.contains("$49.28"), "{tooltip}");
        // No recognised plan → no monthly window, no allowance line at all.
        assert!(!tooltip.contains("Monthly"), "{tooltip}");
        assert!(!tooltip.contains("$20.72 of $70.00"), "{tooltip}");
    }
}
