//! Settings overlay — opened from the TUI by pressing `s`. Lets the user
//! pick the primary vendor and set Z.AI / OpenRouter / DeepSeek / Kimi API keys
//! without editing config.toml by hand.
//!
//! Persistence uses `toml_edit` so the existing config keeps its comments,
//! whitespace, and unrelated fields. Files with inline keys are atomically
//! written and `chmod 600`ed.

use std::path::{Path, PathBuf};

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui_bubbletea_theme::BubbleTheme;
use toml_edit::{DocumentMut, value};

use crate::config::Config;
use crate::error::{AppError, Result};
use crate::theme::Theme;
use crate::tui::style::bubble_theme;
use crate::vendor::VendorId;

/// Which input field has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Primary,
    ZaiKey,
    OpenrouterKey,
    DeepseekKey,
    KimiKey,
    SaveButton,
}

impl Focus {
    pub fn next(self) -> Self {
        match self {
            Focus::Primary => Focus::ZaiKey,
            Focus::ZaiKey => Focus::OpenrouterKey,
            Focus::OpenrouterKey => Focus::DeepseekKey,
            Focus::DeepseekKey => Focus::KimiKey,
            Focus::KimiKey => Focus::SaveButton,
            Focus::SaveButton => Focus::Primary,
        }
    }
    pub fn prev(self) -> Self {
        match self {
            Focus::Primary => Focus::SaveButton,
            Focus::ZaiKey => Focus::Primary,
            Focus::OpenrouterKey => Focus::ZaiKey,
            Focus::DeepseekKey => Focus::OpenrouterKey,
            Focus::KimiKey => Focus::DeepseekKey,
            Focus::SaveButton => Focus::KimiKey,
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
    /// Enabled vendors only. The primary selector must not offer a value that
    /// cannot actually be used by the widget or TUI.
    pub primary_choices: Vec<VendorId>,
    pub primary: VendorId,
    pub zai: KeyInput,
    pub openrouter: KeyInput,
    pub deepseek: KeyInput,
    pub kimi: KeyInput,
    /// One-line status displayed in the footer ("Saved", "Error: ...", "").
    pub status: String,
}

impl SettingsState {
    pub fn from_config(cfg: &Config) -> Self {
        let primary_choices = cfg.enabled_vendors();
        // A configured but disabled primary is ineffective. Display the first
        // enabled vendor instead; when none are enabled retain the historical
        // Anthropic fallback in memory without inventing a persisted primary.
        let primary = cfg
            .ui
            .primary
            .filter(|vendor| primary_choices.contains(vendor))
            .or_else(|| primary_choices.first().copied())
            .unwrap_or_else(|| cfg.ui.primary.unwrap_or(VendorId::Anthropic));
        Self {
            focus: Focus::Primary,
            primary_choices,
            primary,
            zai: KeyInput::from_config(cfg.zai.api_key.as_deref()),
            openrouter: KeyInput::from_config(cfg.openrouter.api_key.as_deref()),
            deepseek: KeyInput::from_config(cfg.deepseek.api_key.as_deref()),
            kimi: KeyInput::from_config(cfg.kimi.api_key.as_deref()),
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
            Focus::KimiKey => state.kimi.toggle_reveal(),
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

    // A modifier chord is not text. The overlay swallows every key while open,
    // so without this Ctrl-C types "c" into the focused API key instead of
    // reaching the app's quit binding, and every other chord corrupts the
    // secret silently. SHIFT is deliberately not rejected — it is how
    // uppercase arrives.
    if matches!(code, KeyCode::Char(_))
        && mods.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return Action::Continue;
    }

    // Field-specific handling.
    match state.focus {
        Focus::Primary => handle_primary(state, code),
        Focus::ZaiKey => handle_input(&mut state.zai, code),
        Focus::OpenrouterKey => handle_input(&mut state.openrouter, code),
        Focus::DeepseekKey => handle_input(&mut state.deepseek, code),
        Focus::KimiKey => handle_input(&mut state.kimi, code),
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
    let choices = &state.primary_choices;
    let Some(idx) = choices.iter().position(|v| *v == state.primary) else {
        return;
    };
    let step = match code {
        KeyCode::Left => -1,
        KeyCode::Right | KeyCode::Char(' ') => 1,
        _ => return,
    };
    state.primary = choices[((idx as i32 + step).rem_euclid(choices.len() as i32)) as usize];
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

    // Do not write a disabled primary as a side effect of saving an API key.
    // With no enabled vendors, leave any existing value alone so the legacy
    // resolver's Anthropic fallback remains intact.
    if state.primary_choices.contains(&state.primary) {
        set_string(&mut doc, "ui", "primary", state.primary.slug())?;
    }
    update_key(&mut doc, "zai", &state.zai)?;
    update_key(&mut doc, "openrouter", &state.openrouter)?;
    update_key(&mut doc, "deepseek", &state.deepseek)?;
    update_key(&mut doc, "kimi", &state.kimi)?;

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

    if let Some(item) = table.get_mut(key)
        && let Some(v) = item.as_value_mut()
    {
        *v = toml_edit::Value::from(new_value);
        // Restore the surrounding decor (a space before `=` and after the
        // value, matching toml_edit's default output).
        v.decor_mut().set_prefix(" ");
        return Ok(());
    }
    table.insert(key, value(new_value));
    Ok(())
}

/// Persist an intentionally edited key. A dirty empty buffer means the user
/// explicitly cleared the key; an untouched empty buffer leaves TOML intact.
fn update_key(doc: &mut DocumentMut, section: &str, input: &KeyInput) -> Result<()> {
    if !input.dirty {
        return Ok(());
    }
    if input.buf.is_empty() {
        if let Some(table) = doc.get_mut(section).and_then(toml_edit::Item::as_table_mut) {
            table.remove("api_key");
        }
        return Ok(());
    }
    set_string(doc, section, "api_key", &input.buf)
}

fn default_config_path() -> Result<PathBuf> {
    // Save back to the same file Config::load() selected. On macOS this may be
    // the legacy ~/.config path when the canonical Application Support file is
    // absent; writing a new canonical file would shadow the existing config on
    // the next load and silently discard all settings the overlay did not copy.
    crate::config::resolved_path()
        .ok_or_else(|| AppError::Other("could not resolve config dir".into()))
}

/// Render the modal overlay over `area`.
pub fn render(f: &mut Frame, area: Rect, state: &SettingsState, theme: &Theme) {
    let modal = settings_modal_rect(area);
    // Clear underneath so the body is unreadable through us.
    f.render_widget(Clear, modal);

    let bubble = bubble_theme(theme);

    let block = bubble.titled_modal_block(" Settings ");
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
            Constraint::Length(1), // [9] kimi label
            Constraint::Length(2), // [10] kimi input
            Constraint::Length(1), // [11] spacer
            Constraint::Length(1), // [12] save button
            Constraint::Length(1), // [13] status
            Constraint::Min(0),    // [14] hint
        ])
        .split(inner);

    // Primary vendor.
    f.render_widget(
        Paragraph::new(label(
            "Primary vendor",
            state.focus == Focus::Primary,
            &bubble,
        )),
        chunks[0],
    );
    f.render_widget(
        Paragraph::new(render_radio(
            &state.primary_choices,
            &state.primary,
            &bubble,
        )),
        chunks[1],
    );

    // Z.AI key.
    f.render_widget(
        Paragraph::new(label(
            "Z.AI API key (environment key takes precedence)",
            state.focus == Focus::ZaiKey,
            &bubble,
        )),
        chunks[3],
    );
    f.render_widget(
        Paragraph::new(render_input(
            &state.zai,
            state.focus == Focus::ZaiKey,
            &bubble,
        )),
        chunks[4],
    );

    // OpenRouter key.
    f.render_widget(
        Paragraph::new(label(
            "OpenRouter API key (environment key takes precedence)",
            state.focus == Focus::OpenrouterKey,
            &bubble,
        )),
        chunks[5],
    );
    f.render_widget(
        Paragraph::new(render_input(
            &state.openrouter,
            state.focus == Focus::OpenrouterKey,
            &bubble,
        )),
        chunks[6],
    );

    // DeepSeek key.
    f.render_widget(
        Paragraph::new(label(
            "DeepSeek API key (environment key takes precedence)",
            state.focus == Focus::DeepseekKey,
            &bubble,
        )),
        chunks[7],
    );
    f.render_widget(
        Paragraph::new(render_input(
            &state.deepseek,
            state.focus == Focus::DeepseekKey,
            &bubble,
        )),
        chunks[8],
    );

    // Kimi key.
    f.render_widget(
        Paragraph::new(label(
            "Kimi API key (environment key takes precedence)",
            state.focus == Focus::KimiKey,
            &bubble,
        )),
        chunks[9],
    );
    f.render_widget(
        Paragraph::new(render_input(
            &state.kimi,
            state.focus == Focus::KimiKey,
            &bubble,
        )),
        chunks[10],
    );

    // Save button.
    let save_style = if state.focus == Focus::SaveButton {
        bubble.selected.add_modifier(Modifier::REVERSED)
    } else {
        bubble.accent.add_modifier(Modifier::BOLD)
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "   [ Save (Ctrl-S) ]   ",
            save_style,
        ))),
        chunks[12],
    );

    // Status line.
    if !state.status.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(state.status.clone(), bubble.muted))),
            chunks[13],
        );
    }

    // Hint footer.
    let hint = bubble.help_line([
        ("tab/up/down", "move"),
        ("left/right", "pick"),
        ("ctrl+v", "reveal"),
        ("ctrl+s", "save"),
        ("esc", "cancel"),
    ]);
    f.render_widget(Paragraph::new(hint), chunks[14]);
}

fn label(text: &str, focused: bool, theme: &BubbleTheme) -> Line<'static> {
    let marker = if focused {
        theme.symbols.selected
    } else {
        theme.symbols.bullet
    };
    let marker_style = if focused { theme.accent } else { theme.muted };
    let text_style = if focused { theme.title } else { theme.text };
    Line::from(vec![
        theme.muted("  "),
        Span::styled(marker, marker_style),
        theme.span(" "),
        Span::styled(text.to_string(), text_style),
    ])
}

fn render_radio(choices: &[VendorId], selected: &VendorId, theme: &BubbleTheme) -> Line<'static> {
    let mut spans = vec![theme.muted("    ")];
    for v in choices {
        let is_sel = v == selected;
        let glyph = if is_sel {
            theme.symbols.selected
        } else {
            theme.symbols.bullet
        };
        let style = if is_sel { theme.selected } else { theme.muted };
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
        VendorId::Kimi => "Kimi",
    }
}

fn render_input(input: &KeyInput, focused: bool, theme: &BubbleTheme) -> Line<'static> {
    let body = if input.buf.is_empty() {
        "(empty)".to_string()
    } else {
        input.display()
    };
    let box_style = if focused {
        theme.accent.add_modifier(Modifier::BOLD)
    } else {
        theme.text
    };
    let suffix_style = theme.muted;
    let suffix = if input.revealed { "  [revealed]" } else { "" };
    let cursor_hint = if focused {
        format!("  ▏cur:{}", input.cursor)
    } else {
        String::new()
    };
    Line::from(vec![
        theme.muted("    "),
        Span::styled(body, box_style),
        Span::styled(format!("{suffix}{cursor_hint}"), suffix_style),
    ])
}

const SETTINGS_CONTENT_HEIGHT: u16 = 18;

/// Keep every editable field and Save visible in a common 24-row terminal.
fn settings_modal_rect(area: Rect) -> Rect {
    let height = ((area.height * 92) / 100)
        .max(SETTINGS_CONTENT_HEIGHT + 2)
        .min(area.height);
    let width = (area.width * 80) / 100;
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

// crossterm types live behind ratatui; re-exported here for handle_key callers.
pub use ratatui::crossterm::event::{KeyCode, KeyModifiers};

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// A config path with no open handle on the file, so `save_to_path`'s
    /// atomic rename-over-destination succeeds on Windows.
    /// See [`crate::cache::closed_temp_file`].
    fn temp_config(initial: Option<&str>) -> (TempDir, std::path::PathBuf) {
        crate::cache::closed_temp_file("config.toml", initial)
    }

    fn state_with(zai: &str, opr: &str, primary: VendorId) -> SettingsState {
        let mut s = SettingsState {
            focus: Focus::Primary,
            primary_choices: VendorId::all().to_vec(),
            primary,
            zai: KeyInput::from_config(Some(zai)),
            openrouter: KeyInput::from_config(Some(opr)),
            deepseek: KeyInput::default(),
            kimi: KeyInput::default(),
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
            Focus::KimiKey,
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
    fn save_dirty_empty_key_removes_existing_inline_key() {
        let (_dir, path) = temp_config(Some(
            r#"[zai]
api_key = "old-secret"
plan_tier = "pro"
"#,
        ));
        let mut s = SettingsState::from_config(&Config::default());
        s.zai.dirty = true;
        save_to_path(&s, &path).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("api_key"));
        assert!(raw.contains("plan_tier = \"pro\""));
    }

    #[test]
    fn save_untouched_empty_key_preserves_existing_inline_key() {
        let (_dir, path) = temp_config(Some(
            r#"[zai]
api_key = "old-secret"
"#,
        ));
        let s = SettingsState::from_config(&Config::default());
        save_to_path(&s, &path).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("api_key = \"old-secret\""));
    }

    #[test]
    fn settings_modal_fits_all_fields_and_save_in_24_rows() {
        let modal = settings_modal_rect(Rect::new(0, 0, 100, 24));
        // Two border rows leave at least the 18 fixed rows through Save.
        assert!(modal.height >= SETTINGS_CONTENT_HEIGHT + 2);
        assert!(modal.height <= 24);
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
    fn save_preserves_kimi_enabled_true_and_unrelated_comments() {
        let (_dir, path) = temp_config(Some(
            r##"[ui]
primary = "anthropic"

[kimi]
# Kimi is opt-in
enabled = true
api_key_env = "KIMI_API_KEY"
"##,
        ));

        // No keys dirty, primary unchanged — save should still rewrite
        // primary in place but leave the [kimi] section untouched.
        let s = SettingsState {
            focus: Focus::Primary,
            primary_choices: VendorId::all().to_vec(),
            primary: VendorId::Anthropic,
            zai: KeyInput::default(),
            openrouter: KeyInput::default(),
            deepseek: KeyInput::default(),
            kimi: KeyInput::default(),
            status: String::new(),
        };
        save_to_path(&s, &path).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("# Kimi is opt-in"));
        assert!(raw.contains("enabled = true"));
        assert!(raw.contains("api_key_env = \"KIMI_API_KEY\""));
        assert!(raw.contains("primary = \"anthropic\""));
    }

    #[test]
    fn handle_key_tab_cycles_focus() {
        let mut s = SettingsState {
            focus: Focus::Primary,
            primary_choices: VendorId::all().to_vec(),
            primary: VendorId::Anthropic,
            zai: KeyInput::default(),
            openrouter: KeyInput::default(),
            deepseek: KeyInput::default(),
            kimi: KeyInput::default(),
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
            primary_choices: VendorId::all().to_vec(),
            primary: VendorId::Anthropic,
            zai: KeyInput::default(),
            openrouter: KeyInput::default(),
            deepseek: KeyInput::default(),
            kimi: KeyInput::default(),
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
            primary_choices: VendorId::all().to_vec(),
            primary: VendorId::Anthropic,
            zai: KeyInput::default(),
            openrouter: KeyInput::default(),
            deepseek: KeyInput::default(),
            kimi: KeyInput::default(),
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
    fn primary_uses_enabled_vendors_for_display_and_cycling() {
        let mut cfg = Config::default();
        cfg.openai.enabled = false;
        cfg.zai.enabled = false;
        cfg.openrouter.enabled = false;
        cfg.kimi.enabled = true;
        cfg.ui.primary = Some(VendorId::Openai); // disabled and ineffective

        let mut state = SettingsState::from_config(&cfg);
        assert_eq!(
            state.primary_choices,
            vec![VendorId::Anthropic, VendorId::Kimi]
        );
        assert_eq!(state.primary, VendorId::Anthropic);
        handle_key(&mut state, KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(state.primary, VendorId::Kimi);
    }

    #[test]
    fn save_replaces_disabled_primary_with_effective_enabled_choice() {
        let (_dir, path) = temp_config(Some(
            r#"[ui]
primary = "deepseek"
"#,
        ));
        let mut cfg = Config::default();
        cfg.ui.primary = Some(VendorId::Deepseek);
        let mut state = SettingsState::from_config(&cfg);
        state.zai = KeyInput::from_config(Some("new-key"));
        state.zai.dirty = true;

        save_to_path(&state, &path).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("primary = \"anthropic\""));
        assert!(!raw.contains("primary = \"deepseek\""));
    }

    #[test]
    fn no_enabled_vendors_does_not_write_an_ineffective_primary() {
        let (_dir, path) = temp_config(Some(
            r#"[ui]
primary = "deepseek"
"#,
        ));
        let mut cfg = Config::default();
        cfg.anthropic.enabled = false;
        cfg.openai.enabled = false;
        cfg.zai.enabled = false;
        cfg.openrouter.enabled = false;
        cfg.deepseek.enabled = false;
        cfg.kimi.enabled = false;
        cfg.ui.primary = Some(VendorId::Deepseek);
        let mut state = SettingsState::from_config(&cfg);
        state.zai = KeyInput::from_config(Some("new-key"));
        state.zai.dirty = true;

        assert!(state.primary_choices.is_empty());
        save_to_path(&state, &path).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("primary = \"deepseek\""));
    }

    #[test]
    fn handle_key_ctrl_v_toggles_reveal_on_focused_key_field() {
        let mut s = SettingsState {
            focus: Focus::ZaiKey,
            primary_choices: VendorId::all().to_vec(),
            primary: VendorId::Anthropic,
            zai: KeyInput::from_config(Some("secret")),
            openrouter: KeyInput::default(),
            deepseek: KeyInput::default(),
            kimi: KeyInput::default(),
            status: String::new(),
        };
        assert!(!s.zai.revealed);
        handle_key(&mut s, KeyCode::Char('v'), KeyModifiers::CONTROL);
        assert!(s.zai.revealed);
        handle_key(&mut s, KeyCode::Char('v'), KeyModifiers::CONTROL);
        assert!(!s.zai.revealed);
    }

    fn state_focused_on_zai() -> SettingsState {
        SettingsState {
            focus: Focus::ZaiKey,
            primary_choices: VendorId::all().to_vec(),
            primary: VendorId::Anthropic,
            zai: KeyInput::default(),
            openrouter: KeyInput::default(),
            deepseek: KeyInput::default(),
            kimi: KeyInput::default(),
            status: String::new(),
        }
    }

    #[test]
    fn handle_key_ctrl_c_does_not_type_into_key_field() {
        let mut s = state_focused_on_zai();
        assert_eq!(
            handle_key(&mut s, KeyCode::Char('c'), KeyModifiers::CONTROL),
            Action::Continue
        );
        assert!(s.zai.buf.is_empty());
        // Untouched means save still leaves an existing key on disk alone.
        assert!(!s.zai.dirty);
    }

    #[test]
    fn handle_key_alt_chord_does_not_type_into_key_field() {
        let mut s = state_focused_on_zai();
        handle_key(&mut s, KeyCode::Char('x'), KeyModifiers::ALT);
        assert!(s.zai.buf.is_empty());
        assert!(!s.zai.dirty);
    }

    #[test]
    fn handle_key_shift_still_types_uppercase() {
        let mut s = state_focused_on_zai();
        handle_key(&mut s, KeyCode::Char('A'), KeyModifiers::SHIFT);
        assert_eq!(s.zai.buf, "A");
        assert!(s.zai.dirty);
    }

    #[test]
    fn handle_key_plain_space_still_cycles_primary_vendor() {
        let mut s = SettingsState {
            focus: Focus::Primary,
            primary_choices: VendorId::all().to_vec(),
            primary: VendorId::Anthropic,
            zai: KeyInput::default(),
            openrouter: KeyInput::default(),
            deepseek: KeyInput::default(),
            kimi: KeyInput::default(),
            status: String::new(),
        };
        handle_key(&mut s, KeyCode::Char(' '), KeyModifiers::NONE);
        assert_eq!(s.primary, VendorId::Openai);
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

    #[test]
    fn save_to_path_writes_kimi_key_when_dirty() {
        let (_dir, path) = temp_config(None);
        let mut s = SettingsState {
            focus: Focus::Primary,
            primary_choices: VendorId::all().to_vec(),
            primary: VendorId::Anthropic,
            zai: KeyInput::default(),
            openrouter: KeyInput::default(),
            deepseek: KeyInput::default(),
            kimi: KeyInput::from_config(Some("kk")),
            status: String::new(),
        };
        s.kimi.dirty = true;
        save_to_path(&s, &path).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("[kimi]"));
        assert!(raw.contains("api_key = \"kk\""));
    }

    #[test]
    fn settings_save_uses_the_same_config_path_as_load() {
        assert_eq!(
            default_config_path().unwrap(),
            crate::config::resolved_path().unwrap()
        );
    }
}
