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
/// diagnostic in a UI cell and wrong for anything that shares a line-oriented
/// stream with a report the user is reading: one embedded newline forges a
/// report line. Collapsing them is what `sync::restore::report` has always
/// done; this is that rule, shared rather than restated.
pub fn sanitize_untrusted_line(value: &str) -> String {
    sanitize_untrusted_field(value).replace('\n', " ")
}

/// A filesystem path on its way to the same place.
///
/// Every component of a restore destination comes from a manifest a hostile
/// remote wrote, and [`std::path::Display`](std::path::Display) escapes
/// nothing. This is what [`crate::error::AppError::Io`] renders its path
/// through, so an attacker-chosen path cannot carry a terminal escape out of
/// *any* error site rather than only the ones that remembered.
pub fn sanitize_untrusted_path(path: &std::path::Path) -> String {
    sanitize_untrusted_line(&path.to_string_lossy())
}

/// Whether ANSI styling may be written to a stream — a terminal, and `NO_COLOR`
/// unset.
///
/// **This lives here rather than beside the renderer that wants it.** The
/// palette and every styled string belong to `sync::report::Style`, but that
/// module is under `src/sync/`, and `passphrase`'s structural guard walks the
/// whole of that subtree refusing `std::env` in production code — because every
/// password input path lives in it and argv and the environment are readable by
/// any local user. The one legitimate environment read for colour therefore
/// lives outside the guarded tree, and the *fact* is injected inward, exactly
/// as `progress::reporter` already takes `is_terminal` rather than asking.
///
/// `NO_COLOR` disables colour when it is **present**, whatever its value —
/// <https://no-color.org>.
pub fn color_enabled(is_terminal: bool) -> bool {
    color_enabled_with(is_terminal, std::env::var_os("NO_COLOR").is_some())
}

/// [`color_enabled`] over an injected environment, so the rule is testable
/// without mutating a process-wide variable other tests are reading in
/// parallel.
pub fn color_enabled_with(is_terminal: bool, no_color_set: bool) -> bool {
    is_terminal && !no_color_set
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

    #[test]
    fn colour_needs_a_terminal_and_an_unset_no_color() {
        assert!(color_enabled_with(true, false));
        assert!(
            !color_enabled_with(false, false),
            "a pipe is never coloured"
        );
        assert!(!color_enabled_with(true, true), "NO_COLOR wins over a tty");
        assert!(!color_enabled_with(false, true));
    }

    #[test]
    fn caps_untrusted_fields_without_splitting_unicode() {
        let input = "é".repeat(MAX_UNTRUSTED_FIELD_CHARS + 10);
        let output = sanitize_untrusted_field(&input);
        assert_eq!(output.chars().count(), MAX_UNTRUSTED_FIELD_CHARS);
        assert!(output.chars().all(|ch| ch == 'é'));
    }
}
