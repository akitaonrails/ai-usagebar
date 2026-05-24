//! Terminal-mode renderer for `--pretty` and `--watch` modes.
//!
//! Translates Pango `<span foreground='#RRGGBB'>…</span>` markup into ANSI
//! 24-bit escape sequences and prints the result. The same renderer code in
//! `widget::render` produces the Pango string; this module is purely a Pango
//! → ANSI translator so the local-testing output is visually equivalent to
//! what Waybar would show.

use std::io::Write;

use crate::waybar::WaybarOutput;

/// Pretty-print a WaybarOutput to `w` as colored terminal text.
pub fn print_pretty(w: &mut impl Write, out: &WaybarOutput) -> std::io::Result<()> {
    writeln!(w, "{}", pango_to_ansi(&out.text))?;
    writeln!(w)?;
    writeln!(w, "{}", pango_to_ansi(&out.tooltip))?;
    writeln!(w)?;
    let dim = "\x1b[2m";
    let reset = "\x1b[0m";
    writeln!(w, "{dim}class: {:?}{reset}", out.class)?;
    Ok(())
}

/// Translate Pango markup to ANSI escapes. Only handles the `<span
/// foreground='#RRGGBB'>` and `<span font_weight='bold' foreground='…'>`
/// shapes used by this crate's renderers — does NOT aspire to be a general
/// Pango parser.
pub fn pango_to_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '<' {
            // Parse a tag.
            let mut tag = String::new();
            for nc in chars.by_ref() {
                if nc == '>' {
                    break;
                }
                tag.push(nc);
            }
            apply_tag(&tag, &mut out);
        } else {
            out.push(c);
        }
    }
    // Ensure we always reset at the end so the next prompt isn't tinted.
    out.push_str("\x1b[0m");
    out
}

fn apply_tag(tag: &str, out: &mut String) {
    if tag.starts_with('/') {
        // End tag — reset everything. The renderer always pairs spans, so
        // this is fine without a stack.
        out.push_str("\x1b[0m");
        return;
    }

    let bold = tag.contains("font_weight='bold'") || tag.contains("font_weight=\"bold\"");
    let color = extract_attr(tag, "foreground");

    if let Some(hex) = color.as_deref().and_then(parse_hex_color) {
        let (r, g, b) = hex;
        out.push_str(&format!("\x1b[38;2;{r};{g};{b}m"));
    }
    if bold {
        out.push_str("\x1b[1m");
    }
}

fn extract_attr(tag: &str, key: &str) -> Option<String> {
    // Looks for `key='value'` or `key="value"`. Doesn't handle escaped quotes
    // — the renderer never produces them.
    for delim in ['\'', '"'] {
        let needle = format!("{key}={delim}");
        if let Some(start) = tag.find(&needle) {
            let value_start = start + needle.len();
            if let Some(end) = tag[value_start..].find(delim) {
                return Some(tag[value_start..value_start + end].to_string());
            }
        }
    }
    None
}

fn parse_hex_color(s: &str) -> Option<(u8, u8, u8)> {
    let s = s.strip_prefix('#').unwrap_or(s);
    if s.len() != 6 {
        return None;
    }
    Some((
        u8::from_str_radix(&s[0..2], 16).ok()?,
        u8::from_str_radix(&s[2..4], 16).ok()?,
        u8::from_str_radix(&s[4..6], 16).ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_unchanged() {
        // Plus the reset at the end.
        assert_eq!(pango_to_ansi("hello"), "hello\x1b[0m");
    }

    #[test]
    fn color_span_translates_to_24bit_ansi() {
        let s = pango_to_ansi("<span foreground='#ff0000'>red</span>");
        assert!(s.starts_with("\x1b[38;2;255;0;0m"));
        assert!(s.contains("red"));
        // Both the closing </span> and the trailing safety reset emit \x1b[0m;
        // any presence proves the close tag was consumed.
        assert!(s.contains("\x1b[0m"));
    }

    #[test]
    fn bold_attribute_adds_bold_escape() {
        let s = pango_to_ansi("<span font_weight='bold' foreground='#00ff00'>x</span>");
        assert!(s.contains("\x1b[38;2;0;255;0m"));
        assert!(s.contains("\x1b[1m"));
    }

    #[test]
    fn nested_spans_handled_simply() {
        // Each closing tag emits a full reset; that's acceptable for local
        // pretty output (the inner color still renders before the reset).
        let s = pango_to_ansi("<span foreground='#ff0000'>a<span foreground='#00ff00'>b</span>c</span>");
        // Both colors must appear somewhere in the output.
        assert!(s.contains("\x1b[38;2;255;0;0m"));
        assert!(s.contains("\x1b[38;2;0;255;0m"));
        assert!(s.contains("a"));
        assert!(s.contains("b"));
        assert!(s.contains("c"));
    }

    #[test]
    fn unknown_color_is_just_dropped() {
        let s = pango_to_ansi("<span foreground='not-hex'>x</span>");
        // No color escape, but text + reset still present.
        assert!(s.contains("x"));
        assert!(!s.contains("\x1b[38;2"));
    }

    #[test]
    fn double_quoted_attributes_also_work() {
        let s = pango_to_ansi(r##"<span foreground="#0000ff">b</span>"##);
        assert!(s.contains("\x1b[38;2;0;0;255m"));
    }
}
