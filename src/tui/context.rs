//! TUI overlay for local Claude Code context-window sessions.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui_bubbletea_components::{Help, KeyBinding, ListItem, SelectList};

use crate::config::ContextLayout;
use crate::context::{ContextScan, ContextSession, ContextUsage};
use crate::format::local_time_hms;
use crate::pango::severity_for;
use crate::theme::Theme;
use crate::tui::panels::{self, Section};
use crate::tui::style::bubble_theme;

#[derive(Debug)]
pub struct ContextState {
    pub selected: usize,
    pub detail: bool,
    pub generation: u64,
    pub load: ContextLoad,
    pub layout: ContextLayout,
    selection_id: Option<String>,
}

#[derive(Debug)]
pub enum ContextLoad {
    Loading,
    Ready(ContextScan),
    Error(String),
}

impl Default for ContextState {
    fn default() -> Self {
        Self {
            selected: 0,
            detail: false,
            generation: 0,
            load: ContextLoad::Loading,
            layout: ContextLayout::default(),
            selection_id: None,
        }
    }
}

impl ContextState {
    /// Open with the configured starting layout; `v` cycles it afterwards.
    pub fn new(layout: ContextLayout) -> Self {
        Self {
            layout,
            ..Self::default()
        }
    }

    /// Begin a scan with an identity allocated by the long-lived host app.
    /// The host, rather than this disposable overlay, owns monotonicity across
    /// close/reopen cycles.
    pub fn begin_refresh(&mut self, generation: u64) {
        if let Some(selection_id) = self
            .selected_session()
            .map(|session| session.session_id.clone())
        {
            self.selection_id = Some(selection_id);
        }
        self.generation = generation;
        self.load = ContextLoad::Loading;
    }

    /// Ignore a result from an earlier `r` scan. The host also drops results
    /// after the overlay closes by having no state to apply them to.
    pub fn apply_scan(
        &mut self,
        generation: u64,
        result: std::result::Result<ContextScan, String>,
    ) -> bool {
        if generation != self.generation {
            return false;
        }
        self.load = match result {
            Ok(scan) => {
                self.selected = self
                    .selection_id
                    .take()
                    .and_then(|id| {
                        scan.sessions
                            .iter()
                            .position(|session| session.session_id == id)
                    })
                    .unwrap_or_else(|| self.selected.min(scan.sessions.len().saturating_sub(1)));
                if scan.sessions.is_empty() {
                    self.detail = false;
                }
                ContextLoad::Ready(scan)
            }
            Err(error) => {
                self.selection_id = None;
                self.detail = false;
                ContextLoad::Error(error)
            }
        };
        true
    }

    pub fn selected_session(&self) -> Option<&ContextSession> {
        match &self.load {
            ContextLoad::Ready(scan) => scan.sessions.get(self.selected),
            ContextLoad::Loading | ContextLoad::Error(_) => None,
        }
    }

    fn session_count(&self) -> usize {
        match &self.load {
            ContextLoad::Ready(scan) => scan.sessions.len(),
            ContextLoad::Loading | ContextLoad::Error(_) => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Continue,
    Close,
    Refresh,
    Quit,
}

pub fn handle_key(state: &mut ContextState, code: KeyCode, mods: KeyModifiers) -> Action {
    if matches!(code, KeyCode::Char('c')) && mods.contains(KeyModifiers::CONTROL) {
        return Action::Quit;
    }
    match code {
        KeyCode::Esc if state.detail => {
            state.detail = false;
            Action::Continue
        }
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('c') => Action::Close,
        KeyCode::Char('r') => Action::Refresh,
        KeyCode::Char('v') => {
            state.layout = state.layout.next();
            Action::Continue
        }
        KeyCode::Enter if state.selected_session().is_some() => {
            state.detail = true;
            Action::Continue
        }
        KeyCode::Up | KeyCode::Char('k') => {
            let count = state.session_count();
            if count > 0 {
                state.selected = (state.selected + count - 1) % count;
            }
            Action::Continue
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let count = state.session_count();
            if count > 0 {
                state.selected = (state.selected + 1) % count;
            }
            Action::Continue
        }
        KeyCode::Home => {
            state.selected = 0;
            Action::Continue
        }
        KeyCode::End => {
            let count = state.session_count();
            state.selected = count.saturating_sub(1);
            Action::Continue
        }
        _ => Action::Continue,
    }
}

/// Render the overlay docked into `area` (the view layer decides how much of
/// the dashboard body that is). Fills `area` like a vendor panel rather than
/// floating, so it reads as its own surface.
pub fn render(f: &mut Frame, area: Rect, state: &ContextState, theme: &Theme) {
    let bubble = bubble_theme(theme);
    f.render_widget(Clear, area);
    let block = bubble
        .titled_block(" Claude context ")
        .border_style(bubble.focused_border);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);
    match &state.load {
        ContextLoad::Loading => panels::render(
            f,
            chunks[0],
            theme,
            &[
                Section::Spacer,
                Section::Text {
                    label: String::new(),
                    value: "  Scanning recent Claude Code sessions…".into(),
                },
            ],
        ),
        ContextLoad::Error(error) => panels::render(
            f,
            chunks[0],
            theme,
            &[
                Section::Spacer,
                Section::Text {
                    label: "Error".into(),
                    value: error.clone(),
                },
                Section::Spacer,
                Section::Text {
                    label: String::new(),
                    value: "Press `r` to retry or `esc` to close.".into(),
                },
            ],
        ),
        ContextLoad::Ready(scan) if state.detail => {
            if let Some(session) = scan.sessions.get(state.selected) {
                panels::render(f, chunks[0], theme, &sections_for(session));
            }
        }
        ContextLoad::Ready(scan) => render_list(f, chunks[0], state, scan, theme),
    }

    let help = if state.detail {
        Help::new([
            KeyBinding::with_keys(["↑/↓", "j/k"], "session"),
            KeyBinding::new("r", "rescan"),
            KeyBinding::new("v", "layout"),
            KeyBinding::new("esc", "back"),
            KeyBinding::new("q", "close"),
        ])
    } else {
        Help::new([
            KeyBinding::with_keys(["↑/↓", "j/k"], "select"),
            KeyBinding::new("enter", "details"),
            KeyBinding::new("r", "rescan"),
            KeyBinding::new("v", "layout"),
            KeyBinding::with_keys(["q", "esc"], "close"),
        ])
    }
    .theme(bubble);
    f.render_widget(&help, chunks[1]);
}

fn render_list(f: &mut Frame, area: Rect, state: &ContextState, scan: &ContextScan, theme: &Theme) {
    let bubble = bubble_theme(theme);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(1)])
        .split(area);

    let showing = scan.sessions.len();
    let mut status = format!("  {showing} recent session");
    if showing != 1 {
        status.push('s');
    }
    if scan.discovered > showing {
        status.push_str(&format!(" · {} discovered", scan.discovered));
    }
    if scan.skipped > 0 {
        status.push_str(&format!(
            " · {} unreadable/invalid records skipped",
            scan.skipped
        ));
    }
    if scan.walk_capped {
        status.push_str(" · directory scan capped");
    }
    status.push_str(&format!(" · layout: {}", state.layout.label()));
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(status, bubble.muted),
            Span::styled(
                "\n  Input-only usage from bounded local transcript tails",
                bubble.muted,
            ),
        ])),
        chunks[0],
    );

    if scan.sessions.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  No Claude Code session transcripts found.",
                bubble.muted,
            ))),
            chunks[1],
        );
        return;
    }

    let items = scan
        .sessions
        .iter()
        .map(|session| {
            ListItem::new(session.display_name()).description(session_description(session))
        })
        .collect::<Vec<_>>();
    let mut list = SelectList::new(items).theme(bubble);
    list.select(Some(state.selected));
    f.render_widget(&list, chunks[1]);
}

fn session_description(session: &ContextSession) -> String {
    let usage = match session.usage {
        ContextUsage::Available {
            input_tokens,
            percent: Some(percent),
            ..
        } => format!("{percent}% · {} input tokens", format_tokens(input_tokens)),
        ContextUsage::Available { input_tokens, .. } => {
            format!(
                "{} input tokens · window unknown",
                format_tokens(input_tokens)
            )
        }
        ContextUsage::Compacted => "compacted · waiting for next response".into(),
        ContextUsage::Unknown => "context usage unavailable".into(),
    };
    let model = session.model.as_deref().unwrap_or("unknown model");
    format!(
        "{} · {model} · {usage} · {}",
        session.project,
        local_time_hms(session.modified_at)
    )
}

pub fn sections_for(session: &ContextSession) -> Vec<Section> {
    let mut sections = vec![Section::Title {
        left: session.display_name(),
        right: Some(format!("Updated {}", local_time_hms(session.modified_at))),
    }];
    match session.usage {
        ContextUsage::Available {
            input_tokens,
            window_tokens: Some(window_tokens),
            percent: Some(percent),
        } => {
            sections.push(Section::Spacer);
            sections.push(Section::Metric {
                label: "Input context".into(),
                pct: percent.min(100),
                severity: severity_for(i32::from(percent)),
                value_label: format!(
                    "{percent}% · {} / {} tokens",
                    format_tokens(input_tokens),
                    format_tokens(window_tokens)
                ),
                footnote: "Latest API input; output tokens are intentionally excluded".into(),
            });
        }
        ContextUsage::Available { input_tokens, .. } => {
            sections.push(Section::Spacer);
            sections.push(Section::Text {
                label: "Input context".into(),
                value: format!(
                    "{} tokens · window size is not configured",
                    format_tokens(input_tokens)
                ),
            });
        }
        ContextUsage::Compacted => {
            sections.push(Section::Spacer);
            sections.push(Section::Text {
                label: "Input context".into(),
                value: "Compacted · waiting for the next assistant response".into(),
            });
        }
        ContextUsage::Unknown => {
            sections.push(Section::Spacer);
            sections.push(Section::Text {
                label: "Input context".into(),
                value: "Unavailable in the bounded transcript tail".into(),
            });
        }
    }
    sections.push(Section::Spacer);
    sections.push(Section::Text {
        label: "Project".into(),
        value: session.project.clone(),
    });
    sections.push(Section::Text {
        label: "Model".into(),
        value: session.model.clone().unwrap_or_else(|| "unknown".into()),
    });
    sections.push(Section::Text {
        label: "Session".into(),
        value: session.session_id.clone(),
    });
    sections
}

fn format_tokens(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

pub use ratatui::crossterm::event::{KeyCode, KeyModifiers};

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    fn session(id: &str, usage: ContextUsage) -> ContextSession {
        ContextSession {
            session_id: id.into(),
            title: None,
            project: "project".into(),
            model: Some("claude-test".into()),
            modified_at: Utc.with_ymd_and_hms(2026, 7, 20, 12, 0, 0).unwrap(),
            usage,
        }
    }

    fn scan(ids: &[&str]) -> ContextScan {
        ContextScan {
            sessions: ids
                .iter()
                .map(|id| session(id, ContextUsage::Unknown))
                .collect(),
            discovered: ids.len(),
            skipped: 0,
            walk_capped: false,
        }
    }

    #[test]
    fn stale_scan_result_is_discarded() {
        let mut state = ContextState::default();
        state.begin_refresh(1);
        let first = 1;
        state.begin_refresh(2);
        let current = 2;
        assert!(!state.apply_scan(first, Ok(scan(&["old"]))));
        assert!(state.apply_scan(current, Ok(scan(&["new"]))));
        assert_eq!(state.selected_session().unwrap().session_id, "new");
    }

    #[test]
    fn a_result_from_a_closed_overlay_cannot_land_after_reopen() {
        let mut reopened = ContextState::default();
        reopened.begin_refresh(2);
        assert!(!reopened.apply_scan(1, Ok(scan(&["closed-overlay"]))));
        assert!(matches!(reopened.load, ContextLoad::Loading));
    }

    #[test]
    fn rescan_preserves_the_selected_session_across_reordering() {
        let mut state = ContextState::default();
        let generation = 1;
        state.begin_refresh(generation);
        state.apply_scan(generation, Ok(scan(&["one", "two"])));
        state.selected = 1;
        state.detail = true;

        let generation = 2;
        state.begin_refresh(generation);
        state.apply_scan(generation, Ok(scan(&["two", "one"])));
        assert_eq!(state.selected, 0);
        assert_eq!(state.selected_session().unwrap().session_id, "two");
        assert!(state.detail);
    }

    #[test]
    fn list_and_detail_navigation_are_bounded_and_wrap() {
        let mut state = ContextState::default();
        let generation = 1;
        state.begin_refresh(generation);
        state.apply_scan(generation, Ok(scan(&["one", "two"])));

        assert_eq!(
            handle_key(&mut state, KeyCode::Up, KeyModifiers::NONE),
            Action::Continue
        );
        assert_eq!(state.selected, 1);
        handle_key(&mut state, KeyCode::Enter, KeyModifiers::NONE);
        assert!(state.detail);
        assert_eq!(
            handle_key(&mut state, KeyCode::Esc, KeyModifiers::NONE),
            Action::Continue
        );
        assert!(!state.detail);
        assert_eq!(
            handle_key(&mut state, KeyCode::Esc, KeyModifiers::NONE),
            Action::Close
        );
    }

    #[test]
    fn v_cycles_the_three_layouts_and_wraps() {
        let mut state = ContextState::new(ContextLayout::Full);
        state.detail = true;
        for expected in [
            ContextLayout::Split,
            ContextLayout::Bottom,
            ContextLayout::Full,
        ] {
            assert_eq!(
                handle_key(&mut state, KeyCode::Char('v'), KeyModifiers::NONE),
                Action::Continue
            );
            assert_eq!(state.layout, expected);
        }
        assert!(
            state.detail,
            "changing layout must not leave the detail view"
        );
    }

    #[test]
    fn refresh_and_global_quit_are_explicit_actions() {
        let mut state = ContextState::default();
        assert_eq!(
            handle_key(&mut state, KeyCode::Char('r'), KeyModifiers::NONE),
            Action::Refresh
        );
        assert_eq!(
            handle_key(&mut state, KeyCode::Char('c'), KeyModifiers::CONTROL),
            Action::Quit
        );
    }

    #[test]
    fn detail_sections_use_existing_severity_and_preserve_unknown_states() {
        let available = session(
            "available",
            ContextUsage::Available {
                input_tokens: 180_000,
                window_tokens: Some(200_000),
                percent: Some(90),
            },
        );
        let sections = sections_for(&available);
        assert!(sections.iter().any(|section| matches!(
            section,
            Section::Metric {
                severity: crate::pacing::PaceSeverity::Critical,
                value_label,
                ..
            } if value_label.contains("180,000 / 200,000")
        )));

        let compacted = sections_for(&session("compacted", ContextUsage::Compacted));
        assert!(compacted.iter().any(|section| matches!(
            section,
            Section::Text { value, .. } if value.contains("waiting for the next assistant")
        )));
    }

    #[test]
    fn token_formatting_is_grouped_without_locale_state() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1_234_567), "1,234,567");
    }

    #[test]
    fn list_and_detail_render_at_a_common_24_row_terminal() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut state = ContextState::default();
        let generation = 1;
        state.begin_refresh(generation);
        state.apply_scan(generation, Ok(scan(&["one", "two"])));
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal
            .draw(|frame| render(frame, frame.area(), &state, &Theme::default()))
            .unwrap();

        state.detail = true;
        terminal
            .draw(|frame| render(frame, frame.area(), &state, &Theme::default()))
            .unwrap();
    }
}
