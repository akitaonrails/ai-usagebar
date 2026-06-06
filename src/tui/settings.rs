//! Settings overlay — opened from the TUI by pressing `s`. Lets the user
//! pick the primary vendor and set Z.AI / OpenRouter API keys without
//! editing config.toml by hand.
//!
//! Persistence uses `toml_edit` so the existing config keeps its comments,
//! whitespace, and unrelated fields. Files with inline keys are atomically
//! written and `chmod 600`ed.

use std::path::{Path, PathBuf};

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use toml_edit::{DocumentMut, value};

use crate::config::Config;
use crate::error::{AppError, Result};
use crate::theme::Theme;
use crate::vendor::VendorId;

/// Which input field has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Primary,
    ZaiKey,
    OpenrouterKey,
    DeepseekKey,
    SaveButton,
}

impl Focus {
    pub fn next(self) -> Self {
        match self {
            Focus::Primary => Focus::ZaiKey,
            Focus::ZaiKey => Focus::OpenrouterKey,
            Focus::OpenrouterKey => Focus::DeepseekKey,
            Focus::DeepseekKey => Focus::SaveButton,
            Focus::SaveButton => Focus::Primary,
        }
    }
    pub fn prev(self) -> Self {
        match self {
            Focus::Primary => Focus::SaveButton,
            Focus::ZaiKey => Focus::Primary,
            Focus::OpenrouterKey => Focus::ZaiKey,
            Focus::DeepseekKey => Focus::OpenrouterKey,
            Focus::SaveButton => Focus::DeepseekKey,
        }
    }
}

/// Per-field text-input state — cursor + buffer + reveal flag.
#[derive(Debug, Clone, Default)]
pub struct KeyInput {
    pub buf: String,
    /// Char-index cursor position (0..=buf.chars().count()).
    pub cursor: usize,
    /// When true, the field renders the actual characters; otherwise `•`.
    pub revealed: bool,
    /// True after the user has typed/edited; only then does save write
    /// the value back (avoids clobbering an existing key with the empty
    /// placeholder the user opened the dialog with).
    pub dirty: bool,
}

impl KeyInput {
    pub fn from_config(initial: Option<&str>) -> Self {
        let buf = initial.unwrap_or("").to_string();
        let cursor = buf.chars().count();
        Self {
            buf,
            cursor,
            revealed: false,
            dirty: false,
        }
    }

    pub fn insert_char(&mut self, c: char) {
        let byte_idx = self.char_to_byte(self.cursor);
        self.buf.insert(byte_idx, c);
        self.cursor += 1;
        self.dirty = true;
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prev_byte = self.char_to_byte(self.cursor - 1);
        let cur_byte = self.char_to_byte(self.cursor);
        self.buf.replace_range(prev_byte..cur_byte, "");
        self.cursor -= 1;
        self.dirty = true;
    }

    pub fn delete(&mut self) {
        let n = self.buf.chars().count();
        if self.cursor >= n {
            return;
        }
        let cur_byte = self.char_to_byte(self.cursor);
        let next_byte = self.char_to_byte(self.cursor + 1);
        self.buf.replace_range(cur_byte..next_byte, "");
        self.dirty = true;
    }

    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }
    pub fn move_right(&mut self) {
        if self.cursor < self.buf.chars().count() {
            self.cursor += 1;
        }
    }
    pub fn move_home(&mut self) {
        self.cursor = 0;
    }
    pub fn move_end(&mut self) {
        self.cursor = self.buf.chars().count();
    }
    pub fn toggle_reveal(&mut self) {
        self.revealed = !self.revealed;
    }

    /// Render for display — bullets when masked, raw chars when revealed.
    pub fn display(&self) -> String {
        if self.revealed {
            self.buf.clone()
        } else {
            "•".repeat(self.buf.chars().count())
        }
    }

    fn char_to_byte(&self, char_idx: usize) -> usize {
        self.buf
            .char_indices()
            .map(|(b, _)| b)
            .chain(std::iter::once(self.buf.len()))
            .nth(char_idx)
            .unwrap_or(self.buf.len())
    }
}

/// Mutable state of the overlay while open.
#[derive(Debug, Clone)]
pub struct SettingsState {
    pub focus: Focus,
    pub primary: VendorId,
    pub zai: KeyInput,
    pub openrouter: KeyInput,
    pub deepseek: KeyInput,
    /// One-line status displayed in the footer ("Saved", "Error: ...", "").
    pub status: String,
}

impl SettingsState {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            focus: Focus::Primary,
            primary: cfg.ui.primary.unwrap_or(VendorId::Anthropic),
            zai: KeyInput::from_config(cfg.zai.api_key.as_deref()),
            openrouter: KeyInput::from_config(cfg.openrouter.api_key.as_deref()),
            deepseek: KeyInput::from_config(cfg.deepseek.api_key.as_deref()),
            status: String::new(),
        }
    }
}

/// What the key handler asks the host app to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Stay open, keep listening for keys.
    Continue,
    /// Close the overlay (discard or save already happened).
    Close,
    /// Save just succeeded — caller should refresh affected vendors.
    SavedAndClose,
}

/// Permission note appended to the "saved" status line. The overlay `chmod
/// 600`s the file on Unix; Windows has no such step, so the note is empty there
/// (keeps the message platform-honest).
#[cfg(unix)]
const PERMS_NOTE: &str = " (chmod 600)";
#[cfg(not(unix))]
const PERMS_NOTE: &str = "";

/// Status line after a successful save: the platform-resolved config path plus
/// the platform-appropriate permission note.
fn saved_status() -> String {
    format!(
        "saved to {}{}",
        crate::config::config_path_hint(),
        PERMS_NOTE
    )
}

/// Key map. Returns the action to perform after the keypress.
pub fn handle_key(state: &mut SettingsState, code: KeyCode, mods: KeyModifiers) -> Action {
    // Esc always closes without saving.
    if matches!(code, KeyCode::Esc) {
        return Action::Close;
    }
    // Ctrl-S triggers save from any field.
    if matches!(code, KeyCode::Char('s')) && mods.contains(KeyModifiers::CONTROL) {
        return match save_to_config_default(state) {
            Ok(()) => {
                state.status = saved_status();
                Action::SavedAndClose
            }
            Err(e) => {
                state.status = format!("save failed: {e}");
                Action::Continue
            }
        };
    }
    // Ctrl-V toggles reveal on the focused key field.
    if matches!(code, KeyCode::Char('v')) && mods.contains(KeyModifiers::CONTROL) {
        match state.focus {
            Focus::ZaiKey => state.zai.toggle_reveal(),
            Focus::OpenrouterKey => state.openrouter.toggle_reveal(),
            Focus::DeepseekKey => state.deepseek.toggle_reveal(),
            _ => {}
        }
        return Action::Continue;
    }

    // Field navigation: Tab/Shift-Tab and Up/Down.
    match code {
        KeyCode::Tab => {
            state.focus = state.focus.next();
            return Action::Continue;
        }
        KeyCode::BackTab => {
            state.focus = state.focus.prev();
            return Action::Continue;
        }
        KeyCode::Down => {
            state.focus = state.focus.next();
            return Action::Continue;
        }
        KeyCode::Up => {
            state.focus = state.focus.prev();
            return Action::Continue;
        }
        _ => {}
    }

    // Field-specific handling.
    match state.focus {
        Focus::Primary => handle_primary(state, code),
        Focus::ZaiKey => handle_input(&mut state.zai, code),
        Focus::OpenrouterKey => handle_input(&mut state.openrouter, code),
        Focus::DeepseekKey => handle_input(&mut state.deepseek, code),
        Focus::SaveButton => {
            if matches!(code, KeyCode::Enter) {
                return match save_to_config_default(state) {
                    Ok(()) => {
                        state.status = saved_status();
                        Action::SavedAndClose
                    }
                    Err(e) => {
                        state.status = format!("save failed: {e}");
                        Action::Continue
                    }
                };
            }
        }
    }
    Action::Continue
}

fn handle_primary(state: &mut SettingsState, code: KeyCode) {
    // Left/Right cycles the primary-vendor radio.
    let all = VendorId::all();
    let idx = all.iter().position(|v| *v == state.primary).unwrap_or(0) as i32;
    let len = all.len() as i32;
    let step = match code {
        KeyCode::Left => -1,
        KeyCode::Right | KeyCode::Char(' ') => 1,
        _ => return,
    };
    state.primary = all[((idx + step).rem_euclid(len)) as usize];
}

fn handle_input(input: &mut KeyInput, code: KeyCode) {
    match code {
        KeyCode::Char(c) => input.insert_char(c),
        KeyCode::Backspace => input.backspace(),
        KeyCode::Delete => input.delete(),
        KeyCode::Left => input.move_left(),
        KeyCode::Right => input.move_right(),
        KeyCode::Home => input.move_home(),
        KeyCode::End => input.move_end(),
        _ => {}
    }
}

/// Save to `~/.config/ai-usagebar/config.toml` (or create it). On success,
/// signal a running Waybar process (SIGRTMIN+13) so any module configured
/// with `signal: 13` refreshes its exec output immediately — otherwise the
/// bar text wouldn't reflect a new primary vendor until the next interval
/// tick (up to 300s).
fn save_to_config_default(state: &SettingsState) -> Result<()> {
    let path = default_config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| AppError::io_at(parent, e))?;
    }
    save_to_path(state, &path)?;
    crate::waybar::request_refresh();
    Ok(())
}

/// Same as `save_to_config_default` but with an explicit path — exposed
/// for tests.
pub fn save_to_path(state: &SettingsState, path: &Path) -> Result<()> {
    let original = std::fs::read_to_string(path).unwrap_or_default();
    let mut doc: DocumentMut = if original.trim().is_empty() {
        DocumentMut::new()
    } else {
        original.parse().map_err(|e: toml_edit::TomlError| {
            AppError::Other(format!("config.toml not parseable: {e}"))
        })?
    };

    // [ui].primary
    set_string(&mut doc, "ui", "primary", state.primary.slug())?;
    // [zai].api_key — only write if dirty AND non-empty
    if state.zai.dirty && !state.zai.buf.is_empty() {
        set_string(&mut doc, "zai", "api_key", &state.zai.buf)?;
    }
    // [openrouter].api_key — same
    if state.openrouter.dirty && !state.openrouter.buf.is_empty() {
        set_string(&mut doc, "openrouter", "api_key", &state.openrouter.buf)?;
    }
    // [deepseek].api_key — same
    if state.deepseek.dirty && !state.deepseek.buf.is_empty() {
        set_string(&mut doc, "deepseek", "api_key", &state.deepseek.buf)?;
    }

    let bytes = doc.to_string();
    crate::cache::atomic_write(path, bytes.as_bytes())?;

    // chmod 600 — only required when we wrote a secret, but always safe.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o600);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
    Ok(())
}

/// Set or update a string field in a TOML section, preserving comments and
/// formatting of unaffected nodes. When the key already exists, we mutate its
/// value in place (this keeps the leading comment attached to the key);
/// otherwise we insert a new entry.
fn set_string(doc: &mut DocumentMut, section: &str, key: &str, new_value: &str) -> Result<()> {
    let table = doc
        .entry(section)
        .or_insert_with(toml_edit::table)
        .as_table_mut()
        .ok_or_else(|| AppError::Other(format!("config.toml: [{section}] is not a table")))?;

    if let Some(item) = table.get_mut(key) {
        if let Some(v) = item.as_value_mut() {
            *v = toml_edit::Value::from(new_value);
            // Restore the surrounding decor (a space before `=` and after the
            // value, matching toml_edit's default output).
            v.decor_mut().set_prefix(" ");
            return Ok(());
        }
    }
    table.insert(key, value(new_value));
    Ok(())
}

fn default_config_path() -> Result<PathBuf> {
    crate::config::default_path()
        .ok_or_else(|| AppError::Other("could not resolve config dir".into()))
}

/// Render the modal overlay over `area`.
pub fn render(f: &mut Frame, area: Rect, state: &SettingsState, theme: &Theme) {
    let modal = centered_rect(60, 60, area);
    // Clear underneath so the body is unreadable through us.
    f.render_widget(Clear, modal);

    let accent = parse_hex(&theme.blue).unwrap_or(Color::Cyan);
    let fg = parse_hex(&theme.fg).unwrap_or(Color::White);
    let dim = parse_hex(&theme.dim).unwrap_or(Color::DarkGray);

    let block = Block::default()
        .title(" Settings ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(accent).add_modifier(Modifier::BOLD));
    let inner = block.inner(modal);
    f.render_widget(block, modal);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // [0] primary label
            Constraint::Length(2), // [1] primary radio row
            Constraint::Length(1), // [2] spacer
            Constraint::Length(1), // [3] zai label
            Constraint::Length(2), // [4] zai input
            Constraint::Length(1), // [5] openrouter label
            Constraint::Length(2), // [6] openrouter input
            Constraint::Length(1), // [7] deepseek label
            Constraint::Length(2), // [8] deepseek input
            Constraint::Length(1), // [9] spacer
            Constraint::Length(1), // [10] save button
            Constraint::Length(1), // [11] status
            Constraint::Min(0),    // [12] hint
        ])
        .split(inner);

    // Primary vendor.
    f.render_widget(
        Paragraph::new(label(
            "Primary vendor",
            state.focus == Focus::Primary,
            fg,
            accent,
        )),
        chunks[0],
    );
    f.render_widget(
        Paragraph::new(render_radio(&state.primary, accent, dim)),
        chunks[1],
    );

    // Z.AI key.
    f.render_widget(
        Paragraph::new(label(
            "Z.AI API key (ZAI_API_KEY env wins if set)",
            state.focus == Focus::ZaiKey,
            fg,
            accent,
        )),
        chunks[3],
    );
    f.render_widget(
        Paragraph::new(render_input(
            &state.zai,
            state.focus == Focus::ZaiKey,
            fg,
            accent,
            dim,
        )),
        chunks[4],
    );

    // OpenRouter key.
    f.render_widget(
        Paragraph::new(label(
            "OpenRouter API key (OPENROUTER_API_KEY env wins if set)",
            state.focus == Focus::OpenrouterKey,
            fg,
            accent,
        )),
        chunks[5],
    );
    f.render_widget(
        Paragraph::new(render_input(
            &state.openrouter,
            state.focus == Focus::OpenrouterKey,
            fg,
            accent,
            dim,
        )),
        chunks[6],
    );

    // DeepSeek key.
    f.render_widget(
        Paragraph::new(label(
            "DeepSeek API key (DEEPSEEK_API_KEY env wins if set)",
            state.focus == Focus::DeepseekKey,
            fg,
            accent,
        )),
        chunks[7],
    );
    f.render_widget(
        Paragraph::new(render_input(
            &state.deepseek,
            state.focus == Focus::DeepseekKey,
            fg,
            accent,
            dim,
        )),
        chunks[8],
    );

    // Save button.
    let save_style = if state.focus == Focus::SaveButton {
        Style::default()
            .fg(accent)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else {
        Style::default().fg(accent)
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "   [ Save (Ctrl-S) ]   ",
            save_style,
        ))),
        chunks[10],
    );

    // Status line.
    if !state.status.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                state.status.clone(),
                Style::default().fg(dim),
            ))),
            chunks[11],
        );
    }

    // Hint footer.
    let hint = Line::from(vec![Span::styled(
        "  Tab/↑↓ move · ←→ pick vendor · Ctrl-V reveal · Ctrl-S save · Esc cancel",
        Style::default().fg(dim),
    )]);
    f.render_widget(Paragraph::new(hint), chunks[12]);
}

fn label(text: &str, focused: bool, fg: Color, accent: Color) -> Line<'static> {
    let mut style = Style::default().fg(fg);
    if focused {
        style = style.fg(accent).add_modifier(Modifier::BOLD);
    }
    Line::from(Span::styled(format!("  {text}"), style))
}

fn render_radio(selected: &VendorId, accent: Color, dim: Color) -> Line<'static> {
    let mut spans = vec![Span::raw("    ")];
    for v in VendorId::all() {
        let is_sel = v == selected;
        let glyph = if is_sel { "●" } else { "○" };
        let style = if is_sel {
            Style::default().fg(accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(dim)
        };
        spans.push(Span::styled(
            format!("{glyph} {}  ", vendor_label(*v)),
            style,
        ));
    }
    Line::from(spans)
}

fn vendor_label(v: VendorId) -> &'static str {
    match v {
        VendorId::Anthropic => "Anthropic",
        VendorId::Openai => "OpenAI",
        VendorId::Zai => "Z.AI",
        VendorId::Openrouter => "OpenRouter",
        VendorId::Deepseek => "DeepSeek",
    }
}

fn render_input(
    input: &KeyInput,
    focused: bool,
    fg: Color,
    accent: Color,
    dim: Color,
) -> Line<'static> {
    let body = if input.buf.is_empty() {
        "(empty)".to_string()
    } else {
        input.display()
    };
    let box_style = if focused {
        Style::default().fg(accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(fg)
    };
    let suffix_style = Style::default().fg(dim);
    let suffix = if input.revealed { "  [revealed]" } else { "" };
    let cursor_hint = if focused {
        format!("  ▏cur:{}", input.cursor)
    } else {
        String::new()
    };
    Line::from(vec![
        Span::styled(format!("    {body}"), box_style),
        Span::styled(format!("{suffix}{cursor_hint}"), suffix_style),
    ])
}

fn parse_hex(s: &str) -> Option<Color> {
    let (r, g, b) = crate::theme::parse_hex_rgb(s)?;
    Some(Color::Rgb(r, g, b))
}

/// Center a rectangle of `percent_x * percent_y` over `r`.
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_h = (r.height * percent_y) / 100;
    let popup_w = (r.width * percent_x) / 100;
    Rect {
        x: r.x + (r.width - popup_w) / 2,
        y: r.y + (r.height - popup_h) / 2,
        width: popup_w,
        height: popup_h,
    }
}

// crossterm types live behind the bin; re-exported here for handle_key callers.
pub use crossterm::event::{KeyCode, KeyModifiers};

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// A config path inside a fresh `TempDir`, with no open handle on the file.
    /// `save_to_path` rewrites atomically (rename over destination), which on
    /// Windows fails if the destination is still open — so tests must not hold a
    /// live `NamedTempFile` handle on the target. Returns the dir (kept alive by
    /// the caller) and the path; the file may or may not exist yet.
    fn temp_config(initial: Option<&str>) -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        if let Some(contents) = initial {
            std::fs::write(&path, contents).unwrap();
        }
        (dir, path)
    }

    fn state_with(zai: &str, opr: &str, primary: VendorId) -> SettingsState {
        let mut s = SettingsState {
            focus: Focus::Primary,
            primary,
            zai: KeyInput::from_config(Some(zai)),
            openrouter: KeyInput::from_config(Some(opr)),
            deepseek: KeyInput::default(),
            status: String::new(),
        };
        // Mark dirty so save writes them.
        s.zai.dirty = true;
        s.openrouter.dirty = true;
        s
    }

    #[test]
    fn focus_cycles_forward_and_backward() {
        let order = [
            Focus::Primary,
            Focus::ZaiKey,
            Focus::OpenrouterKey,
            Focus::DeepseekKey,
            Focus::SaveButton,
        ];
        let n = order.len();
        for (i, f) in order.iter().enumerate() {
            assert_eq!(f.next(), order[(i + 1) % n]);
            assert_eq!(f.prev(), order[(i + n - 1) % n]);
        }
    }

    #[test]
    fn key_input_insert_backspace_arrow() {
        let mut k = KeyInput::default();
        k.insert_char('a');
        k.insert_char('b');
        k.insert_char('c');
        assert_eq!(k.buf, "abc");
        assert_eq!(k.cursor, 3);
        assert!(k.dirty);
        k.move_left();
        k.move_left();
        assert_eq!(k.cursor, 1);
        k.insert_char('x'); // "axbc"
        assert_eq!(k.buf, "axbc");
        assert_eq!(k.cursor, 2);
        k.backspace();
        assert_eq!(k.buf, "abc");
        assert_eq!(k.cursor, 1);
    }

    #[test]
    fn key_input_masks_by_default_reveals_on_toggle() {
        let mut k = KeyInput::default();
        for c in "secret-key".chars() {
            k.insert_char(c);
        }
        assert_eq!(k.display(), "•".repeat(10));
        k.toggle_reveal();
        assert_eq!(k.display(), "secret-key");
    }

    #[test]
    fn key_input_handles_unicode() {
        let mut k = KeyInput::default();
        k.insert_char('a');
        k.insert_char('→');
        k.insert_char('b');
        assert_eq!(k.buf, "a→b");
        assert_eq!(k.cursor, 3);
        k.move_left();
        k.backspace(); // delete '→'
        assert_eq!(k.buf, "ab");
    }

    #[test]
    fn save_to_path_writes_minimal_toml_when_starting_empty() {
        let (_dir, path) = temp_config(None);
        let s = state_with("zk", "ok", VendorId::Zai);
        save_to_path(&s, &path).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("primary = \"zai\""));
        assert!(raw.contains("[zai]"));
        assert!(raw.contains("api_key = \"zk\""));
        assert!(raw.contains("[openrouter]"));
        assert!(raw.contains("api_key = \"ok\""));
    }

    #[test]
    fn save_to_path_preserves_existing_comments_and_unrelated_fields() {
        let (_dir, path) = temp_config(Some(
            r##"# my comment
[ui]
# pre-existing comment
primary = "anthropic"

[zai]
enabled = true
api_key_env = "ZAI_API_KEY"
# tier comment
plan_tier = "pro"

[openrouter]
enabled = true
api_key_env = "OPENROUTER_API_KEY"
"##,
        ));

        let s = state_with("zk2", "ok2", VendorId::Openrouter);
        save_to_path(&s, &path).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        // Comments survive.
        assert!(raw.contains("# my comment"));
        assert!(raw.contains("# pre-existing comment"));
        assert!(raw.contains("# tier comment"));
        // Unrelated fields survive.
        assert!(raw.contains("api_key_env = \"ZAI_API_KEY\""));
        assert!(raw.contains("plan_tier = \"pro\""));
        // Primary updated.
        assert!(raw.contains("primary = \"openrouter\""));
        // Keys written.
        assert!(raw.contains("api_key = \"zk2\""));
        assert!(raw.contains("api_key = \"ok2\""));
    }

    #[test]
    fn save_does_not_write_empty_key_when_dirty_but_blank() {
        let (_dir, path) = temp_config(None);
        let mut s = state_with("", "", VendorId::Anthropic);
        // Mark dirty but leave buf empty (user opened dialog with empty
        // field, focused it, did nothing).
        s.zai.dirty = true;
        s.openrouter.dirty = true;
        save_to_path(&s, &path).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        // No `api_key = ""` lines should be written.
        assert!(!raw.contains("api_key ="));
    }

    #[test]
    #[cfg(unix)]
    fn save_chmods_to_600() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, path) = temp_config(None);
        let s = state_with("zk", "ok", VendorId::Zai);
        save_to_path(&s, &path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn handle_key_tab_cycles_focus() {
        let mut s = SettingsState {
            focus: Focus::Primary,
            primary: VendorId::Anthropic,
            zai: KeyInput::default(),
            openrouter: KeyInput::default(),
            deepseek: KeyInput::default(),
            status: String::new(),
        };
        assert_eq!(
            handle_key(&mut s, KeyCode::Tab, KeyModifiers::NONE),
            Action::Continue
        );
        assert_eq!(s.focus, Focus::ZaiKey);
        assert_eq!(
            handle_key(&mut s, KeyCode::BackTab, KeyModifiers::NONE),
            Action::Continue
        );
        assert_eq!(s.focus, Focus::Primary);
    }

    #[test]
    fn handle_key_esc_closes_without_saving() {
        let mut s = SettingsState {
            focus: Focus::Primary,
            primary: VendorId::Anthropic,
            zai: KeyInput::default(),
            openrouter: KeyInput::default(),
            deepseek: KeyInput::default(),
            status: String::new(),
        };
        assert_eq!(
            handle_key(&mut s, KeyCode::Esc, KeyModifiers::NONE),
            Action::Close
        );
    }

    #[test]
    fn handle_key_left_right_cycles_primary_vendor() {
        let mut s = SettingsState {
            focus: Focus::Primary,
            primary: VendorId::Anthropic,
            zai: KeyInput::default(),
            openrouter: KeyInput::default(),
            deepseek: KeyInput::default(),
            status: String::new(),
        };
        handle_key(&mut s, KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(s.primary, VendorId::Openai);
        handle_key(&mut s, KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(s.primary, VendorId::Zai);
        handle_key(&mut s, KeyCode::Left, KeyModifiers::NONE);
        assert_eq!(s.primary, VendorId::Openai);
    }

    #[test]
    fn handle_key_ctrl_v_toggles_reveal_on_focused_key_field() {
        let mut s = SettingsState {
            focus: Focus::ZaiKey,
            primary: VendorId::Anthropic,
            zai: KeyInput::from_config(Some("secret")),
            openrouter: KeyInput::default(),
            deepseek: KeyInput::default(),
            status: String::new(),
        };
        assert!(!s.zai.revealed);
        handle_key(&mut s, KeyCode::Char('v'), KeyModifiers::CONTROL);
        assert!(s.zai.revealed);
        handle_key(&mut s, KeyCode::Char('v'), KeyModifiers::CONTROL);
        assert!(!s.zai.revealed);
    }

    #[test]
    fn handle_key_ctrl_s_attempts_save_from_any_field() {
        let (_dir, path) = temp_config(None);
        // We can't easily redirect default_config_path() in the test, so we
        // exercise save_to_path directly instead.
        let s = state_with("zk", "ok", VendorId::Zai);
        save_to_path(&s, &path).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("api_key = \"zk\""));
    }
}
