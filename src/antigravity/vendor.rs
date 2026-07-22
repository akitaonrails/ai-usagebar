//! Antigravity renderer — bar text + bordered Pango tooltip.
//!
//! Antigravity splits its quota into two independent pools (Gemini, and
//! Claude/GPT-OSS), each with a 5-hour and a weekly window. The two pools do
//! **not** share a budget: exhausting Gemini leaves Claude & GPT OSS untouched.
//! Their reset times only coincide while both are unused — an untouched bucket's
//! reset slides with the clock and anchors on first use — so every window keeps
//! its own countdown and the grouping below is presentational only.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::countdown;
use crate::format::{placeholders, substitute, updated_at_hm};
use crate::pacing::{self, PaceSeverity};
use crate::pango::{color_span, escape, severity_color};
use crate::theme::Theme;
use crate::tooltip::{Line as TooltipLine, push_window, render_bordered};
use crate::usage::{AntigravitySnapshot, UsageWindow};
use crate::vendor::{RenderOpts, VendorOutcome};
use crate::waybar::{Class, WaybarOutput};

pub const DEFAULT_FORMAT: &str = "{icon} {session_pct}% · {weekly_pct}%";

/// Model-group names, as Antigravity's own Model Quota screen labels them.
/// Both surfaces put these under a "Session"/"Weekly" heading, so the rows
/// carry no window suffix of their own.
pub const GROUP_PRIMARY: &str = "Gemini";
pub const GROUP_THIRD_PARTY: &str = "Claude & GPT OSS";

/// Every window this vendor reports, in dropdown order: the two 5-hour windows
/// under "Session", then the two weekly ones under "Weekly".
fn windows(snap: &AntigravitySnapshot) -> [(&'static str, Option<&UsageWindow>); 4] {
    [
        (GROUP_PRIMARY, Some(&snap.session)),
        (GROUP_THIRD_PARTY, snap.third_party_session.as_ref()),
        (GROUP_PRIMARY, Some(&snap.weekly)),
        (GROUP_THIRD_PARTY, snap.third_party_weekly.as_ref()),
    ]
}

fn elapsed_pct(w: &UsageWindow, now: DateTime<Utc>, tolerance: u32) -> i32 {
    pacing::calc(
        w.utilization_pct,
        w.resets_at,
        now,
        w.window_duration,
        tolerance,
    )
    .elapsed_pct
}

pub fn build_placeholders(
    snap: &AntigravitySnapshot,
    now: DateTime<Utc>,
    pace_tolerance: u32,
) -> HashMap<&'static str, String> {
    // Slot mapping is fixed by the shared widget contract: the Gemini pool owns
    // the generic session/weekly slots, the third-party pool owns the scoped
    // slot (5h) and the extra slot (weekly). Only the labels and visual order
    // differ, which keeps the panel bar and the show-session/show-weekly
    // toggles working off d.session / d.weekly as they do for every vendor.
    let optional = |w: Option<&UsageWindow>, label: &'static str| match w {
        Some(w) => (
            label.to_string(),
            w.utilization_pct.to_string(),
            countdown::format(w.resets_at, now),
            elapsed_pct(w, now, pace_tolerance).to_string(),
        ),
        None => (String::new(), String::new(), String::new(), String::new()),
    };

    let (tp_5h_model, tp_5h_pct, tp_5h_reset, tp_5h_elapsed) =
        optional(snap.third_party_session.as_ref(), GROUP_THIRD_PARTY);
    let (tp_wk_model, tp_wk_pct, tp_wk_reset, tp_wk_elapsed) =
        optional(snap.third_party_weekly.as_ref(), GROUP_THIRD_PARTY);

    placeholders(vec![
        ("icon", "󰧑".to_string()),
        ("vendor_short", "agy".to_string()),
        ("plan", snap.plan.clone()),
        // Naming the primary rows is what puts the native dropdown into its
        // grouped layout; no other vendor emits these.
        ("session_model", GROUP_PRIMARY.to_string()),
        ("weekly_model", GROUP_PRIMARY.to_string()),
        ("session_pct", snap.session.utilization_pct.to_string()),
        (
            "session_reset",
            countdown::format(snap.session.resets_at, now),
        ),
        (
            "session_elapsed",
            elapsed_pct(&snap.session, now, pace_tolerance).to_string(),
        ),
        ("weekly_pct", snap.weekly.utilization_pct.to_string()),
        (
            "weekly_reset",
            countdown::format(snap.weekly.resets_at, now),
        ),
        (
            "weekly_elapsed",
            elapsed_pct(&snap.weekly, now, pace_tolerance).to_string(),
        ),
        ("scoped_model", tp_5h_model),
        ("scoped_pct", tp_5h_pct),
        ("scoped_reset", tp_5h_reset),
        ("scoped_elapsed", tp_5h_elapsed),
        ("extra_model", tp_wk_model),
        ("extra_pct", tp_wk_pct),
        ("extra_reset", tp_wk_reset),
        ("extra_elapsed", tp_wk_elapsed),
    ])
}

/// Worst of **all four** windows. Grading on the Gemini pool alone would show a
/// calm panel while the Claude & GPT OSS pool is exhausted.
pub fn severity(snap: &AntigravitySnapshot) -> PaceSeverity {
    let worst = windows(snap)
        .iter()
        .filter_map(|(_, w)| w.map(|w| w.utilization_pct))
        .max()
        .unwrap_or(0);
    if worst >= 90 {
        PaceSeverity::Critical
    } else if worst >= 75 {
        PaceSeverity::High
    } else if worst >= 50 {
        PaceSeverity::Mid
    } else {
        PaceSeverity::Low
    }
}

pub fn render(
    outcome: &VendorOutcome,
    snap: &AntigravitySnapshot,
    theme: &Theme,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> WaybarOutput {
    let sev = severity(snap);
    let class = Class::from(sev);
    let format = opts
        .format
        .clone()
        .unwrap_or_else(|| DEFAULT_FORMAT.to_string());
    let values = build_placeholders(snap, now, opts.pace_tolerance);

    let mut text = substitute(&format, &values);
    if outcome.stale {
        text.push_str(" ⏸");
    }

    let wrapper_color = severity_color(sev, theme).to_string();
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
    snap: &AntigravitySnapshot,
    theme: &Theme,
    opts: &RenderOpts,
    now: DateTime<Utc>,
) -> String {
    let blue = &theme.blue;
    let fg = &theme.fg;
    let dim = &theme.dim;

    let mut lines: Vec<TooltipLine> = Vec::new();
    lines.push(TooltipLine::Center(format!(
        "<span font_weight='bold' foreground='{blue}'>{}</span>",
        escape(&snap.plan)
    )));
    lines.push(TooltipLine::Sep);
    lines.push(TooltipLine::Body("".into()));

    let all = windows(snap);
    let pace = |w: &UsageWindow| {
        opts.tooltip_pace_pts
            .then(|| elapsed_pct(w, now, opts.pace_tolerance))
    };

    // Two groups, each holding the same pair of pools. A Sep between them marks
    // the pools as independent budgets (same device Z.AI uses before its MCP
    // block) without implying they share a reset.
    for (group_idx, (heading, icon)) in [("Session", "󰔟"), ("Weekly", "󰃰")].iter().enumerate()
    {
        if group_idx > 0 {
            lines.push(TooltipLine::Body("".into()));
            lines.push(TooltipLine::Sep);
        }
        lines.push(TooltipLine::Body(format!(
            " <span font_weight='bold' foreground='{fg}'>  {icon}  {heading}</span>"
        )));
        for (slot, (label, window)) in all.iter().enumerate().skip(group_idx * 2).take(2) {
            let Some(w) = window else { continue };
            if slot % 2 == 1 {
                lines.push(TooltipLine::Body("".into()));
            }
            push_window(
                &mut lines,
                &format!("  󰆧  {}", escape(label)),
                w,
                theme,
                now,
                pace(w),
            );
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> Option<DateTime<Utc>> {
        Some(DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc))
    }

    fn now() -> DateTime<Utc> {
        at("2026-07-22T12:00:00Z").unwrap()
    }

    fn window(pct: i32, resets: &str, weekly: bool) -> UsageWindow {
        UsageWindow {
            utilization_pct: pct,
            resets_at: at(resets),
            window_duration: if weekly {
                chrono::Duration::days(7)
            } else {
                chrono::Duration::hours(5)
            },
        }
    }

    fn snapshot() -> AntigravitySnapshot {
        AntigravitySnapshot {
            plan: "Google AI Pro".into(),
            account: "acct:test".into(),
            session: window(43, "2026-07-22T14:00:00Z", false),
            weekly: window(8, "2026-07-28T17:39:58Z", true),
            third_party_session: Some(window(75, "2026-07-22T16:30:00Z", false)),
            third_party_weekly: Some(window(0, "2026-07-29T12:47:00Z", true)),
        }
    }

    fn outcome(last_error: Option<(u16, String)>) -> VendorOutcome {
        VendorOutcome {
            snapshot: crate::usage::VendorSnapshot::Antigravity(snapshot()),
            stale: false,
            last_error,
            cache_age: None,
        }
    }

    fn tooltip(opts: &RenderOpts, err: Option<(u16, String)>) -> String {
        render_tooltip(&outcome(err), &snapshot(), &Theme::default(), opts, now())
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
    fn each_window_gets_its_own_placeholder_values() {
        let v = build_placeholders(&snapshot(), now(), 5);
        assert_eq!(v["session_pct"], "43");
        assert_eq!(v["weekly_pct"], "8");
        assert_eq!(v["scoped_pct"], "75");
        assert_eq!(v["extra_pct"], "0");
        // The four countdowns are independent; the pools do not share a reset.
        assert_eq!(v["session_reset"], "2h 00m");
        assert_eq!(v["scoped_reset"], "4h 30m");
        assert_ne!(v["session_reset"], v["weekly_reset"]);
        assert_ne!(v["scoped_reset"], v["extra_reset"]);
    }

    #[test]
    fn every_row_is_labelled_by_its_model_group() {
        let v = build_placeholders(&snapshot(), now(), 5);
        assert_eq!(v["session_model"], GROUP_PRIMARY);
        assert_eq!(v["weekly_model"], GROUP_PRIMARY);
        assert_eq!(v["scoped_model"], GROUP_THIRD_PARTY);
        assert_eq!(v["extra_model"], GROUP_THIRD_PARTY);
        // The Session/Weekly heading supplies the window, so rows carry no
        // suffix of their own.
        assert!(v.values().all(|s| !s.contains("(weekly)")));
    }

    #[test]
    fn no_placeholder_smuggles_a_label_into_a_value() {
        let v = build_placeholders(&snapshot(), now(), 5);
        // Regression: an earlier build fused label and countdown with " -- "
        // and had the extension split it back apart.
        assert!(v.values().all(|s| !s.contains(" -- ")));
        // The money-budget placeholders belong to $-budget vendors only.
        assert!(!v.contains_key("extra_spent"));
        assert!(!v.contains_key("extra_limit"));
    }

    #[test]
    fn pace_markers_are_emitted_for_all_four_windows() {
        let v = build_placeholders(&snapshot(), now(), 5);
        for key in [
            "session_elapsed",
            "weekly_elapsed",
            "scoped_elapsed",
            "extra_elapsed",
        ] {
            assert!(
                v[key].parse::<i32>().is_ok_and(|e| (0..=100).contains(&e)),
                "{key} = {:?}",
                v[key]
            );
        }
        // 2h left of a 5h window → 60% elapsed.
        assert_eq!(v["session_elapsed"], "60");
    }

    #[test]
    fn absent_third_party_group_blanks_its_slots() {
        let mut snap = snapshot();
        snap.third_party_session = None;
        snap.third_party_weekly = None;
        let v = build_placeholders(&snap, now(), 5);
        for key in ["scoped_model", "extra_model", "extra_pct", "scoped_elapsed"] {
            assert_eq!(v[key], "", "{key}");
        }
    }

    #[test]
    fn severity_tracks_the_worst_of_all_four_windows() {
        let mut snap = snapshot();
        snap.session.utilization_pct = 0;
        snap.weekly.utilization_pct = 0;
        snap.third_party_session = Some(window(0, "2026-07-22T16:30:00Z", false));
        snap.third_party_weekly = Some(window(0, "2026-07-29T12:47:00Z", true));
        assert_eq!(severity(&snap), PaceSeverity::Low);

        // A third-party pool running dry must still raise the panel.
        snap.third_party_weekly = Some(window(95, "2026-07-29T12:47:00Z", true));
        assert_eq!(severity(&snap), PaceSeverity::Critical);

        snap.third_party_weekly = Some(window(60, "2026-07-29T12:47:00Z", true));
        assert_eq!(severity(&snap), PaceSeverity::Mid);
    }

    #[test]
    fn tooltip_groups_the_four_windows_under_session_and_weekly() {
        let out = tooltip(&opts(), None);
        assert!(out.contains("Session"), "{out}");
        assert!(out.contains("Weekly"), "{out}");
        // Gemini appears once per group; the third-party pool likewise.
        assert_eq!(out.matches(GROUP_PRIMARY).count(), 2, "{out}");
        assert!(out.contains("Claude &amp; GPT OSS"), "{out}");
        assert!(!out.contains("Claude & GPT OSS"), "unescaped: {out}");
        // Four reset countdowns — one per window.
        assert_eq!(out.matches("Resets in").count(), 4, "{out}");
    }

    #[test]
    fn tooltip_carries_the_shared_footer_and_error_block() {
        assert!(tooltip(&opts(), None).contains("Updated"));

        let with_err = tooltip(&opts(), Some((503, "upstream is down".into())));
        assert!(with_err.contains("HTTP 503"), "{with_err}");
        assert!(with_err.contains("upstream is down"), "{with_err}");
        // A zero code is an internal marker, not an HTTP failure.
        let internal = tooltip(&opts(), Some((0, "cache miss".into())));
        assert!(!internal.contains("HTTP"), "{internal}");
    }

    #[test]
    fn tooltip_box_stays_flush_with_escaped_labels() {
        let out = tooltip(&opts(), None);
        let widths: Vec<usize> = out.lines().map(crate::pango::visible_width).collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "ragged box: {widths:?}\n{out}"
        );
    }

    #[test]
    fn pace_points_are_opt_in() {
        let plain = tooltip(&opts(), None);
        let paced = tooltip(
            &RenderOpts {
                tooltip_pace_pts: true,
                ..opts()
            },
            None,
        );
        assert_ne!(plain, paced, "pace marker should change the bars");
    }
}
