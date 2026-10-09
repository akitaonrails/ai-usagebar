//! Ollama Cloud renderer — bar text + bordered Pango tooltip.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::countdown;
use crate::format::{placeholders, substitute, updated_at_hm};
use crate::pacing::{self, PaceSeverity};
use crate::pango::{color_span, escape, severity_color, severity_for};
use crate::theme::Theme;
use crate::tooltip::{Line as TooltipLine, WindowRow, push_window_with_row, render_bordered};
use crate::usage::{OllamaSnapshot, UsageWindow};
use crate::vendor::{RenderOpts, VendorId, VendorOutcome};
use crate::waybar::{Class, WaybarOutput};

use super::fetch::FetchOutcome;

pub const DEFAULT_FORMAT: &str = "{oll_session_pct}% · {oll_weekly_pct}%w";
/// Bar text for historical monthly-only quota caches, without dollar credits.
pub const MONTHLY_FORMAT: &str = "{oll_monthly_pct}%";

/// Pick the bar format that names a window the account actually reported.
/// Fabricating `0% · 0%w` for a monthly-only payload is a lie: those windows
/// were omitted, not exhausted. A present window at 0% used still renders `0`.
pub fn default_format(snap: &OllamaSnapshot) -> &'static str {
    if snap.credits.is_some() {
        "{oll_balance} / {oll_allowance}"
    } else if snap.session.is_some() || snap.weekly.is_some() {
        DEFAULT_FORMAT
    } else if snap.monthly.is_some() {
        MONTHLY_FORMAT
    } else {
        "{plan}"
    }
}

/// Build placeholders with the historical default pacing tolerance.
pub fn build_placeholders(
    snap: &OllamaSnapshot,
    now: DateTime<Utc>,
) -> HashMap<&'static str, String> {
    build_placeholders_with_tolerance(snap, pacing::DEFAULT_TOLERANCE, now)
}

fn build_placeholders_with_tolerance(
    snap: &OllamaSnapshot,
    pace_tolerance: u32,
    now: DateTime<Utc>,
) -> HashMap<&'static str, String> {
    let session_pct = window_pct(snap.session.as_ref());
    let weekly_pct = window_pct(snap.weekly.as_ref());
    let monthly_pct = window_pct(snap.monthly.as_ref());
    let session = window_pacing(snap.session.as_ref(), pace_tolerance, now);
    let weekly = window_pacing(snap.weekly.as_ref(), pace_tolerance, now);
    let monthly = window_pacing(snap.monthly.as_ref(), pace_tolerance, now);
    let cost = snap.activity_cost.clone().unwrap_or_else(|| "—".into());

    placeholders(vec![
        ("icon", "🦙".to_string()),
        ("vendor_short", VendorId::Ollama.short_name().to_string()),
        // Cross-vendor aliases for scroll-cycle friendly formats. Empty when
        // the account did not report that window, so a native surface cannot
        // mistake an omitted 5h/7d pair for a confident 0% bar. A present
        // window at 0% used still emits "0".
        ("session_pct", session_pct.clone()),
        (
            "session_reset",
            window_reset_text(snap.session.as_ref(), now),
        ),
        ("weekly_pct", weekly_pct.clone()),
        ("weekly_reset", window_reset_text(snap.weekly.as_ref(), now)),
        ("session_elapsed", session.elapsed.clone()),
        ("weekly_elapsed", weekly.elapsed.clone()),
        ("plan", snap.plan.clone()),
        ("oll_plan", snap.plan.clone()),
        ("oll_session_pct", session_pct),
        (
            "oll_session_reset",
            window_reset_text(snap.session.as_ref(), now),
        ),
        ("oll_weekly_pct", weekly_pct),
        (
            "oll_weekly_reset",
            window_reset_text(snap.weekly.as_ref(), now),
        ),
        ("oll_monthly_pct", monthly_pct),
        (
            "oll_monthly_reset",
            window_reset_text(snap.monthly.as_ref(), now),
        ),
        ("oll_session_elapsed", session.elapsed),
        ("oll_session_pace", session.ratio_pace),
        ("oll_session_pace_indicator", session.point_pace),
        ("oll_weekly_elapsed", weekly.elapsed),
        ("oll_weekly_pace", weekly.ratio_pace),
        ("oll_weekly_pace_indicator", weekly.point_pace),
        ("oll_monthly_elapsed", monthly.elapsed),
        ("oll_monthly_pace", monthly.ratio_pace),
        ("oll_monthly_pace_indicator", monthly.point_pace),
        ("oll_cost", cost),
        (
            "oll_balance",
            snap.credits
                .as_ref()
                .map(|c| c.balance.clone())
                .unwrap_or_default(),
        ),
        (
            "oll_allowance",
            snap.credits
                .as_ref()
                .map(|c| c.allowance.clone())
                .unwrap_or_default(),
        ),
        (
            "oll_purchased",
            snap.credits
                .as_ref()
                .and_then(|c| c.purchased.clone())
                .unwrap_or_default(),
        ),
    ])
}

fn window_pct(window: Option<&UsageWindow>) -> String {
    window
        .map(|w| w.utilization_pct.to_string())
        .unwrap_or_default()
}

/// Countdown for a window that exists. An omitted window stays empty so a
/// native parser's `isReported` / `quotaWindow` cannot revive it; a present
/// window with no timestamp still renders the shared `"—"` sentinel.
fn window_reset_text(window: Option<&UsageWindow>, now: DateTime<Utc>) -> String {
    match window {
        Some(w) => countdown::format(w.resets_at, now),
        None => String::new(),
    }
}

#[derive(Default)]
struct WindowPacing {
    elapsed: String,
    ratio_pace: String,
    point_pace: String,
}

fn window_pacing(w: Option<&UsageWindow>, pace_tolerance: u32, now: DateTime<Utc>) -> WindowPacing {
    let Some(w) = w else {
        return WindowPacing::default();
    };
    let p = pacing::calc(
        w.utilization_pct,
        w.resets_at,
        now,
        w.window_duration,
        pace_tolerance,
    );
    WindowPacing {
        elapsed: p.elapsed_pct.to_string(),
        ratio_pace: p.ratio_pace.glyph().to_string(),
        point_pace: p.point_pace.glyph().to_string(),
    }
}

pub fn severity(snap: &OllamaSnapshot) -> PaceSeverity {
    // Worst of the three windows — session fills fastest and is the one
    // that actually interrupts a chat mid-stream.
    let session = snap
        .session
        .as_ref()
        .map(|w| w.utilization_pct)
        .unwrap_or(0);
    let weekly = snap.weekly.as_ref().map(|w| w.utilization_pct).unwrap_or(0);
    let monthly = snap
        .monthly
        .as_ref()
        .map(|w| w.utilization_pct)
        .unwrap_or(0);
    severity_for(session.max(weekly).max(monthly))
}

pub fn render(
    outcome: &VendorOutcome,
    snap: &OllamaSnapshot,
    theme: &Theme,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> WaybarOutput {
    let class = Class::from(severity(snap));
    let format = opts
        .format
        .clone()
        .unwrap_or_else(|| default_format(snap).to_string());
    let mut values = build_placeholders_with_tolerance(snap, opts.pace_tolerance, now);
    // Both sinks fed by this map (bar text and --tooltip-format) are Pango
    // markup. The plan label is configurable, so escape its aliases at the
    // projection boundary. The default tooltip escapes the raw snapshot.
    for key in ["plan", "oll_plan"] {
        if let Some(value) = values.get_mut(key) {
            *value = escape(value);
        }
    }

    let mut text = substitute(&format, &values);
    if outcome.stale {
        text.push('…');
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
        render_tooltip(outcome, snap, theme, opts.pace_tolerance, now)
    };

    WaybarOutput {
        text: bar_text,
        tooltip,
        class,
    }
}

fn row(w: &UsageWindow) -> WindowRow {
    // Keep the compact tooltip row; resets are rendered by the shared helper.
    let _ = w;
    WindowRow::default()
}

fn render_tooltip(
    outcome: &VendorOutcome,
    snap: &OllamaSnapshot,
    theme: &Theme,
    _pace_tolerance: u32,
    now: DateTime<Utc>,
) -> String {
    let blue = &theme.blue;
    let dim = &theme.dim;

    let mut lines: Vec<TooltipLine> = Vec::new();
    lines.push(TooltipLine::Center(format!(
        "<span font_weight='bold' foreground='{blue}'>Ollama Cloud</span>"
    )));
    if !snap.plan.is_empty() {
        lines.push(TooltipLine::Center(format!(
            "<span foreground='{dim}'>{}</span>",
            escape(&snap.plan)
        )));
    }
    lines.push(TooltipLine::Sep);
    lines.push(TooltipLine::Body("".into()));

    if let Some(w) = snap.session.as_ref() {
        push_window_with_row(&mut lines, "  Session (5h)", w, theme, now, row(w));
        if !snap.session_models.is_empty() {
            push_model_rows(&mut lines, &snap.session_models, dim);
        }
        lines.push(TooltipLine::Body("".into()));
    }

    if let Some(w) = snap.weekly.as_ref() {
        push_window_with_row(&mut lines, "  Weekly", w, theme, now, row(w));
        if !snap.weekly_models.is_empty() {
            push_model_rows(&mut lines, &snap.weekly_models, dim);
        }
        lines.push(TooltipLine::Body("".into()));
    }

    if let Some(w) = snap.monthly.as_ref() {
        push_window_with_row(&mut lines, "  Monthly", w, theme, now, row(w));
        if !snap.monthly_models.is_empty() {
            push_model_rows(&mut lines, &snap.monthly_models, dim);
        }
        lines.push(TooltipLine::Body("".into()));
    }

    if let Some(credits) = &snap.credits {
        lines.push(TooltipLine::Body(format!(
            "   {} remaining of {}",
            escape(&credits.balance),
            escape(&credits.allowance)
        )));
        if let Some(purchased) = &credits.purchased {
            lines.push(TooltipLine::Body(format!(
                "   Purchased credits: {}",
                escape(purchased)
            )));
        }
        lines.push(TooltipLine::Body("".into()));
    }

    if let Some(cost) = snap.activity_cost.as_deref() {
        let period = snap.activity_period.as_deref().unwrap_or("activity");
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{dim}'>  $  {period}</span>"
        )));
        lines.push(TooltipLine::Body(format!(
            "   <span foreground='{dim}'>${}</span>",
            escape(cost)
        )));
    }

    if let Some((code, msg)) = outcome.last_error.as_ref()
        && *code != 0
    {
        let (icon, ecolor) = if *code >= 500 {
            ("!", theme.red.as_str())
        } else {
            ("!", theme.orange.as_str())
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
        " <span foreground='{dim}'>  Updated {updated}</span>"
    )));

    render_bordered(&lines, theme)
}

fn push_model_rows(
    lines: &mut Vec<TooltipLine>,
    models: &[crate::usage::OllamaModelUsage],
    dim: &str,
) {
    // Cap the tooltip — a long week can have many models; show the top few.
    for m in models.iter().take(6) {
        lines.push(TooltipLine::Body(format!(
            "     <span foreground='{dim}'>{} · {} req</span>",
            escape(&m.name),
            m.request_count
        )));
    }
    if models.len() > 6 {
        lines.push(TooltipLine::Body(format!(
            "     <span foreground='{dim}'>… +{} more</span>",
            models.len() - 6
        )));
    }
}

impl From<FetchOutcome> for VendorOutcome {
    fn from(o: FetchOutcome) -> Self {
        o.map(crate::usage::VendorSnapshot::Ollama)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{OllamaModelUsage, OllamaSnapshot, UsageWindow};

    fn sample_snap() -> OllamaSnapshot {
        OllamaSnapshot {
            plan: "pro".into(),
            session: Some(UsageWindow {
                utilization_pct: 82,
                resets_at: None,
                window_duration: chrono::Duration::hours(5),
            }),
            weekly: Some(UsageWindow {
                utilization_pct: 23,
                resets_at: None,
                window_duration: chrono::Duration::days(7),
            }),
            monthly: None,
            session_models: vec![OllamaModelUsage {
                name: "kimi-k3".into(),
                request_count: 180,
            }],
            weekly_models: vec![
                OllamaModelUsage {
                    name: "kimi-k3".into(),
                    request_count: 180,
                },
                OllamaModelUsage {
                    name: "minimax-m3".into(),
                    request_count: 554,
                },
            ],
            monthly_models: vec![],
            activity_cost: Some("0.00000".into()),
            activity_period: Some("last_4_weeks".into()),
            credits: None,
        }
    }

    fn sample_outcome(snap: OllamaSnapshot) -> VendorOutcome {
        VendorOutcome {
            snapshot: crate::usage::VendorSnapshot::Ollama(snap),
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
            pace_tolerance: pacing::DEFAULT_TOLERANCE,
            format_pace_color: false,
            tooltip_pace_pts: false,
        }
    }

    fn monthly_snap(pct: i32) -> OllamaSnapshot {
        OllamaSnapshot {
            plan: "pro".into(),
            session: None,
            weekly: None,
            monthly: Some(UsageWindow {
                utilization_pct: pct,
                resets_at: None,
                window_duration: chrono::Duration::days(30),
            }),
            session_models: vec![],
            weekly_models: vec![],
            monthly_models: vec![OllamaModelUsage {
                name: "gpt-oss:120b".into(),
                request_count: 100,
            }],
            activity_cost: Some("0.00000".into()),
            activity_period: Some("last_4_weeks".into()),
            credits: None,
        }
    }

    #[test]
    fn placeholders_expose_session_and_weekly() {
        let now = Utc::now();
        let values = build_placeholders(&sample_snap(), now);
        assert_eq!(
            values.get("oll_session_pct").map(String::as_str),
            Some("82")
        );
        assert_eq!(values.get("oll_weekly_pct").map(String::as_str), Some("23"));
        assert_eq!(values.get("session_pct").map(String::as_str), Some("82"));
        assert_eq!(values.get("weekly_pct").map(String::as_str), Some("23"));
        assert_eq!(values.get("oll_monthly_pct").map(String::as_str), Some(""));
        assert_eq!(values.get("oll_cost").map(String::as_str), Some("0.00000"));
        assert_eq!(values.get("vendor_short").map(String::as_str), Some("oll"));
        assert_eq!(values.get("plan").map(String::as_str), Some("pro"));
        assert_eq!(default_format(&sample_snap()), DEFAULT_FORMAT);
    }

    #[test]
    fn monthly_only_account_leaves_session_and_weekly_placeholders_empty() {
        // Fails on main: unwrap_or(0) fabricates "0" for omitted windows, so a
        // monthly-only account paints a fake 0% 5h/7d pair. Empty means absent;
        // a present window at 0% used still emits "0" (see zero_used below).
        let now = Utc::now();
        let values = build_placeholders(&monthly_snap(42), now);
        for key in [
            "session_pct",
            "session_reset",
            "weekly_pct",
            "weekly_reset",
            "oll_session_pct",
            "oll_weekly_pct",
        ] {
            assert_eq!(
                values[key], "",
                "{key} must render empty, not a fabricated 0"
            );
        }
        assert_eq!(values["oll_monthly_pct"], "42");
        assert_eq!(values["oll_monthly_reset"], "—");
        assert_eq!(default_format(&monthly_snap(42)), MONTHLY_FORMAT);

        let zero_used = build_placeholders(&monthly_snap(0), now);
        assert_eq!(zero_used["oll_monthly_pct"], "0");
        assert_eq!(zero_used["session_pct"], "");
    }

    #[test]
    fn monthly_only_default_bar_shows_the_monthly_percent() {
        let snap = monthly_snap(42);
        let outcome = sample_outcome(snap.clone());
        let out = render(&outcome, &snap, &Theme::default(), &opts(), Utc::now());
        assert!(!out.text.contains('{'), "{}", out.text);
        assert!(out.text.contains("42%"), "{}", out.text);
        assert!(
            !out.text.contains("0%w") && !out.text.contains("0% ·"),
            "monthly-only must not paint the session/weekly pair: {}",
            out.text
        );
        assert!(out.tooltip.contains("Monthly"), "{}", out.tooltip);
        assert!(out.tooltip.contains("gpt-oss:120b"), "{}", out.tooltip);
    }

    #[test]
    fn no_windows_default_bar_shows_the_plan_not_a_zero_pair() {
        let mut snap = monthly_snap(42);
        snap.monthly = None;
        snap.monthly_models.clear();
        assert_eq!(default_format(&snap), "{plan}");
        let out = render(
            &sample_outcome(snap.clone()),
            &snap,
            &Theme::default(),
            &opts(),
            Utc::now(),
        );
        assert!(out.text.contains("pro"), "{}", out.text);
        assert!(
            !out.text.contains("0%"),
            "no-window payload must not fabricate zeros: {}",
            out.text
        );
    }

    #[test]
    fn credit_plan_renders_dollars_and_monthly_reset() {
        let body: crate::ollama::types::Body = serde_json::from_str(include_str!(
            "../../tests/fixtures/ollama/balance_credits.json"
        ))
        .unwrap();
        let snap = body.into_snapshot("pro".into());
        let now = "2026-10-09T08:00:00Z".parse().unwrap();
        let values = build_placeholders(&snap, now);
        assert_eq!(values["oll_balance"], "$45.00");
        assert_eq!(values["oll_allowance"], "$60.00");
        assert_eq!(values["oll_purchased"], "$12.50");
        assert_eq!(values["oll_monthly_pct"], "25");
        assert_eq!(values["session_pct"], "");
        assert_ne!(values["oll_monthly_reset"], "—");
        let out = render(
            &sample_outcome(snap.clone()),
            &snap,
            &Theme::default(),
            &opts(),
            now,
        );
        assert!(out.text.contains("$45.00 / $60.00"), "{}", out.text);
        assert!(out.tooltip.contains("$45.00 remaining of $60.00"));
        assert!(out.tooltip.contains("Purchased credits: $12.50"));
        assert!(out.tooltip.contains("Monthly"));
        assert!(!out.tooltip.contains("Session (5h)"));
    }

    #[test]
    fn severity_tracks_worst_window() {
        assert_eq!(severity(&sample_snap()), severity_for(82));
        assert_eq!(severity(&monthly_snap(42)), severity_for(42));
    }

    #[test]
    fn render_produces_nonempty_bar_and_tooltip() {
        let snap = sample_snap();
        let outcome = sample_outcome(snap.clone());
        let out = render(&outcome, &snap, &Theme::default(), &opts(), Utc::now());
        assert!(!out.text.is_empty());
        assert!(out.tooltip.contains("Ollama Cloud"));
        assert!(out.tooltip.contains("kimi-k3"));
        assert!(out.tooltip.contains("Session") || out.tooltip.contains("82"));
        assert!(out.text.contains("82%"), "{}", out.text);
        assert!(out.text.contains("23%w"), "{}", out.text);
    }

    #[test]
    fn plan_is_pango_escaped_in_custom_formats() {
        let mut snap = sample_snap();
        snap.plan = "Cloud Pro & Enterprise <preview>".into();
        let outcome = sample_outcome(snap.clone());
        let mut o = opts();
        o.format = Some("{plan}".into());
        o.tooltip_format = Some("{oll_plan}".into());

        let out = render(&outcome, &snap, &Theme::default(), &o, Utc::now());
        assert!(!out.text.contains(" & "));
        assert!(!out.tooltip.contains('<'));
        assert!(
            out.text
                .contains("Cloud Pro &amp; Enterprise &lt;preview&gt;")
        );
        assert_eq!(out.tooltip, "Cloud Pro &amp; Enterprise &lt;preview&gt;");
    }
}
