//! Convert Pango markup → ratatui `Text` so the existing tooltip renderers
//! can drive the TUI without code duplication.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};

/// Translate Pango `<span foreground='…' [font_weight='bold']>…</span>` to
/// ratatui spans. Same subset the widget renderers produce — no general
/// XML parsing.
pub fn to_text(input: &str) -> Text<'static> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    for raw_line in input.lines() {
        lines.push(line_from(raw_line));
    }
    Text::from(lines)
}

fn line_from(input: &str) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut chars = input.chars().peekable();
    let mut style_stack: Vec<Style> = vec![Style::default()];
    let mut buf = String::new();

    while let Some(c) = chars.next() {
        if c == '<' {
            // Flush buffer.
            if !buf.is_empty() {
                spans.push(Span::styled(
                    std::mem::take(&mut buf),
                    *style_stack.last().unwrap(),
                ));
            }
            // Read tag.
            let mut tag = String::new();
            for nc in chars.by_ref() {
                if nc == '>' {
                    break;
                }
                tag.push(nc);
            }
            if let Some(stripped) = tag.strip_prefix('/') {
                let _ = stripped;
                style_stack.pop();
                if style_stack.is_empty() {
                    style_stack.push(Style::default());
                }
            } else {
                let mut style = *style_stack.last().unwrap();
                if let Some(color) = extract_attr(&tag, "foreground").and_then(parse_hex) {
                    style = style.fg(color);
                }
                if tag.contains("font_weight='bold'") || tag.contains("font_weight=\"bold\"") {
                    style = style.add_modifier(Modifier::BOLD);
                }
                style_stack.push(style);
            }
        } else {
            buf.push(c);
        }
    }
    if !buf.is_empty() {
        spans.push(Span::styled(buf, *style_stack.last().unwrap()));
    }
    Line::from(spans)
}

fn extract_attr(tag: &str, key: &str) -> Option<String> {
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

fn parse_hex(s: String) -> Option<Color> {
    let s = s.strip_prefix('#').unwrap_or(&s);
    if s.len() != 6 {
        return None;
    }
    Some(Color::Rgb(
        u8::from_str_radix(&s[0..2], 16).ok()?,
        u8::from_str_radix(&s[2..4], 16).ok()?,
        u8::from_str_radix(&s[4..6], 16).ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_line_yields_one_span() {
        let t = to_text("hello");
        assert_eq!(t.lines.len(), 1);
        assert_eq!(t.lines[0].spans.len(), 1);
        assert_eq!(t.lines[0].spans[0].content, "hello");
    }

    #[test]
    fn colored_span_carries_fg() {
        let t = to_text("<span foreground='#ff0000'>red</span>");
        let span = &t.lines[0].spans[0];
        assert_eq!(span.content, "red");
        assert_eq!(span.style.fg, Some(Color::Rgb(255, 0, 0)));
    }

    #[test]
    fn bold_modifier_applied() {
        let t = to_text("<span font_weight='bold' foreground='#00ff00'>x</span>");
        let span = &t.lines[0].spans[0];
        assert!(span.style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(span.style.fg, Some(Color::Rgb(0, 255, 0)));
    }

    #[test]
    fn multi_line_splits_correctly() {
        let t = to_text("a\nb");
        assert_eq!(t.lines.len(), 2);
        assert_eq!(t.lines[0].spans[0].content, "a");
        assert_eq!(t.lines[1].spans[0].content, "b");
    }

    #[test]
    fn nested_spans_stack_styles() {
        let t = to_text("<span foreground='#ff0000'>a<span foreground='#00ff00'>b</span>c</span>");
        let spans = &t.lines[0].spans;
        // Should produce 3 spans: red "a", green "b", red "c"
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].content, "a");
        assert_eq!(spans[0].style.fg, Some(Color::Rgb(255, 0, 0)));
        assert_eq!(spans[1].content, "b");
        assert_eq!(spans[1].style.fg, Some(Color::Rgb(0, 255, 0)));
        assert_eq!(spans[2].content, "c");
        assert_eq!(spans[2].style.fg, Some(Color::Rgb(255, 0, 0)));
    }
}
