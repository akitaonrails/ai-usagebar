//! TUI rendering — Bubble Tea-style shell + vendor detail card + footer.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui_bubbletea_components::{Help, KeyBinding, ListItem, SelectList};

use crate::format::local_time_hms;
use crate::tui::app::App;
use crate::tui::app::TabId;
use crate::tui::app::TabState;
use crate::tui::panels;
use crate::tui::style::{bubble_theme, color, severity_color};
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
    draw_body(f, app, chunks[1]);
    draw_footer(f, app, chunks[2]);

    // Settings still floats on top of everything.
    if let Some(s) = &app.settings {
        crate::tui::settings::render(f, f.area(), s, &app.theme);
    }
}

/// The dashboard body, plus the context view docked into it when open: `full`
/// takes it over, `split` sits beside it, `bottom` sits below it.
fn draw_body(f: &mut Frame, app: &App, area: Rect) {
    use crate::config::ContextLayout;
    use crate::tui::context;

    let Some(state) = &app.context else {
        draw_main(f, app, area);
        return;
    };
    match state.layout {
        ContextLayout::Full => context::render(f, area, state, &app.theme),
        ContextLayout::Split => {
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(area);
            draw_main(f, app, cols[0]);
            context::render(f, cols[1], state, &app.theme);
        }
        ContextLayout::Bottom => {
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(area);
            draw_main(f, app, rows[0]);
            context::render(f, rows[1], state, &app.theme);
        }
    }
}

fn vendor_label(id: VendorId) -> &'static str {
    match id {
        VendorId::Anthropic => "Claude",
        VendorId::AnthropicApi => "Anthropic API",
        VendorId::Openai => "OpenAI",
        VendorId::Zai => "GLM (Z.AI)",
        VendorId::Openrouter => "OpenRouter",
        VendorId::Deepseek => "DeepSeek",
        VendorId::Kimi => "Kimi",
        VendorId::Kilo => "Kilo",
        VendorId::Novita => "Novita",
        VendorId::Moonshot => "Moonshot",
        VendorId::Grok => "Grok",
        VendorId::Antigravity => "Antigravity",
        VendorId::Cursor => "Cursor",
        VendorId::Minimax => "MiniMax",
    }
}

fn compact_vendor_label(id: VendorId) -> &'static str {
    match id {
        VendorId::Anthropic => "Claude",
        VendorId::AnthropicApi => "Anthropic API",
        VendorId::Openai => "OpenAI",
        VendorId::Zai => "Z.AI",
        VendorId::Openrouter => "OpenRouter",
        VendorId::Deepseek => "DeepSeek",
        VendorId::Kimi => "Kimi",
        VendorId::Kilo => "Kilo",
        VendorId::Novita => "Novita",
        VendorId::Moonshot => "Moonshot",
        VendorId::Grok => "Grok",
        VendorId::Antigravity => "Antigravity",
        VendorId::Cursor => "Cursor",
        VendorId::Minimax => "MiniMax",
    }
}

/// Tab label for the header/sidebar/detail title. A named Anthropic account
/// (#14/#17) appends its label, e.g. `Claude · work`; a plain vendor tab is
/// just the vendor name.
fn tab_label(tab: &TabId) -> String {
    let label = match &tab.account {
        Some(acct) => format!("{} · {}", vendor_label(tab.vendor), acct),
        None => vendor_label(tab.vendor).to_string(),
    };
    crate::display::sanitize_untrusted_field(&label)
}

/// Compact variant for the narrow top-nav strip.
fn compact_tab_label(tab: &TabId) -> String {
    let label = match &tab.account {
        Some(acct) => format!("{} · {}", compact_vendor_label(tab.vendor), acct),
        None => compact_vendor_label(tab.vendor).to_string(),
    };
    crate::display::sanitize_untrusted_field(&label)
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let theme = bubble_theme(&app.theme);
    let block = theme.titled_block(" ai-usagebar ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let active = if app.overview {
        "Overview".to_string()
    } else {
        app.active_tab_id()
            .map(tab_label)
            .unwrap_or_else(|| "no vendor".to_string())
    };
    let mut spans = vec![
        theme.accent("  Usage dashboard"),
        theme.muted(" · "),
        theme.span(format!("{} tabs", app.tabs_meta.len())),
        theme.muted(" · "),
        theme.span(format!("active {active}")),
        theme.muted(" · "),
        theme.muted(header_refresh_text(app)),
    ];
    if app.is_refreshing() {
        // Keep the marker after the timestamp so refresh activity does not
        // shift the stable header text left or right.
        spans.push(theme.muted("  ↻"));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), inner);
}

/// The header's refresh stamp, read from the ACTIVE tab's own `fetched_at`.
///
/// This used to be a single `App::last_refresh` bumped by whichever vendor
/// finished last, so a tab that was still loading — or had failed minutes ago —
/// advertised a sibling's success as its own. A tab with no landed response has
/// no time to show, so it gets the same `—` the panels use for an unknown
/// fetched-at rather than a borrowed or invented one.
fn header_refresh_text(app: &App) -> String {
    let fetched_at = match app.tabs.get(app.active) {
        Some(TabState::Ready(ready)) => ready.fetched_at,
        _ => None,
    };
    match fetched_at {
        Some(at) => format!("last refresh {}", local_time_hms(at)),
        None => "last refresh —".to_string(),
    }
}

fn draw_main(f: &mut Frame, app: &App, area: Rect) {
    use crate::config::VendorBoxStyle;

    match app.vendor_box {
        VendorBoxStyle::Sidebar => {
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
        VendorBoxStyle::Navbar => {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(3), Constraint::Min(1)])
                .split(area);
            draw_top_nav(f, app, chunks[0]);
            draw_detail(f, app, chunks[1]);
        }
        VendorBoxStyle::None => {
            draw_detail(f, app, area);
        }
    }
}

fn draw_sidebar(f: &mut Frame, app: &App, area: Rect) {
    let theme = bubble_theme(&app.theme);
    let block = theme
        .titled_block(" vendors ")
        .border_style(theme.focused_border);
    let inner = block.inner(area);
    f.render_widget(block, area);

    // The Overview is the virtual first entry, before the per-vendor tabs.
    let mut items = vec![ListItem::new("Overview").description("all vendors")];
    items.extend(app.tabs_meta.iter().enumerate().map(|(index, tab)| {
        ListItem::new(tab_label(tab)).description(tab_status(app.tabs.get(index)))
    }));
    let mut list = SelectList::new(items).theme(theme);
    list.select(Some(if app.overview { 0 } else { app.active + 1 }));
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
    // Overview entry first, then each vendor tab.
    let push_entry = |spans: &mut Vec<Span>, first: bool, selected: bool, label: String| {
        if !first {
            spans.push(theme.muted("  "));
        }
        let marker = if selected {
            theme.symbols.selected
        } else {
            theme.symbols.bullet
        };
        let marker_style = if selected { theme.accent } else { theme.muted };
        let label_style = if selected { theme.selected } else { theme.text };
        spans.push(Span::styled(marker, marker_style));
        spans.push(theme.span(" "));
        spans.push(Span::styled(label, label_style));
    };
    push_entry(&mut spans, true, app.overview, "Overview".to_string());
    for (index, tab) in app.tabs_meta.iter().enumerate() {
        let selected = !app.overview && index == app.active;
        push_entry(&mut spans, false, selected, compact_tab_label(tab));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), inner);
}

fn draw_detail(f: &mut Frame, app: &App, area: Rect) {
    let theme = bubble_theme(&app.theme);
    let title = if app.overview {
        " Overview ".to_string()
    } else {
        app.active_tab_id()
            .map(|tab| format!(" {} ", tab_label(tab)))
            .unwrap_or_else(|| " details ".to_string())
    };
    let block = theme.titled_block(title);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.overview {
        draw_overview(f, app, inner);
        return;
    }

    let Some(tab) = app.tabs.get(app.active) else {
        return;
    };
    let sections = panels::sections_for(tab, chrono::Utc::now(), 5);
    panels::render(f, inner, &app.theme, &sections);
}

/// Render the Overview: one compact row per configured vendor — its name, a
/// plan/tier sub-label, and its key metric cells colored by severity.
/// Width of the per-row mini bar in the Overview (cells).
const OVERVIEW_BAR_W: usize = 12;

fn draw_overview(f: &mut Frame, app: &App, area: Rect) {
    let theme = bubble_theme(&app.theme);
    let idxs = app.overview_tabs();
    if idxs.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(theme.muted("  No vendors to summarize."))),
            area,
        );
        return;
    }
    // Left-align the metric columns by padding the vendor-name column to the
    // widest name (bounded so one long account label can't blow out the layout).
    let name_w = idxs
        .iter()
        .map(|&i| tab_label(&app.tabs_meta[i]).chars().count())
        .max()
        .unwrap_or(6)
        .clamp(6, 22);

    let mut lines: Vec<Line> = Vec::new();
    for &i in &idxs {
        let name = tab_label(&app.tabs_meta[i]);
        let pad = name_w.saturating_sub(name.chars().count());
        let mut spans = vec![
            Span::styled(name, theme.text),
            theme.span(" ".repeat(pad + 2)),
        ];
        match app.tabs.get(i) {
            Some(TabState::Ready(r)) => {
                // Mini bar for the vendor's headline metric, mirroring the
                // menu-bar overview; balance-only vendors have none.
                if let Some(p) = panels::headline_pct(&r.snapshot) {
                    let filled = (p.clamp(0, 100) as usize * OVERVIEW_BAR_W).div_ceil(100);
                    let sev_color =
                        severity_color(&app.theme, &theme, crate::pango::severity_for(p));
                    spans.push(Span::styled(
                        "█".repeat(filled),
                        Style::default().fg(sev_color),
                    ));
                    let empty =
                        color(&app.theme.bar_empty).unwrap_or(theme.palette.selected_background);
                    spans.push(Span::styled(
                        "░".repeat(OVERVIEW_BAR_W - filled),
                        Style::default().fg(empty),
                    ));
                    spans.push(theme.span("  "));
                }
                let (plan, cells) = panels::compact_cells(&r.snapshot);
                if !plan.is_empty() {
                    spans.push(theme.muted(format!("{plan}  ")));
                }
                for (j, (text, sev)) in cells.iter().enumerate() {
                    if j > 0 {
                        spans.push(theme.span("  "));
                    }
                    let color = severity_color(&app.theme, &theme, *sev);
                    spans.push(Span::styled(text.clone(), Style::default().fg(color)));
                }
                if r.stale {
                    spans.push(theme.muted("  ⏸"));
                }
            }
            Some(TabState::Error(_)) => spans.push(Span::styled(
                "error",
                Style::default().fg(theme.palette.error),
            )),
            Some(TabState::Loading) | None => spans.push(theme.muted("fetching…")),
        }
        lines.push(Line::from(spans));
        lines.push(Line::from(""));
    }
    f.render_widget(Paragraph::new(lines), area);
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
    let mut bindings = vec![
        KeyBinding::with_keys(["tab", "h/l"], "switch"),
        KeyBinding::new("r", "refresh"),
        KeyBinding::new("R", "refresh all"),
        KeyBinding::new("s", "settings"),
    ];
    if app.context_enabled {
        bindings.push(KeyBinding::new("c", "context"));
    }
    bindings.push(KeyBinding::with_keys(["q", "esc"], "quit"));
    let help = Help::new(bindings).theme(theme);
    f.render_widget(&help, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;
    use crate::tui::app::ReadyTab;
    use crate::usage::{OpenRouterSnapshot, VendorSnapshot};
    use chrono::{DateTime, TimeZone, Utc};

    fn ready_at(fetched_at: Option<DateTime<Utc>>) -> TabState {
        TabState::Ready(Box::new(ReadyTab {
            snapshot: VendorSnapshot::Openrouter(OpenRouterSnapshot {
                label: "test".into(),
                total_credits: 0.0,
                total_usage: 0.0,
                usage_daily: 0.0,
                usage_weekly: 0.0,
                usage_monthly: 0.0,
                is_free_tier: false,
                limit: None,
                limit_remaining: None,
            }),
            stale: false,
            last_error: None,
            fetched_at,
        }))
    }

    // `App::with_theme(.., Theme::default())` rather than `App::new`, which
    // would read the real Omarchy theme file + `$HOME`. The header stamp under
    // test is theme-agnostic.
    fn app_with(tabs: Vec<TabState>) -> App {
        let mut app = App::with_theme(
            vec![
                TabId::vendor(VendorId::Anthropic),
                TabId::vendor(VendorId::Openrouter),
            ],
            Theme::default(),
        );
        app.tabs = tabs;
        app
    }

    #[test]
    fn header_refresh_follows_the_active_tab() {
        let anthropic_at = Utc.with_ymd_and_hms(2026, 5, 23, 12, 0, 0).unwrap();
        let openrouter_at = Utc.with_ymd_and_hms(2026, 5, 23, 9, 30, 0).unwrap();
        let mut app = app_with(vec![
            ready_at(Some(anthropic_at)),
            ready_at(Some(openrouter_at)),
        ]);

        // Compare against the formatting helper, not a literal, so the test
        // doesn't depend on the machine's timezone.
        let anthropic_header = format!("last refresh {}", local_time_hms(anthropic_at));
        let openrouter_header = format!("last refresh {}", local_time_hms(openrouter_at));
        assert_ne!(anthropic_header, openrouter_header);

        assert_eq!(header_refresh_text(&app), anthropic_header);
        app.next_tab();
        assert_eq!(header_refresh_text(&app), openrouter_header);
    }

    #[test]
    fn header_refresh_is_dash_when_active_tab_never_fetched() {
        // The sibling's successful fetch is exactly what the old global clock
        // would have displayed here.
        let sibling = ready_at(Some(Utc.with_ymd_and_hms(2026, 5, 23, 12, 0, 0).unwrap()));
        let mut app = app_with(vec![TabState::Loading, sibling]);
        assert_eq!(header_refresh_text(&app), "last refresh —");

        app.tabs[0] = TabState::Error("401 Unauthorized".into());
        assert_eq!(header_refresh_text(&app), "last refresh —");
    }

    #[test]
    fn header_refresh_is_dash_when_ready_tab_has_no_fetched_at() {
        // Ready but the cache never reported an age — show nothing rather than
        // passing off "now" as a response time.
        let app = app_with(vec![ready_at(None), TabState::Loading]);
        assert_eq!(header_refresh_text(&app), "last refresh —");
    }

    #[test]
    fn header_adds_refresh_marker_after_the_timestamp() {
        let tab = TabId::vendor(VendorId::Anthropic);
        let at = Utc.with_ymd_and_hms(2026, 5, 23, 12, 0, 0).unwrap();
        let mut app = app_with(vec![ready_at(Some(at))]);
        app.tabs_meta[0] = tab.clone();
        app.begin_refresh(&tab);

        let out = body_text(&app);
        let stamp = format!("last refresh {}  ↻", local_time_hms(at));
        assert!(out.contains(&stamp), "{out}");
    }

    fn app_with_context(layout: crate::config::ContextLayout) -> App {
        let mut app = app_with(vec![TabState::Loading, TabState::Loading]);
        app.context_enabled = true;
        app.context = Some({
            let mut state = crate::tui::context::ContextState::new(layout);
            state.apply_scan(0, Err("scan error".into()));
            state
        });
        app
    }

    fn body_text(app: &App) -> String {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut terminal = Terminal::new(TestBackend::new(160, 40)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<Vec<_>>()
            .concat()
    }

    #[test]
    fn full_layout_takes_the_body_and_hides_the_vendor_sidebar() {
        use crate::config::ContextLayout;
        let out = body_text(&app_with_context(ContextLayout::Full));
        assert!(out.contains("Claude context"), "{out}");
        assert!(
            !out.contains("vendors"),
            "full layout must not leave the dashboard around it"
        );
    }

    #[test]
    fn split_and_bottom_layouts_keep_the_dashboard_visible() {
        use crate::config::ContextLayout;
        for layout in [ContextLayout::Split, ContextLayout::Bottom] {
            let out = body_text(&app_with_context(layout));
            assert!(out.contains("Claude context"), "{layout:?}: {out}");
            assert!(out.contains("vendors"), "{layout:?}: {out}");
        }
    }

    #[test]
    fn context_footer_hint_is_visible_only_when_the_feature_is_enabled() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        fn rendered(mut app: App, enabled: bool) -> String {
            app.context_enabled = enabled;
            let mut terminal = Terminal::new(TestBackend::new(160, 24)).unwrap();
            terminal.draw(|frame| draw(frame, &app)).unwrap();
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<Vec<_>>()
                .concat()
        }

        let disabled = rendered(app_with(vec![TabState::Loading, TabState::Loading]), false);
        assert!(!disabled.contains("context"));

        let enabled = rendered(app_with(vec![TabState::Loading, TabState::Loading]), true);
        assert!(enabled.contains("context"));
    }

    /// Renders `draw_main` alone (no header/footer) into `width x height` and
    /// returns it as one string per row, so a test can inspect which titled
    /// blocks landed on which row — that's what tells a horizontal sidebar
    /// split (both titles on row 0) apart from a stacked navbar (nav title on
    /// row 0, detail title only once the 3-row nav strip ends).
    fn main_rows(app: &App, width: u16, height: u16) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| draw_main(frame, app, frame.area()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|row| {
                (0..width)
                    .map(|col| buffer[(col, row)].symbol())
                    .collect::<Vec<_>>()
                    .concat()
            })
            .collect()
    }

    #[test]
    fn vendor_box_sidebar_splits_horizontally_on_wide_terminals() {
        let mut app = app_with(vec![TabState::Loading, TabState::Loading]);
        app.overview = true;
        let rows = main_rows(&app, 160, 24);
        // Sidebar and detail panel sit side by side, so their titles share row 0.
        assert!(rows[0].contains("vendors"), "{:?}", rows[0]);
        assert!(rows[0].contains("Overview"), "{:?}", rows[0]);
    }

    #[test]
    fn vendor_box_sidebar_falls_back_to_top_nav_on_narrow_terminals() {
        let mut app = app_with(vec![TabState::Loading, TabState::Loading]);
        app.overview = true;
        let rows = main_rows(&app, 60, 24);
        assert!(rows[0].contains("vendors"), "{:?}", rows[0]);
        // Stacked layout: the detail panel's own title lands below the 3-row
        // nav strip, not sharing row 0 with it.
        assert!(!rows[0].contains(" Overview "), "{:?}", rows[0]);
        assert!(
            rows.iter().any(|r| r.contains(" Overview ")),
            "detail title missing entirely: {rows:?}"
        );
    }

    #[test]
    fn vendor_box_navbar_forces_top_nav_even_on_wide_terminals() {
        let mut app = app_with(vec![TabState::Loading, TabState::Loading]);
        app.overview = true;
        app.vendor_box = crate::config::VendorBoxStyle::Navbar;
        let rows = main_rows(&app, 160, 24);
        assert!(rows[0].contains("vendors"), "{:?}", rows[0]);
        assert!(
            !rows[0].contains(" Overview "),
            "navbar must stack, not sit beside the detail panel: {:?}",
            rows[0]
        );
    }

    #[test]
    fn vendor_box_none_hides_navigation_and_uses_full_width() {
        let mut app = app_with(vec![TabState::Loading, TabState::Loading]);
        app.overview = true;
        app.vendor_box = crate::config::VendorBoxStyle::None;
        let rows = main_rows(&app, 160, 24);
        assert!(
            !rows.iter().any(|r| r.contains("vendors")),
            "vendor nav must be fully hidden: {rows:?}"
        );
        assert!(rows[0].contains(" Overview "), "{:?}", rows[0]);
    }
}
