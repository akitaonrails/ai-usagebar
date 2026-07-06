//! TUI rendering — Bubble Tea-style shell + vendor detail card + footer.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui_bubbletea_components::{Help, KeyBinding, ListItem, SelectList};

use crate::format::local_time_hms;
use crate::tui::app::App;
use crate::tui::app::TabId;
use crate::tui::app::TabState;
use crate::tui::panels;
use crate::tui::style::bubble_theme;
use crate::vendor::VendorId;

const WIDE_LAYOUT_MIN_WIDTH: u16 = 86;
const SIDEBAR_WIDTH: u16 = 28;

pub fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // header
            Constraint::Min(1),    // nav + active panel
            Constraint::Length(1), // footer
        ])
        .split(f.area());

    draw_header(f, app, chunks[0]);
    draw_main(f, app, chunks[1]);
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
        VendorId::Deepseek => "DeepSeek",
    }
}

fn compact_vendor_label(id: VendorId) -> &'static str {
    match id {
        VendorId::Anthropic => "Claude",
        VendorId::Openai => "OpenAI",
        VendorId::Zai => "Z.AI",
        VendorId::Openrouter => "OpenRouter",
        VendorId::Deepseek => "DeepSeek",
    }
}

/// Tab label for the header/sidebar/detail title. A named Anthropic account
/// (#14/#17) appends its label, e.g. `Claude · work`; a plain vendor tab is
/// just the vendor name.
fn tab_label(tab: &TabId) -> String {
    match &tab.account {
        Some(acct) => format!("{} · {}", vendor_label(tab.vendor), acct),
        None => vendor_label(tab.vendor).to_string(),
    }
}

/// Compact variant for the narrow top-nav strip.
fn compact_tab_label(tab: &TabId) -> String {
    match &tab.account {
        Some(acct) => format!("{} · {}", compact_vendor_label(tab.vendor), acct),
        None => compact_vendor_label(tab.vendor).to_string(),
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let theme = bubble_theme(&app.theme);
    let block = theme.titled_block(" ai-usagebar ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let active = app
        .active_tab_id()
        .map(tab_label)
        .unwrap_or_else(|| "no vendor".to_string());
    let line = Line::from(vec![
        theme.accent("  Usage dashboard"),
        theme.muted(" · "),
        theme.span(format!("{} tabs", app.tabs_meta.len())),
        theme.muted(" · "),
        theme.span(format!("active {active}")),
        theme.muted(" · "),
        theme.muted(format!("last refresh {}", local_time_hms(app.last_refresh))),
    ]);
    f.render_widget(Paragraph::new(line), inner);
}

fn draw_main(f: &mut Frame, app: &App, area: Rect) {
    if area.width >= WIDE_LAYOUT_MIN_WIDTH {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(SIDEBAR_WIDTH), Constraint::Min(1)])
            .split(area);
        draw_sidebar(f, app, chunks[0]);
        draw_detail(f, app, chunks[1]);
    } else {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(1)])
            .split(area);
        draw_top_nav(f, app, chunks[0]);
        draw_detail(f, app, chunks[1]);
    }
}

fn draw_sidebar(f: &mut Frame, app: &App, area: Rect) {
    let theme = bubble_theme(&app.theme);
    let block = theme
        .titled_block(" vendors ")
        .border_style(theme.focused_border);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let items = app
        .tabs_meta
        .iter()
        .enumerate()
        .map(|(index, tab)| {
            ListItem::new(tab_label(tab)).description(tab_status(app.tabs.get(index)))
        })
        .collect::<Vec<_>>();
    let mut list = SelectList::new(items).theme(theme);
    list.select(Some(app.active));
    f.render_widget(&list, inner);
}

fn draw_top_nav(f: &mut Frame, app: &App, area: Rect) {
    let theme = bubble_theme(&app.theme);
    let block = theme
        .titled_block(" vendors ")
        .border_style(theme.focused_border);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut spans = vec![theme.muted(" ")];
    for (index, tab) in app.tabs_meta.iter().enumerate() {
        if index > 0 {
            spans.push(theme.muted("  "));
        }
        let selected = index == app.active;
        let marker = if selected {
            theme.symbols.selected
        } else {
            theme.symbols.bullet
        };
        let marker_style = if selected { theme.accent } else { theme.muted };
        let label_style = if selected { theme.selected } else { theme.text };
        spans.push(Span::styled(marker, marker_style));
        spans.push(theme.span(" "));
        spans.push(Span::styled(compact_tab_label(tab), label_style));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), inner);
}

fn draw_detail(f: &mut Frame, app: &App, area: Rect) {
    let theme = bubble_theme(&app.theme);
    let title = app
        .active_tab_id()
        .map(|tab| format!(" {} ", tab_label(tab)))
        .unwrap_or_else(|| " details ".to_string());
    let block = theme.titled_block(title);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(tab) = app.tabs.get(app.active) else {
        return;
    };
    let sections = panels::sections_for(tab, chrono::Utc::now(), 5);
    panels::render(f, inner, &app.theme, &sections);
}

fn tab_status(tab: Option<&TabState>) -> &'static str {
    match tab {
        Some(TabState::Loading) => "fetching",
        Some(TabState::Error(_)) => "error",
        Some(TabState::Ready(ready)) if ready.stale => "stale cache",
        Some(TabState::Ready(ready))
            if ready
                .last_error
                .as_ref()
                .is_some_and(|(code, _)| *code != 0) =>
        {
            "cached"
        }
        Some(TabState::Ready(_)) => "ready",
        None => "waiting",
    }
}

fn draw_footer(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    // The "updated HH:MM:SS" suffix used to live here, but it was
    // (a) redundant with the per-tab "Updated …" now right-aligned on the
    // title row of every panel, and (b) prone to getting cropped on narrow
    // 875x600 windows. Keep the footer to just the keybinding hints.
    let theme = bubble_theme(&app.theme);
    let help = Help::new([
        KeyBinding::with_keys(["tab", "h/l"], "switch"),
        KeyBinding::new("r", "refresh"),
        KeyBinding::new("R", "refresh all"),
        KeyBinding::new("s", "settings"),
        KeyBinding::with_keys(["q", "esc"], "quit"),
    ])
    .theme(theme);
    f.render_widget(&help, area);
}
