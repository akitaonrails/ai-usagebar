//! Settings overlay — opened from the TUI by pressing `s`. Lets the user pick
//! the primary vendor and paste a credential for any API-key-authenticated vendor
//! (including Z.AI, Kimi, MiniMax, and the balance vendors) without hand-editing
//! config.toml. Anthropic, OpenAI, GitHub Copilot, Cursor, Kiro, Antigravity, and
//! Command Code authenticate through official or local product state, so they have
//! no credential field here — there is nothing to paste, and a field would only
//! imply otherwise. Kimi keeps
//! its credential field because a platform key is still one of its two credentials, but
//! a subscriber whose credential is the Kimi Code CLI login has nothing to paste
//! and enables `[kimi]` in config.toml instead.
//!
//! Persistence uses `toml_edit` so the existing config keeps its comments,
//! whitespace, and unrelated fields. Writing a key also flips that vendor's
//! `enabled = true` (the opt-in vendors are disabled by default), so "paste the
//! credential and save" is all it takes. Files with inline credentials are atomically written
//! and `chmod 600`ed.

use std::collections::BTreeMap;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui_bubbletea_theme::BubbleTheme;
use serde::{Deserialize, Serialize};
use toml_edit::{DocumentMut, value};

use crate::config::Config;
use crate::error::{AppError, Result};
use crate::theme::Theme;
use crate::tui::style::bubble_theme;
use crate::vendor::VendorId;

/// A vendor that authenticates with an inline credential. The order of this
/// table is the tab order of the credential fields and the layout of the
/// state's `keys` vec.
pub struct KeyVendor {
    pub id: VendorId,
    pub label: &'static str,
    pub section: &'static str,
    /// Config field that stores the credential (`api_key` for most vendors).
    pub config_key: &'static str,
    /// Human-readable name shown in the native settings form.
    pub secret_label: &'static str,
    /// Extra hint after the env var (e.g. "management key"). Empty for none.
    pub note: &'static str,
}

pub const KEY_VENDORS: &[KeyVendor] = &[
    KeyVendor {
        id: VendorId::AnthropicApi,
        label: "Anthropic API",
        section: "anthropic_api",
        config_key: "api_key",
        secret_label: "API key",
        note: "admin key — monthly spend",
    },
    KeyVendor {
        id: VendorId::Zai,
        label: "Z.AI",
        section: "zai",
        config_key: "api_key",
        secret_label: "API key",
        note: "",
    },
    KeyVendor {
        id: VendorId::Openrouter,
        label: "OpenRouter",
        section: "openrouter",
        config_key: "api_key",
        secret_label: "API key",
        note: "",
    },
    KeyVendor {
        id: VendorId::Deepseek,
        label: "DeepSeek",
        section: "deepseek",
        config_key: "api_key",
        secret_label: "API key",
        note: "",
    },
    KeyVendor {
        id: VendorId::Kimi,
        label: "Kimi",
        section: "kimi",
        config_key: "api_key",
        secret_label: "API key",
        note: "coding-plan usage",
    },
    KeyVendor {
        id: VendorId::Kilo,
        label: "Kilo",
        section: "kilo",
        config_key: "api_key",
        secret_label: "API key",
        note: "",
    },
    KeyVendor {
        id: VendorId::Novita,
        label: "Novita",
        section: "novita",
        config_key: "api_key",
        secret_label: "API key",
        note: "",
    },
    KeyVendor {
        id: VendorId::Moonshot,
        label: "Moonshot",
        section: "moonshot",
        config_key: "api_key",
        secret_label: "API key",
        note: "account balance",
    },
    KeyVendor {
        id: VendorId::Grok,
        label: "Grok",
        section: "grok",
        config_key: "api_key",
        secret_label: "API key",
        note: "management key, not the inference key",
    },
    KeyVendor {
        id: VendorId::Minimax,
        label: "MiniMax",
        section: "minimax",
        config_key: "api_key",
        secret_label: "API key",
        note: "Token Plan subscription key",
    },
    KeyVendor {
        id: VendorId::OpenCodeGo,
        label: "OpenCode Go",
        section: "opencode-go",
        config_key: "api_key",
        secret_label: "API key",
        note: "usage quota",
    },
];

/// Which control has keyboard focus. `Key(i)` indexes into [`KEY_VENDORS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Primary,
    Key(usize),
    Save,
}

impl Focus {
    pub fn next(self) -> Self {
        match self {
            Focus::Primary => Focus::Key(0),
            Focus::Key(i) if i + 1 < KEY_VENDORS.len() => Focus::Key(i + 1),
            Focus::Key(_) => Focus::Save,
            Focus::Save => Focus::Primary,
        }
    }
    pub fn prev(self) -> Self {
        match self {
            Focus::Primary => Focus::Save,
            Focus::Key(0) => Focus::Primary,
            Focus::Key(i) => Focus::Key(i - 1),
            Focus::Save => Focus::Key(KEY_VENDORS.len() - 1),
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
    /// True after the user has typed/edited; only then does save write the
    /// value back (avoids clobbering an existing key with the empty
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
    /// One input per [`KEY_VENDORS`] entry, same order.
    pub keys: Vec<KeyInput>,
    /// One-line status displayed in the footer ("saved …", "save failed …").
    pub status: String,
}

impl SettingsState {
    pub fn from_config(cfg: &Config) -> Self {
        let keys = KEY_VENDORS
            .iter()
            .map(|kv| KeyInput::from_config(cfg.inline_api_key(kv.id)))
            .collect();
        let mut primary_choices = cfg.enabled_vendors();
        // Copilot credentials belong to GitHub CLI, so a login cannot write a
        // local key that would also opt it in. Offer it explicitly instead:
        // selecting it persists both the primary and `enabled = true`.
        if !primary_choices.contains(&VendorId::Copilot) {
            primary_choices.push(VendorId::Copilot);
        }
        // A configured but disabled primary is ineffective. Display the first
        // enabled vendor instead; when none are enabled retain the historical
        // Anthropic fallback in memory without inventing a persisted primary.
        let primary = cfg
            .ui
            .primary
            .filter(|vendor| {
                primary_choices.contains(vendor)
                    && (*vendor != VendorId::Copilot || cfg.copilot.enabled)
            })
            .or_else(|| primary_choices.first().copied())
            .unwrap_or_else(|| cfg.ui.primary.unwrap_or(VendorId::Anthropic));
        Self {
            focus: Focus::Primary,
            primary_choices,
            primary,
            keys,
            status: String::new(),
        }
    }

    /// The focused key input, if a key row is focused.
    fn focused_key_mut(&mut self) -> Option<&mut KeyInput> {
        match self.focus {
            Focus::Key(i) => self.keys.get_mut(i),
            _ => None,
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
    /// Quit the host TUI. Ctrl-C remains global even while the overlay owns
    /// keyboard focus.
    Quit,
}

/// Permission note appended to the "saved" status line. The overlay `chmod
/// 600`s the file on Unix; Windows has no such step, so the note is empty there.
#[cfg(unix)]
const PERMS_NOTE: &str = " (chmod 600)";
#[cfg(not(unix))]
const PERMS_NOTE: &str = "";

fn saved_status() -> String {
    format!(
        "saved to {}{}",
        crate::config::config_path_hint(),
        PERMS_NOTE
    )
}

/// Key map. Returns the action to perform after the keypress.
pub fn handle_key(state: &mut SettingsState, code: KeyCode, mods: KeyModifiers) -> Action {
    if matches!(code, KeyCode::Esc) {
        return Action::Close;
    }
    if matches!(code, KeyCode::Char('c')) && mods.contains(KeyModifiers::CONTROL) {
        return Action::Quit;
    }
    // Ctrl-S triggers save from any field.
    if matches!(code, KeyCode::Char('s')) && mods.contains(KeyModifiers::CONTROL) {
        return try_save(state);
    }
    if matches!(code, KeyCode::Char('v')) && mods.contains(KeyModifiers::CONTROL) {
        if let Some(input) = state.focused_key_mut() {
            input.toggle_reveal();
        }
        return Action::Continue;
    }
    match code {
        KeyCode::Tab | KeyCode::Down => {
            state.focus = state.focus.next();
            return Action::Continue;
        }
        KeyCode::BackTab | KeyCode::Up => {
            state.focus = state.focus.prev();
            return Action::Continue;
        }
        _ => {}
    }

    // A modifier chord is not text. The overlay swallows every key while open,
    // so every unhandled chord must be ignored rather than corrupting the
    // secret silently. SHIFT is deliberately not rejected — it is how
    // uppercase arrives. Ctrl-C was handled above because it is a global quit.
    if matches!(code, KeyCode::Char(_))
        && mods.intersects(
            KeyModifiers::CONTROL
                | KeyModifiers::ALT
                | KeyModifiers::SUPER
                | KeyModifiers::HYPER
                | KeyModifiers::META,
        )
    {
        return Action::Continue;
    }

    // Field-specific handling.
    match state.focus {
        Focus::Primary => handle_primary(state, code),
        Focus::Key(i) => {
            if let Some(input) = state.keys.get_mut(i) {
                handle_input(input, code);
            }
        }
        Focus::Save => {
            if matches!(code, KeyCode::Enter) {
                return try_save(state);
            }
        }
    }
    Action::Continue
}

fn try_save(state: &mut SettingsState) -> Action {
    match save_to_config_default(state) {
        Ok(()) => {
            state.status = saved_status();
            Action::SavedAndClose
        }
        Err(e) => {
            state.status = format!("save failed: {e}");
            Action::Continue
        }
    }
}

fn handle_primary(state: &mut SettingsState, code: KeyCode) {
    // Left/Right cycles the primary-vendor radio over enabled vendors only.
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

/// Save to the platform config path (creating it). On success, signal a running
/// Waybar (`SIGRTMIN+13`) so a `signal: 13` module refreshes immediately.
fn save_to_config_default(state: &SettingsState) -> Result<()> {
    let path = default_config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| AppError::io_at(parent, e))?;
    }
    save_to_path(state, &path)?;
    crate::waybar::request_refresh();
    Ok(())
}

/// Same as `save_to_config_default` but with an explicit path — exposed for
/// tests. Writing a non-empty credential also sets that vendor's `enabled = true`.
pub fn save_to_path(state: &SettingsState, path: &Path) -> Result<()> {
    let original = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(AppError::io_at(path, error)),
    };
    let mut doc: DocumentMut = if original.trim().is_empty() {
        DocumentMut::new()
    } else {
        original.parse().map_err(|e: toml_edit::TomlError| {
            AppError::Other(format!("config.toml not parseable: {e}"))
        })?
    };

    // Remove fields written by the short-lived inline Copilot-token design.
    // GitHub CLI owns the OAuth credential now; retaining a secret this app
    // neither reads nor supports would be misleading and unsafe.
    if let Some(table) = doc
        .get_mut("copilot")
        .and_then(toml_edit::Item::as_table_mut)
    {
        table.remove("token");
        table.remove("token_env");
    }

    // Only Copilot is deliberately offered before it is enabled: choosing it
    // is the explicit opt-in after the GitHub CLI login. All other choices
    // remain enabled-only, so no failed provider is persisted as primary.
    if state.primary_choices.contains(&state.primary) {
        set_string(&mut doc, "ui", "primary", state.primary.slug())?;
        if state.primary == VendorId::Copilot {
            set_bool(&mut doc, "copilot", "enabled", true)?;
        }
    }

    for (i, kv) in KEY_VENDORS.iter().enumerate() {
        let Some(input) = state.keys.get(i) else {
            continue;
        };
        update_key(&mut doc, kv, input)?;
    }

    let bytes = doc.to_string();
    crate::cache::atomic_write(path, bytes.as_bytes())?;

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

/// Apply one credential field to the document. Untouched fields are left
/// alone; a field the user cleared is *removed*, so an inline secret can be
/// deleted from the overlay rather than lingering in the file. Writing a
/// non-empty credential also opts the vendor in — the opt-in vendors would
/// otherwise never fetch.
fn update_key(doc: &mut DocumentMut, vendor: &KeyVendor, input: &KeyInput) -> Result<()> {
    if !input.dirty {
        return Ok(());
    }
    if input.buf.is_empty() {
        if let Some(table) = doc
            .get_mut(vendor.section)
            .and_then(toml_edit::Item::as_table_mut)
        {
            table.remove(vendor.config_key);
        }
        return Ok(());
    }
    set_string(doc, vendor.section, vendor.config_key, &input.buf)?;
    set_bool(doc, vendor.section, "enabled", true)
}

/// Set or update a string field in a TOML section, preserving comments and
/// formatting of unaffected nodes.
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
        v.decor_mut().set_prefix(" ");
        return Ok(());
    }
    table.insert(key, value(new_value));
    Ok(())
}

/// Same as [`set_string`] for a boolean field.
fn set_bool(doc: &mut DocumentMut, section: &str, key: &str, new_value: bool) -> Result<()> {
    let table = doc
        .entry(section)
        .or_insert_with(toml_edit::table)
        .as_table_mut()
        .ok_or_else(|| AppError::Other(format!("config.toml: [{section}] is not a table")))?;

    if let Some(item) = table.get_mut(key)
        && let Some(v) = item.as_value_mut()
    {
        *v = toml_edit::Value::from(new_value);
        v.decor_mut().set_prefix(" ");
        return Ok(());
    }
    table.insert(key, value(new_value));
    Ok(())
}

fn default_config_path() -> Result<PathBuf> {
    // Save back to the same file Config::load() selected. On macOS this may be
    // the legacy ~/.config path when the canonical Application Support file is
    // absent; writing a new canonical file would shadow the existing config on
    // the next load and silently discard all settings the overlay did not copy.
    crate::config::resolved_path()
        .ok_or_else(|| AppError::Other("could not resolve config dir".into()))
}

// ─── Native frontend bridge ───────────────────────────────────────────────

/// Versioned, non-secret description consumed by native desktop frontends.
/// Inline key values are deliberately represented only as booleans: a
/// long-lived shell process never needs to receive credentials just to draw a
/// settings form.
#[derive(Debug, Serialize)]
struct SettingsSnapshot {
    schema_version: u8,
    primary: String,
    primary_choices: Vec<PrimaryChoice>,
    keys: Vec<KeyStatus>,
}

#[derive(Debug, Serialize)]
struct PrimaryChoice {
    id: String,
    label: String,
}

#[derive(Debug, Serialize)]
struct KeyStatus {
    id: String,
    label: String,
    environment: String,
    secret_label: String,
    note: String,
    configured: bool,
    inline_configured: bool,
    environment_configured: bool,
}

/// Additive patch accepted on stdin by `ai-usagebar settings apply`.
/// Missing keys remain byte-for-byte untouched. `clear` explicitly removes an
/// inline key, matching the TUI overlay's existing empty-dirty-field behavior.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyRequest {
    schema_version: u8,
    primary: Option<String>,
    #[serde(default)]
    keys: BTreeMap<String, KeyMutation>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "lowercase", deny_unknown_fields)]
enum KeyMutation {
    Set { value: String },
    Clear,
}

const SETTINGS_SCHEMA_VERSION: u8 = 1;
const MAX_SETTINGS_REQUEST_BYTES: u64 = 64 * 1024;
const MAX_API_KEY_BYTES: usize = 16 * 1024;

fn snapshot_from_config_with(
    cfg: &Config,
    environment_configured: impl Fn(&str) -> bool,
) -> SettingsSnapshot {
    let state = SettingsState::from_config(cfg);
    let primary_choices = state
        .primary_choices
        .iter()
        .map(|id| PrimaryChoice {
            id: id.slug().to_string(),
            label: id.display_name().to_string(),
        })
        .collect();
    let keys = KEY_VENDORS
        .iter()
        .map(|vendor| {
            let environment = cfg.api_key_env_for(vendor.id);
            let inline_configured = cfg.inline_api_key(vendor.id).is_some();
            let environment_configured = environment_configured(environment);
            KeyStatus {
                id: vendor.id.slug().to_string(),
                label: vendor.label.to_string(),
                environment: environment.to_string(),
                secret_label: vendor.secret_label.to_string(),
                note: vendor.note.to_string(),
                configured: inline_configured || environment_configured,
                inline_configured,
                environment_configured,
            }
        })
        .collect();
    SettingsSnapshot {
        schema_version: SETTINGS_SCHEMA_VERSION,
        primary: state.primary.slug().to_string(),
        primary_choices,
        keys,
    }
}

fn settings_snapshot_json(cfg: &Config) -> Result<String> {
    Ok(serde_json::to_string(&snapshot_from_config_with(
        cfg,
        |environment| std::env::var_os(environment).is_some_and(|value| !value.is_empty()),
    ))?)
}

#[cfg(test)]
fn settings_snapshot_json_with(
    cfg: &Config,
    environment_configured: impl Fn(&str) -> bool,
) -> Result<String> {
    Ok(serde_json::to_string(&snapshot_from_config_with(
        cfg,
        environment_configured,
    ))?)
}

fn vendor_from_slug(slug: &str) -> Option<VendorId> {
    VendorId::all().iter().copied().find(|id| id.slug() == slug)
}

fn state_from_apply_request(cfg: &Config, raw: &str) -> Result<SettingsState> {
    let request: ApplyRequest = serde_json::from_str(raw)?;
    if request.schema_version != SETTINGS_SCHEMA_VERSION {
        return Err(AppError::Other(format!(
            "unsupported settings schema version {}",
            request.schema_version
        )));
    }

    let mut state = SettingsState::from_config(cfg);
    if let Some(primary) = request.primary {
        let id = vendor_from_slug(&primary)
            .ok_or_else(|| AppError::Other(format!("unknown primary vendor {primary:?}")))?;
        if !state.primary_choices.contains(&id) {
            return Err(AppError::Other(format!(
                "primary vendor {primary:?} is not enabled"
            )));
        }
        state.primary = id;
    }

    for (id, mutation) in request.keys {
        let index = KEY_VENDORS
            .iter()
            .position(|vendor| vendor.id.slug() == id)
            .ok_or_else(|| AppError::Other(format!("unknown credential vendor {id:?}")))?;
        let input = &mut state.keys[index];
        match mutation {
            KeyMutation::Set { value } => {
                if value.is_empty() {
                    return Err(AppError::Other(format!(
                        "{} for {id:?} is empty; use the clear action to remove it",
                        KEY_VENDORS[index].secret_label
                    )));
                }
                if value.len() > MAX_API_KEY_BYTES {
                    return Err(AppError::Other(format!(
                        "{} for {id:?} exceeds {MAX_API_KEY_BYTES} bytes",
                        KEY_VENDORS[index].secret_label
                    )));
                }
                if value.chars().any(char::is_control) {
                    return Err(AppError::Other(format!(
                        "{} for {id:?} contains control characters",
                        KEY_VENDORS[index].secret_label
                    )));
                }
                input.buf = value;
            }
            KeyMutation::Clear => input.buf.clear(),
        }
        input.cursor = input.buf.chars().count();
        input.dirty = true;
        input.revealed = false;
    }
    Ok(state)
}

#[cfg(test)]
fn apply_settings_json_to_path(cfg: &Config, raw: &str, path: &Path) -> Result<()> {
    let state = state_from_apply_request(cfg, raw)?;
    save_to_path(&state, path)
}

fn read_settings_request<R: BufRead>(reader: R) -> Result<String> {
    let mut limited = reader.take(MAX_SETTINGS_REQUEST_BYTES + 1);
    let mut bytes = Vec::new();
    limited.read_until(b'\n', &mut bytes)?;
    if bytes.len() as u64 > MAX_SETTINGS_REQUEST_BYTES {
        return Err(AppError::Other(format!(
            "settings request exceeds {MAX_SETTINGS_REQUEST_BYTES} bytes"
        )));
    }
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    String::from_utf8(bytes)
        .map_err(|_| AppError::Other("settings request is not valid UTF-8".into()))
}

fn apply_settings_from_stdin() -> Result<()> {
    let raw = read_settings_request(std::io::stdin().lock())?;
    let cfg = Config::load()?;
    let state = state_from_apply_request(&cfg, &raw)?;
    save_to_config_default(&state)
}

/// Administrative settings bridge for native frontends. `show` never emits a
/// secret; `apply` accepts its patch only over stdin so keys do not appear in
/// argv or the process environment.
pub fn run_cli(action: &crate::widget::cli::SettingsAction) -> i32 {
    let result = match action {
        crate::widget::cli::SettingsAction::Show => Config::load()
            .and_then(|cfg| settings_snapshot_json(&cfg))
            .map(|json| println!("{json}")),
        crate::widget::cli::SettingsAction::Apply => {
            apply_settings_from_stdin().map(|()| println!(r#"{{"ok":true}}"#))
        }
    };
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("settings: {error}");
            1
        }
    }
}

// ─── Render ────────────────────────────────────────────────────────────────

/// Render the modal overlay over `area`.
pub fn render(f: &mut Frame, area: Rect, state: &SettingsState, theme: &Theme) {
    let modal = centered_rect(74, 88, area);
    f.render_widget(Clear, modal);

    let bubble = bubble_theme(theme);
    let block = bubble.titled_modal_block(" Settings ");
    let inner = block.inner(modal);
    f.render_widget(block, modal);

    // Body (everything but the pinned hint) + a 1-line hint footer.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(inner);

    // — Primary vendor + credentials header —
    let mut lines: Vec<Line> = vec![
        section_header("Primary vendor", "shown first on the bar / TUI", &bubble),
        primary_line(state, &bubble),
        Line::from(""),
        section_header(
            "Credentials",
            "pick a row, type the credential, then Ctrl-S — Claude & Codex use CLI login",
            &bubble,
        ),
    ];
    for (i, kv) in KEY_VENDORS.iter().enumerate() {
        let focused = state.focus == Focus::Key(i);
        lines.push(key_row(kv, &state.keys[i], focused, &bubble));
    }
    lines.push(Line::from(""));

    // — Save + status —
    lines.push(save_line(state.focus == Focus::Save, &bubble));
    if !state.status.is_empty() {
        let ok = state.status.starts_with("saved");
        let mark = if ok { "  ✓ " } else { "  ✗ " };
        let style = if ok { bubble.accent } else { bubble.selected };
        lines.push(Line::from(vec![
            Span::styled(mark, style.add_modifier(Modifier::BOLD)),
            Span::styled(state.status.clone(), bubble.muted),
        ]));
    }

    f.render_widget(Paragraph::new(lines), chunks[0]);

    // Context-aware hint footer.
    let hint = match state.focus {
        Focus::Primary => bubble.help_line([
            ("↑↓/tab", "move"),
            ("←→", "change vendor"),
            ("^S", "save"),
            ("esc", "close"),
        ]),
        Focus::Key(_) => bubble.help_line([
            ("↑↓/tab", "move"),
            ("type", "edit key"),
            ("^V", "reveal"),
            ("^S", "save"),
            ("esc", "close"),
        ]),
        Focus::Save => {
            bubble.help_line([("↑↓/tab", "move"), ("enter/^S", "save"), ("esc", "close")])
        }
    };
    f.render_widget(Paragraph::new(hint), chunks[1]);
}

fn section_header(title: &str, sub: &str, theme: &BubbleTheme) -> Line<'static> {
    Line::from(vec![
        theme.span(" "),
        Span::styled(title.to_string(), theme.title.add_modifier(Modifier::BOLD)),
        theme.muted(format!("   — {sub}")),
    ])
}

fn primary_line(state: &SettingsState, theme: &BubbleTheme) -> Line<'static> {
    let focused = state.focus == Focus::Primary;
    let name = state.primary.display_name().to_string();
    if focused {
        Line::from(vec![
            theme.span("   "),
            Span::styled("▸ ", theme.accent.add_modifier(Modifier::BOLD)),
            Span::styled("◀ ", theme.accent),
            Span::styled(
                format!(" {name} "),
                theme
                    .selected
                    .add_modifier(Modifier::REVERSED | Modifier::BOLD),
            ),
            Span::styled(" ▶", theme.accent),
            theme.muted("    ← → to change"),
        ])
    } else {
        Line::from(vec![theme.span("     "), Span::styled(name, theme.text)])
    }
}

fn key_row(kv: &KeyVendor, input: &KeyInput, focused: bool, theme: &BubbleTheme) -> Line<'static> {
    let label = format!("{:<11}", kv.label);
    let value = value_text(input, focused);

    // Env / status suffix: env-var name, whether an env override is set, note.
    let env_name = kv.id.api_key_env();
    let env_set = std::env::var(env_name)
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    let mut suffix = format!("   {env_name}");
    if env_set {
        suffix.push_str(" · env set (overrides)");
    }
    if !kv.note.is_empty() {
        suffix.push_str(&format!(" · {}", kv.note));
    }

    if focused {
        let val_style = if input.buf.is_empty() {
            theme.accent.add_modifier(Modifier::BOLD)
        } else {
            theme.selected.add_modifier(Modifier::REVERSED)
        };
        let mut spans = vec![
            theme.span("  "),
            Span::styled("▸ ", theme.accent.add_modifier(Modifier::BOLD)),
            Span::styled(label, theme.title.add_modifier(Modifier::BOLD)),
            Span::styled(format!(" {value} "), val_style),
        ];
        if input.revealed {
            spans.push(theme.muted("  [revealed]"));
        }
        spans.push(theme.muted(suffix));
        Line::from(spans)
    } else {
        let val_style = if input.buf.is_empty() {
            theme.muted
        } else {
            theme.text
        };
        Line::from(vec![
            theme.span("    "),
            Span::styled(label, theme.text),
            Span::styled(format!(" {value}"), val_style),
            theme.muted(suffix),
        ])
    }
}

/// The value column: `(empty)` / a cursor when focused-empty / masked or
/// revealed buffer with a cursor mark inserted when focused.
fn value_text(input: &KeyInput, focused: bool) -> String {
    if input.buf.is_empty() {
        return if focused {
            "‸".to_string()
        } else {
            "(empty)".to_string()
        };
    }
    let base = input.display();
    if !focused {
        return base;
    }
    let mut chars: Vec<char> = base.chars().collect();
    let pos = input.cursor.min(chars.len());
    chars.insert(pos, '‸');
    chars.into_iter().collect()
}

fn save_line(focused: bool, theme: &BubbleTheme) -> Line<'static> {
    let style = if focused {
        theme
            .selected
            .add_modifier(Modifier::REVERSED | Modifier::BOLD)
    } else {
        theme.accent.add_modifier(Modifier::BOLD)
    };
    let marker = if focused { "▸ " } else { "  " };
    Line::from(vec![
        theme.span("   "),
        Span::styled(marker, theme.accent.add_modifier(Modifier::BOLD)),
        Span::styled("  Save  (Ctrl-S)  ", style),
    ])
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

// crossterm types live behind ratatui; re-exported here for handle_key callers.
pub use ratatui::crossterm::event::{KeyCode, KeyModifiers};

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_config(initial: Option<&str>) -> (TempDir, std::path::PathBuf) {
        crate::cache::closed_temp_file("config.toml", initial)
    }

    fn key_index(id: VendorId) -> usize {
        KEY_VENDORS.iter().position(|kv| kv.id == id).unwrap()
    }

    fn blank_state(primary: VendorId) -> SettingsState {
        SettingsState {
            focus: Focus::Primary,
            primary_choices: VendorId::all().to_vec(),
            primary,
            keys: KEY_VENDORS.iter().map(|_| KeyInput::default()).collect(),
            status: String::new(),
        }
    }

    /// State with a Z.AI key and an OpenRouter key, both marked dirty.
    fn state_with(zai: &str, opr: &str, primary: VendorId) -> SettingsState {
        let mut s = blank_state(primary);
        s.keys[key_index(VendorId::Zai)] = KeyInput::from_config(Some(zai));
        s.keys[key_index(VendorId::Zai)].dirty = true;
        s.keys[key_index(VendorId::Openrouter)] = KeyInput::from_config(Some(opr));
        s.keys[key_index(VendorId::Openrouter)].dirty = true;
        s
    }

    #[test]
    fn focus_cycles_through_primary_all_keys_and_save() {
        let mut f = Focus::Primary;
        let mut seen = vec![f];
        // Full cycle = Primary + N key rows + Save.
        for _ in 0..(KEY_VENDORS.len() + 2) {
            f = f.next();
            seen.push(f);
        }
        // Primary, Key(0..n), Save, back to Primary.
        assert_eq!(seen.first(), Some(&Focus::Primary));
        assert_eq!(seen.last(), Some(&Focus::Primary));
        assert!(seen.contains(&Focus::Key(0)));
        assert!(seen.contains(&Focus::Key(KEY_VENDORS.len() - 1)));
        assert!(seen.contains(&Focus::Save));
        // prev() is the inverse of next().
        assert_eq!(Focus::Primary.next().prev(), Focus::Primary);
        assert_eq!(Focus::Save.prev().next(), Focus::Save);
        assert_eq!(Focus::Primary.prev(), Focus::Save);
    }

    #[test]
    fn every_key_vendor_has_a_field() {
        // Every enabled-by-key vendor must be reachable in the form.
        for id in [
            VendorId::Zai,
            VendorId::Openrouter,
            VendorId::Deepseek,
            VendorId::Kilo,
            VendorId::Novita,
            VendorId::Moonshot,
            VendorId::Grok,
        ] {
            assert!(
                KEY_VENDORS.iter().any(|kv| kv.id == id),
                "{id:?} has no key field"
            );
        }
        // OAuth vendors are intentionally absent.
        assert!(!KEY_VENDORS.iter().any(|kv| kv.id == VendorId::Anthropic));
        assert!(!KEY_VENDORS.iter().any(|kv| kv.id == VendorId::Openai));
    }

    #[test]
    fn from_config_prefills_existing_keys() {
        let mut cfg = Config::default();
        cfg.kilo.api_key = Some("sk-kilo".into());
        let s = SettingsState::from_config(&cfg);
        assert_eq!(s.keys[key_index(VendorId::Kilo)].buf, "sk-kilo");
        assert!(!s.keys[key_index(VendorId::Kilo)].dirty);
    }

    #[test]
    fn copilot_has_no_editable_credential_field() {
        assert!(
            !KEY_VENDORS
                .iter()
                .any(|vendor| vendor.id == VendorId::Copilot)
        );
    }

    #[test]
    fn from_config_offers_enabled_vendors_only() {
        let cfg = Config::default();
        let s = SettingsState::from_config(&cfg);
        let mut expected = cfg.enabled_vendors();
        expected.push(VendorId::Copilot);
        assert_eq!(s.primary_choices, expected);
        // API-key opt-in vendors are disabled by default and must not be
        // offered. Copilot is the exception: choosing it enables it safely.
        assert!(!s.primary_choices.contains(&VendorId::Grok));
        assert!(s.primary_choices.contains(&s.primary));
        assert!(s.primary_choices.contains(&VendorId::Copilot));
    }

    #[test]
    fn from_config_falls_back_when_configured_primary_is_disabled() {
        // Grok is opt-in; a config naming it as primary without enabling it
        // must display the first enabled vendor instead.
        let mut cfg = Config::default();
        cfg.ui.primary = Some(VendorId::Grok);
        let s = SettingsState::from_config(&cfg);
        assert_ne!(s.primary, VendorId::Grok);
        assert_eq!(Some(s.primary), cfg.enabled_vendors().first().copied());
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
        k.insert_char('x');
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
        k.backspace();
        assert_eq!(k.buf, "ab");
    }

    #[test]
    fn value_text_shows_cursor_and_empty_states() {
        let mut k = KeyInput::default();
        assert_eq!(value_text(&k, false), "(empty)");
        assert_eq!(value_text(&k, true), "‸");
        k.insert_char('a');
        k.insert_char('b');
        // masked + cursor at end
        assert_eq!(value_text(&k, true), "••‸");
        assert_eq!(value_text(&k, false), "••");
    }

    #[test]
    fn save_writes_key_and_enables_vendor() {
        let (_dir, path) = temp_config(None);
        let mut s = blank_state(VendorId::Kilo);
        s.keys[key_index(VendorId::Kilo)] = KeyInput::from_config(Some("sk-kilo"));
        s.keys[key_index(VendorId::Kilo)].dirty = true;
        save_to_path(&s, &path).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("primary = \"kilo\""));
        assert!(raw.contains("[kilo]"));
        assert!(raw.contains("api_key = \"sk-kilo\""));
        assert!(raw.contains("enabled = true"));
    }

    #[test]
    fn save_writes_minimal_toml_when_starting_empty() {
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
    fn save_preserves_existing_comments_and_unrelated_fields() {
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

[[openrouter.accounts]]
label = "work"
api_key_env = "OPENROUTER_WORK_API_KEY"
"##,
        ));

        let s = state_with("zk2", "ok2", VendorId::Openrouter);
        save_to_path(&s, &path).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("# my comment"));
        assert!(raw.contains("# pre-existing comment"));
        assert!(raw.contains("# tier comment"));
        assert!(raw.contains("api_key_env = \"ZAI_API_KEY\""));
        assert!(raw.contains("[[openrouter.accounts]]"));
        assert!(raw.contains("api_key_env = \"OPENROUTER_WORK_API_KEY\""));
        assert!(raw.contains("plan_tier = \"pro\""));
        assert!(raw.contains("primary = \"openrouter\""));
        assert!(raw.contains("api_key = \"zk2\""));
        assert!(raw.contains("api_key = \"ok2\""));
    }

    #[test]
    fn save_refuses_to_replace_an_unreadable_existing_config() {
        let (_dir, path) = temp_config(None);
        let original = [0xff, 0xfe, 0xfd];
        std::fs::write(&path, original).unwrap();
        let state = state_with("new-secret", "", VendorId::Zai);

        assert!(save_to_path(&state, &path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[test]
    fn save_does_not_write_empty_key_when_dirty_but_blank() {
        let (_dir, path) = temp_config(None);
        let mut s = blank_state(VendorId::Anthropic);
        // Focus each key, do nothing but mark dirty (blank).
        for k in &mut s.keys {
            k.dirty = true;
        }
        save_to_path(&s, &path).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
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
    fn tab_cycles_focus_from_primary_to_first_key() {
        let mut s = blank_state(VendorId::Anthropic);
        assert_eq!(
            handle_key(&mut s, KeyCode::Tab, KeyModifiers::NONE),
            Action::Continue
        );
        assert_eq!(s.focus, Focus::Key(0));
        assert_eq!(
            handle_key(&mut s, KeyCode::BackTab, KeyModifiers::NONE),
            Action::Continue
        );
        assert_eq!(s.focus, Focus::Primary);
    }

    #[test]
    fn esc_closes_without_saving() {
        let mut s = blank_state(VendorId::Anthropic);
        assert_eq!(
            handle_key(&mut s, KeyCode::Esc, KeyModifiers::NONE),
            Action::Close
        );
    }

    #[test]
    fn left_right_cycles_primary_vendor() {
        // Canonical order (VendorId::all): Anthropic, AnthropicApi, Openai, …
        let mut s = blank_state(VendorId::Anthropic);
        handle_key(&mut s, KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(s.primary, VendorId::AnthropicApi);
        handle_key(&mut s, KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(s.primary, VendorId::Openai);
        handle_key(&mut s, KeyCode::Left, KeyModifiers::NONE);
        assert_eq!(s.primary, VendorId::AnthropicApi);
    }

    #[test]
    fn left_right_offers_enabled_vendors_only() {
        // The selector must never land on a vendor the widget cannot use.
        let mut s = blank_state(VendorId::Anthropic);
        s.primary_choices = vec![VendorId::Anthropic, VendorId::Grok];
        handle_key(&mut s, KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(s.primary, VendorId::Grok);
        // Wraps within the enabled set rather than walking into disabled ones.
        handle_key(&mut s, KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(s.primary, VendorId::Anthropic);
        handle_key(&mut s, KeyCode::Left, KeyModifiers::NONE);
        assert_eq!(s.primary, VendorId::Grok);
    }

    #[test]
    fn no_enabled_vendors_leaves_primary_selector_inert() {
        let mut s = blank_state(VendorId::Anthropic);
        s.primary_choices = vec![];
        handle_key(&mut s, KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(s.primary, VendorId::Anthropic);
    }

    #[test]
    fn disabled_copilot_is_offered_but_not_shown_as_the_current_primary() {
        let mut cfg = Config::default();
        cfg.ui.primary = Some(VendorId::Copilot);
        let state = SettingsState::from_config(&cfg);

        assert!(state.primary_choices.contains(&VendorId::Copilot));
        assert_eq!(state.primary, VendorId::Anthropic);
    }

    #[test]
    fn save_does_not_write_a_disabled_primary() {
        // Saving an API key must not persist a primary the resolver would
        // ignore; an existing value in the file stays untouched.
        let (_dir, path) = temp_config(Some("[ui]\nprimary = \"anthropic\"\n"));
        let mut s = state_with("zk", "ok", VendorId::Grok);
        s.primary_choices = vec![VendorId::Anthropic];
        save_to_path(&s, &path).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("primary = \"anthropic\""));
        assert!(!raw.contains("primary = \"grok\""));
        // The keys still saved.
        assert!(raw.contains("zk"));
    }

    #[test]
    fn save_removes_an_inline_key_the_user_cleared() {
        // Clearing the field in the overlay must delete the secret from the
        // file — otherwise there is no way to remove it short of hand-editing.
        let (_dir, path) = temp_config(Some(
            "[zai]\nenabled = true\napi_key = \"old-secret\"\nplan_tier = \"pro\"\n",
        ));
        let mut s = blank_state(VendorId::Zai);
        s.primary_choices = vec![VendorId::Zai];
        s.keys[key_index(VendorId::Zai)] = KeyInput::default();
        s.keys[key_index(VendorId::Zai)].dirty = true;
        save_to_path(&s, &path).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("old-secret"));
        assert!(!raw.contains("api_key"));
        // Unrelated fields in the same section survive.
        assert!(raw.contains("plan_tier = \"pro\""));
    }

    #[test]
    fn untouched_key_field_is_left_alone() {
        // Not dirty => the file's existing secret must survive a save.
        let (_dir, path) = temp_config(Some("[zai]\napi_key = \"keep-me\"\n"));
        let mut s = blank_state(VendorId::Zai);
        s.primary_choices = vec![VendorId::Zai];
        save_to_path(&s, &path).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("keep-me"));
    }

    #[test]
    fn typing_edits_the_focused_key_only() {
        let mut s = blank_state(VendorId::Anthropic);
        s.focus = Focus::Key(key_index(VendorId::Grok));
        for c in "xai-abc".chars() {
            handle_key(&mut s, KeyCode::Char(c), KeyModifiers::NONE);
        }
        assert_eq!(s.keys[key_index(VendorId::Grok)].buf, "xai-abc");
        assert!(s.keys[key_index(VendorId::Grok)].dirty);
        // No other field was touched.
        assert!(s.keys[key_index(VendorId::Zai)].buf.is_empty());
    }

    #[test]
    fn ctrl_v_toggles_reveal_on_focused_key_field() {
        let mut s = blank_state(VendorId::Anthropic);
        let zi = key_index(VendorId::Zai);
        s.focus = Focus::Key(zi);
        s.keys[zi] = KeyInput::from_config(Some("secret"));
        assert!(!s.keys[zi].revealed);
        handle_key(&mut s, KeyCode::Char('v'), KeyModifiers::CONTROL);
        assert!(s.keys[zi].revealed);
        handle_key(&mut s, KeyCode::Char('v'), KeyModifiers::CONTROL);
        assert!(!s.keys[zi].revealed);
    }

    #[test]
    fn control_chorded_chars_do_not_type_into_fields() {
        let mut s = blank_state(VendorId::Anthropic);
        s.focus = Focus::Key(0);
        // Ctrl-A must NOT insert a literal 'a' or mark the field dirty.
        handle_key(&mut s, KeyCode::Char('a'), KeyModifiers::CONTROL);
        assert!(s.keys[0].buf.is_empty());
        assert!(!s.keys[0].dirty);
        // Ctrl-C quits the host TUI even while the overlay owns focus.
        assert_eq!(
            handle_key(&mut s, KeyCode::Char('c'), KeyModifiers::CONTROL),
            Action::Quit
        );
        // A plain char still types normally.
        handle_key(&mut s, KeyCode::Char('x'), KeyModifiers::NONE);
        assert_eq!(s.keys[0].buf, "x");
    }

    #[test]
    fn ctrl_v_on_non_key_focus_is_noop() {
        let mut s = blank_state(VendorId::Anthropic);
        s.focus = Focus::Primary;
        // Must not panic when no key field is focused.
        assert_eq!(
            handle_key(&mut s, KeyCode::Char('v'), KeyModifiers::CONTROL),
            Action::Continue
        );
    }

    fn state_focused_on_zai() -> SettingsState {
        let mut state = blank_state(VendorId::Anthropic);
        state.focus = Focus::Key(key_index(VendorId::Zai));
        state
    }

    #[test]
    fn handle_key_ctrl_c_quits_without_typing_into_key_field() {
        let mut s = state_focused_on_zai();
        let zi = key_index(VendorId::Zai);
        assert_eq!(
            handle_key(&mut s, KeyCode::Char('c'), KeyModifiers::CONTROL),
            Action::Quit
        );
        assert!(s.keys[zi].buf.is_empty());
        // Untouched means save still leaves an existing key on disk alone.
        assert!(!s.keys[zi].dirty);
    }

    #[test]
    fn handle_key_alt_chord_does_not_type_into_key_field() {
        let mut s = state_focused_on_zai();
        let zi = key_index(VendorId::Zai);
        handle_key(&mut s, KeyCode::Char('x'), KeyModifiers::ALT);
        assert!(s.keys[zi].buf.is_empty());
        assert!(!s.keys[zi].dirty);
    }

    #[test]
    fn handle_key_platform_modifier_chords_do_not_type_into_key_field() {
        for modifier in [KeyModifiers::SUPER, KeyModifiers::HYPER, KeyModifiers::META] {
            let mut s = state_focused_on_zai();
            let zi = key_index(VendorId::Zai);
            handle_key(&mut s, KeyCode::Char('x'), modifier);
            assert!(s.keys[zi].buf.is_empty(), "modifier {modifier:?}");
            assert!(!s.keys[zi].dirty, "modifier {modifier:?}");
        }
    }

    #[test]
    fn handle_key_shift_still_types_uppercase() {
        let mut s = state_focused_on_zai();
        let zi = key_index(VendorId::Zai);
        handle_key(&mut s, KeyCode::Char('A'), KeyModifiers::SHIFT);
        assert_eq!(s.keys[zi].buf, "A");
        assert!(s.keys[zi].dirty);
    }

    #[test]
    fn handle_key_plain_space_still_cycles_primary_vendor() {
        let mut s = blank_state(VendorId::Anthropic);
        handle_key(&mut s, KeyCode::Char(' '), KeyModifiers::NONE);
        assert_eq!(s.primary, VendorId::AnthropicApi);
    }

    #[test]
    fn handle_key_ctrl_s_attempts_save_from_any_field() {
        let (_dir, path) = temp_config(None);
        let s = state_with("zk", "ok", VendorId::Zai);
        save_to_path(&s, &path).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("api_key = \"zk\""));
    }
    #[test]
    fn save_to_path_writes_kimi_key_when_dirty() {
        let (_dir, path) = temp_config(None);
        let mut s = blank_state(VendorId::Anthropic);
        let kimi = key_index(VendorId::Kimi);
        s.keys[kimi] = KeyInput::from_config(Some("kk"));
        s.keys[kimi].dirty = true;
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

    #[test]
    fn native_snapshot_reports_key_state_without_serializing_secrets() {
        let mut cfg = Config::default();
        cfg.zai.api_key = Some("never-leak-this-key".into());
        cfg.zai.api_key_env = "CUSTOM_ZAI_KEY".into();
        let raw = settings_snapshot_json_with(&cfg, |name| name == "CUSTOM_ZAI_KEY").unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();

        assert_eq!(parsed["schema_version"], 1);
        assert_eq!(parsed["primary"], "anthropic");
        let zai = parsed["keys"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == "zai")
            .unwrap();
        assert_eq!(zai["configured"], true);
        assert_eq!(zai["inline_configured"], true);
        assert_eq!(zai["environment_configured"], true);
        assert_eq!(zai["environment"], "CUSTOM_ZAI_KEY");
        assert!(!raw.contains("never-leak-this-key"));
        assert!(parsed.get("api_key").is_none());
    }

    #[test]
    fn native_snapshot_offers_copilot_primary_without_a_token_field() {
        let cfg = Config::default();
        let raw = settings_snapshot_json_with(&cfg, |_| false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(
            parsed["primary_choices"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["id"] == "copilot")
        );
        assert!(
            !parsed["keys"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["id"] == "copilot")
        );
    }

    #[test]
    fn native_key_only_patch_does_not_require_or_replace_primary() {
        let cfg = Config::default();
        let original_primary = SettingsState::from_config(&cfg).primary;
        let request = serde_json::json!({
            "schema_version": 1,
            "keys": {"kimi": {"action": "set", "value": "new-kimi-key"}}
        });

        let state = state_from_apply_request(&cfg, &request.to_string()).unwrap();
        assert_eq!(state.primary, original_primary);
        let kimi_index = KEY_VENDORS
            .iter()
            .position(|vendor| vendor.id == VendorId::Kimi)
            .unwrap();
        assert!(state.keys[kimi_index].dirty);
        assert_eq!(state.keys[kimi_index].buf, "new-kimi-key");
    }

    #[test]
    fn native_patch_reuses_tui_persistence_and_preserves_existing_config() {
        let (_dir, path) = temp_config(Some(
            r#"# keep this comment
[ui]
primary = "anthropic"

[zai]
enabled = true
api_key_env = "ZAI_API_KEY"
plan_tier = "pro"

[openrouter]
enabled = true
"#,
        ));
        let cfg = Config::load_from(&path).unwrap();
        let request = serde_json::json!({
            "schema_version": 1,
            "primary": "openrouter",
            "keys": {
                "zai": {"action": "set", "value": "new-zai-key"}
            }
        });

        apply_settings_json_to_path(&cfg, &request.to_string(), &path).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("# keep this comment"));
        assert!(raw.contains("plan_tier = \"pro\""));
        assert!(raw.contains("api_key_env = \"ZAI_API_KEY\""));
        assert!(raw.contains("primary = \"openrouter\""));
        assert!(raw.contains("api_key = \"new-zai-key\""));
    }

    #[test]
    fn native_patch_distinguishes_clear_from_unchanged() {
        let (_dir, path) = temp_config(Some(
            "[zai]\nenabled = true\napi_key = \"remove-me\"\n\
             [openrouter]\nenabled = true\napi_key = \"keep-me\"\n",
        ));
        let cfg = Config::load_from(&path).unwrap();
        let request = serde_json::json!({
            "schema_version": 1,
            "primary": "zai",
            "keys": {"zai": {"action": "clear"}}
        });

        apply_settings_json_to_path(&cfg, &request.to_string(), &path).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("remove-me"));
        assert!(raw.contains("keep-me"));
    }

    #[test]
    fn native_primary_selection_enables_copilot_without_writing_a_token() {
        let (_dir, path) = temp_config(Some(
            "[copilot]\nenabled = false\ntoken = \"legacy-value\"\ntoken_env = \"OLD_TOKEN\"\n",
        ));
        let cfg = Config::load_from(&path).unwrap();
        let select = serde_json::json!({
            "schema_version": 1,
            "primary": "copilot"
        });
        apply_settings_json_to_path(&cfg, &select.to_string(), &path).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("enabled = true"));
        assert!(raw.contains("primary = \"copilot\""));
        assert!(!raw.contains("token ="));
        assert!(!raw.contains("token_env ="));
    }

    #[test]
    fn native_patch_errors_never_echo_key_values() {
        let raw = serde_json::json!({
            "schema_version": 1,
            "primary": "anthropic",
            "keys": {
                "zai": {"action": "set", "value": "secret\nwith-control"}
            }
        })
        .to_string();
        let error = state_from_apply_request(&Config::default(), &raw)
            .unwrap_err()
            .to_string();
        assert!(!error.contains("secret"));
        assert!(error.contains("control characters"));
    }

    #[test]
    fn native_patch_input_is_bounded_before_json_parsing() {
        let oversized = vec![b'x'; MAX_SETTINGS_REQUEST_BYTES as usize + 1];
        let error = read_settings_request(std::io::Cursor::new(oversized))
            .unwrap_err()
            .to_string();
        assert!(error.contains("exceeds"));
    }
}
