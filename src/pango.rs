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
use unicode_width::UnicodeWidthChar;

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
    crate::display::sanitize_untrusted_field(s)
        .replace('&', "&amp;")
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

/// Map a *remaining balance* to a severity tier — the prepaid-credit analogue
/// of [`severity_for`], which keys on a percentage.
///
/// Balance vendors have no denominator to take a percentage of, so the tiers
/// are absolute amounts and therefore currency-dependent: CNY is scaled ≈7×
/// USD (rough parity). An unknown currency is treated as USD-scale, which is
/// the same choice `format::with_currency` makes for an unknown code.
///
/// Five vendors carried their own copy of this ladder, two of them including
/// the CNY row. A "low balance" must not mean ¥5 in one panel and $5 in
/// another, so the numbers live here once. Callers keep their own
/// *pre*-checks — DeepSeek's `is_available` and Moonshot's `<= 0` shortcut are
/// about the account, not the amount.
pub fn balance_severity(balance: f64, currency: &str) -> PaceSeverity {
    let (critical, high, mid) = match currency {
        "CNY" => (7.0_f64, 35.0, 140.0),
        _ => (1.0_f64, 5.0, 20.0),
    };
    if balance < critical {
        PaceSeverity::Critical
    } else if balance < high {
        PaceSeverity::High
    } else if balance < mid {
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

/// Count the visible width of a Pango-marked string: its width in terminal
/// columns with all `<span …>…</span>` tags stripped. Used by the bordered-box
/// renderer for padding alignment — claudebar implements this with
/// `sed 's/<[^>]*>//g'` plus a character count.
///
/// Columns, not characters: a CJK ideograph occupies two cells, so counting
/// `日本語` as 3 leaves every line of a Japanese tooltip three columns short of
/// the border. Combining marks occupy none.
///
/// **Always `width()`, never `width_cjk()`.** Nearly every glyph this project
/// draws with — the box border `╭─╮│`, the bar's `█`, the arrows, `·`, `…`, `€`
/// and the Nerd Font mark — is East Asian Width *Ambiguous*. `width()` gives
/// those 1, which is what the renderer has always assumed; `width_cjk()` would
/// give them 2 and tear the tooltip box apart.
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
                    count += UnicodeWidthChar::width(ch).unwrap_or(0);
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
    if end == 0 {
        return None;
    }
    let name = &body[..end];
    // GMarkup/Pango accepts XML's five predefined named entities plus
    // decimal and lowercase-x hexadecimal character references. Treating an
    // arbitrary short `&name;` or malformed `&#...;` as decoded would repeat
    // the original width bug in the other direction.
    let named = matches!(name, "amp" | "lt" | "gt" | "quot" | "apos");
    let numeric = name
        .strip_prefix("#x")
        .and_then(|digits| parse_xml_char(digits, 16))
        .or_else(|| {
            name.strip_prefix('#')
                .and_then(|digits| parse_xml_char(digits, 10))
        })
        .is_some();
    (named || numeric).then_some(1 + end + 1)
}

fn parse_xml_char(digits: &str, radix: u32) -> Option<u32> {
    if digits.is_empty() {
        return None;
    }
    let value = u32::from_str_radix(digits, radix).ok()?;
    matches!(
        value,
        0x9 | 0xA | 0xD | 0x20..=0xD7FF | 0xE000..=0xFFFD | 0x10000..=0x10FFFF
    )
    .then_some(value)
}

#[cfg(test)]
mod tests {

    /// Five vendors carried this ladder. Grok, Kilo and Novita bill only in
    /// USD; DeepSeek and Moonshot also sell in CNY, and the two of them had
    /// the scaled row while the other three did not. Pin both rows so a
    /// "running low" balance cannot come to mean two different amounts.
    #[test]
    fn the_balance_ladder_is_one_table_in_both_currencies() {
        use crate::pacing::PaceSeverity::*;
        for (balance, expected) in [
            (-1.0, Critical),
            (0.0, Critical),
            (0.99, Critical),
            (1.0, High),
            (4.99, High),
            (5.0, Mid),
            (19.99, Mid),
            (20.0, Low),
        ] {
            assert_eq!(balance_severity(balance, "USD"), expected, "USD {balance}");
            // An unknown code is USD-scale, matching format::with_currency.
            assert_eq!(balance_severity(balance, "SEK"), expected, "SEK {balance}");
        }
        for (balance, expected) in [
            (6.99, Critical),
            (7.0, High),
            (34.99, High),
            (35.0, Mid),
            (139.99, Mid),
            (140.0, Low),
        ] {
            assert_eq!(balance_severity(balance, "CNY"), expected, "CNY {balance}");
        }
    }
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
    fn escape_removes_terminal_controls() {
        assert_eq!(escape("safe\x1b[2J\x07text"), "safe[2Jtext");
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
    fn visible_width_counts_a_cjk_ideograph_as_two_columns() {
        // Japanese and Korean labels occupy two terminal cells per glyph.
        // Counting characters left every line of a translated tooltip short of
        // its border by exactly the number of ideographs on it.
        assert_eq!(visible_width("日本語"), 6);
        assert_eq!(visible_width("사용량"), 6);
        assert_eq!(visible_width("セッション"), 10);
        // Mixed runs add up per glyph, not per character.
        assert_eq!(visible_width("5h 日本語"), 3 + 6);
        // Tags are still stripped before measuring.
        assert_eq!(visible_width("<span foreground='#fff'>日本語</span>"), 6);
    }

    #[test]
    fn visible_width_counts_a_combining_mark_as_zero() {
        // "e" + U+0301 renders as one cell, unlike the precomposed "é" which is
        // already one char. Both must measure 1 or accented locales ragged-edge.
        assert_eq!(visible_width("e\u{301}"), 1);
        assert_eq!(visible_width("é"), 1);
        assert_eq!(visible_width("cafe\u{301}"), 4);
    }

    #[test]
    fn visible_width_keeps_every_ambiguous_glyph_this_project_draws_at_one() {
        // Nearly every glyph in this UI is East Asian Width *Ambiguous*: the
        // box border, the bar block, the arrows, the separators. `width()`
        // gives them 1 — which is what the renderer has always assumed — while
        // `width_cjk()` would give them 2 and tear the tooltip box apart. This
        // test is the tripwire for anyone who reaches for the CJK variant.
        for g in [
            "╭", "╮", "╯", "╰", "─", "│", "█", "░", "▶", "◀", "▸", "◆", "←",
            "↑", "→", "↓", "↻", "≈", "·", "…", "—", "•", "€", "£", "¥", "é",
            "α", "β", "⚠", "✓", "✗", "❯", "⏱", "⏸", "‸",
        ] {
            assert_eq!(visible_width(g), 1, "{g} must measure one column");
        }
    }

    #[test]
    fn visible_width_counts_an_entity_as_one_glyph() {
        assert_eq!(visible_width("&amp;"), 1);
        assert_eq!(visible_width("Claude &amp; GPT"), 12);
        assert_eq!(visible_width(escape("Claude & GPT").as_str()), 12);
        assert_eq!(visible_width("&lt;&gt;&quot;&apos;"), 4);
        assert_eq!(visible_width("&#38;"), 1);
        assert_eq!(visible_width("&#x26;"), 1);
        assert_eq!(visible_width("&#00000038;"), 1);
        assert_eq!(visible_width("<span>a &amp; b</span>"), 5);
    }

    #[test]
    fn visible_width_treats_a_bare_ampersand_as_one_glyph() {
        assert_eq!(visible_width("a & b"), 5);
        assert_eq!(visible_width("&"), 1);
        assert_eq!(visible_width("&;"), 2);
        assert_eq!(visible_width("&nope;"), 6);
        assert_eq!(visible_width("&AMP;"), 5);
        assert_eq!(visible_width("&#GG;"), 5);
        assert_eq!(visible_width("&#0;"), 4);
        assert_eq!(visible_width("&#x110000;"), 10);
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
