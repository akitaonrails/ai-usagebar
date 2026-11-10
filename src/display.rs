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

/// Remove `secret` from text that is about to be persisted or displayed.
///
/// A provider's answer can quote the credential it was sent with — Z.AI
/// returns the reason for an in-band failure in `msg`, inside an HTTP 200 —
/// and the first point of writing is the vendor cache, so the key must be
/// gone before the failure is recorded, not scrubbed after the fact. Exact
/// occurrences become `[key]`; when the secret is at least eight characters
/// long and any consecutive eight-character window of it — counted in
/// `char`, not bytes — still survives in the text, the provider quoted the
/// key in part, and a partial echo is the key with a missing tail, so the
/// whole text is replaced with a fixed note. An empty secret leaves the text
/// alone.
pub(crate) fn redact_secret(text: &str, secret: &str) -> String {
    const PARTIAL_ECHO: &str = "The provider's answer quoted the key, so it is not shown.";
    if secret.is_empty() {
        return text.to_string();
    }
    let redacted = text.replace(secret, "[key]");
    let chars: Vec<char> = secret.chars().collect();
    let quoted_partial = chars.len() >= 8
        && (0..=chars.len() - 8).any(|start| {
            redacted.contains(chars[start..start + 8].iter().collect::<String>().as_str())
        });
    if quoted_partial {
        PARTIAL_ECHO.to_string()
    } else {
        redacted
    }
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

    /// A provider that quotes the key a fetch was sent with must not persist
    /// it: exact echoes become the marker, and for a key of eight characters
    /// or more a surviving eight-character window is a partial echo — the key
    /// with a missing tail — so the whole text is replaced instead.
    #[test]
    fn redact_secret_replaces_echoed_keys_and_partial_echoes() {
        let key = "abcdefghijklmnop";
        // A verbatim echo, on its own and alongside the provider's wording.
        assert_eq!(redact_secret(key, key), "[key]");
        assert_eq!(
            redact_secret("bad key abcdefghijklmnop", key),
            "bad key [key]"
        );
        // An eight-char window is the key in all but the missing tail.
        assert_eq!(
            redact_secret("bad key abcdefgh…", key),
            "The provider's answer quoted the key, so it is not shown."
        );
        // Text without the key passes through unchanged.
        assert_eq!(redact_secret("HTTP 500", key), "HTTP 500");
        // A short key has no eight-char window: exact replacement only.
        assert_eq!(redact_secret("short", "short"), "[key]");
        // An empty secret leaves the text alone.
        assert_eq!(redact_secret("nothing to hide", ""), "nothing to hide");
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

/// Right-align `s` in a field `width` columns wide — the [`pad_end`]
/// counterpart, measured in display width for the same reason.
pub fn pad_start(s: &str, width: usize) -> String {
    let w = text_width(s);
    if w >= width {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + (width - w));
    out.extend(std::iter::repeat_n(' ', width - w));
    out.push_str(s);
    out
}

#[cfg(test)]
mod width_tests {
    use super::{pad_end, pad_start, text_width};

    #[test]
    fn text_width_measures_columns_not_characters() {
        assert_eq!(text_width("Weekly"), 6);
        assert_eq!(text_width("日本語"), 6); // 3 chars, 6 columns
        assert_eq!(text_width("사용량"), 6);
        assert_eq!(text_width("e\u{301}"), 1); // combining mark adds nothing
    }

    #[test]
    fn pad_start_pads_by_columns_not_characters() {
        // The right-align half of the report's columns: a wide or combining
        // value must end at the same column as an ASCII one.
        assert_eq!(pad_start("7", 6), "     7");
        assert_eq!(pad_start("日本語", 8), "  日本語");
        assert_eq!(pad_start("e\u{301}", 3), "  e\u{301}");
        // Already wide enough: untouched, never truncated.
        assert_eq!(pad_start("日本語", 4), "日本語");
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
