//! Sanitization for text that crosses an untrusted data boundary into a UI.
//!
//! Vendor responses and cached diagnostics are data, not terminal programs.
//! Keep ordinary Unicode and line breaks, but remove terminal control bytes
//! before the text is persisted or handed to Pango/ratatui/ANSI renderers.

/// Generous bound for one remote label or diagnostic field. Legitimate values
/// are normally a few dozen characters; the cap prevents a valid-but-hostile
/// JSON response from turning one UI cell or cache sidecar into megabytes.
pub const MAX_UNTRUSTED_FIELD_CHARS: usize = 4 * 1024;

/// Strip terminal control characters while preserving readable line layout.
///
/// Newlines are safe and useful in diagnostics. Tabs and carriage returns are
/// normalized to spaces; every other Unicode control character (including ESC,
/// BEL, DEL, and C1 controls) is removed. Invisible bidirectional markers and
/// overrides are also removed so an untrusted label cannot visually reorder
/// neighboring UI text. The result is capped by character, not byte, so UTF-8
/// is never split.
pub fn sanitize_untrusted_field(value: &str) -> String {
    value
        .chars()
        .filter_map(|ch| match ch {
            '\n' => Some('\n'),
            '\t' | '\r' => Some(' '),
            _ if ch.is_control() || is_bidi_control(ch) => None,
            _ => Some(ch),
        })
        .take(MAX_UNTRUSTED_FIELD_CHARS)
        .collect()
}

/// One line of untrusted text on its way to a terminal or a log.
///
/// [`sanitize_untrusted_field`] keeps newlines, which is right for a multi-line
/// diagnostic in a UI cell and wrong for anything sharing a line-oriented
/// stream with output the user is reading: one embedded newline forges a line.
/// A subprocess's stderr is exactly that case — it is one or more lines on
/// their way into an error message.
pub fn sanitize_untrusted_line(value: &str) -> String {
    sanitize_untrusted_field(value).replace('\n', " ")
}

/// A filesystem path on its way to the same place.
///
/// [`std::path::Display`] escapes nothing, and a path is not always a literal
/// this program chose — it can carry a component from an account name, a
/// vendor response, or an archive member. This is what [`crate::error::AppError::Io`]
/// renders its path through, so an attacker-chosen path cannot carry a terminal
/// escape out of *any* error site rather than only the ones that remembered.
pub fn sanitize_untrusted_path(path: &std::path::Path) -> String {
    sanitize_untrusted_line(&path.to_string_lossy())
}

fn is_bidi_control(ch: char) -> bool {
    matches!(
        ch,
        '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_terminal_sequences_but_keeps_text_and_newlines() {
        let input = "before\x1b]52;c;Y2xpcGJvYXJk\x07after\nnext\tcolumn\rreturn\u{202e}spoof";
        assert_eq!(
            sanitize_untrusted_field(input),
            "before]52;c;Y2xpcGJvYXJkafter\nnext column returnspoof"
        );
    }

    /// A subprocess's stderr shares a line-oriented stream with the message
    /// carrying it, so a newline in it forges a line the program never wrote.
    /// This is the shape `security` and `tar` diagnostics arrive in.
    #[test]
    fn collapses_newlines_so_untrusted_text_cannot_forge_a_line() {
        let stderr = "tar: \x1b[2Kall good\nRESTORED: 0 files";
        let out = sanitize_untrusted_line(stderr);
        assert!(!out.contains('\n'), "{out:?}");
        assert!(!out.contains('\u{1b}'), "{out:?}");
        assert_eq!(out, "tar: [2Kall good RESTORED: 0 files");
    }

    #[test]
    fn a_path_carrying_an_escape_renders_without_it() {
        let path = std::path::Path::new("/tmp/\x1b[2Kspoofed");
        assert_eq!(sanitize_untrusted_path(path), "/tmp/[2Kspoofed");
    }

    #[test]
    fn caps_untrusted_fields_without_splitting_unicode() {
        let input = "é".repeat(MAX_UNTRUSTED_FIELD_CHARS + 10);
        let output = sanitize_untrusted_field(&input);
        assert_eq!(output.chars().count(), MAX_UNTRUSTED_FIELD_CHARS);
        assert!(output.chars().all(|ch| ch == 'é'));
    }
}

/// Width of plain (non-markup) text in terminal columns.
///
/// Use this for layout arithmetic — column padding, gauge widths, the widest
/// label in a report. A character count is wrong for any locale with CJK text,
/// where one glyph occupies two cells, and for combining marks, which occupy
/// none.
///
/// This is deliberately separate from [`crate::pango::visible_width`], which
/// additionally strips `<span>` markup. Feeding plain text to that function
/// would treat a literal `<` as the start of a tag and silently undercount the
/// rest of the line; feeding markup to this one would count the tags.
///
/// Character counts remain correct for *limits* (a config field documented as
/// "1 to 48 characters") and for edit-cursor positions, which move per
/// character rather than per column. Those are not layout.
pub fn text_width(s: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(s)
}

/// Left-align `s` in a field `width` columns wide, padding with spaces.
///
/// `format!("{s:width$}")` cannot do this: Rust's fill/align pads a `str` by
/// character count, so a label of three CJK ideographs in a field of ten gets
/// seven trailing spaces and renders thirteen columns wide, pushing the next
/// column out of line. Padding is computed from [`text_width`] instead.
///
/// A string already at or past `width` is returned unpadded rather than
/// truncated — the callers use this for column alignment, where clipping a
/// label is worse than a single long row.
pub fn pad_end(s: &str, width: usize) -> String {
    let w = text_width(s);
    if w >= width {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + (width - w));
    out.push_str(s);
    out.extend(std::iter::repeat_n(' ', width - w));
    out
}

#[cfg(test)]
mod width_tests {
    use super::{pad_end, text_width};

    #[test]
    fn text_width_measures_columns_not_characters() {
        assert_eq!(text_width("Weekly"), 6);
        assert_eq!(text_width("日本語"), 6); // 3 chars, 6 columns
        assert_eq!(text_width("사용량"), 6);
        assert_eq!(text_width("e\u{301}"), 1); // combining mark adds nothing
    }

    #[test]
    fn pad_end_pads_by_columns_not_characters() {
        // The bug this exists to prevent: `format!("{:10}", "日本語")` appends
        // seven spaces to a six-column string, yielding thirteen columns.
        assert_eq!(text_width(&pad_end("日本語", 10)), 10);
        assert_eq!(text_width(&pad_end("Weekly", 10)), 10);
        assert_eq!(pad_end("Weekly", 10), "Weekly    ");
        // Already wide enough: returned unchanged rather than truncated.
        assert_eq!(pad_end("Weekly", 3), "Weekly");
        assert_eq!(pad_end("日本語", 6), "日本語");
    }

    #[test]
    fn text_width_counts_a_literal_angle_bracket() {
        // The reason this is not `pango::visible_width`: that function would
        // read `<` as a tag opening and drop everything after it.
        assert_eq!(text_width("a<b"), 3);
        assert_eq!(text_width("Claude & GPT"), 12);
    }
}
