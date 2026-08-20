//! Settings overlay — opened from the TUI by pressing `s`. Lets the user pick
//! the primary vendor and paste an API key for any key-authenticated vendor
//! (including Z.AI, Kimi, MiniMax, and the balance vendors) without hand-editing
//! config.toml. Anthropic, OpenAI, Cursor, and Antigravity authenticate through
//! local product state, so they have no key field here.
//!
//! Persistence uses `toml_edit` so the existing config keeps its comments,
//! whitespace, and unrelated fields. Writing a key also flips that vendor's
//! `enabled = true` (the opt-in vendors are disabled by default), so "paste the
//! key and save" is all it takes. Files with inline keys are atomically written
//! and `chmod 600`ed.

use std::collections::BTreeMap;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui_bubbletea_theme::BubbleTheme;
use serde::{Deserialize, Serialize};
use toml_edit::{DocumentMut, value};

use crate::config::{Config, SyncCategory};
use crate::error::{AppError, Result};
use crate::theme::Theme;
use crate::tui::style::bubble_theme;
use crate::vendor::VendorId;

/// A vendor that authenticates with an inline API key (vs. OAuth). The order of
/// this table is the tab order of the key fields and the layout of the state's
/// `keys` vec.
pub struct KeyVendor {
    pub id: VendorId,
    pub label: &'static str,
    pub env: &'static str,
    pub section: &'static str,
    /// Extra hint after the env var (e.g. "management key"). Empty for none.
    pub note: &'static str,
}

pub const KEY_VENDORS: &[KeyVendor] = &[
    KeyVendor {
        id: VendorId::AnthropicApi,
        label: "Anthropic API",
        env: "ANTHROPIC_ADMIN_KEY",
        section: "anthropic_api",
        note: "admin key — monthly spend",
    },
    KeyVendor {
        id: VendorId::Zai,
        label: "Z.AI",
        env: "ZAI_API_KEY",
        section: "zai",
        note: "",
    },
    KeyVendor {
        id: VendorId::Openrouter,
        label: "OpenRouter",
        env: "OPENROUTER_API_KEY",
        section: "openrouter",
        note: "",
    },
    KeyVendor {
        id: VendorId::Deepseek,
        label: "DeepSeek",
        env: "DEEPSEEK_API_KEY",
        section: "deepseek",
        note: "",
    },
    KeyVendor {
        id: VendorId::Kimi,
        label: "Kimi",
        env: "KIMI_API_KEY",
        section: "kimi",
        note: "coding-plan usage",
    },
    KeyVendor {
        id: VendorId::Kilo,
        label: "Kilo",
        env: "KILO_API_KEY",
        section: "kilo",
        note: "",
    },
    KeyVendor {
        id: VendorId::Novita,
        label: "Novita",
        env: "NOVITA_API_KEY",
        section: "novita",
        note: "",
    },
    KeyVendor {
        id: VendorId::Moonshot,
        label: "Moonshot",
        env: "MOONSHOT_API_KEY",
        section: "moonshot",
        note: "account balance",
    },
    KeyVendor {
        id: VendorId::Grok,
        label: "Grok",
        env: "XAI_MANAGEMENT_KEY",
        section: "grok",
        note: "management key, not the inference key",
    },
    KeyVendor {
        id: VendorId::Minimax,
        label: "MiniMax",
        env: "MINIMAX_API_KEY",
        section: "minimax",
        note: "Token Plan subscription key",
    },
    KeyVendor {
        id: VendorId::OpenCodeGo,
        label: "OpenCode Go",
        env: "OPENCODE_GO_API_KEY",
        section: "opencode-go",
        note: "usage quota",
    },
];

/// Read the inline `api_key` currently in config for a given section, so the
/// field opens pre-filled (masked) when one is already set.
fn config_inline_key<'a>(cfg: &'a Config, section: &str) -> Option<&'a str> {
    match section {
        "anthropic_api" => cfg.anthropic_api.api_key.as_deref(),
        "zai" => cfg.zai.api_key.as_deref(),
        "openrouter" => cfg.openrouter.api_key.as_deref(),
        "deepseek" => cfg.deepseek.api_key.as_deref(),
        "kimi" => cfg.kimi.api_key.as_deref(),
        "kilo" => cfg.kilo.api_key.as_deref(),
        "novita" => cfg.novita.api_key.as_deref(),
        "moonshot" => cfg.moonshot.api_key.as_deref(),
        "grok" => cfg.grok.api_key.as_deref(),
        "minimax" => cfg.minimax.api_key.as_deref(),
        "opencode-go" => cfg.opencode_go.api_key.as_deref(),
        _ => None,
    }
}

/// Which control has keyboard focus. `Key(i)` indexes into [`KEY_VENDORS`];
/// `SyncCategory(i)` indexes into [`SyncCategory::ALL`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Primary,
    Key(usize),
    SyncCategory(usize),
    Save,
}

impl Focus {
    pub fn next(self) -> Self {
        match self {
            Focus::Primary => Focus::Key(0),
            Focus::Key(i) if i + 1 < KEY_VENDORS.len() => Focus::Key(i + 1),
            Focus::Key(_) => Focus::SyncCategory(0),
            Focus::SyncCategory(i) if i + 1 < SyncCategory::ALL.len() => Focus::SyncCategory(i + 1),
            Focus::SyncCategory(_) => Focus::Save,
            Focus::Save => Focus::Primary,
        }
    }
    pub fn prev(self) -> Self {
        match self {
            Focus::Primary => Focus::Save,
            Focus::Key(0) => Focus::Primary,
            Focus::Key(i) => Focus::Key(i - 1),
            Focus::SyncCategory(0) => Focus::Key(KEY_VENDORS.len() - 1),
            Focus::SyncCategory(i) => Focus::SyncCategory(i - 1),
            Focus::Save => Focus::SyncCategory(SyncCategory::ALL.len() - 1),
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
    /// What encrypted sync collects — one row per [`SyncCategory::ALL`] entry,
    /// in that order. This is a projection of [`crate::config::SyncConfig::categories`],
    /// not a parallel truth: it round-trips through the same TOML key.
    pub sync_categories: Vec<(SyncCategory, bool)>,
    /// When sync last completed, as the local index reports it. `None` is the
    /// normal never-synced answer *and* the answer when the index could not be
    /// read — the same conflation `ai-usagebar sync status` already makes.
    ///
    /// It arrives on the state rather than being read here: the overlay must
    /// not open a database (nor walk a transcript tree) to draw itself.
    pub sync_last_sync: Option<DateTime<Utc>>,
    /// True once the user has toggled a row. Only then does save write
    /// `[sync] categories` — an untouched save must not turn "never chose"
    /// into a persisted choice, the same discipline the primary selector and
    /// the key fields already follow.
    pub sync_dirty: bool,
    /// One-line status displayed in the footer ("saved …", "save failed …").
    pub status: String,
}

impl SettingsState {
    /// Pure: config in, state out. No filesystem, no clock, no `$HOME`.
    /// Last-sync is unknown here; see [`SettingsState::from_config_with_sync`].
    pub fn from_config(cfg: &Config) -> Self {
        Self::from_config_with_sync(cfg, None)
    }

    /// Same, plus the last-sync instant the caller already had. The caller
    /// owns that read because it is the one that may touch the index file.
    pub fn from_config_with_sync(cfg: &Config, last_sync: Option<DateTime<Utc>>) -> Self {
        let keys = KEY_VENDORS
            .iter()
            .map(|kv| KeyInput::from_config(config_inline_key(cfg, kv.section)))
            .collect();
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
        let sync_categories = SyncCategory::ALL
            .iter()
            .map(|cat| (*cat, cfg.sync.includes(*cat)))
            .collect();
        Self {
            focus: Focus::Primary,
            primary_choices,
            primary,
            keys,
            sync_categories,
            sync_last_sync: last_sync,
            sync_dirty: false,
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
        Focus::SyncCategory(i) => toggle_sync_category(state, i, code),
        Focus::Save => {
            if matches!(code, KeyCode::Enter) {
                return try_save(state);
            }
        }
    }
    Action::Continue
}

/// Space or Enter flips exactly one row. Deliberately **not** Left/Right:
/// those mean "cycle a choice" on the Primary row, and a mis-aimed arrow must
/// not be able to change which credentials are eligible to leave the machine
/// (T-6-20). Modifier chords were already swallowed above.
fn toggle_sync_category(state: &mut SettingsState, index: usize, code: KeyCode) {
    if !matches!(code, KeyCode::Char(' ') | KeyCode::Enter) {
        return;
    }
    if let Some(row) = state.sync_categories.get_mut(index) {
        row.1 = !row.1;
        state.sync_dirty = true;
    }
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
/// tests. Writing a non-empty key also sets that vendor's `enabled = true`.
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

    // Do not write a disabled primary as a side effect of saving an API key.
    // With no enabled vendors, leave any existing value alone so the legacy
    // resolver's Anthropic fallback remains intact.
    if state.primary_choices.contains(&state.primary) {
        set_string(&mut doc, "ui", "primary", state.primary.slug())?;
    }

    for (i, kv) in KEY_VENDORS.iter().enumerate() {
        let Some(input) = state.keys.get(i) else {
            continue;
        };
        update_key(&mut doc, kv.section, input)?;
    }

    update_sync_categories(&mut doc, state)?;

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

/// Apply one key field to the document. Untouched fields are left alone; a
/// field the user cleared is *removed*, so an inline secret can be deleted
/// from the overlay rather than lingering in the file. Writing a non-empty key
/// also opts the vendor in — the opt-in vendors would otherwise never fetch.
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
    set_string(doc, section, "api_key", &input.buf)?;
    set_bool(doc, section, "enabled", true)
}

/// Write `[sync] categories` from the overlay's rows.
///
/// Untouched rows write nothing: opening Settings to paste one API key must
/// not also commit the user to a sync selection they never made, and a missing
/// key means "the default" while an empty array means "nothing" — two
/// different statements (T-6-22). An empty selection is therefore written
/// explicitly and never elided.
///
/// The labels come from [`SyncCategory::label`], the same spelling the config
/// parser reads, so there is one place the token is spelled.
fn update_sync_categories(doc: &mut DocumentMut, state: &SettingsState) -> Result<()> {
    if !state.sync_dirty {
        return Ok(());
    }
    let mut array = toml_edit::Array::new();
    for (cat, _) in state.sync_categories.iter().filter(|(_, on)| *on) {
        array.push(cat.label());
    }

    let table = doc
        .entry("sync")
        .or_insert_with(toml_edit::table)
        .as_table_mut()
        .ok_or_else(|| AppError::Other("config.toml: [sync] is not a table".into()))?;

    if let Some(item) = table.get_mut("categories")
        && let Some(v) = item.as_value_mut()
    {
        *v = toml_edit::Value::Array(array);
        v.decor_mut().set_prefix(" ");
        return Ok(());
    }
    table.insert("categories", value(array));
    Ok(())
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

fn configured_key_env<'a>(cfg: &'a Config, section: &str, fallback: &'a str) -> &'a str {
    match section {
        "anthropic_api" => &cfg.anthropic_api.api_key_env,
        "zai" => &cfg.zai.api_key_env,
        "openrouter" => &cfg.openrouter.api_key_env,
        "deepseek" => &cfg.deepseek.api_key_env,
        "kimi" => &cfg.kimi.api_key_env,
        "kilo" => &cfg.kilo.api_key_env,
        "novita" => &cfg.novita.api_key_env,
        "moonshot" => &cfg.moonshot.api_key_env,
        "grok" => &cfg.grok.api_key_env,
        "minimax" => &cfg.minimax.api_key_env,
        "opencode-go" => &cfg.opencode_go.api_key_env,
        _ => fallback,
    }
}

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
            let environment = configured_key_env(cfg, vendor.section, vendor.env);
            let inline_configured =
                config_inline_key(cfg, vendor.section).is_some_and(|v| !v.is_empty());
            let environment_configured = environment_configured(environment);
            KeyStatus {
                id: vendor.id.slug().to_string(),
                label: vendor.label.to_string(),
                environment: environment.to_string(),
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
            .ok_or_else(|| AppError::Other(format!("unknown API-key vendor {id:?}")))?;
        let input = &mut state.keys[index];
        match mutation {
            KeyMutation::Set { value } => {
                if value.is_empty() {
                    return Err(AppError::Other(format!(
                        "API key for {id:?} is empty; use the clear action to remove it"
                    )));
                }
                if value.len() > MAX_API_KEY_BYTES {
                    return Err(AppError::Other(format!(
                        "API key for {id:?} exceeds {MAX_API_KEY_BYTES} bytes"
                    )));
                }
                if value.chars().any(char::is_control) {
                    return Err(AppError::Other(format!(
                        "API key for {id:?} contains control characters"
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
    // 96 rather than 88: the Sync block added nine lines, and a modal whose
    // Save row falls off the bottom of a short terminal is worse than a thin
    // margin. Below roughly 30 rows the Paragraph still truncates — it does
    // not scroll, and it does not panic.
    let modal = centered_rect(74, 96, area);
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

    // — Primary vendor + API keys header —
    let mut lines: Vec<Line> = vec![
        section_header("Primary vendor", "shown first on the bar / TUI", &bubble),
        primary_line(state, &bubble),
        Line::from(""),
        section_header(
            "API keys",
            "pick a row, type the key, then Ctrl-S — Claude & Codex use CLI login",
            &bubble,
        ),
    ];
    // Which body line the focused control sits on, so a terminal too short to
    // hold the whole form scrolls to it rather than hiding it. Recorded while
    // building rather than derived from a second layout table, which would
    // drift the moment a row moves.
    let mut focus_line = 1; // the primary row
    for (i, kv) in KEY_VENDORS.iter().enumerate() {
        let focused = state.focus == Focus::Key(i);
        if focused {
            focus_line = lines.len();
        }
        lines.push(key_row(kv, &state.keys[i], focused, &bubble));
    }
    lines.push(Line::from(""));

    // — Sync —
    if let Focus::SyncCategory(i) = state.focus {
        focus_line = lines.len() + SYNC_PREAMBLE_LINES + i;
    }
    lines.extend(sync_lines(state, &bubble));
    lines.push(Line::from(""));

    // — Save + status —
    if state.focus == Focus::Save {
        focus_line = lines.len();
    }
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

    // Scroll only far enough to keep the focused row on screen. Tab can reach
    // rows a short terminal cannot show, and a control the user is editing
    // blind is worse than one that is merely off-screen.
    let visible = chunks[0].height as usize;
    let scroll = focus_line.saturating_sub(visible.saturating_sub(1)) as u16;
    f.render_widget(Paragraph::new(lines).scroll((scroll, 0)), chunks[0]);

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
        Focus::SyncCategory(_) => bubble.help_line([
            ("↑↓/tab", "move"),
            ("space/enter", "toggle"),
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
    let env_set = std::env::var(kv.env)
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    let mut suffix = format!("   {}", kv.env);
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

/// Lines [`sync_lines`] emits before the first toggle row: the section header,
/// last-sync, and the pointer at `sync status`. `render` needs it to know
/// which body line a focused row lands on.
const SYNC_PREAMBLE_LINES: usize = 3;

/// The Sync block — a pure function of the state and theme.
///
/// No filesystem read, no clock read, no index open: the last-sync value
/// arrives on the state. A status panel that stat-walked a multi-gigabyte
/// transcript tree on every keypress would freeze the render loop (T-6-23),
/// so the per-category counts are *pointed at* rather than computed.
fn sync_lines(state: &SettingsState, theme: &BubbleTheme) -> Vec<Line<'static>> {
    debug_assert_eq!(SYNC_PREAMBLE_LINES, 3);
    let mut lines = vec![
        section_header(
            "Sync",
            "what encrypted sync carries — space/enter toggles a row",
            theme,
        ),
        Line::from(vec![
            theme.span("     "),
            theme.muted(format!("last sync: {}", last_sync_text(state))),
        ]),
        Line::from(vec![
            theme.span("     "),
            theme.muted("file counts and sizes: ai-usagebar sync status"),
        ]),
    ];
    for (i, (cat, on)) in state.sync_categories.iter().enumerate() {
        lines.push(sync_row(
            *cat,
            *on,
            state.focus == Focus::SyncCategory(i),
            theme,
        ));
    }
    lines
}

/// `never` is both the never-synced answer and the could-not-read-the-index
/// answer — the same conflation `ai-usagebar sync status` already prints.
fn last_sync_text(state: &SettingsState) -> String {
    state
        .sync_last_sync
        .map_or_else(|| "never".to_string(), |at| at.to_rfc3339())
}

/// What each category costs or carries, in the user's terms. Transcripts is
/// called out because it is the one toggle that turns a ~30 MB bundle into a
/// multi-gigabyte one; it must not look like every other row.
fn sync_note(cat: SyncCategory) -> &'static str {
    match cat {
        SyncCategory::Config => "this file, inline keys included",
        SyncCategory::Credentials => "saved logins — encrypted before anything leaves",
        SyncCategory::Routines => "scheduled tasks",
        SyncCategory::ChatIndex => "Claude Desktop session index",
        SyncCategory::Transcripts => "opt-in · large — gigabytes of local JSONL",
    }
}

fn sync_row(cat: SyncCategory, on: bool, focused: bool, theme: &BubbleTheme) -> Line<'static> {
    let mark = if on { "[x]" } else { "[ ]" };
    let label = format!("{:<12}", cat.label());
    let note = format!("  {}", sync_note(cat));
    if focused {
        Line::from(vec![
            theme.span("  "),
            Span::styled("▸ ", theme.accent.add_modifier(Modifier::BOLD)),
            Span::styled(
                format!(" {mark} "),
                theme
                    .selected
                    .add_modifier(Modifier::REVERSED | Modifier::BOLD),
            ),
            Span::styled(label, theme.title.add_modifier(Modifier::BOLD)),
            theme.muted(note),
        ])
    } else {
        Line::from(vec![
            theme.span("     "),
            Span::styled(
                format!("{mark} "),
                if on { theme.accent } else { theme.muted },
            ),
            Span::styled(label, theme.text),
            theme.muted(note),
        ])
    }
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
            sync_categories: default_sync_rows(),
            sync_last_sync: None,
            sync_dirty: false,
            status: String::new(),
        }
    }

    /// The sync rows a default `Config` produces — the shape every state
    /// literal in these tests starts from.
    fn default_sync_rows() -> Vec<(SyncCategory, bool)> {
        let cfg = Config::default();
        SyncCategory::ALL
            .iter()
            .map(|cat| (*cat, cfg.sync.includes(*cat)))
            .collect()
    }

    fn sync_row_state(focus_index: usize) -> SettingsState {
        let mut s = blank_state(VendorId::Anthropic);
        s.focus = Focus::SyncCategory(focus_index);
        s
    }

    fn on(state: &SettingsState, cat: SyncCategory) -> bool {
        state
            .sync_categories
            .iter()
            .find(|(c, _)| *c == cat)
            .map(|(_, flag)| *flag)
            .unwrap()
    }

    const TRANSCRIPTS: usize = 4;

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
        // Full cycle = Primary + N key rows + 5 sync rows + Save.
        for _ in 0..(KEY_VENDORS.len() + SyncCategory::ALL.len() + 2) {
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
    fn from_config_offers_enabled_vendors_only() {
        let cfg = Config::default();
        let s = SettingsState::from_config(&cfg);
        assert_eq!(s.primary_choices, cfg.enabled_vendors());
        // Opt-in vendors are disabled by default and must not be offered.
        assert!(!s.primary_choices.contains(&VendorId::Grok));
        assert!(s.primary_choices.contains(&s.primary));
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

    // ─── Sync section ──────────────────────────────────────────────────────

    #[test]
    fn sync_rows_follow_the_canonical_category_order() {
        let cfg = Config::default();
        let s = SettingsState::from_config(&cfg);
        assert_eq!(
            s.sync_categories
                .iter()
                .map(|(cat, _)| *cat)
                .collect::<Vec<_>>(),
            SyncCategory::ALL.to_vec(),
            "the overlay must list what `sync status` lists, in the same order"
        );
    }

    #[test]
    fn sync_rows_mirror_the_configs_own_selection() {
        let cfg = Config::default();
        let s = SettingsState::from_config(&cfg);
        for (cat, flag) in &s.sync_categories {
            assert_eq!(*flag, cfg.sync.includes(*cat), "{cat:?}");
        }
        // The default is the four cheap categories; transcripts is opt-in.
        assert_eq!(s.sync_categories.iter().filter(|(_, f)| *f).count(), 4);
        assert!(!on(&s, SyncCategory::Transcripts));
        assert!(on(&s, SyncCategory::Credentials));
    }

    #[test]
    fn an_empty_category_list_is_every_row_off_not_the_default_set() {
        // "sync nothing" is a legal choice and must survive a round trip
        // through the overlay rather than being silently re-defaulted.
        let mut cfg = Config::default();
        cfg.sync.categories.clear();
        let s = SettingsState::from_config(&cfg);
        assert_eq!(s.sync_categories.len(), SyncCategory::ALL.len());
        assert!(s.sync_categories.iter().all(|(_, flag)| !flag));
    }

    #[test]
    fn from_config_leaves_last_sync_unknown_and_the_sync_seam_carries_it() {
        // `from_config` stays pure — no index, no clock, no $HOME.
        let cfg = Config::default();
        assert!(SettingsState::from_config(&cfg).sync_last_sync.is_none());

        let at = chrono::DateTime::parse_from_rfc3339("2026-08-19T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let s = SettingsState::from_config_with_sync(&cfg, Some(at));
        assert_eq!(s.sync_last_sync, Some(at));
        // The seam changes nothing else about the state.
        assert_eq!(
            s.sync_categories,
            SettingsState::from_config(&cfg).sync_categories
        );
    }

    #[test]
    fn the_focus_walk_reaches_every_sync_row_and_stays_a_closed_cycle() {
        let last_key = Focus::Key(KEY_VENDORS.len() - 1);
        assert_eq!(last_key.next(), Focus::SyncCategory(0));
        assert_eq!(Focus::SyncCategory(0).prev(), last_key);

        let last_sync = Focus::SyncCategory(SyncCategory::ALL.len() - 1);
        assert_eq!(last_sync.next(), Focus::Save);
        assert_eq!(Focus::Save.prev(), last_sync);

        // Every sync row is reachable, and next/prev are inverses on each.
        for i in 0..SyncCategory::ALL.len() {
            let f = Focus::SyncCategory(i);
            assert_eq!(f.next().prev(), f, "row {i} is a trap going forward");
            assert_eq!(f.prev().next(), f, "row {i} is a trap going backward");
        }
    }

    #[test]
    fn space_and_enter_toggle_exactly_the_focused_sync_row() {
        for key in [KeyCode::Char(' '), KeyCode::Enter] {
            let mut s = sync_row_state(TRANSCRIPTS);
            let before = s.sync_categories.clone();
            assert_eq!(
                handle_key(&mut s, key, KeyModifiers::NONE),
                Action::Continue
            );
            assert!(on(&s, SyncCategory::Transcripts), "{key:?} did not toggle");
            assert!(s.sync_dirty, "{key:?} left the row unmarked as edited");
            // Every other row is untouched.
            for (i, (cat, flag)) in s.sync_categories.iter().enumerate() {
                if i != TRANSCRIPTS {
                    assert_eq!((*cat, *flag), before[i], "row {i} moved");
                }
            }
            // And it flips back.
            handle_key(&mut s, key, KeyModifiers::NONE);
            assert!(!on(&s, SyncCategory::Transcripts));
        }
    }

    #[test]
    fn arrows_never_flip_a_sync_row() {
        // Left/Right mean "cycle a choice" on the Primary row. A mis-aimed
        // arrow must not change which credentials are eligible to leave.
        for key in [KeyCode::Left, KeyCode::Right] {
            let mut s = sync_row_state(TRANSCRIPTS);
            handle_key(&mut s, key, KeyModifiers::NONE);
            assert!(!on(&s, SyncCategory::Transcripts), "{key:?} flipped a row");
            assert!(!s.sync_dirty);
            assert_eq!(s.focus, Focus::SyncCategory(TRANSCRIPTS));
        }
    }

    #[test]
    fn a_modifier_chord_on_a_sync_row_is_a_no_op() {
        for mods in [
            KeyModifiers::CONTROL,
            KeyModifiers::ALT,
            KeyModifiers::SUPER,
        ] {
            let mut s = sync_row_state(TRANSCRIPTS);
            handle_key(&mut s, KeyCode::Char(' '), mods);
            assert!(!on(&s, SyncCategory::Transcripts), "{mods:?} flipped a row");
            assert!(!s.sync_dirty);
        }
    }

    #[test]
    fn esc_and_ctrl_c_keep_their_meaning_on_a_sync_row() {
        let mut s = sync_row_state(0);
        assert_eq!(
            handle_key(&mut s, KeyCode::Esc, KeyModifiers::NONE),
            Action::Close
        );
        let mut s = sync_row_state(0);
        assert_eq!(
            handle_key(&mut s, KeyCode::Char('c'), KeyModifiers::CONTROL),
            Action::Quit
        );
        assert!(!s.sync_dirty);
    }

    #[test]
    fn tab_still_moves_off_a_sync_row_in_both_directions() {
        let mut s = sync_row_state(0);
        handle_key(&mut s, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(s.focus, Focus::SyncCategory(1));
        handle_key(&mut s, KeyCode::BackTab, KeyModifiers::NONE);
        assert_eq!(s.focus, Focus::SyncCategory(0));
        assert!(!s.sync_dirty, "moving focus is not an edit");
    }

    // ─── Sync section: persistence ─────────────────────────────────────────

    /// A state whose sync rows have been edited, so the writer engages.
    fn toggled_sync_state(cfg: &Config, flip: SyncCategory) -> SettingsState {
        let mut s = SettingsState::from_config(cfg);
        s.primary_choices = VendorId::all().to_vec();
        let i = SyncCategory::ALL.iter().position(|c| *c == flip).unwrap();
        s.focus = Focus::SyncCategory(i);
        handle_key(&mut s, KeyCode::Char(' '), KeyModifiers::NONE);
        s
    }

    fn written_categories(path: &Path) -> Vec<SyncCategory> {
        Config::load_from(path).unwrap().sync.categories
    }

    #[test]
    fn toggling_transcripts_on_writes_all_five_labels_and_off_writes_four() {
        let cfg = Config::default();
        let (_dir, path) = temp_config(Some("[sync]\ncategories = [\"config\"]\n"));

        let on = toggled_sync_state(&cfg, SyncCategory::Transcripts);
        save_to_path(&on, &path).unwrap();
        assert_eq!(written_categories(&path), SyncCategory::ALL.to_vec());
        // The labels are the enum's own spelling, not re-typed in the writer.
        let text = std::fs::read_to_string(&path).unwrap();
        for cat in SyncCategory::ALL {
            assert!(text.contains(cat.label()), "{cat:?} missing from {text}");
        }

        // The overlay reopened on the file it just wrote, and flipped back.
        let reopened = Config::load_from(&path).unwrap();
        let off = toggled_sync_state(&reopened, SyncCategory::Transcripts);
        save_to_path(&off, &path).unwrap();
        assert_eq!(written_categories(&path).len(), 4);
        assert!(
            !Config::load_from(&path)
                .unwrap()
                .sync
                .includes(SyncCategory::Transcripts)
        );
    }

    #[test]
    fn the_sync_write_inherits_the_overlays_chmod() {
        // The point of extending `save_to_path` instead of adding a writer:
        // mode 0600 and the waybar signal come with it. This pins the mode;
        // `save_to_config_default` is the only thing that signals, and it
        // still calls straight through here.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let cfg = Config::default();
            let (_dir, path) = temp_config(Some("[sync]\ncategories = [\"config\"]\n"));
            let s = toggled_sync_state(&cfg, SyncCategory::Transcripts);
            save_to_path(&s, &path).unwrap();
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn every_category_off_writes_an_empty_array_that_reloads_as_syncing_nothing() {
        // "sync nothing" must stay distinguishable from "never chose" (T-6-22).
        let mut cfg = Config::default();
        cfg.sync.categories.clear();
        let mut s = SettingsState::from_config(&cfg);
        s.sync_dirty = true;

        let (_dir, path) = temp_config(Some("[sync]\ncategories = [\"config\"]\n"));
        save_to_path(&s, &path).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("categories = []"), "{text}");
        let reloaded = Config::load_from(&path).unwrap();
        assert!(reloaded.sync.categories.is_empty());
        for cat in SyncCategory::ALL {
            assert!(!reloaded.sync.includes(cat), "{cat:?} came back");
        }
    }

    #[test]
    fn save_preserves_a_hand_written_commented_sync_section() {
        // A comment beside the sync keys may carry anything the user put
        // there; toml_edit must not relocate it (T-6-24).
        let original = "\
# how much leaves this machine
[sync]
# transcripts are gigabytes — left off on purpose
categories = [\"config\"]
transcript_days = 7        # two weeks was too much
keep_snapshots = 3
repo = \"me/private-backup\"
";
        let (_dir, path) = temp_config(Some(original));
        let cfg = Config::load_from(&path).unwrap();
        let s = toggled_sync_state(&cfg, SyncCategory::Credentials);
        save_to_path(&s, &path).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        for kept in [
            "# how much leaves this machine",
            "# transcripts are gigabytes — left off on purpose",
            "transcript_days = 7",
            "# two weeks was too much",
            "keep_snapshots = 3",
            "repo = \"me/private-backup\"",
        ] {
            assert!(text.contains(kept), "lost {kept:?} from:\n{text}");
        }
        let reloaded = Config::load_from(&path).unwrap();
        assert_eq!(reloaded.sync.keep_snapshots, 3);
        assert_eq!(reloaded.sync.repo.as_deref(), Some("me/private-backup"));
        assert!(reloaded.sync.includes(SyncCategory::Credentials));
    }

    #[test]
    fn save_creates_a_sync_section_when_the_file_has_none() {
        let (_dir, path) = temp_config(Some("[zai]\nenabled = true\n"));
        let cfg = Config::default();
        let s = toggled_sync_state(&cfg, SyncCategory::Transcripts);
        save_to_path(&s, &path).unwrap();

        assert_eq!(written_categories(&path), SyncCategory::ALL.to_vec());
        // and the section it did not own is still there.
        assert!(std::fs::read_to_string(&path).unwrap().contains("[zai]"));
    }

    #[test]
    fn a_section_the_overlay_does_not_own_survives_byte_for_byte() {
        // `[context]` belongs to the context monitor, not to this overlay.
        // (The plan named `[ui]`; the overlay does own `ui.primary`, so the
        // honest fixture is a section it has no key in at all.)
        let original = "\
[context]
enabled = true
layout = \"split\"   # trailing comment

[sync]
categories = [\"config\"]
";
        let (_dir, path) = temp_config(Some(original));
        let cfg = Config::load_from(&path).unwrap();
        let s = toggled_sync_state(&cfg, SyncCategory::Routines);
        save_to_path(&s, &path).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let context_block = "[context]\nenabled = true\nlayout = \"split\"   # trailing comment\n";
        assert!(
            text.contains(context_block),
            "[context] was rewritten:\n{text}"
        );
    }

    #[test]
    fn an_untouched_save_never_invents_a_sync_section() {
        // Opening Settings to paste one API key must not also commit the user
        // to a persisted sync selection they never made.
        let (_dir, path) = temp_config(Some("[zai]\nenabled = true\n"));
        let s = state_with("zk", "ok", VendorId::Zai);
        assert!(!s.sync_dirty);
        save_to_path(&s, &path).unwrap();
        assert!(
            !std::fs::read_to_string(&path).unwrap().contains("[sync]"),
            "an untouched save wrote a sync section"
        );
    }

    #[test]
    fn re_saving_the_same_state_leaves_the_file_byte_identical() {
        let (_dir, path) = temp_config(Some(
            "[sync]\ncategories = [\"config\"]\nkeep_snapshots = 3\n",
        ));
        let cfg = Config::load_from(&path).unwrap();
        let s = toggled_sync_state(&cfg, SyncCategory::Transcripts);

        save_to_path(&s, &path).unwrap();
        let first = std::fs::read(&path).unwrap();
        save_to_path(&s, &path).unwrap();
        assert_eq!(
            first,
            std::fs::read(&path).unwrap(),
            "save is not idempotent"
        );
    }

    // ─── Sync section: rendering ───────────────────────────────────────────

    fn rendered(state: &SettingsState) -> Vec<String> {
        // Theme::default() is pure — no Omarchy file, no $XDG read.
        let theme = bubble_theme(&Theme::default());
        sync_lines(state, &theme)
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn the_block_shows_one_row_per_category_in_canonical_order() {
        let s = SettingsState::from_config(&Config::default());
        let lines = rendered(&s);
        // header + last-sync + the pointer + one row each.
        assert_eq!(lines.len(), 3 + SyncCategory::ALL.len());
        for (i, cat) in SyncCategory::ALL.iter().enumerate() {
            assert!(
                lines[3 + i].contains(cat.label()),
                "row {i} is not {cat:?}: {}",
                lines[3 + i]
            );
        }
    }

    #[test]
    fn a_row_shows_its_own_on_off_state() {
        let mut s = SettingsState::from_config(&Config::default());
        assert!(rendered(&s)[3 + TRANSCRIPTS].contains("[ ]"));
        s.focus = Focus::SyncCategory(TRANSCRIPTS);
        handle_key(&mut s, KeyCode::Char(' '), KeyModifiers::NONE);
        assert!(rendered(&s)[3 + TRANSCRIPTS].contains("[x]"));
        // Flipping one row did not flip the drawing of another.
        assert!(rendered(&s)[3].contains("[x]"), "config is on by default");
    }

    #[test]
    fn transcripts_is_flagged_as_the_expensive_opt_in() {
        let s = SettingsState::from_config(&Config::default());
        let row = &rendered(&s)[3 + TRANSCRIPTS];
        assert!(row.contains("opt-in"), "{row}");
        assert!(row.contains("large"), "{row}");
        // and it is the only row carrying that flag.
        assert_eq!(
            rendered(&s).iter().filter(|l| l.contains("opt-in")).count(),
            1
        );
    }

    #[test]
    fn the_focused_row_is_the_only_one_marked() {
        let mut s = SettingsState::from_config(&Config::default());
        s.focus = Focus::SyncCategory(1);
        let marked: Vec<usize> = rendered(&s)
            .iter()
            .enumerate()
            .filter(|(_, l)| l.contains('▸'))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(marked, vec![3 + 1]);
    }

    #[test]
    fn last_sync_reads_never_until_the_caller_supplies_one() {
        let cfg = Config::default();
        let s = SettingsState::from_config(&cfg);
        assert!(rendered(&s)[1].contains("last sync: never"));

        let at = chrono::DateTime::parse_from_rfc3339("2026-08-19T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let s = SettingsState::from_config_with_sync(&cfg, Some(at));
        assert!(
            rendered(&s)[1].contains(&at.to_rfc3339()),
            "{}",
            rendered(&s)[1]
        );
    }

    #[test]
    fn the_counts_the_block_does_not_compute_are_named_not_left_blank() {
        // A user who sees no numbers and is told nothing assumes it is broken.
        let s = SettingsState::from_config(&Config::default());
        assert!(rendered(&s)[2].contains("ai-usagebar sync status"));
    }

    /// Draw the whole overlay onto a fixed-size test backend and read it back.
    fn drawn(state: &SettingsState, width: u16, height: u16) -> String {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let theme = Theme::default();
        terminal
            .draw(|frame| render(frame, frame.area(), state, &theme))
            .unwrap();
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
    fn the_overlay_draws_the_sync_section_and_still_reaches_save() {
        let s = SettingsState::from_config(&Config::default());
        let painted = drawn(&s, 120, 44);
        for cat in SyncCategory::ALL {
            assert!(painted.contains(cat.label()), "{cat:?} not drawn");
        }
        assert!(painted.contains("last sync: never"));
        assert!(painted.contains("ai-usagebar sync status"));
        assert!(painted.contains("Save"), "the Save row fell off the modal");
    }

    #[test]
    fn a_short_terminal_truncates_the_overlay_instead_of_panicking() {
        // The modal is a percentage of the frame and the body is a Paragraph:
        // a window too short to hold every row clips, it does not overflow the
        // buffer. Both the floor and a one-row frame are drawn here because a
        // panic inside `draw` takes the whole TUI down.
        let s = SettingsState::from_config(&Config::default());
        for (w, h) in [(80, 24), (40, 10), (20, 3), (1, 1)] {
            let _ = drawn(&s, w, h);
        }
    }

    #[test]
    fn a_short_terminal_scrolls_to_the_focused_row_instead_of_hiding_it() {
        // 80x24 is the default Terminal.app window and cannot hold the whole
        // form. Every row Tab can reach must still be visible when it has
        // focus — toggling what leaves the machine blind is not acceptable.
        let mut s = SettingsState::from_config(&Config::default());
        for i in 0..SyncCategory::ALL.len() {
            s.focus = Focus::SyncCategory(i);
            let painted = drawn(&s, 80, 24);
            let label = SyncCategory::ALL[i].label();
            assert!(
                painted.contains(label),
                "{label} is off-screen when focused"
            );
            assert!(painted.contains('▸'), "{label} lost its focus marker");
        }
        s.focus = Focus::Save;
        assert!(drawn(&s, 80, 24).contains("Save"));
        s.focus = Focus::Primary;
        assert!(drawn(&s, 80, 24).contains("Primary vendor"));
    }
}
