//! Droid renderer — Standard quota windows (5h/weekly/monthly), the Droid
//! Core bucket, and the optional extra-usage balance.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::format::{placeholders, substitute, updated_at_hm, window_placeholders};
use crate::pacing::PaceSeverity;
use crate::pango::{color_span, escape, severity_color, severity_for};
use crate::theme::Theme;
use crate::tooltip::{Line as TooltipLine, push_window, render_bordered};
use crate::usage::DroidSnapshot;
use crate::vendor::{RenderOpts, VendorId, VendorOutcome};
use crate::waybar::{Class, WaybarOutput};

use super::fetch::FetchOutcome;

/// The bar's short text: the busiest active Standard window, matching what
/// the `droid` CLI's `/limits` leads with.
pub const DEFAULT_FORMAT: &str = "{droid_headline_pct}%";

/// `extraUsageBalanceCents` is cents; every surface that shows it goes
/// through this one helper, which hands the integer to the shared minor-unit
/// formatter (a balance can go negative, and the sign stays outside the
/// symbol).
pub fn extra_balance(balance_cents: i64) -> String {
    crate::usage::fmt_minor(balance_cents, 2, None)
}

fn cents(balance: Option<i64>) -> String {
    balance.map(extra_balance).unwrap_or_default()
}

/// Largest `usedPercent` among the *started* Standard windows — the single
/// headline policy for the bar text, the bar's severity class, and the TUI
/// overview headline. A window the account has not started (absent, or
/// `windowEnd: null` -> `resets_at: None`) does not participate, so an
/// all-unstarted account reports no headline. Droid Core never counts: it
/// has its own section.
pub fn headline_utilization(snapshot: &DroidSnapshot) -> Option<i32> {
    [&snapshot.session, &snapshot.weekly, &snapshot.monthly]
        .into_iter()
        .flatten()
        .filter(|window| window.resets_at.is_some())
        .map(|window| window.utilization_pct)
        .max()
}

fn headline_pct(snapshot: &DroidSnapshot) -> String {
    headline_utilization(snapshot)
        .map(|pct| pct.to_string())
        .unwrap_or_default()
}

pub fn build_placeholders(
    snapshot: &DroidSnapshot,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> HashMap<&'static str, String> {
    let session = window_placeholders(snapshot.session.as_ref(), opts, now);
    let weekly = window_placeholders(snapshot.weekly.as_ref(), opts, now);
    let monthly = window_placeholders(snapshot.monthly.as_ref(), opts, now);
    let core_session = window_placeholders(snapshot.core_session.as_ref(), opts, now);
    let core_weekly = window_placeholders(snapshot.core_weekly.as_ref(), opts, now);
    let core_monthly = window_placeholders(snapshot.core_monthly.as_ref(), opts, now);
    placeholders(vec![
        ("icon", VendorId::Droid.bar_icon().to_string()),
        ("vendor_short", VendorId::Droid.short_name().to_string()),
        ("plan", VendorId::Droid.display_name().to_string()),
        // The generic aliases the macOS bar and shared formats read:
        // Standard's 5-hour and weekly windows.
        ("session_pct", session.pct.clone()),
        ("session_reset", session.reset.clone()),
        ("session_elapsed", session.elapsed.clone()),
        ("weekly_pct", weekly.pct.clone()),
        ("weekly_reset", weekly.reset.clone()),
        ("weekly_elapsed", weekly.elapsed.clone()),
        ("droid_headline_pct", headline_pct(snapshot)),
        ("droid_session_pct", session.pct),
        ("droid_session_reset", session.reset),
        ("droid_session_elapsed", session.elapsed),
        ("droid_session_pace", session.ratio_pace),
        ("droid_session_pace_indicator", session.point_pace),
        ("droid_weekly_pct", weekly.pct),
        ("droid_weekly_reset", weekly.reset),
        ("droid_weekly_elapsed", weekly.elapsed),
        ("droid_weekly_pace", weekly.ratio_pace),
        ("droid_weekly_pace_indicator", weekly.point_pace),
        ("droid_monthly_pct", monthly.pct),
        ("droid_monthly_reset", monthly.reset),
        ("droid_monthly_elapsed", monthly.elapsed),
        ("droid_monthly_pace", monthly.ratio_pace),
        ("droid_monthly_pace_indicator", monthly.point_pace),
        ("droid_core_session_pct", core_session.pct),
        ("droid_core_session_reset", core_session.reset),
        ("droid_core_session_elapsed", core_session.elapsed),
        ("droid_core_session_pace", core_session.ratio_pace),
        ("droid_core_session_pace_indicator", core_session.point_pace),
        ("droid_core_weekly_pct", core_weekly.pct),
        ("droid_core_weekly_reset", core_weekly.reset),
        ("droid_core_weekly_elapsed", core_weekly.elapsed),
        ("droid_core_weekly_pace", core_weekly.ratio_pace),
        ("droid_core_weekly_pace_indicator", core_weekly.point_pace),
        ("droid_core_monthly_pct", core_monthly.pct),
        ("droid_core_monthly_reset", core_monthly.reset),
        ("droid_core_monthly_elapsed", core_monthly.elapsed),
        ("droid_core_monthly_pace", core_monthly.ratio_pace),
        ("droid_core_monthly_pace_indicator", core_monthly.point_pace),
        ("droid_extra_balance", cents(snapshot.extra_balance_cents)),
    ])
}

/// The bar's class follows the same headline policy: only started Standard
/// windows — a saturated Droid Core bucket must not paint the bar critical
/// while the Standard headline reads a low figure.
pub fn severity(snapshot: &DroidSnapshot) -> PaceSeverity {
    headline_utilization(snapshot)
        .map(severity_for)
        .unwrap_or(PaceSeverity::Low)
}

pub fn render(
    outcome: &VendorOutcome,
    snapshot: &DroidSnapshot,
    theme: &Theme,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> WaybarOutput {
    let severity = severity(snapshot);
    let class = Class::from(severity);
    let format = opts
        .format
        .clone()
        .unwrap_or_else(|| default_format(snapshot));
    let values = build_placeholders(snapshot, opts, now);
    let mut text = substitute(&format, &values);
    if outcome.stale {
        text.push_str(" ⏸");
    }
    let icon = opts
        .icon
        .as_deref()
        .filter(|icon| !icon.is_empty())
        .map(|icon| format!("{icon} "))
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

fn default_format(snapshot: &DroidSnapshot) -> String {
    if headline_utilization(snapshot).is_some() {
        DEFAULT_FORMAT.to_string()
    } else {
        VendorId::Droid.display_name().to_string()
    }
}

fn render_tooltip(
    outcome: &VendorOutcome,
    snapshot: &DroidSnapshot,
    theme: &Theme,
    now: DateTime<Utc>,
) -> String {
    let blue = &theme.blue;
    let dim = &theme.dim;
    let mut lines = vec![
        TooltipLine::Center(format!(
            "<span font_weight='bold' foreground='{blue}'>{}</span>",
            escape(VendorId::Droid.display_name())
        )),
        TooltipLine::Sep,
        TooltipLine::Body(String::new()),
    ];
    let standard = [&snapshot.session, &snapshot.weekly, &snapshot.monthly];
    if standard.iter().any(|w| w.is_some()) {
        for (label, window) in [
            ("  Standard 5h", &snapshot.session),
            ("  Standard weekly", &snapshot.weekly),
            ("  Standard monthly", &snapshot.monthly),
        ] {
            if let Some(window) = window.as_ref() {
                push_window(&mut lines, label, window, theme, now, None);
            }
        }
    }
    let core = [
        &snapshot.core_session,
        &snapshot.core_weekly,
        &snapshot.core_monthly,
    ];
    if core.iter().any(|w| w.is_some()) {
        if standard.iter().any(|w| w.is_some()) {
            lines.push(TooltipLine::Body(String::new()));
            lines.push(TooltipLine::Body(format!(
                " <span foreground='{dim}'>  Droid Core</span>"
            )));
        }
        for (label, window) in [
            ("  Droid Core 5h", &snapshot.core_session),
            ("  Droid Core weekly", &snapshot.core_weekly),
            ("  Droid Core monthly", &snapshot.core_monthly),
        ] {
            if let Some(window) = window.as_ref() {
                push_window(&mut lines, label, window, theme, now, None);
            }
        }
    }
    if standard.iter().all(|w| w.is_none())
        && core.iter().all(|w| w.is_none())
        && snapshot.extra_balance_cents.is_none()
    {
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{dim}'>  no quota fields reported</span>"
        )));
    }
    if let Some(balance) = snapshot.extra_balance_cents {
        lines.push(TooltipLine::Body(String::new()));
        lines.push(TooltipLine::Body(format!(
            "  Extra usage balance: {}",
            escape(&extra_balance(balance))
        )));
    }
    if let Some((code, message)) = outcome.last_error.as_ref() {
        if *code != 0 {
            lines.push(TooltipLine::Body(String::new()));
            lines.push(TooltipLine::Sep);
            lines.push(TooltipLine::Body(format!(
                " <span foreground='{}'>  HTTP {code}</span>",
                theme.orange
            )));
        }
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{dim}'>  {}</span>",
            escape(message)
        )));
    }
    let updated = updated_at_hm(now, outcome.cache_age);
    lines.push(TooltipLine::Body(String::new()));
    lines.push(TooltipLine::Sep);
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{dim}'>  Updated {updated}</span>"
    )));
    render_bordered(&lines, theme)
}

impl From<FetchOutcome> for VendorOutcome {
    fn from(outcome: FetchOutcome) -> Self {
        outcome.map(crate::usage::VendorSnapshot::Droid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::UsageWindow;

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_791_014_400, 0).unwrap()
    }

    fn window(pct: i32, hours: i64) -> UsageWindow {
        UsageWindow {
            utilization_pct: pct,
            resets_at: Some(now() + chrono::Duration::hours(2)),
            window_duration: chrono::Duration::hours(hours),
        }
    }

    fn snapshot() -> DroidSnapshot {
        DroidSnapshot {
            session: Some(window(2, 5)),
            weekly: Some(window(1, 168)),
            monthly: Some(window(11, 720)),
            core_session: Some(UsageWindow {
                utilization_pct: 0,
                resets_at: None,
                window_duration: chrono::Duration::hours(5),
            }),
            core_weekly: None,
            core_monthly: None,
            extra_balance_cents: Some(250),
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

    /// A window the account has not started: reported as
    /// `{"usedPercent":0,"windowEnd":null}`, so it carries a percentage but
    /// no reset.
    fn unstarted(pct: i32) -> UsageWindow {
        UsageWindow {
            utilization_pct: pct,
            resets_at: None,
            window_duration: chrono::Duration::hours(5),
        }
    }

    fn outcome(snapshot: &DroidSnapshot) -> VendorOutcome {
        VendorOutcome {
            snapshot: crate::usage::VendorSnapshot::Droid(snapshot.clone()),
            stale: false,
            last_error: None,
            cache_age: Some(std::time::Duration::ZERO),
        }
    }

    #[test]
    fn the_headline_is_the_busiest_active_standard_window() {
        let values = build_placeholders(&snapshot(), &opts(), now());
        assert_eq!(
            values.get("droid_headline_pct").map(String::as_str),
            Some("11")
        );
        let mut none = snapshot();
        none.session = None;
        none.weekly = None;
        none.monthly = None;
        assert_eq!(headline_pct(&none), "");
        // A started Core window never feeds the headline either.
        let mut core_only = none.clone();
        core_only.core_session = Some(window(80, 5));
        assert_eq!(headline_pct(&core_only), "");
    }

    #[test]
    fn unstarted_standard_windows_stay_out_of_the_bar_text() {
        let mut unstarted_only = snapshot();
        unstarted_only.session = Some(unstarted(0));
        unstarted_only.weekly = Some(unstarted(0));
        unstarted_only.monthly = Some(unstarted(0));
        assert_eq!(headline_pct(&unstarted_only), "");

        let rendered = render(
            &outcome(&unstarted_only),
            &unstarted_only,
            &Theme::default(),
            &opts(),
            now(),
        );
        assert!(rendered.text.contains("Droid"), "{}", rendered.text);
        assert!(!rendered.text.contains('%'), "{}", rendered.text);
    }

    #[test]
    fn a_busy_core_bucket_sets_neither_the_bar_text_nor_its_class() {
        // Core 100% + Standard 2%: the bar reads "2%" with no critical class.
        let mut snapshot = snapshot();
        snapshot.session = Some(window(2, 5));
        snapshot.weekly = Some(unstarted(0));
        snapshot.monthly = Some(unstarted(0));
        snapshot.core_session = Some(window(100, 5));
        assert_eq!(severity(&snapshot), PaceSeverity::Low);

        let rendered = render(
            &outcome(&snapshot),
            &snapshot,
            &Theme::default(),
            &opts(),
            now(),
        );
        assert!(rendered.text.contains("2%"), "{}", rendered.text);
        assert_eq!(rendered.class, Class::Low);
    }

    #[test]
    fn generic_aliases_and_droid_placeholders_share_the_standard_windows() {
        let values = build_placeholders(&snapshot(), &opts(), now());
        assert_eq!(values.get("session_pct").map(String::as_str), Some("2"));
        assert_eq!(values.get("weekly_pct").map(String::as_str), Some("1"));
        assert_eq!(
            values.get("droid_monthly_pct").map(String::as_str),
            Some("11")
        );
        assert_eq!(
            values.get("droid_core_session_pct").map(String::as_str),
            Some("0")
        );
        assert_eq!(
            values.get("droid_extra_balance").map(String::as_str),
            Some("$2.50")
        );
        assert_eq!(values.get("plan").map(String::as_str), Some("Droid"));
        assert_eq!(values.get("vendor_short").map(String::as_str), Some("drd"));
    }

    #[test]
    fn render_shows_the_headline_and_the_balance() {
        let snapshot = snapshot();
        let outcome = VendorOutcome {
            snapshot: crate::usage::VendorSnapshot::Droid(snapshot.clone()),
            stale: false,
            last_error: None,
            cache_age: Some(std::time::Duration::ZERO),
        };
        let rendered = render(&outcome, &snapshot, &Theme::default(), &opts(), now());
        assert!(rendered.text.contains("11%"), "{}", rendered.text);
        assert!(
            rendered.tooltip.contains("Droid Core"),
            "{}",
            rendered.tooltip
        );
        assert!(rendered.tooltip.contains("$2.50"));
    }
}
