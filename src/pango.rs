//! Pango-markup rendering helpers shared by the widget bar text and tooltip.
//!
//! Two primary primitives:
//! - [`progress_bar`] — fixed-width filled+empty bar, optional elapsed marker.
//!   Mirrors claudebar's `make_bar()` (claudebar:225-250).
//! - [`color_span`] — wraps text in `<span foreground='…'>` (the only Pango
//!   tag claudebar uses for color).
//!
//! Helpers also include the bordered tooltip frame builder
//! ([`render_bordered_box`]), used by the default tooltip layout.

use crate::pacing::PaceSeverity;
use crate::theme::Theme;

/// Width of the progress bar in characters. Matches `BAR_LEN=20` (claudebar:169).
pub const BAR_LEN: u32 = 20;

const FILLED: char = '█';
const EMPTY: char = '░';

/// Wrap `text` in a Pango `<span foreground='COLOR'>…</span>`.
///
/// `text` must already be Pango-safe (no raw `<` / `>` / `&`). Callers passing
/// user-controlled strings should escape first via [`escape`].
pub fn color_span(color: &str, text: &str) -> String {
    format!("<span foreground='{color}'>{text}</span>")
}

/// Escape `&`, `<`, `>` for Pango markup (which is XML-ish).
pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Map a usage percentage to a severity tier, matching `color_for`
/// (claudebar:198-205):
///   >= 90 → critical (red); >= 75 → high (orange);
///   >= 50 → mid (yellow); else low (green).
pub fn severity_for(pct: i32) -> PaceSeverity {
    if pct >= 90 {
        PaceSeverity::Critical
    } else if pct >= 75 {
        PaceSeverity::High
    } else if pct >= 50 {
        PaceSeverity::Mid
    } else {
        PaceSeverity::Low
    }
}

/// Resolve a severity tier to a concrete hex color from the theme.
pub fn severity_color(sev: PaceSeverity, theme: &Theme) -> &str {
    match sev {
        PaceSeverity::Low => &theme.green,
        PaceSeverity::Mid => &theme.yellow,
        PaceSeverity::High => &theme.orange,
        PaceSeverity::Critical => &theme.red,
    }
}

/// Build a fixed-width progress bar in Pango markup.
///
/// - `pct` is clamped to `0..=100`.
/// - `fill_color` colors the filled (`█`) cells; `theme.bar_empty` colors the
///   empty (`░`) cells.
/// - If `marker_pct` is `Some`, a single `█` in `theme.marker` color is placed
///   at the corresponding cell, displacing one empty cell and (when usage
///   exceeds the marker position) splitting the filled run around it.
///
/// Implementation note: this mirrors claudebar:225-250's two-branch logic but
/// in a single expression. The resulting markup is byte-identical for the same
/// inputs.
pub fn progress_bar(pct: i32, fill_color: &str, theme: &Theme, marker_pct: Option<i32>) -> String {
    let pct = pct.clamp(0, 100) as u32;
    let bar_len = BAR_LEN;
    let filled = (pct * bar_len) / 100;

    let Some(marker) = marker_pct.map(|p| p.clamp(0, 100) as u32) else {
        // Simple two-segment bar.
        let empty = bar_len - filled;
        return format!(
            "<span foreground='{fill_color}'>{f}</span><span foreground='{empty_color}'>{e}</span>",
            f = repeat_char(FILLED, filled),
            e = repeat_char(EMPTY, empty),
            empty_color = theme.bar_empty,
        );
    };

    // Marker placement (claudebar:238-249).
    let mut m = (marker * bar_len) / 100;
    if m > bar_len - 1 {
        m = bar_len - 1;
    }
    let pre_f = filled.min(m);
    let post_f = if filled > m + 1 { filled - m - 1 } else { 0 };
    let pre_e = m - pre_f;
    let post_e = bar_len - m - 1 - post_f;

    let mut out = String::with_capacity(256);
    // Pre-marker segment: filled run, then empties up to the marker.
    out.push_str(&format!(
        "<span foreground='{fill_color}'>{}</span>",
        repeat_char(FILLED, pre_f)
    ));
    out.push_str(&format!(
        "<span foreground='{}'>{}</span>",
        theme.bar_empty,
        repeat_char(EMPTY, pre_e)
    ));
    // Marker (single filled cell in marker color).
    out.push_str(&format!(
        "<span foreground='{}'>{}</span>",
        theme.marker, FILLED
    ));
    // Post-marker segment: filled run, then empties to fill the bar.
    out.push_str(&format!(
        "<span foreground='{fill_color}'>{}</span>",
        repeat_char(FILLED, post_f)
    ));
    out.push_str(&format!(
        "<span foreground='{}'>{}</span>",
        theme.bar_empty,
        repeat_char(EMPTY, post_e)
    ));
    out
}

fn repeat_char(c: char, n: u32) -> String {
    std::iter::repeat_n(c, n as usize).collect()
}

/// Count the visible width of a Pango-marked string (its character count with
/// all `<span …>…</span>` tags stripped). Used by the bordered-box renderer
/// for padding alignment — claudebar implements this with `sed 's/<[^>]*>//g'`.
pub fn visible_width(s: &str) -> usize {
    let mut depth = 0usize;
    let mut count = 0usize;
    let mut rest = s;
    while let Some(ch) = rest.chars().next() {
        let step = match ch {
            '<' => {
                depth += 1;
                ch.len_utf8()
            }
            '>' if depth > 0 => {
                depth -= 1;
                ch.len_utf8()
            }
            // An entity such as "&amp;" is five bytes but one glyph; measuring
            // it as five over-counts the line and ragged-edges the tooltip box.
            '&' if depth == 0 => {
                count += 1;
                entity_len(rest).unwrap_or(ch.len_utf8())
            }
            _ => {
                if depth == 0 {
                    count += 1;
                }
                ch.len_utf8()
            }
        };
        rest = &rest[step..];
    }
    count
}

/// Byte length of the XML entity starting at `s` (which begins with `&`), or
/// `None` when this is a bare ampersand rather than an entity.
fn entity_len(s: &str) -> Option<usize> {
    let body = s.strip_prefix('&')?;
    let end = body.find(';')?;
    if end == 0 || end > 8 {
        return None;
    }
    let name = &body[..end];
    let named = name.chars().all(|c| c.is_ascii_alphanumeric());
    let numeric = name
        .strip_prefix('#')
        .is_some_and(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_alphanumeric()));
    (named || numeric).then_some(1 + end + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme::default()
    }

    #[test]
    fn severity_thresholds_match_claudebar() {
        assert_eq!(severity_for(0), PaceSeverity::Low);
        assert_eq!(severity_for(49), PaceSeverity::Low);
        assert_eq!(severity_for(50), PaceSeverity::Mid);
        assert_eq!(severity_for(74), PaceSeverity::Mid);
        assert_eq!(severity_for(75), PaceSeverity::High);
        assert_eq!(severity_for(89), PaceSeverity::High);
        assert_eq!(severity_for(90), PaceSeverity::Critical);
        assert_eq!(severity_for(100), PaceSeverity::Critical);
    }

    #[test]
    fn color_span_wraps_pango() {
        assert_eq!(
            color_span("#ff0000", "hi"),
            "<span foreground='#ff0000'>hi</span>"
        );
    }

    #[test]
    fn escape_handles_markup_chars() {
        // `&` must come first so we don't double-escape produced `&` chars.
        assert_eq!(escape("a < b & c > d"), "a &lt; b &amp; c &gt; d");
    }

    #[test]
    fn bar_zero_pct_is_all_empty() {
        let b = progress_bar(0, "#000000", &theme(), None);
        // Should contain 20 ░ chars and no █ chars.
        assert_eq!(b.matches('░').count(), BAR_LEN as usize);
        assert_eq!(b.matches('█').count(), 0);
    }

    #[test]
    fn bar_hundred_pct_is_all_filled() {
        let b = progress_bar(100, "#ff0000", &theme(), None);
        assert_eq!(b.matches('█').count(), BAR_LEN as usize);
        assert_eq!(b.matches('░').count(), 0);
    }

    #[test]
    fn bar_clamps_overflow() {
        let b = progress_bar(150, "#ff0000", &theme(), None);
        assert_eq!(b.matches('█').count(), BAR_LEN as usize);
    }

    #[test]
    fn bar_fifty_pct_splits_evenly() {
        let b = progress_bar(50, "#ff0000", &theme(), None);
        assert_eq!(b.matches('█').count(), 10);
        assert_eq!(b.matches('░').count(), 10);
    }

    #[test]
    fn bar_with_marker_keeps_total_width() {
        // 50% usage, 50% marker → marker occupies cell 10, displacing one
        // empty cell. Total visible width stays at BAR_LEN (claudebar
        // semantics — marker replaces, doesn't append).
        let b = progress_bar(50, "#ff0000", &theme(), Some(50));
        assert!(b.contains("#ff0000"));
        assert!(b.contains(&theme().marker));
        assert_eq!(visible_width(&b), BAR_LEN as usize);
    }

    #[test]
    fn bar_marker_at_zero_is_renderable() {
        // Marker at 0 with 0% usage → no panic on underflow; width preserved.
        let b = progress_bar(0, "#ff0000", &theme(), Some(0));
        assert_eq!(visible_width(&b), BAR_LEN as usize);
    }

    #[test]
    fn bar_marker_at_hundred_is_renderable() {
        // Marker clamped to BAR_LEN - 1 (claudebar:240). 100% usage fills
        // everything to the left of the marker; the marker is the last cell.
        let b = progress_bar(100, "#ff0000", &theme(), Some(100));
        assert_eq!(visible_width(&b), BAR_LEN as usize);
        // Filled cells before marker = 19, marker = 1, nothing after.
        assert_eq!(b.matches('█').count(), BAR_LEN as usize);
        assert_eq!(b.matches('░').count(), 0);
    }

    #[test]
    fn visible_width_strips_tags() {
        assert_eq!(visible_width("<span foreground='#fff'>hello</span>"), 5);
        assert_eq!(visible_width("a<x>b</x>c"), 3);
        assert_eq!(visible_width("plain text"), 10);
    }

    #[test]
    fn visible_width_handles_nested_tags() {
        assert_eq!(visible_width("<a><b>xy</b></a>"), 2);
    }

    /// An escaped character occupies one cell on screen. Counting its source
    /// bytes instead left every tooltip row containing one short on padding.
    #[test]
    fn visible_width_counts_an_entity_as_one_glyph() {
        assert_eq!(visible_width("&amp;"), 1);
        assert_eq!(visible_width("Claude &amp; GPT"), 12);
        assert_eq!(visible_width(escape("Claude & GPT").as_str()), 12);
        assert_eq!(visible_width("&lt;&gt;&quot;&apos;"), 4);
        assert_eq!(visible_width("&#38;"), 1);
        assert_eq!(visible_width("<span>a &amp; b</span>"), 5);
    }

    #[test]
    fn visible_width_treats_a_bare_ampersand_as_one_glyph() {
        assert_eq!(visible_width("a & b"), 5);
        assert_eq!(visible_width("&"), 1);
        assert_eq!(visible_width("&;"), 2);
        // Too long to be an entity, so every character counts.
        assert_eq!(visible_width("&notanentityatall;"), 18);
    }

    #[test]
    fn escaped_and_plain_labels_measure_the_same() {
        for label in ["Claude & GPT (weekly)", "a<b>c", "quote\"d", "plain"] {
            assert_eq!(
                visible_width(escape(label).as_str()),
                label.chars().count(),
                "{label}"
            );
        }
    }
}
