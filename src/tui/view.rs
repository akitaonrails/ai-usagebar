//! TUI rendering — tabs + body + footer.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Tabs};

use crate::tui::app::{App, TabState};
use crate::vendor::VendorId;

pub fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // tabs
            Constraint::Min(1),     // body
            Constraint::Length(1),  // footer
        ])
        .split(f.area());

    draw_tabs(f, app, chunks[0]);
    draw_body(f, app, chunks[1]);
    draw_footer(f, app, chunks[2]);
}

fn vendor_label(id: VendorId) -> &'static str {
    match id {
        VendorId::Anthropic => "Claude",
        VendorId::Openai => "OpenAI",
        VendorId::Zai => "GLM (Z.AI)",
        VendorId::Openrouter => "OpenRouter",
    }
}

fn accent(theme: &crate::theme::Theme) -> Color {
    parse_hex(&theme.blue).unwrap_or(Color::Cyan)
}

fn parse_hex(s: &str) -> Option<Color> {
    let s = s.strip_prefix('#').unwrap_or(s);
    if s.len() != 6 {
        return None;
    }
    Some(Color::Rgb(
        u8::from_str_radix(&s[0..2], 16).ok()?,
        u8::from_str_radix(&s[2..4], 16).ok()?,
        u8::from_str_radix(&s[4..6], 16).ok()?,
    ))
}

fn draw_tabs(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let titles: Vec<Line> = app
        .vendors
        .iter()
        .map(|v| Line::from(vendor_label(*v)))
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" ai-usagebar ")
        .border_style(Style::default().fg(accent(&app.theme)));

    let tabs = Tabs::new(titles)
        .block(block)
        .select(app.active)
        .style(Style::default().fg(parse_hex(&app.theme.fg).unwrap_or(Color::Gray)))
        .highlight_style(
            Style::default()
                .fg(accent(&app.theme))
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )
        .divider(" · ");
    f.render_widget(tabs, area);
}

fn draw_body(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let body: Text = match app.tabs.get(app.active) {
        Some(TabState::Loading) => Text::from(vec![
            Line::from(""),
            Line::from(Span::styled(
                "  Loading…",
                Style::default().fg(Color::DarkGray),
            )),
        ]),
        Some(TabState::Ready { tooltip_pango, .. }) => {
            crate::tui::pango::to_text(tooltip_pango)
        }
        Some(TabState::Error(e)) => Text::from(vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("  Error: {e}"),
                Style::default().fg(Color::Red),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Press `r` to retry, `q` to quit.",
                Style::default().fg(Color::DarkGray),
            )),
        ]),
        None => Text::from(""),
    };

    let block = Block::default()
        .borders(Borders::LEFT | Borders::RIGHT)
        .border_style(Style::default().fg(accent(&app.theme)));
    let para = Paragraph::new(body).block(block);
    f.render_widget(para, area);
}

fn draw_footer(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let dim_color = parse_hex(&app.theme.dim).unwrap_or(Color::DarkGray);
    let text = Line::from(vec![
        Span::styled(" [Tab/h-l]", Style::default().fg(accent(&app.theme))),
        Span::styled(" switch · ", Style::default().fg(dim_color)),
        Span::styled("[r]", Style::default().fg(accent(&app.theme))),
        Span::styled(" refresh · ", Style::default().fg(dim_color)),
        Span::styled("[q]", Style::default().fg(accent(&app.theme))),
        Span::styled(" quit", Style::default().fg(dim_color)),
        Span::styled(
            format!("   ·   updated {}", app.last_refresh.format("%H:%M:%S")),
            Style::default().fg(dim_color),
        ),
    ]);
    f.render_widget(Paragraph::new(text), area);
}
