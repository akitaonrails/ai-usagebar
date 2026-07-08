//! Pango-markup rendering for the Anthropic widget — both the bar text and
//! the bordered tooltip.
//!
//! Closely mirrors claudebar:625-860. Pure functions over an immutable
//! [`RenderInput`] so all of the visual logic is unit-testable without I/O.

use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::anthropic::fetch::FetchOutcome;
use crate::countdown;
use crate::format::{placeholders, substitute, updated_at_hm};
use crate::pacing;
use crate::pango::{self, color_span, escape, severity_for};
use crate::theme::Theme;
use crate::tooltip::{self, Line};
use crate::usage::anthropic_severity;
use crate::waybar::{Class, WaybarOutput};

/// Default format string when `--format` is omitted (claudebar:55).
pub const DEFAULT_FORMAT: &str = "{session_pct}% · {session_reset}";

/// All inputs needed to render the widget — packaged so tests can construct
/// it without any I/O.
pub struct RenderInput<'a> {
    pub outcome: &'a FetchOutcome,
    pub theme: &'a Theme,
    pub format: &'a str,
    pub tooltip_format: Option<&'a str>,
    pub icon: Option<&'a str>,
    pub pace_tolerance: u32,
    pub format_pace_color: bool,
    pub tooltip_pace_pts: bool,
    pub now: DateTime<Utc>,
}

/// Compose the full Waybar output for an Anthropic snapshot.
pub fn render_anthropic(input: &RenderInput) -> WaybarOutput {
    let snap = &input.outcome.snapshot;
    let class = Class::from(anthropic_severity(snap));
    let bar_text = render_bar_text(input, class);
    let tooltip = if let Some(fmt) = input.tooltip_format {
        // Custom tooltip uses the same placeholder set as the bar.
        let values = build_placeholders(input);
        substitute(fmt, &values)
    } else {
        render_default_tooltip(input)
    };

    WaybarOutput {
        text: bar_text,
        tooltip,
        class,
    }
}

/// Build the bar-text string with all placeholders substituted and the
/// surrounding `<span foreground='…'>` wrapper applied.
fn render_bar_text(input: &RenderInput, class: Class) -> String {
    let values = build_placeholders(input);
    let mut text = substitute(input.format, &values);

    // Append stale indicator (claudebar:687-690).
    if input.outcome.stale {
        text.push_str(" ⏸");
    }

    // Wrap in the global color or the neutral foreground (when individual
    // pace placeholders supply their own color via --format-pace-color).
    let wrapper_color = if input.format_pace_color && input.format.contains("_pace") {
        input.theme.fg.clone()
    } else {
        bar_color_for(class, input.theme).to_string()
    };
    let icon_prefix = match input.icon {
        Some(ic) if !ic.is_empty() => format!("{ic} "),
        _ => String::new(),
    };
    color_span(&wrapper_color, &format!("{icon_prefix}{text}"))
}

fn bar_color_for(class: Class, theme: &Theme) -> &str {
    match class {
        Class::Low => &theme.green,
        Class::Mid => &theme.yellow,
        Class::High => &theme.orange,
        Class::Critical => &theme.red,
    }
}

/// Build the full placeholder map for an Anthropic snapshot.
///
/// Mirrors claudebar's "{...}" surface (claudebar:625-667). Per-window pacing
/// is pre-computed once; bars are rendered both raw (for `{*_bar}`) and with
/// elapsed-position markers in the tooltip when `--tooltip-pace-pts` is set.
fn build_placeholders(input: &RenderInput) -> HashMap<&'static str, String> {
    let snap = &input.outcome.snapshot;
    let theme = input.theme;

    let session = pacing::calc(
        snap.session.utilization_pct,
        snap.session.resets_at,
        input.now,
        snap.session.window_duration,
        input.pace_tolerance,
    );
    let weekly = pacing::calc(
        snap.weekly.utilization_pct,
        snap.weekly.resets_at,
        input.now,
        snap.weekly.window_duration,
        input.pace_tolerance,
    );
    let sonnet_window = snap.sonnet.as_ref();
    let sonnet = sonnet_window.map(|w| {
        pacing::calc(
            w.utilization_pct,
            w.resets_at,
            input.now,
            w.window_duration,
            input.pace_tolerance,
        )
    });

    let session_color = pango::severity_color(severity_for(snap.session.utilization_pct), theme);
    let weekly_color = pango::severity_color(severity_for(snap.weekly.utilization_pct), theme);
    let sonnet_color =
        sonnet_window.map(|w| pango::severity_color(severity_for(w.utilization_pct), theme));
    let extra_color = snap
        .extra
        .as_ref()
        .map(|e| pango::severity_color(severity_for(e.percent()), theme));

    let session_bar = pango::progress_bar(snap.session.utilization_pct, session_color, theme, None);
    let weekly_bar = pango::progress_bar(snap.weekly.utilization_pct, weekly_color, theme, None);
    let sonnet_bar = if let (Some(w), Some(c)) = (sonnet_window, sonnet_color) {
        pango::progress_bar(w.utilization_pct, c, theme, None)
    } else {
        String::new()
    };
    let extra_bar = if let (Some(e), Some(c)) = (snap.extra.as_ref(), extra_color) {
        pango::progress_bar(e.percent(), c, theme, None)
    } else {
        String::new()
    };

    let mut v = placeholders(vec![
        ("icon", "󰚩".to_string()),
        ("vendor_short", "cld".to_string()),
        ("plan", snap.plan.clone()),
        ("session_pct", snap.session.utilization_pct.to_string()),
        (
            "session_reset",
            countdown::format(snap.session.resets_at, input.now),
        ),
        ("session_elapsed", session.elapsed_pct.to_string()),
        ("session_bar", session_bar.clone()),
        ("weekly_pct", snap.weekly.utilization_pct.to_string()),
        (
            "weekly_reset",
            countdown::format(snap.weekly.resets_at, input.now),
        ),
        ("weekly_elapsed", weekly.elapsed_pct.to_string()),
        ("weekly_bar", weekly_bar.clone()),
        (
            "sonnet_pct",
            sonnet_window
                .map(|w| w.utilization_pct.to_string())
                .unwrap_or_else(|| "0".into()),
        ),
        (
            "sonnet_reset",
            sonnet_window
                .map(|w| countdown::format(w.resets_at, input.now))
                .unwrap_or_else(|| "—".into()),
        ),
        (
            "sonnet_elapsed",
            sonnet
                .as_ref()
                .map(|s| s.elapsed_pct.to_string())
                .unwrap_or_else(|| "0".into()),
        ),
        ("sonnet_bar", sonnet_bar.clone()),
        (
            "extra_spent",
            snap.extra
                .map(|e| e.spent.fmt_dollars())
                .unwrap_or_default(),
        ),
        (
            "extra_limit",
            snap.extra
                .map(|e| e.limit.fmt_dollars())
                .unwrap_or_default(),
        ),
        (
            "extra_pct",
            snap.extra
                .map(|e| e.percent().to_string())
                .unwrap_or_else(|| "0".into()),
        ),
        ("extra_bar", extra_bar),
    ]);

    insert_pace(&mut v, "session", &session, input.format_pace_color, theme);
    insert_pace(&mut v, "weekly", &weekly, input.format_pace_color, theme);
    if let Some(sp) = sonnet.as_ref() {
        insert_pace(&mut v, "sonnet", sp, input.format_pace_color, theme);
    } else {
        // Empty placeholders so `{sonnet_pace}` etc. don't render the literal
        // brace text when sonnet is absent.
        insert_pace(
            &mut v,
            "sonnet",
            &pacing::Pacing::neutral(),
            input.format_pace_color,
            theme,
        );
    }
    v
}

fn insert_pace(
    map: &mut HashMap<&'static str, String>,
    prefix: &'static str,
    p: &pacing::Pacing,
    pace_color: bool,
    theme: &Theme,
) {
    let pace_glyph = p.ratio_pace.glyph();
    let indicator_glyph = p.point_pace.glyph();
    let delta = p.delta.to_string();
    let abs_delta = p.delta.unsigned_abs().to_string();
    let pct = &p.ratio_label;
    let pts = &p.point_label;

    let wrap = |s: &str| -> String {
        if pace_color {
            let sev = pacing::pace_severity(p.delta);
            let color = pango::severity_color(sev, theme);
            color_span(color, s)
        } else {
            s.to_string()
        }
    };

    let keys: [(&'static str, String); 6] = match prefix {
        "session" => [
            ("session_pace", wrap(pace_glyph)),
            ("session_pace_indicator", wrap(indicator_glyph)),
            ("session_pace_pct", wrap(pct)),
            ("session_pace_pts", wrap(pts)),
            ("session_pace_delta", wrap(&delta)),
            ("session_pace_abs_delta", wrap(&abs_delta)),
        ],
        "weekly" => [
            ("weekly_pace", wrap(pace_glyph)),
            ("weekly_pace_indicator", wrap(indicator_glyph)),
            ("weekly_pace_pct", wrap(pct)),
            ("weekly_pace_pts", wrap(pts)),
            ("weekly_pace_delta", wrap(&delta)),
            ("weekly_pace_abs_delta", wrap(&abs_delta)),
        ],
        "sonnet" => [
            ("sonnet_pace", wrap(pace_glyph)),
            ("sonnet_pace_indicator", wrap(indicator_glyph)),
            ("sonnet_pace_pct", wrap(pct)),
            ("sonnet_pace_pts", wrap(pts)),
            ("sonnet_pace_delta", wrap(&delta)),
            ("sonnet_pace_abs_delta", wrap(&abs_delta)),
        ],
        _ => return,
    };
    for (k, v) in keys {
        map.insert(k, v);
    }
}

/// The bordered Pango tooltip (claudebar:707-860).
fn render_default_tooltip(input: &RenderInput) -> String {
    let snap = &input.outcome.snapshot;
    let theme = input.theme;
    let blue = &theme.blue;
    let dim = &theme.dim;
    let fg = &theme.fg;

    let session_color = pango::severity_color(severity_for(snap.session.utilization_pct), theme);
    let weekly_color = pango::severity_color(severity_for(snap.weekly.utilization_pct), theme);

    let session_pacing = pacing::calc(
        snap.session.utilization_pct,
        snap.session.resets_at,
        input.now,
        snap.session.window_duration,
        input.pace_tolerance,
    );
    let weekly_pacing = pacing::calc(
        snap.weekly.utilization_pct,
        snap.weekly.resets_at,
        input.now,
        snap.weekly.window_duration,
        input.pace_tolerance,
    );

    let session_bar = if input.tooltip_pace_pts {
        pango::progress_bar(
            snap.session.utilization_pct,
            session_color,
            theme,
            Some(session_pacing.elapsed_pct),
        )
    } else {
        pango::progress_bar(snap.session.utilization_pct, session_color, theme, None)
    };
    let weekly_bar = if input.tooltip_pace_pts {
        pango::progress_bar(
            snap.weekly.utilization_pct,
            weekly_color,
            theme,
            Some(weekly_pacing.elapsed_pct),
        )
    } else {
        pango::progress_bar(snap.weekly.utilization_pct, weekly_color, theme, None)
    };

    let session_pace_glyph = pick_pace_glyph(input.tooltip_pace_pts, &session_pacing);
    let weekly_pace_glyph = pick_pace_glyph(input.tooltip_pace_pts, &weekly_pacing);

    let mut lines: Vec<Line> = Vec::new();
    let _ = pango::severity_color; // silence unused-import warning if any
    lines.push(Line::Center(format!(
        "<span font_weight='bold' foreground='{blue}'>Claude {plan}</span>",
        plan = escape(&snap.plan)
    )));
    lines.push(Line::Sep);
    lines.push(Line::Body("".into()));

    lines.push(Line::Body(format!(
        " <span foreground='{fg}'>  󰔟  Session</span>"
    )));
    lines.push(Line::Body(format!(
        "   {bar}  <span font_weight='bold' foreground='{color}'>{pct}% {glyph}</span>",
        bar = session_bar,
        color = session_color,
        pct = snap.session.utilization_pct,
        glyph = session_pace_glyph
    )));
    lines.push(Line::Body(format!(
        " <span foreground='{dim}'>  ⏱  Resets in {cd}</span>",
        cd = escape(&countdown::format(snap.session.resets_at, input.now))
    )));
    lines.push(Line::Body("".into()));

    lines.push(Line::Body(format!(
        " <span foreground='{fg}'>  󰃰  Weekly</span>"
    )));
    lines.push(Line::Body(format!(
        "   {bar}  <span font_weight='bold' foreground='{color}'>{pct}% {glyph}</span>",
        bar = weekly_bar,
        color = weekly_color,
        pct = snap.weekly.utilization_pct,
        glyph = weekly_pace_glyph
    )));
    lines.push(Line::Body(format!(
        " <span foreground='{dim}'>  ⏱  Resets in {cd}</span>",
        cd = escape(&countdown::format(snap.weekly.resets_at, input.now))
    )));

    if let Some(sw) = snap.sonnet.as_ref() {
        let sonnet_color = pango::severity_color(severity_for(sw.utilization_pct), theme);
        let sonnet_pacing = pacing::calc(
            sw.utilization_pct,
            sw.resets_at,
            input.now,
            sw.window_duration,
            input.pace_tolerance,
        );
        let sonnet_bar = if input.tooltip_pace_pts {
            pango::progress_bar(
                sw.utilization_pct,
                sonnet_color,
                theme,
                Some(sonnet_pacing.elapsed_pct),
            )
        } else {
            pango::progress_bar(sw.utilization_pct, sonnet_color, theme, None)
        };
        lines.push(Line::Body("".into()));
        lines.push(Line::Body(format!(
            " <span foreground='{fg}'>  󱤔  Sonnet only</span>"
        )));
        lines.push(Line::Body(format!(
            "   {bar}  <span font_weight='bold' foreground='{color}'>{pct}%</span>",
            bar = sonnet_bar,
            color = sonnet_color,
            pct = sw.utilization_pct
        )));
        lines.push(Line::Body(format!(
            " <span foreground='{dim}'>  ⏱  Resets in {cd}</span>",
            cd = escape(&countdown::format(sw.resets_at, input.now))
        )));
    }

    for sw in &snap.scoped {
        let scoped_color = pango::severity_color(severity_for(sw.window.utilization_pct), theme);
        let scoped_pacing = pacing::calc(
            sw.window.utilization_pct,
            sw.window.resets_at,
            input.now,
            sw.window.window_duration,
            input.pace_tolerance,
        );
        let scoped_bar = if input.tooltip_pace_pts {
            pango::progress_bar(
                sw.window.utilization_pct,
                scoped_color,
                theme,
                Some(scoped_pacing.elapsed_pct),
            )
        } else {
            pango::progress_bar(sw.window.utilization_pct, scoped_color, theme, None)
        };
        lines.push(Line::Body("".into()));
        lines.push(Line::Body(format!(
            " <span foreground='{fg}'>  󰆧  {label} weekly</span>",
            label = escape(&sw.label)
        )));
        lines.push(Line::Body(format!(
            "   {bar}  <span font_weight='bold' foreground='{color}'>{pct}%</span>",
            bar = scoped_bar,
            color = scoped_color,
            pct = sw.window.utilization_pct
        )));
        lines.push(Line::Body(format!(
            " <span foreground='{dim}'>  ⏱  Resets in {cd}</span>",
            cd = escape(&countdown::format(sw.window.resets_at, input.now))
        )));
    }

    if let Some(extra) = snap.extra {
        let extra_color = pango::severity_color(severity_for(extra.percent()), theme);
        let extra_bar = pango::progress_bar(extra.percent(), extra_color, theme, None);
        lines.push(Line::Body("".into()));
        lines.push(Line::Sep);
        lines.push(Line::Body(format!(
            " <span foreground='{fg}'>  󰄑  Extra usage</span>"
        )));
        lines.push(Line::Body(format!(
            "   {bar}  <span font_weight='bold' foreground='{color}'>{spent}</span>",
            bar = extra_bar,
            color = extra_color,
            spent = escape(&extra.spent.fmt_dollars())
        )));
        lines.push(Line::Body(format!(
            " <span foreground='{dim}'>  󰀓  Limit: {lim}</span>",
            lim = escape(&extra.limit.fmt_dollars())
        )));
    }

    if let Some((code, msg)) = input.outcome.last_error.as_ref()
        && *code != 0
    {
        let (icon, color) = if *code >= 500 {
            ("󰅚", theme.red.as_str())
        } else {
            ("󰀪", theme.orange.as_str())
        };
        lines.push(Line::Body("".into()));
        lines.push(Line::Sep);
        lines.push(Line::Body(format!(
            " <span foreground='{color}'>  {icon}  HTTP {code}</span>"
        )));
        for wrapped in wrap_words(&escape(msg), 35) {
            lines.push(Line::Body(format!(
                "     <span foreground='{dim}'>{wrapped}</span>"
            )));
        }
    }

    let updated = updated_at_hm(input.now, input.outcome.cache_age);
    lines.push(Line::Body("".into()));
    lines.push(Line::Sep);
    lines.push(Line::Body(format!(
        " <span foreground='{dim}'>  󰅐  Updated {updated}</span>"
    )));

    tooltip::render_bordered(&lines, theme)
}

fn pick_pace_glyph(point_mode: bool, p: &pacing::Pacing) -> &'static str {
    if point_mode {
        p.point_pace.glyph()
    } else {
        p.ratio_pace.glyph()
    }
}

/// Greedy word-wrap to a target column. Used for the API-error message
/// in the tooltip (claudebar:779-790).
fn wrap_words(s: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    for word in s.split_whitespace() {
        if buf.is_empty() {
            buf = word.into();
        } else if buf.len() + 1 + word.len() <= width {
            buf.push(' ');
            buf.push_str(word);
        } else {
            out.push(std::mem::take(&mut buf));
            buf = word.into();
        }
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anthropic::fetch::FetchOutcome;
    use crate::usage::{AnthropicSnapshot, Cents, ExtraUsage, UsageWindow};
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, 23, 12, 0, 0).unwrap()
    }

    fn sample_outcome() -> FetchOutcome {
        let session = UsageWindow {
            utilization_pct: 62,
            resets_at: Some(now() + chrono::Duration::minutes(90)),
            window_duration: chrono::Duration::hours(5),
        };
        let weekly = UsageWindow {
            utilization_pct: 27,
            resets_at: Some(now() + chrono::Duration::days(4) + chrono::Duration::hours(1)),
            window_duration: chrono::Duration::days(7),
        };
        let sonnet = UsageWindow {
            utilization_pct: 4,
            resets_at: Some(now() + chrono::Duration::hours(2) + chrono::Duration::minutes(24)),
            window_duration: chrono::Duration::days(7),
        };
        let snap = AnthropicSnapshot {
            plan: "Max 5x".into(),
            session,
            weekly,
            sonnet: Some(sonnet),
            scoped: vec![],
            extra: Some(ExtraUsage {
                limit: Cents(5000),
                spent: Cents(250),
            }),
        };
        FetchOutcome {
            snapshot: snap,
            stale: false,
            last_error: None,
            cache_age: Some(std::time::Duration::from_secs(30)),
        }
    }

    fn input<'a>(outcome: &'a FetchOutcome, theme: &'a Theme) -> RenderInput<'a> {
        RenderInput {
            outcome,
            theme,
            format: DEFAULT_FORMAT,
            tooltip_format: None,
            icon: None,
            pace_tolerance: 5,
            format_pace_color: false,
            tooltip_pace_pts: false,
            now: now(),
        }
    }

    #[test]
    fn default_format_renders_pct_and_reset() {
        let oc = sample_outcome();
        let theme = Theme::default();
        let out = render_anthropic(&input(&oc, &theme));
        // Bar text wraps in a span; content should include "62%" and the
        // session countdown "1h 30m".
        assert!(out.text.contains("62%"));
        assert!(out.text.contains("1h 30m"));
        assert_eq!(out.class, Class::Mid); // session=62 → mid
    }

    #[test]
    fn stale_appends_pause_indicator() {
        let mut oc = sample_outcome();
        oc.stale = true;
        let theme = Theme::default();
        let out = render_anthropic(&input(&oc, &theme));
        assert!(out.text.contains("⏸"));
    }

    #[test]
    fn icon_prepends() {
        let oc = sample_outcome();
        let theme = Theme::default();
        let mut inp = input(&oc, &theme);
        inp.icon = Some("󰚩");
        let out = render_anthropic(&inp);
        assert!(out.text.contains("󰚩 "));
    }

    #[test]
    fn custom_tooltip_format_uses_placeholders() {
        let oc = sample_outcome();
        let theme = Theme::default();
        let mut inp = input(&oc, &theme);
        inp.tooltip_format = Some("S:{session_pct} W:{weekly_pct}");
        let out = render_anthropic(&inp);
        assert_eq!(out.tooltip, "S:62 W:27");
    }

    #[test]
    fn default_tooltip_contains_all_sections() {
        let oc = sample_outcome();
        let theme = Theme::default();
        let out = render_anthropic(&input(&oc, &theme));
        assert!(out.tooltip.contains("Claude Max 5x"));
        assert!(out.tooltip.contains("Session"));
        assert!(out.tooltip.contains("Weekly"));
        assert!(out.tooltip.contains("Sonnet only"));
        assert!(out.tooltip.contains("Extra usage"));
        assert!(out.tooltip.contains("Updated"));
        assert!(out.tooltip.contains("62%"));
        assert!(out.tooltip.contains("27%"));
        assert!(out.tooltip.contains("$2.50"));
        assert!(out.tooltip.contains("$50.00"));
    }

    #[test]
    fn tooltip_omits_sonnet_and_extra_when_absent() {
        let mut oc = sample_outcome();
        oc.snapshot.sonnet = None;
        oc.snapshot.extra = None;
        let theme = Theme::default();
        let out = render_anthropic(&input(&oc, &theme));
        assert!(!out.tooltip.contains("Sonnet only"));
        assert!(!out.tooltip.contains("Extra usage"));
        // Still contains the basics.
        assert!(out.tooltip.contains("Session"));
        assert!(out.tooltip.contains("Weekly"));
    }

    #[test]
    fn tooltip_includes_http_error_when_last_error_present() {
        let mut oc = sample_outcome();
        oc.last_error = Some((429, "rate limited".into()));
        let theme = Theme::default();
        let out = render_anthropic(&input(&oc, &theme));
        assert!(out.tooltip.contains("HTTP 429"));
        assert!(out.tooltip.contains("rate limited"));
    }

    #[test]
    fn tooltip_omits_http_zero() {
        // claudebar treats code 0 (no HTTP response) as "don't render"
        // because it would be misleading.
        let mut oc = sample_outcome();
        oc.last_error = Some((0, "n/a".into()));
        let theme = Theme::default();
        let out = render_anthropic(&input(&oc, &theme));
        assert!(!out.tooltip.contains("HTTP 0"));
    }

    #[test]
    fn worst_window_promotes_class_to_critical() {
        let mut oc = sample_outcome();
        oc.snapshot.weekly.utilization_pct = 95;
        let theme = Theme::default();
        let out = render_anthropic(&input(&oc, &theme));
        assert_eq!(out.class, Class::Critical);
    }

    #[test]
    fn pace_color_mode_uses_neutral_wrapper() {
        let oc = sample_outcome();
        let theme = Theme::default();
        let mut inp = input(&oc, &theme);
        inp.format = "{session_pct}% {session_pace}";
        inp.format_pace_color = true;
        let out = render_anthropic(&inp);
        // Wrapper color should be the foreground (neutral), not severity.
        assert!(out.text.contains(&theme.fg));
    }

    #[test]
    fn wrap_words_breaks_on_width_boundary() {
        let lines = wrap_words("aaa bbb ccc ddd eee fff", 8);
        // "aaa bbb" (7) fits; "ccc ddd" (7) fits next; "eee fff" (7) next.
        assert_eq!(lines, vec!["aaa bbb", "ccc ddd", "eee fff"]);
    }
}
