//! OpenAI renderer — mirrors anthropic's layout closely since both expose the
//! same primary+secondary window pattern.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::countdown;
use crate::format::{placeholders, substitute};
use crate::openrouter::vendor::{TooltipLine, render_bordered};
use crate::pacing::{self, PaceSeverity};
use crate::pango::{self, color_span, escape, severity_color, severity_for};
use crate::theme::Theme;
use crate::usage::{OpenAiSnapshot, OpenAiSource};
use crate::vendor::{RenderOpts, VendorOutcome};
use crate::waybar::{Class, WaybarOutput};

use super::fetch::FetchOutcome;

pub const DEFAULT_FORMAT: &str = "{oai_session_pct}% · {oai_session_reset}";

pub struct OpenAiVendor;

pub fn build_placeholders(
    snap: &OpenAiSnapshot,
    _theme: &Theme,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> HashMap<&'static str, String> {
    let session_p = pacing::calc(
        snap.session.utilization_pct,
        snap.session.resets_at,
        now,
        snap.session.window_duration,
        opts.pace_tolerance,
    );
    let weekly_p = pacing::calc(
        snap.weekly.utilization_pct,
        snap.weekly.resets_at,
        now,
        snap.weekly.window_duration,
        opts.pace_tolerance,
    );
    let cr_pct = snap
        .code_review
        .as_ref()
        .map(|w| w.utilization_pct)
        .unwrap_or(0);
    let credit_balance = snap
        .credits
        .as_ref()
        .map(|c| c.balance.clone())
        .unwrap_or_else(|| "n/a".into());
    let local_msgs = snap
        .credits
        .as_ref()
        .and_then(|c| c.approx_local_messages)
        .map(|(a, b)| format!("{a}-{b}"))
        .unwrap_or_default();
    let cloud_msgs = snap
        .credits
        .as_ref()
        .and_then(|c| c.approx_cloud_messages)
        .map(|(a, b)| format!("{a}-{b}"))
        .unwrap_or_default();

    placeholders(vec![
        ("icon", "󱢆".to_string()),
        ("oai_plan", snap.plan.clone()),
        ("oai_session_pct", snap.session.utilization_pct.to_string()),
        (
            "oai_session_reset",
            countdown::format(snap.session.resets_at, now),
        ),
        ("oai_session_elapsed", session_p.elapsed_pct.to_string()),
        ("oai_session_pace", session_p.ratio_pace.glyph().to_string()),
        (
            "oai_session_pace_indicator",
            session_p.point_pace.glyph().to_string(),
        ),
        ("oai_weekly_pct", snap.weekly.utilization_pct.to_string()),
        (
            "oai_weekly_reset",
            countdown::format(snap.weekly.resets_at, now),
        ),
        ("oai_weekly_elapsed", weekly_p.elapsed_pct.to_string()),
        ("oai_weekly_pace", weekly_p.ratio_pace.glyph().to_string()),
        (
            "oai_weekly_pace_indicator",
            weekly_p.point_pace.glyph().to_string(),
        ),
        ("oai_code_review_pct", cr_pct.to_string()),
        ("oai_credit_balance", credit_balance),
        ("oai_local_msgs", local_msgs),
        ("oai_cloud_msgs", cloud_msgs),
    ])
}

pub fn severity(snap: &OpenAiSnapshot) -> PaceSeverity {
    let mut max = snap.session.utilization_pct.max(snap.weekly.utilization_pct);
    if let Some(c) = &snap.code_review {
        max = max.max(c.utilization_pct);
    }
    severity_for(max)
}

pub fn render(
    outcome: &VendorOutcome,
    snap: &OpenAiSnapshot,
    theme: &Theme,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> WaybarOutput {
    let class = Class::from(severity(snap));
    let format = opts.format.clone().unwrap_or_else(|| DEFAULT_FORMAT.to_string());
    let values = build_placeholders(snap, theme, opts, now);

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
        render_tooltip(outcome, snap, theme, opts, now)
    };

    WaybarOutput {
        text: bar_text,
        tooltip,
        class,
    }
}

fn render_tooltip(
    outcome: &VendorOutcome,
    snap: &OpenAiSnapshot,
    theme: &Theme,
    _opts: &RenderOpts,
    now: DateTime<Utc>,
) -> String {
    let blue = &theme.blue;
    let dim = &theme.dim;
    let fg = &theme.fg;

    let mut lines: Vec<TooltipLine> = Vec::new();
    lines.push(TooltipLine::Center(format!(
        "<span font_weight='bold' foreground='{blue}'>{plan}</span>",
        plan = escape(&snap.plan)
    )));
    lines.push(TooltipLine::Sep);
    lines.push(TooltipLine::Body("".into()));

    push_window(&mut lines, "  󰔟  Codex 5h", &snap.session, theme, now);
    lines.push(TooltipLine::Body("".into()));
    push_window(&mut lines, "  󰃰  Codex weekly", &snap.weekly, theme, now);

    if let Some(cr) = snap.code_review.as_ref() {
        lines.push(TooltipLine::Body("".into()));
        push_window(&mut lines, "  󱦰  Code review (weekly)", cr, theme, now);
    }

    if let Some(c) = snap.credits.as_ref() {
        lines.push(TooltipLine::Body("".into()));
        lines.push(TooltipLine::Sep);
        let label = if c.unlimited {
            "unlimited".to_string()
        } else {
            c.balance.clone()
        };
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{fg}'>  󰄑  Credits</span>"
        )));
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{dim}'>     balance: {b}</span>",
            b = escape(&label)
        )));
        if let Some((lo, hi)) = c.approx_local_messages {
            lines.push(TooltipLine::Body(format!(
                " <span foreground='{dim}'>     ~ {lo}-{hi} local messages</span>"
            )));
        }
        if let Some((lo, hi)) = c.approx_cloud_messages {
            lines.push(TooltipLine::Body(format!(
                " <span foreground='{dim}'>     ~ {lo}-{hi} cloud messages</span>"
            )));
        }
    }

    if matches!(snap.source, OpenAiSource::Unavailable) {
        lines.push(TooltipLine::Body("".into()));
        lines.push(TooltipLine::Sep);
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{dim}'>OpenAI plan usage requires Codex OAuth.</span>"
        )));
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{dim}'>Run `codex login` to enable.</span>"
        )));
    }

    if let Some((code, msg)) = outcome.last_error.as_ref() {
        if *code != 0 {
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
    }

    let updated = match outcome.cache_age {
        Some(age) => {
            let when = now - chrono::Duration::from_std(age).unwrap_or_default();
            when.format("%H:%M").to_string()
        }
        None => "—".to_string(),
    };
    lines.push(TooltipLine::Body("".into()));
    lines.push(TooltipLine::Sep);
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{dim}'>  󰅐  Updated {updated}</span>"
    )));

    render_bordered(&lines, theme)
}

fn push_window(
    lines: &mut Vec<TooltipLine>,
    label: &str,
    w: &crate::usage::UsageWindow,
    theme: &Theme,
    now: DateTime<Utc>,
) {
    let color = severity_color(severity_for(w.utilization_pct), theme);
    let bar = pango::progress_bar(w.utilization_pct, color, theme, None);
    let fg = &theme.fg;
    let dim = &theme.dim;
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{fg}'>{label}</span>"
    )));
    lines.push(TooltipLine::Body(format!(
        "   {bar}  <span font_weight='bold' foreground='{color}'>{pct}%</span>",
        pct = w.utilization_pct
    )));
    lines.push(TooltipLine::Body(format!(
        " <span foreground='{dim}'>  ⏱  Resets in {cd}</span>",
        cd = escape(&countdown::format(w.resets_at, now))
    )));
}

impl From<FetchOutcome> for VendorOutcome {
    fn from(o: FetchOutcome) -> Self {
        Self {
            snapshot: crate::usage::VendorSnapshot::Openai(o.snapshot),
            stale: o.stale,
            last_error: o.last_error,
            cache_age: o.cache_age,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{OpenAiSnapshot, OpenAiSource, UsageWindow};

    fn sample() -> OpenAiSnapshot {
        OpenAiSnapshot {
            plan: "ChatGPT Plus".into(),
            session: UsageWindow {
                utilization_pct: 1,
                resets_at: Some(Utc::now() + chrono::Duration::hours(5)),
                window_duration: chrono::Duration::hours(5),
            },
            weekly: UsageWindow {
                utilization_pct: 0,
                resets_at: Some(Utc::now() + chrono::Duration::days(7)),
                window_duration: chrono::Duration::days(7),
            },
            code_review: None,
            credits: None,
            source: OpenAiSource::CodexOauth,
        }
    }

    fn oc(s: OpenAiSnapshot) -> VendorOutcome {
        VendorOutcome {
            snapshot: crate::usage::VendorSnapshot::Openai(s),
            stale: false,
            last_error: None,
            cache_age: Some(std::time::Duration::from_secs(15)),
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
    fn default_format_renders_session() {
        let s = sample();
        let out = render(&oc(s.clone()), &s, &Theme::default(), &opts(), Utc::now());
        assert!(out.text.contains("1%"));
    }

    #[test]
    fn tooltip_has_both_windows() {
        let s = sample();
        let out = render(&oc(s.clone()), &s, &Theme::default(), &opts(), Utc::now());
        assert!(out.tooltip.contains("Codex 5h"));
        assert!(out.tooltip.contains("Codex weekly"));
        assert!(!out.tooltip.contains("Code review"));
        assert!(!out.tooltip.contains("Credits"));
    }

    #[test]
    fn tooltip_includes_credits_block_when_present() {
        let mut s = sample();
        s.credits = Some(crate::usage::OpenAiCredits {
            balance: "$5.00".into(),
            has_credits: true,
            unlimited: false,
            approx_local_messages: Some((100, 200)),
            approx_cloud_messages: Some((30, 50)),
        });
        let out = render(&oc(s.clone()), &s, &Theme::default(), &opts(), Utc::now());
        assert!(out.tooltip.contains("Credits"));
        assert!(out.tooltip.contains("$5.00"));
        assert!(out.tooltip.contains("100-200 local messages"));
        assert!(out.tooltip.contains("30-50 cloud messages"));
    }

    #[test]
    fn unavailable_source_shows_codex_login_hint() {
        let mut s = sample();
        s.source = OpenAiSource::Unavailable;
        let out = render(&oc(s.clone()), &s, &Theme::default(), &opts(), Utc::now());
        assert!(out.tooltip.contains("codex login"));
    }

    #[test]
    fn severity_picks_worst_window() {
        let mut s = sample();
        s.weekly.utilization_pct = 95;
        assert_eq!(severity(&s), PaceSeverity::Critical);
    }
}
