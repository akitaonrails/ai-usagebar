//! TUI rendering — tabs + body + footer.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Tabs};

use crate::tui::app::App;
use crate::tui::panels;
use crate::vendor::VendorId;

pub fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // tabs
            Constraint::Min(1),    // body
            Constraint::Length(1), // footer
        ])
        .split(f.area());

    draw_tabs(f, app, chunks[0]);
    draw_body(f, app, chunks[1]);
    draw_footer(f, app, chunks[2]);

    // Settings overlay sits on top — rendered last so it covers everything.
    if let Some(s) = &app.settings {
        crate::tui::settings::render(f, f.area(), s, &app.theme);
    }
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
    let (r, g, b) = crate::theme::parse_hex_rgb(s)?;
    Some(Color::Rgb(r, g, b))
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
    let block = Block::default()
        .borders(Borders::LEFT | Borders::RIGHT)
        .border_style(Style::default().fg(accent(&app.theme)));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(tab) = app.tabs.get(app.active) else {
        return;
    };
    let sections = panels::sections_for(tab, chrono::Utc::now(), 5);
    panels::render(f, inner, &app.theme, &sections);
}

fn draw_footer(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let dim_color = parse_hex(&app.theme.dim).unwrap_or(Color::DarkGray);
    let text = Line::from(vec![
        Span::styled(" [Tab/h-l]", Style::default().fg(accent(&app.theme))),
        Span::styled(" switch · ", Style::default().fg(dim_color)),
        Span::styled("[r]", Style::default().fg(accent(&app.theme))),
        Span::styled(" refresh · ", Style::default().fg(dim_color)),
        Span::styled("[s]", Style::default().fg(accent(&app.theme))),
        Span::styled(" settings · ", Style::default().fg(dim_color)),
        Span::styled("[q]", Style::default().fg(accent(&app.theme))),
        Span::styled(" quit", Style::default().fg(dim_color)),
        Span::styled(
            format!(
                "   ·   updated {}",
                crate::format::local_time_hms(app.last_refresh)
            ),
            Style::default().fg(dim_color),
        ),
    ]);
    f.render_widget(Paragraph::new(text), area);
}
