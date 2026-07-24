//! OpenAI renderer — mirrors Anthropic's rolling-window layout while omitting
//! any 5h or weekly window the Codex endpoint does not report.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::countdown;
use crate::format::{placeholders, substitute, updated_at_hm};
use crate::pacing::{self, PaceSeverity};
use crate::pango::{color_span, escape, severity_color, severity_for};
use crate::theme::Theme;
use crate::tooltip::{Line as TooltipLine, push_window, render_bordered};
use crate::usage::{OpenAiSnapshot, OpenAiSource, UsageWindow};
use crate::vendor::{RenderOpts, VendorOutcome};
use crate::waybar::{Class, WaybarOutput};

use super::fetch::FetchOutcome;

pub const DEFAULT_FORMAT: &str = "{oai_session_pct}% · {oai_session_reset}";
const WEEKLY_ONLY_FORMAT: &str = "{oai_weekly_pct}% · {oai_weekly_reset}";
const NO_WINDOWS_FORMAT: &str = "{oai_plan}";

pub fn build_placeholders(
    snap: &OpenAiSnapshot,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> HashMap<&'static str, String> {
    let session = window_placeholders(snap.session.as_ref(), opts, now);
    let weekly = window_placeholders(snap.weekly.as_ref(), opts, now);
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
        ("vendor_short", "gpt".to_string()),
        // Cross-vendor aliases — same names work across all vendors so a
        // single `--format '{vendor_short} {session_pct}% · {session_reset}'`
        // renders correctly during scroll-cycle.
        ("session_pct", session.pct.clone()),
        ("session_reset", session.reset.clone()),
        ("weekly_pct", weekly.pct.clone()),
        ("weekly_reset", weekly.reset.clone()),
        ("plan", snap.plan.clone()),
        ("oai_plan", snap.plan.clone()),
        ("oai_session_pct", session.pct),
        ("oai_session_reset", session.reset),
        ("oai_session_elapsed", session.elapsed),
        ("oai_session_pace", session.ratio_pace),
        ("oai_session_pace_indicator", session.point_pace),
        ("oai_weekly_pct", weekly.pct),
        ("oai_weekly_reset", weekly.reset),
        ("oai_weekly_elapsed", weekly.elapsed),
        ("oai_weekly_pace", weekly.ratio_pace),
        ("oai_weekly_pace_indicator", weekly.point_pace),
        ("oai_code_review_pct", cr_pct.to_string()),
        ("oai_credit_balance", credit_balance),
        ("oai_local_msgs", local_msgs),
        ("oai_cloud_msgs", cloud_msgs),
    ])
}

struct WindowPlaceholderValues {
    pct: String,
    reset: String,
    elapsed: String,
    ratio_pace: String,
    point_pace: String,
}

fn window_placeholders(
    window: Option<&UsageWindow>,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> WindowPlaceholderValues {
    let Some(window) = window else {
        return WindowPlaceholderValues::empty();
    };
    let pace = window_pacing(window, opts, now);
    WindowPlaceholderValues {
        pct: window.utilization_pct.to_string(),
        reset: countdown::format(window.resets_at, now),
        elapsed: pace.elapsed_pct.to_string(),
        ratio_pace: pace.ratio_pace.glyph().to_string(),
        point_pace: pace.point_pace.glyph().to_string(),
    }
}

fn window_pacing(window: &UsageWindow, opts: &RenderOpts, now: DateTime<Utc>) -> pacing::Pacing {
    pacing::calc(
        window.utilization_pct,
        window.resets_at,
        now,
        window.window_duration,
        opts.pace_tolerance,
    )
}

impl WindowPlaceholderValues {
    fn empty() -> Self {
        Self {
            pct: String::new(),
            reset: String::new(),
            elapsed: String::new(),
            ratio_pace: String::new(),
            point_pace: String::new(),
        }
    }
}

pub fn severity(snap: &OpenAiSnapshot) -> PaceSeverity {
    let windows = [
        snap.session.as_ref(),
        snap.weekly.as_ref(),
        snap.code_review.as_ref(),
    ];
    let max = windows
        .into_iter()
        .flatten()
        .map(|window| window.utilization_pct)
        .max()
        .unwrap_or(0);
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
    let format = opts
        .format
        .clone()
        .unwrap_or_else(|| default_format(snap).to_string());
    let values = build_placeholders(snap, opts, now);

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

fn default_format(snap: &OpenAiSnapshot) -> &'static str {
    if snap.session.is_some() {
        return DEFAULT_FORMAT;
    }
    if snap.weekly.is_some() {
        return WEEKLY_ONLY_FORMAT;
    }
    NO_WINDOWS_FORMAT
}

fn render_tooltip(
    outcome: &VendorOutcome,
    snap: &OpenAiSnapshot,
    theme: &Theme,
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

    if let Some(session) = snap.session.as_ref() {
        push_window(&mut lines, "  󰔟  Codex 5h", session, theme, now, None);
    }
    if let Some(weekly) = snap.weekly.as_ref() {
        if snap.session.is_some() {
            lines.push(TooltipLine::Body("".into()));
        }
        push_window(&mut lines, "  󰃰  Codex weekly", weekly, theme, now, None);
    }
    if snap.session.is_none() && snap.weekly.is_none() {
        lines.push(TooltipLine::Body(format!(
            " <span foreground='{dim}'>no usage windows reported</span>"
        )));
    }

    if let Some(cr) = snap.code_review.as_ref() {
        lines.push(TooltipLine::Body("".into()));
        push_window(
            &mut lines,
            "  󱦰  Code review (weekly)",
            cr,
            theme,
            now,
            None,
        );
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
            session: Some(UsageWindow {
                utilization_pct: 1,
                resets_at: Some(Utc::now() + chrono::Duration::hours(5)),
                window_duration: chrono::Duration::hours(5),
            }),
            weekly: Some(UsageWindow {
                utilization_pct: 0,
                resets_at: Some(Utc::now() + chrono::Duration::days(7)),
                window_duration: chrono::Duration::days(7),
            }),
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
    fn dual_window_placeholders_keep_existing_values() {
        let s = sample();
        let values = build_placeholders(&s, &opts(), Utc::now());
        assert_eq!(values["oai_session_pct"], "1");
        assert_eq!(values["oai_weekly_pct"], "0");
        assert_eq!(values["session_pct"], "1");
        assert_eq!(values["weekly_pct"], "0");
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
    fn weekly_only_snapshot_uses_weekly_default_and_hides_session() {
        let mut s = sample();
        s.session = None;
        s.weekly.as_mut().unwrap().utilization_pct = 66;
        let out = render(&oc(s.clone()), &s, &Theme::default(), &opts(), Utc::now());
        assert!(out.text.contains("66%"));
        assert!(out.tooltip.contains("Codex weekly"));
        assert!(!out.tooltip.contains("Codex 5h"));

        let values = build_placeholders(&s, &opts(), Utc::now());
        assert_eq!(values["oai_session_pct"], "");
        assert_eq!(values["oai_weekly_pct"], "66");
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
        s.weekly.as_mut().unwrap().utilization_pct = 95;
        assert_eq!(severity(&s), PaceSeverity::Critical);
    }
}
