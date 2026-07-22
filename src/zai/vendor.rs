//! Z.AI renderer — bar text + bordered Pango tooltip.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::countdown;
use crate::format::{placeholders, substitute, updated_at_hm};
use crate::pacing::PaceSeverity;
use crate::pango::{color_span, escape, severity_color, severity_for};
use crate::theme::Theme;
use crate::tooltip::{Line as TooltipLine, push_window, render_bordered};
use crate::usage::{UsageWindow, ZaiSnapshot};
use crate::vendor::{RenderOpts, VendorOutcome};
use crate::waybar::{Class, WaybarOutput};

use super::fetch::FetchOutcome;

pub const DEFAULT_FORMAT: &str = "{zai_session_pct}% · {zai_session_reset}";

pub fn build_placeholders(snap: &ZaiSnapshot, now: DateTime<Utc>) -> HashMap<&'static str, String> {
    let session_pct = snap
        .session
        .as_ref()
        .map(|w| w.utilization_pct)
        .unwrap_or(0);
    let weekly_pct = snap.weekly.as_ref().map(|w| w.utilization_pct).unwrap_or(0);
    let mcp_pct = snap.mcp.as_ref().map(|w| w.utilization_pct).unwrap_or(0);
    placeholders(vec![
        ("icon", "󰚩".to_string()),
        ("vendor_short", "zai".to_string()),
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
        ("plan", snap.plan.clone()),
        ("zai_plan", snap.plan.clone()),
        ("zai_session_pct", session_pct.to_string()),
        (
            "zai_session_reset",
            countdown::format(window_reset(&snap.session), now),
        ),
        ("zai_weekly_pct", weekly_pct.to_string()),
        (
            "zai_weekly_reset",
            countdown::format(window_reset(&snap.weekly), now),
        ),
        ("zai_mcp_pct", mcp_pct.to_string()),
        (
            "zai_mcp_reset",
            countdown::format(window_reset(&snap.mcp), now),
        ),
    ])
}

fn window_reset(w: &Option<UsageWindow>) -> Option<DateTime<Utc>> {
    w.as_ref().and_then(|w| w.resets_at)
}

pub fn severity(snap: &ZaiSnapshot) -> PaceSeverity {
    let session = snap
        .session
        .as_ref()
        .map(|w| w.utilization_pct)
        .unwrap_or(0);
    let weekly = snap.weekly.as_ref().map(|w| w.utilization_pct).unwrap_or(0);
    let mcp = snap.mcp.as_ref().map(|w| w.utilization_pct).unwrap_or(0);
    severity_for([session, weekly, mcp].into_iter().max().unwrap_or(0))
}

pub fn render(
    outcome: &VendorOutcome,
    snap: &ZaiSnapshot,
    theme: &Theme,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> WaybarOutput {
    let class = Class::from(severity(snap));
    let format = opts
        .format
        .clone()
        .unwrap_or_else(|| DEFAULT_FORMAT.to_string());
    let values = build_placeholders(snap, now);

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
    snap: &ZaiSnapshot,
    theme: &Theme,
    now: DateTime<Utc>,
) -> String {
    let blue = &theme.blue;
    let dim = &theme.dim;
    let mut lines: Vec<TooltipLine> = Vec::new();
    lines.push(TooltipLine::Center(format!(
        "<span font_weight='bold' foreground='{blue}'>{plan}</span>",
        plan = escape(&snap.plan)
    )));
    lines.push(TooltipLine::Sep);
    lines.push(TooltipLine::Body("".into()));

    if let Some(w) = snap.session.as_ref() {
        push_window(&mut lines, "  󰔟  Session (5h)", w, theme, now, None);
    }
    if let Some(w) = snap.weekly.as_ref() {
        if snap.session.is_some() {
            lines.push(TooltipLine::Body("".into()));
        }
        push_window(&mut lines, "  󰃰  Weekly", w, theme, now, None);
    }
    if let Some(w) = snap.mcp.as_ref() {
        lines.push(TooltipLine::Body("".into()));
        lines.push(TooltipLine::Sep);
        push_window(&mut lines, "  󰓹  MCP tools (monthly)", w, theme, now, None);
    }
    if snap.session.is_none() && snap.weekly.is_none() && snap.mcp.is_none() {
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{dim}'>no usage windows reported</span>"
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
        Self {
            snapshot: crate::usage::VendorSnapshot::Zai(o.snapshot),
            stale: o.stale,
            last_error: o.last_error,
            cache_age: o.cache_age,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{UsageWindow, ZaiSnapshot};

    fn sample_snap() -> ZaiSnapshot {
        let now = Utc::now();
        ZaiSnapshot {
            plan: "GLM Coding Pro".into(),
            session: Some(UsageWindow {
                utilization_pct: 42,
                resets_at: Some(now + chrono::Duration::hours(2)),
                window_duration: chrono::Duration::hours(5),
            }),
            weekly: Some(UsageWindow {
                utilization_pct: 15,
                resets_at: Some(now + chrono::Duration::days(3)),
                window_duration: chrono::Duration::days(7),
            }),
            mcp: None,
        }
    }

    fn outcome(s: ZaiSnapshot) -> VendorOutcome {
        VendorOutcome {
            snapshot: crate::usage::VendorSnapshot::Zai(s),
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
            pace_tolerance: 5,
            format_pace_color: false,
            tooltip_pace_pts: false,
        }
    }

    #[test]
    fn default_format_renders_session_pct() {
        let snap = sample_snap();
        let oc = outcome(snap.clone());
        let out = render(&oc, &snap, &Theme::default(), &opts(), Utc::now());
        assert!(out.text.contains("42%"));
    }

    #[test]
    fn tooltip_contains_all_windows_present() {
        let snap = sample_snap();
        let oc = outcome(snap.clone());
        let out = render(&oc, &snap, &Theme::default(), &opts(), Utc::now());
        assert!(out.tooltip.contains("Session"));
        assert!(out.tooltip.contains("Weekly"));
        assert!(!out.tooltip.contains("MCP"));
    }

    #[test]
    fn empty_snapshot_renders_no_windows_message() {
        let snap = ZaiSnapshot {
            plan: "GLM Coding Unknown".into(),
            session: None,
            weekly: None,
            mcp: None,
        };
        let oc = outcome(snap.clone());
        let out = render(&oc, &snap, &Theme::default(), &opts(), Utc::now());
        assert!(out.tooltip.contains("no usage windows reported"));
    }

    #[test]
    fn severity_picks_worst_window() {
        let mut snap = sample_snap();
        snap.weekly.as_mut().unwrap().utilization_pct = 95;
        assert_eq!(severity(&snap), PaceSeverity::Critical);
    }

    #[test]
    fn custom_tooltip_uses_placeholders() {
        let snap = sample_snap();
        let oc = outcome(snap.clone());
        let mut o = opts();
        o.tooltip_format = Some("S:{zai_session_pct} W:{zai_weekly_pct}".into());
        let out = render(&oc, &snap, &Theme::default(), &o, Utc::now());
        assert_eq!(out.tooltip, "S:42 W:15");
    }
}
