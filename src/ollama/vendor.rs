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
    let session_pct = snap
        .session
        .as_ref()
        .map(|w| w.utilization_pct)
        .unwrap_or(0);
    let weekly_pct = snap.weekly.as_ref().map(|w| w.utilization_pct).unwrap_or(0);
    let session = window_pacing(snap.session.as_ref(), pace_tolerance, now);
    let weekly = window_pacing(snap.weekly.as_ref(), pace_tolerance, now);
    let cost = snap.activity_cost.clone().unwrap_or_else(|| "—".into());

    placeholders(vec![
        ("icon", "🦙".to_string()),
        ("vendor_short", VendorId::Ollama.short_name().to_string()),
        // Cross-vendor aliases for scroll-cycle friendly formats.
        ("session_pct", session_pct.to_string()),
        (
            "session_reset",
            countdown::format(window_reset(&snap.session), now),
        ),
        ("weekly_pct", weekly_pct.to_string()),
        (
            "weekly_reset",
            countdown::format(window_reset(&snap.weekly), now),
        ),
        ("session_elapsed", session.elapsed.clone()),
        ("weekly_elapsed", weekly.elapsed.clone()),
        ("plan", snap.plan.clone()),
        ("oll_plan", snap.plan.clone()),
        ("oll_session_pct", session_pct.to_string()),
        (
            "oll_session_reset",
            countdown::format(window_reset(&snap.session), now),
        ),
        ("oll_weekly_pct", weekly_pct.to_string()),
        (
            "oll_weekly_reset",
            countdown::format(window_reset(&snap.weekly), now),
        ),
        ("oll_session_elapsed", session.elapsed),
        ("oll_session_pace", session.ratio_pace),
        ("oll_session_pace_indicator", session.point_pace),
        ("oll_weekly_elapsed", weekly.elapsed),
        ("oll_weekly_pace", weekly.ratio_pace),
        ("oll_weekly_pace_indicator", weekly.point_pace),
        ("oll_cost", cost),
    ])
}

fn window_reset(w: &Option<UsageWindow>) -> Option<DateTime<Utc>> {
    w.as_ref().and_then(|w| w.resets_at)
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
    // Worst of the two windows — session fills faster and is the one that
    // actually interrupts a chat mid-stream.
    let session = snap
        .session
        .as_ref()
        .map(|w| w.utilization_pct)
        .unwrap_or(0);
    let weekly = snap.weekly.as_ref().map(|w| w.utilization_pct).unwrap_or(0);
    severity_for(session.max(weekly))
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
        .unwrap_or_else(|| DEFAULT_FORMAT.to_string());
    let values = build_placeholders_with_tolerance(snap, opts.pace_tolerance, now);

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
    // No reset timestamp on the wire, so no elapsed marker / pace glyph.
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
            activity_cost: Some("0.00000".into()),
            activity_period: Some("last_4_weeks".into()),
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

    #[test]
    fn placeholders_expose_session_and_weekly() {
        let now = Utc::now();
        let values = build_placeholders(&sample_snap(), now);
        assert_eq!(
            values.get("oll_session_pct").map(String::as_str),
            Some("82")
        );
        assert_eq!(values.get("oll_weekly_pct").map(String::as_str), Some("23"));
        assert_eq!(values.get("oll_cost").map(String::as_str), Some("0.00000"));
        assert_eq!(values.get("vendor_short").map(String::as_str), Some("oll"));
        assert_eq!(values.get("plan").map(String::as_str), Some("pro"));
    }

    #[test]
    fn severity_tracks_worst_window() {
        assert_eq!(severity(&sample_snap()), severity_for(82));
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
    }
}
