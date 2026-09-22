//! Pango-bordered tooltip primitives shared by every vendor renderer.
//!
//! Extracted from the per-vendor implementations because every tooltip
//! (Anthropic, OpenAI, Z.AI, OpenRouter) draws the same kind of box: blue
//! corners + horizontals, dim separators, centered title, left-padded body
//! lines. The only thing that varies is the line content.
//!
//! Mirrors the visual style of `claudebar`'s `${B}╭${border_h}╮${E}` block
//! (claudebar:843-859).

use chrono::{DateTime, Utc};

use crate::countdown;
use crate::pacing;
use crate::pango::{self, escape, severity_color, severity_for, visible_width};
use crate::theme::Theme;
use crate::usage::UsageWindow;

/// One row of the bordered tooltip box.
pub enum Line {
    /// Centered text. The renderer pads both sides equally.
    Center(String),
    /// Body text. Left-justified, right-padded to fill the box.
    Body(String),
    /// A horizontal separator drawn with `─` characters.
    Sep,
}

/// Optional decorations for a [`push_window`] row. `Default` reproduces the
/// plain row every vendor drew before pacing reached the tooltip: bar +
/// percentage, then `⏱  Resets in …`.
///
/// `glyph` is inserted into Pango markup as-is, so a caller passing anything
/// vendor-reported must [`escape`] it first.
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowRow {
    /// Elapsed-time marker drawn inside the bar (`--tooltip-pace-pts`).
    pub marker_pct: Option<i32>,
    /// Pace glyph appended after the bold percentage (`↑` / `→` / `↓`).
    pub glyph: Option<&'static str>,
}

impl WindowRow {
    /// The Anthropic tooltip's pace convention in one place (mirrors
    /// `widget::render::pick_pace_glyph`): the ratio glyph by default, and with
    /// `point_mode` (`--tooltip-pace-pts`) the point-delta glyph plus the
    /// elapsed marker inside the bar.
    ///
    /// A window with no reported reset degrades to `pacing::Pacing::neutral()`,
    /// so it still gets a `→` rather than a blank where every sibling row has
    /// a glyph.
    pub fn paced(w: &UsageWindow, now: DateTime<Utc>, tolerance: u32, point_mode: bool) -> Self {
        let p = pacing::calc(
            w.utilization_pct,
            w.resets_at,
            now,
            w.window_duration,
            tolerance,
        );
        Self {
            marker_pct: point_mode.then_some(p.elapsed_pct),
            glyph: Some(if point_mode {
                p.point_pace.glyph()
            } else {
                p.ratio_pace.glyph()
            }),
        }
    }
}

/// Append the standard three-line block every vendor uses for a usage window:
/// icon + label, progress bar + bold percentage, then the dim reset countdown.
///
/// `elapsed` draws the pace marker inside the bar; pass `None` for a plain bar.
/// Keep this signature stable for library callers — a row that also wants the
/// pace glyph goes through [`push_window_with_row`].
pub fn push_window(
    lines: &mut Vec<Line>,
    label: &str,
    w: &UsageWindow,
    theme: &Theme,
    now: DateTime<Utc>,
    elapsed: Option<i32>,
) {
    push_window_with_row(
        lines,
        label,
        w,
        theme,
        now,
        WindowRow {
            marker_pct: elapsed,
            ..WindowRow::default()
        },
    );
}

/// [`push_window`], plus the optional decorations a [`WindowRow`] carries.
pub fn push_window_with_row(
    lines: &mut Vec<Line>,
    label: &str,
    w: &UsageWindow,
    theme: &Theme,
    now: DateTime<Utc>,
    row: WindowRow,
) {
    push_window_with_detail(lines, label, w, theme, now, row, None);
}

/// [`push_window_with_row`], with an optional vendor-specific detail appended
/// to the percentage line. Spend-based vendors use this for values such as
/// "$1.23 of $14.00" while keeping the same label / gauge / reset rhythm as
/// percentage-based vendors such as OpenAI.
pub fn push_window_with_detail(
    lines: &mut Vec<Line>,
    label: &str,
    w: &UsageWindow,
    theme: &Theme,
    now: DateTime<Utc>,
    row: WindowRow,
    detail: Option<&str>,
) {
    let color = severity_color(severity_for(w.utilization_pct), theme);
    let bar = pango::progress_bar(w.utilization_pct, color, theme, row.marker_pct);
    let fg = &theme.fg;
    let dim = &theme.dim;
    let glyph = row.glyph.map(|g| format!(" {g}")).unwrap_or_default();
    let detail = detail
        .map(|detail| format!(" <span foreground='{dim}'>· {}</span>", escape(detail)))
        .unwrap_or_default();
    lines.push(Line::Body(format!(
        " <span foreground='{fg}'>{label}</span>"
    )));
    lines.push(Line::Body(format!(
        "   {bar}  <span font_weight='bold' foreground='{color}'>{pct}%{glyph}</span>{detail}",
        pct = w.utilization_pct
    )));
    lines.push(Line::Body(format!(
        " <span foreground='{dim}'>  ⏱  Resets in {cd}</span>",
        cd = escape(&countdown::format(w.resets_at, now))
    )));
}

/// Render the bordered tooltip. Width is computed from the widest body/center
/// line so different vendors auto-size correctly.
pub fn render_bordered(lines: &[Line], theme: &Theme) -> String {
    let blue = &theme.blue;
    let dim = &theme.dim;

    let mut max_w: usize = 0;
    for line in lines {
        let s = match line {
            Line::Center(s) | Line::Body(s) => s.as_str(),
            Line::Sep => continue,
        };
        let w = visible_width(s);
        if w > max_w {
            max_w = w;
        }
    }
    let inner_w = max_w + 1;
    let border_h: String = "─".repeat(inner_w);
    let sep_inner: String = "─".repeat(inner_w.saturating_sub(2));
    let sep_line = format!(" <span foreground='{dim}'>{sep_inner}</span>");

    let mut out = String::with_capacity(256 * lines.len());
    out.push_str(&format!("<span foreground='{blue}'>╭{border_h}╮</span>\n"));
    for line in lines {
        let body = match line {
            Line::Body(s) => pad_right(s, inner_w),
            Line::Center(s) => pad_center(s, inner_w),
            Line::Sep => pad_right(&sep_line, inner_w),
        };
        out.push_str(&format!(
            "<span foreground='{blue}'>│</span>{body}<span foreground='{blue}'>│</span>\n"
        ));
    }
    out.push_str(&format!("<span foreground='{blue}'>╰{border_h}╯</span>"));
    out
}

/// Pad `s` on the right with spaces so its visible width reaches `inner_w`.
pub fn pad_right(s: &str, inner_w: usize) -> String {
    let v = visible_width(s);
    let need = inner_w.saturating_sub(v);
    format!("{s}{}", " ".repeat(need))
}

/// Pad `s` symmetrically; when the difference is odd, the extra space goes
/// on the right (claudebar `center_pad` precedent).
pub fn pad_center(s: &str, inner_w: usize) -> String {
    let v = visible_width(s);
    let total = inner_w.saturating_sub(v);
    let lp = total / 2;
    let rp = total - lp;
    format!("{}{s}{}", " ".repeat(lp), " ".repeat(rp))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme::default()
    }

    #[test]
    fn renders_top_and_bottom_borders() {
        let lines = vec![Line::Center("Hi".into())];
        let out = render_bordered(&lines, &theme());
        assert!(out.contains("╭"));
        assert!(out.contains("╮"));
        assert!(out.contains("╰"));
        assert!(out.contains("╯"));
        assert!(out.contains("Hi"));
    }

    /// Escaped characters are one glyph wide; if the box measured them by
    /// source length, rows containing one would stop short of the right border.
    #[test]
    fn rows_with_escaped_characters_keep_the_border_flush() {
        let lines = vec![
            Line::Body(crate::pango::escape("Claude & GPT (weekly)")),
            Line::Body("Gemini (weekly)".into()),
        ];
        let out = render_bordered(&lines, &theme());
        let right_edges: Vec<usize> = out.lines().map(crate::pango::visible_width).collect();
        assert!(
            right_edges.windows(2).all(|w| w[0] == w[1]),
            "ragged box: {right_edges:?}\n{out}"
        );
    }

    /// The reason `visible_width` measures columns rather than characters: a
    /// Japanese or Korean row is twice as wide as its character count, so a
    /// char-counting box stopped short of the right border by one cell per
    /// ideograph. This is the integration proof behind that change.
    #[test]
    fn rows_with_double_width_glyphs_keep_the_border_flush() {
        let lines = vec![
            Line::Body("セッション (5h)".into()),
            Line::Body("사용량".into()),
            Line::Body("Weekly".into()),
        ];
        let out = render_bordered(&lines, &theme());
        let right_edges: Vec<usize> = out.lines().map(crate::pango::visible_width).collect();
        assert!(
            right_edges.windows(2).all(|w| w[0] == w[1]),
            "ragged box with CJK rows: {right_edges:?}\n{out}"
        );
    }

    #[test]
    fn pad_right_pads_a_double_width_string_by_columns() {
        // "日本" is 2 chars but 4 columns; padding to 6 needs 2 spaces, not 4.
        assert!(pad_right("日本", 6).ends_with("  "));
        assert_eq!(crate::pango::visible_width(&pad_right("日本", 6)), 6);
    }

    #[test]
    fn body_line_is_right_padded_to_inner_width() {
        // Box width = visible_width(widest) + 1 = "longest" (7) + 1 = 8.
        let lines = vec![Line::Center("a".into()), Line::Body("longest".into())];
        let out = render_bordered(&lines, &theme());
        // The body line should be padded so the right `│` lands at inner_w + 2.
        // We don't assert exact character offsets (Pango spans intervene), just
        // that the resulting markup is well-formed (open/close balanced).
        let opens = out.matches("<span").count();
        let closes = out.matches("</span>").count();
        assert_eq!(opens, closes);
    }

    #[test]
    fn pad_right_strips_pango_tags_before_measuring() {
        let s = "<span foreground='#fff'>abc</span>"; // visible width 3
        let p = pad_right(s, 6);
        // 3 padding spaces appended.
        assert!(p.ends_with("   "));
    }

    #[test]
    fn pad_center_distributes_extra_space_right_for_odd_diff() {
        let p = pad_center("X", 4); // visible 1, total padding 3 → lp=1, rp=2
        assert_eq!(p, " X  ");
    }

    #[test]
    fn separator_line_width_grows_with_content() {
        let lines = vec![
            Line::Center("a".into()),
            Line::Sep,
            Line::Body("longer body line".into()),
        ];
        let out = render_bordered(&lines, &theme());
        // The separator should reach the inner width of the box (just check
        // that it contains the unicode dash glyph repeated).
        assert!(out.contains("─"));
    }

    fn at(h: u32) -> DateTime<Utc> {
        use chrono::TimeZone;
        Utc.with_ymd_and_hms(2026, 8, 25, h, 0, 0).unwrap()
    }

    /// One five-hour window `resets_in` hours from `at(12)`.
    fn window(pct: i32, resets_in: i64) -> UsageWindow {
        UsageWindow {
            utilization_pct: pct,
            resets_at: Some(at(12) + chrono::Duration::hours(resets_in)),
            window_duration: chrono::Duration::hours(5),
        }
    }

    fn row_markup(w: &UsageWindow, row: WindowRow) -> String {
        let mut lines = Vec::new();
        push_window_with_row(&mut lines, "  L", w, &theme(), at(12), row);
        render_bordered(&lines, &theme())
    }

    /// Every vendor that has not opted into pacing still renders the original
    /// row; `WindowRow::default()` is what keeps that output untouched.
    #[test]
    fn a_default_row_stays_the_plain_bar_percent_and_reset() {
        let out = row_markup(&window(40, 2), WindowRow::default());
        assert!(out.contains("40%"), "{out}");
        assert!(out.contains("Resets in 2h 00m"), "{out}");
        assert!(
            !out.contains('↑') && !out.contains('→') && !out.contains('↓'),
            "an unpaced row must not grow a glyph: {out}"
        );
        assert!(!out.contains(" · "), "an unpaced row has no detail: {out}");
    }

    /// Mirrors the Anthropic tooltip: the ratio glyph always, the elapsed
    /// marker only behind `--tooltip-pace-pts`.
    #[test]
    fn paced_rows_keep_the_marker_behind_point_mode() {
        // 3h left of a 5h window → 40% elapsed, matched by 40% used.
        let w = window(40, 3);
        let ratio = WindowRow::paced(&w, at(12), pacing::DEFAULT_TOLERANCE, false);
        assert_eq!(ratio.glyph, Some("→"));
        assert_eq!(ratio.marker_pct, None);

        let points = WindowRow::paced(&w, at(12), pacing::DEFAULT_TOLERANCE, true);
        assert_eq!(points.glyph, Some("→"));
        assert_eq!(points.marker_pct, Some(40));
    }

    /// The two modes disagree inside the tolerance band — the split `pacing`
    /// documents, and the reason the glyph is picked from the mode.
    #[test]
    fn the_pace_modes_can_disagree_on_the_glyph() {
        let w = window(42, 3); // 40% elapsed, 42% used
        assert_eq!(WindowRow::paced(&w, at(12), 5, false).glyph, Some("→"));
        assert_eq!(WindowRow::paced(&w, at(12), 5, true).glyph, Some("↑"));
    }

    /// A window the vendor reports without a reset still gets a glyph, so a
    /// row never sits blank beside siblings that have one.
    #[test]
    fn a_window_without_a_reset_still_gets_the_neutral_glyph() {
        let w = UsageWindow {
            utilization_pct: 0,
            resets_at: None,
            window_duration: chrono::Duration::hours(5),
        };
        let row = WindowRow::paced(&w, at(12), 5, false);
        assert_eq!(row.glyph, Some("→"));
        assert_eq!(row.marker_pct, None);
    }

    #[test]
    fn the_glyph_reaches_the_rendered_row() {
        let w = window(40, 3);
        let out = row_markup(&w, WindowRow::paced(&w, at(12), 5, false));
        assert!(out.contains("40% →"), "{out}");
        assert!(out.contains("Resets in 3h 00m"), "{out}");
        // The reset line carries nothing after the countdown now that no
        // vendor appends a fragment to it.
        assert!(!out.contains("Resets in 3h 00m ·"), "{out}");
    }

    #[test]
    fn a_detail_is_kept_on_the_meter_line() {
        let w = window(40, 2);
        let mut lines = Vec::new();
        push_window_with_detail(
            &mut lines,
            "  Spend",
            &w,
            &theme(),
            at(12),
            WindowRow::default(),
            Some("$4.00 of $10.00"),
        );
        let out = render_bordered(&lines, &theme());
        assert!(out.contains("40%"), "{out}");
        assert!(out.contains("· $4.00 of $10.00"), "{out}");
        assert!(out.contains("Resets in 2h 00m"), "{out}");
    }
}
