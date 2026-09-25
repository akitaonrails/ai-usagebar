//! Settings overlay — opened from the TUI by pressing `s`. Lets the user pick
//! the primary vendor and paste a credential for any API-key-authenticated vendor
//! (including Z.AI, Kimi, MiniMax, and the balance vendors) without hand-editing
//! config.toml. Anthropic, OpenAI, GitHub Copilot, Cursor, Kiro, Antigravity,
//! Grok Bot, and Command Code authenticate through official or local product
//! state, so they have no credential field here — there is nothing to paste,
//! and a field would only imply otherwise. Kimi keeps
//! its credential field because a platform key is still one of its two credentials, but
//! a subscriber whose credential is the Kimi Code CLI login has nothing to paste
//! and enables `[kimi]` in config.toml instead.
//!
//! Below the credentials sits the Providers section (#244): one on/off switch
//! per known vendor, writing `enabled = true/false` under its own config
//! section. Below that are the `[notifications]` fields: a toggle for the
//! quota-threshold alerts and their threshold in percent. Everything persists
//! through `toml_edit` so the existing config keeps its comments,
//! whitespace, and unrelated fields. Writing a key also flips that vendor's
//! `enabled = true` (the opt-in vendors are disabled by default), so "paste the
//! credential and save" is all it takes — an explicit off switch toggled in
//! the same save wins. Files with inline credentials are atomically written
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

use crate::config::{
    Config, is_valid_env_var_name, read_config_document, set_bool, set_value, write_config_document,
};
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
        section: VendorId::AnthropicApi.config_section(),
        config_key: "api_key",
        secret_label: "API key",
        note: "admin key — monthly spend",
    },
    KeyVendor {
        id: VendorId::Zai,
        label: "Z.AI",
        section: VendorId::Zai.config_section(),
        config_key: "api_key",
        secret_label: "API key",
        note: "",
    },
    KeyVendor {
        id: VendorId::Openrouter,
        label: "OpenRouter",
        section: VendorId::Openrouter.config_section(),
        config_key: "api_key",
        secret_label: "API key",
        note: "",
    },
    KeyVendor {
        id: VendorId::Deepseek,
        label: "DeepSeek",
        section: VendorId::Deepseek.config_section(),
        config_key: "api_key",
        secret_label: "API key",
        note: "",
    },
    KeyVendor {
        id: VendorId::Kimi,
        label: "Kimi",
        section: VendorId::Kimi.config_section(),
        config_key: "api_key",
        secret_label: "API key",
        note: "coding-plan usage",
    },
    KeyVendor {
        id: VendorId::Kilo,
        label: "Kilo",
        section: VendorId::Kilo.config_section(),
        config_key: "api_key",
        secret_label: "API key",
        note: "",
    },
    KeyVendor {
        id: VendorId::Novita,
        label: "Novita",
        section: VendorId::Novita.config_section(),
        config_key: "api_key",
        secret_label: "API key",
        note: "",
    },
    KeyVendor {
        id: VendorId::Moonshot,
        label: "Moonshot",
        section: VendorId::Moonshot.config_section(),
        config_key: "api_key",
        secret_label: "API key",
        note: "account balance",
    },
    KeyVendor {
        id: VendorId::Grok,
        label: "Grok",
        section: VendorId::Grok.config_section(),
        config_key: "api_key",
        secret_label: "API key",
        note: "management key, not the inference key",
    },
    KeyVendor {
        id: VendorId::Minimax,
        label: "MiniMax",
        section: VendorId::Minimax.config_section(),
        config_key: "api_key",
        secret_label: "API key",
        note: "Token Plan subscription key",
    },
    KeyVendor {
        id: VendorId::OpenCodeGo,
        label: "OpenCode Go",
        section: VendorId::OpenCodeGo.config_section(),
        config_key: "api_key",
        secret_label: "API key",
        note: "usage quota",
    },
    KeyVendor {
        id: VendorId::Ollama,
        label: "Ollama Cloud",
        section: VendorId::Ollama.config_section(),
        config_key: "api_key",
        secret_label: "API key",
        note: "ollama.com/settings/keys",
    },
    KeyVendor {
        id: VendorId::OrcaRouter,
        label: "OrcaRouter",
        section: VendorId::OrcaRouter.config_section(),
        config_key: "api_key",
        secret_label: "API key",
        note: "credit balance",
    },
];

/// How many providers the on/off section lists: every known vendor, in
/// `VendorId::all()` order (#244). A new vendor grows this automatically; a
/// guard test pins the focus ring to the real enumeration.
pub const PROVIDER_SWITCH_COUNT: usize = VendorId::all().len();

/// Which control has keyboard focus. `Key(i)` indexes into [`KEY_VENDORS`];
/// `Vendor(i)` indexes the provider on/off rows, parallel to
/// `SettingsState::vendors` / `VendorId::all()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Primary,
    Key(usize),
    Vendor(usize),
    NotifyEnabled,
    NotifyThreshold,
    Save,
}

impl Focus {
    pub fn next(self) -> Self {
        match self {
            Focus::Primary => Focus::Key(0),
            Focus::Key(i) if i + 1 < KEY_VENDORS.len() => Focus::Key(i + 1),
            Focus::Key(_) => Focus::Vendor(0),
            Focus::Vendor(i) if i + 1 < PROVIDER_SWITCH_COUNT => Focus::Vendor(i + 1),
            Focus::Vendor(_) => Focus::NotifyEnabled,
            Focus::NotifyEnabled => Focus::NotifyThreshold,
            Focus::NotifyThreshold => Focus::Save,
            Focus::Save => Focus::Primary,
        }
    }
    pub fn prev(self) -> Self {
        match self {
            Focus::Primary => Focus::Save,
            Focus::Key(0) => Focus::Primary,
            Focus::Key(i) => Focus::Key(i - 1),
            Focus::Vendor(0) => Focus::Key(KEY_VENDORS.len() - 1),
            Focus::Vendor(i) => Focus::Vendor(i - 1),
            Focus::NotifyEnabled => Focus::Vendor(PROVIDER_SWITCH_COUNT - 1),
            Focus::NotifyThreshold => Focus::NotifyEnabled,
            Focus::Save => Focus::NotifyThreshold,
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

/// One provider's on/off switch (#244), parallel to `VendorId::all()`.
/// `dirty` follows the same only-write-what-the-user-touched rule the
/// notification fields use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderSwitch {
    pub enabled: bool,
    pub dirty: bool,
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
    /// One on/off switch per known vendor ([`VendorId::all()` order), so any
    /// provider can be turned on or off from the overlay (#244).
    pub vendors: Vec<ProviderSwitch>,
    /// `[notifications] enabled` toggle. Written back only once toggled.
    pub notify_enabled: bool,
    pub notify_enabled_dirty: bool,
    /// `[notifications] threshold`, edited as digits only (1..=100 at save).
    pub notify_threshold: KeyInput,
    /// One-line status displayed in the footer ("saved …", "save failed …").
    pub status: String,
    /// First body line scrolled into view. The overlay body outgrew every
    /// terminal once every provider got a row, so rendering follows focus.
    pub scroll: u16,
}

impl SettingsState {
    pub fn from_config(cfg: &Config) -> Self {
        Self::from_config_with(cfg, |name| {
            std::env::var_os(name).is_some_and(|v| !v.is_empty())
        })
    }

    /// [`Self::from_config`] with an injected environment lookup.
    ///
    /// The overlay offers a key-only vendor whose env var is already exported,
    /// so a user need not hand-edit `config.toml` to select it. That is a read
    /// of ambient state, which a test must never depend on: the AUR `check()`
    /// runs `cargo test` during `makepkg`, so a test that branched on the real
    /// environment would fail the install for anyone who exports, say,
    /// `OLLAMA_API_KEY`. Tests pass their own lookup here.
    pub fn from_config_with(cfg: &Config, env_set: impl Fn(&str) -> bool) -> Self {
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
        // Key-only vendors are opt-in: without an inline `api_key` and with
        // the env var empty, the fetch would fail on the first cycle. A user
        // who already exported the env var (e.g. `OLLAMA_API_KEY`) and wants
        // to flip `enabled = true` from inside the TUI has to be able to
        // select the vendor here — otherwise they'd have to hand-edit
        // `config.toml`, which is the workflow this overlay exists to avoid.
        // We treat a non-empty env var as a sufficient signal of reachability.
        for kv in KEY_VENDORS {
            if primary_choices.contains(&kv.id) {
                continue;
            }
            let env = cfg.api_key_env_for(kv.id);
            let exported = is_valid_env_var_name(env) && env_set(env);
            if cfg.inline_api_key(kv.id).is_some() || exported {
                primary_choices.push(kv.id);
            }
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
            vendors: VendorId::all()
                .iter()
                .map(|id| ProviderSwitch {
                    enabled: cfg.is_enabled(*id),
                    dirty: false,
                })
                .collect(),
            notify_enabled: cfg.notifications.enabled,
            notify_enabled_dirty: false,
            notify_threshold: KeyInput::from_config(Some(&cfg.notifications.threshold.to_string())),
            status: String::new(),
            scroll: 0,
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
        Focus::Vendor(i) => handle_provider_switch(state, i, code),
        Focus::NotifyEnabled => handle_notify_enabled(state, code),
        Focus::NotifyThreshold => handle_threshold_input(&mut state.notify_threshold, code),
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

/// The `[notifications] enabled` toggle. Left/Right/Space flip it; any other
/// key leaves it alone.
fn handle_notify_enabled(state: &mut SettingsState, code: KeyCode) {
    if matches!(code, KeyCode::Left | KeyCode::Right | KeyCode::Char(' ')) {
        state.notify_enabled = !state.notify_enabled;
        state.notify_enabled_dirty = true;
    }
}

/// One `[<vendor>] enabled` row (#244): Left/Right/Space flip the switch;
/// any other key leaves it — and its dirty flag — alone, so an untouched
/// overlay save never writes a section the config never had.
fn handle_provider_switch(state: &mut SettingsState, index: usize, code: KeyCode) {
    if matches!(code, KeyCode::Left | KeyCode::Right | KeyCode::Char(' '))
        && let Some(switch) = state.vendors.get_mut(index)
    {
        switch.enabled = !switch.enabled;
        switch.dirty = true;
    }
}

/// The `[notifications] threshold` field: an edit buffer that only accepts
/// digits, so the value can never be something save would have to reject as
/// non-numeric (range is still checked at save).
fn handle_threshold_input(input: &mut KeyInput, code: KeyCode) {
    match code {
        KeyCode::Char(c) if c.is_ascii_digit() => input.insert_char(c),
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
    let mut doc = read_config_document(path)?;

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
    // is the explicit opt-in after the GitHub CLI login. The env-only key
    // vendors (any `KEY_VENDORS` entry whose credential the user reached via
    // `OLLAMA_API_KEY` or another env var) are also offered before they are
    // enabled, and selecting them is the explicit opt-in for that vendor —
    // otherwise a user could pick a vendor in the overlay and the fetch
    // would still fail because `enabled = false`. Every other choice remains
    // enabled-only, so no failed provider is persisted as primary.
    if state.primary_choices.contains(&state.primary) {
        set_string(&mut doc, "ui", "primary", state.primary.slug())?;
        if state.primary == VendorId::Copilot || KEY_VENDORS.iter().any(|kv| kv.id == state.primary)
        {
            set_bool(&mut doc, state.primary.config_section(), "enabled", true)?;
        }
    }

    for (i, kv) in KEY_VENDORS.iter().enumerate() {
        let Some(input) = state.keys.get(i) else {
            continue;
        };
        update_key(&mut doc, kv, input)?;
    }

    // Provider on/off switches (#244): written after the credential fields so
    // an explicit off wins over the enable-a-pasted-key-implied rule above,
    // and each one goes through the same whitelisted setter the native
    // settings bridge uses. Only toggled rows are written, so an untouched
    // overlay save never creates a `[<vendor>]` section the config never had.
    for (i, id) in VendorId::all().iter().enumerate() {
        let Some(switch) = state.vendors.get(i) else {
            continue;
        };
        if switch.dirty {
            crate::config::set_vendor_enabled_in_doc(&mut doc, id.slug(), switch.enabled)?;
        }
    }

    // [notifications]: each field is written only when the user touched it,
    // so an untouched overlay save leaves an absent/commented section alone.
    if state.notify_enabled_dirty {
        set_bool(&mut doc, "notifications", "enabled", state.notify_enabled)?;
    }
    if state.notify_threshold.dirty {
        let raw = state.notify_threshold.buf.trim();
        let threshold = raw.parse::<u8>();
        match threshold {
            Ok(threshold) if (1..=100).contains(&threshold) => {
                set_value(
                    &mut doc,
                    "notifications",
                    "threshold",
                    Some(toml_edit::Value::from(i64::from(threshold))),
                )?;
            }
            _ => {
                return Err(AppError::Other(format!(
                    "[notifications] threshold must be a whole number between 1 and 100, got {raw:?}"
                )));
            }
        }
    }

    write_config_document(path, &doc)
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
    /// Every known provider's on/off switch (#244) — enabled ones and
    /// disabled ones, because turning a provider on is the point.
    vendors: Vec<VendorStatus>,
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

/// One provider row of the native settings form: its shared display name and
/// whether config.toml currently enables it (#244).
#[derive(Debug, Serialize)]
struct VendorStatus {
    id: String,
    label: String,
    enabled: bool,
}

/// Additive patch accepted on stdin by `ai-usagebar settings apply`.
/// Missing keys remain byte-for-byte untouched. `clear` explicitly removes an
/// inline key, matching the TUI overlay's existing empty-dirty-field behavior.
/// `vendors` carries provider on/off toggles (#244); its slugs are validated
/// against the built-in vendor list before anything is written.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyRequest {
    schema_version: u8,
    primary: Option<String>,
    #[serde(default)]
    keys: BTreeMap<String, KeyMutation>,
    #[serde(default)]
    vendors: BTreeMap<String, bool>,
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
    let vendors = VendorId::all()
        .iter()
        .map(|id| VendorStatus {
            id: id.slug().to_string(),
            label: id.display_name().to_string(),
            enabled: cfg.is_enabled(*id),
        })
        .collect();
    SettingsSnapshot {
        schema_version: SETTINGS_SCHEMA_VERSION,
        primary: state.primary.slug().to_string(),
        primary_choices,
        keys,
        vendors,
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
    VendorId::from_slug(slug)
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

    // Provider on/off toggles (#244): the slug is the whitelist — anything
    // that names no built-in vendor is refused, so a patch can never create
    // or flip an arbitrary config section.
    for (slug, enabled) in request.vendors {
        let index = VendorId::all()
            .iter()
            .position(|id| id.slug() == slug)
            .ok_or_else(|| AppError::Other(format!("unknown provider {slug:?}")))?;
        state.vendors[index] = ProviderSwitch {
            enabled,
            dirty: true,
        };
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

/// Explicit user opt-in, unlike discovery which respects an existing false.
fn enable_vendor_at(path: &Path, vendor: VendorId) -> Result<()> {
    let mut doc = read_config_document(path)?;
    let before = doc.to_string();
    set_bool(&mut doc, vendor.config_section(), "enabled", true)?;
    if doc.to_string() != before {
        write_config_document(path, &doc)?;
    }
    Ok(())
}

/// Administrative settings bridge for native frontends. `show` never emits a
/// secret; `apply` accepts its patch only over stdin so keys do not appear in
/// argv or the process environment.
pub fn run_cli(action: &crate::widget::cli::SettingsAction) -> i32 {
    let result = match action {
        crate::widget::cli::SettingsAction::Enable { vendor } => default_config_path()
            .and_then(|path| enable_vendor_at(&path, vendor.to_id()))
            .map(|()| {
                crate::waybar::request_refresh();
                println!(r#"{{"ok":true}}"#);
            }),
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

/// Line positions inside the rendered overlay body. `render` builds its lines
/// in exactly this order and `focus_line` reads them back, so the two cannot
/// drift apart silently — the guard tests pin them to each other.
struct BodyLayout {
    primary: usize,
    first_key: usize,
    first_vendor: usize,
    notify_enabled: usize,
    notify_threshold: usize,
    save: usize,
    /// Body line count without the optional status row.
    rows: usize,
}

fn body_layout(keys: usize, vendors: usize) -> BodyLayout {
    let first_key = 4;
    let first_vendor = first_key + keys + 2;
    let notify_enabled = first_vendor + vendors + 2;
    BodyLayout {
        primary: 1,
        first_key,
        first_vendor,
        notify_enabled,
        notify_threshold: notify_enabled + 1,
        save: notify_enabled + 3,
        rows: notify_enabled + 4,
    }
}

/// The body line the focused control renders on.
fn focus_line(state: &SettingsState) -> usize {
    let layout = body_layout(KEY_VENDORS.len(), state.vendors.len());
    match state.focus {
        Focus::Primary => layout.primary,
        Focus::Key(i) => layout.first_key + i,
        Focus::Vendor(i) => layout.first_vendor + i,
        Focus::NotifyEnabled => layout.notify_enabled,
        Focus::NotifyThreshold => layout.notify_threshold,
        Focus::Save => layout.save,
    }
}

/// Scroll offset keeping `focused` on screen: unchanged while it is already
/// visible, the smallest jump that reveals it when not, clamped to the
/// scrollable range. Pure over its inputs.
fn follow_focus_scroll(focused: usize, total: usize, viewport: u16, current: u16) -> u16 {
    if viewport == 0 || total == 0 {
        return 0;
    }
    let max_offset = (total as u16).saturating_sub(viewport);
    let focused = focused.min(total - 1) as u16;
    let wanted = if focused < current {
        focused
    } else if focused >= current.saturating_add(viewport) {
        // Pin the newly focused line to the bottom edge so what follows it
        // (another row, the Save button) still peeks into view.
        (focused + 1).saturating_sub(viewport)
    } else {
        current
    };
    wanted.min(max_offset)
}

/// Render the modal overlay over `area`.
pub fn render(f: &mut Frame, area: Rect, state: &mut SettingsState, theme: &Theme) {
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

    // — Providers on/off (#244) —
    lines.push(section_header(
        "Providers",
        "which vendors the report, bar and TUI fetch at all",
        &bubble,
    ));
    for (i, id) in VendorId::all().iter().enumerate() {
        let switch = state.vendors.get(i).copied().unwrap_or(ProviderSwitch {
            enabled: false,
            dirty: false,
        });
        lines.push(provider_row(
            id,
            switch,
            state.focus == Focus::Vendor(i),
            &bubble,
        ));
    }
    lines.push(Line::from(""));

    // — Notifications —
    lines.push(section_header(
        "Notifications",
        "desktop alert when a quota window crosses the threshold",
        &bubble,
    ));
    lines.push(notify_enabled_line(state, &bubble));
    lines.push(notify_threshold_line(state, &bubble));
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

    // The body long outgrew every terminal once each provider got a row, so
    // the view follows the focus ring.
    let layout = body_layout(KEY_VENDORS.len(), state.vendors.len());
    let total = layout.rows + usize::from(!state.status.is_empty());
    state.scroll = follow_focus_scroll(focus_line(state), total, chunks[0].height, state.scroll);
    f.render_widget(Paragraph::new(lines).scroll((state.scroll, 0)), chunks[0]);

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
        Focus::Vendor(_) => bubble.help_line([
            ("↑↓/tab", "move"),
            ("←→/space", "toggle"),
            ("^S", "save"),
            ("esc", "close"),
        ]),
        Focus::NotifyEnabled => bubble.help_line([
            ("↑↓/tab", "move"),
            ("←→/space", "toggle"),
            ("^S", "save"),
            ("esc", "close"),
        ]),
        Focus::NotifyThreshold => bubble.help_line([
            ("↑↓/tab", "move"),
            ("type", "digits 1-100"),
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

/// One `[<vendor>] enabled` row (#244): the shared display name plus an
/// on/off value styled like the notification toggle.
fn provider_row(
    id: &VendorId,
    switch: ProviderSwitch,
    focused: bool,
    theme: &BubbleTheme,
) -> Line<'static> {
    let label = format!("{:<11}", id.display_name());
    let value = if switch.enabled { "on" } else { "off" };
    if focused {
        Line::from(vec![
            theme.span("   "),
            Span::styled("▸ ", theme.accent.add_modifier(Modifier::BOLD)),
            Span::styled(label, theme.title.add_modifier(Modifier::BOLD)),
            Span::styled(
                format!(" ◀ {value} ▶ "),
                theme
                    .selected
                    .add_modifier(Modifier::REVERSED | Modifier::BOLD),
            ),
        ])
    } else {
        Line::from(vec![
            theme.span("     "),
            Span::styled(label, theme.text),
            Span::styled(format!("  {value}"), theme.muted),
        ])
    }
}

/// `[notifications] enabled` row — a plain on/off toggle styled like the
/// primary selector.
fn notify_enabled_line(state: &SettingsState, theme: &BubbleTheme) -> Line<'static> {
    let focused = state.focus == Focus::NotifyEnabled;
    let label = format!("{:<11}", "Quota alerts");
    let value = if state.notify_enabled { "on" } else { "off" };
    if focused {
        Line::from(vec![
            theme.span("   "),
            Span::styled("▸ ", theme.accent.add_modifier(Modifier::BOLD)),
            Span::styled(label, theme.title.add_modifier(Modifier::BOLD)),
            Span::styled(
                format!(" ◀ {value} ▶ "),
                theme
                    .selected
                    .add_modifier(Modifier::REVERSED | Modifier::BOLD),
            ),
        ])
    } else {
        Line::from(vec![
            theme.span("     "),
            Span::styled(label, theme.text),
            Span::styled(format!("  {value}"), theme.muted),
        ])
    }
}

/// `[notifications] threshold` row — the digits are never masked, so the
/// number is readable at a glance.
fn notify_threshold_line(state: &SettingsState, theme: &BubbleTheme) -> Line<'static> {
    let focused = state.focus == Focus::NotifyThreshold;
    let label = format!("{:<11}", "Threshold %");
    let input = &state.notify_threshold;
    let value = if input.buf.is_empty() {
        if focused {
            "‸".to_string()
        } else {
            "(97)".to_string()
        }
    } else {
        let mut chars: Vec<char> = input.buf.chars().collect();
        if focused {
            let pos = input.cursor.min(chars.len());
            chars.insert(pos, '‸');
        }
        chars.into_iter().collect()
    };
    if focused {
        Line::from(vec![
            theme.span("  "),
            Span::styled("▸ ", theme.accent.add_modifier(Modifier::BOLD)),
            Span::styled(label, theme.title.add_modifier(Modifier::BOLD)),
            Span::styled(
                format!(" {value} "),
                theme
                    .selected
                    .add_modifier(Modifier::REVERSED | Modifier::BOLD),
            ),
            theme.muted("   1-100"),
        ])
    } else {
        Line::from(vec![
            theme.span("    "),
            Span::styled(label, theme.text),
            Span::styled(format!(" {value}"), theme.text),
            theme.muted("   1-100"),
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

    #[test]
    fn explicit_enable_preserves_other_settings_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let original = "# keep me\n[anthropic]\nenabled = false # intentional\n[openrouter]\napi_key = \"test-key\"\nenabled = false\n";
        std::fs::write(&path, original).unwrap();
        enable_vendor_at(&path, VendorId::Anthropic).unwrap();
        let expected = original.replacen("enabled = false", "enabled = true", 1);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
        enable_vendor_at(&path, VendorId::Anthropic).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
    }

    #[test]
    fn explicit_enable_creates_missing_config_and_rejects_malformed_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/config.toml");
        enable_vendor_at(&path, VendorId::Anthropic).unwrap();
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("enabled = true")
        );
        let broken = "[anthropic\n";
        std::fs::write(&path, broken).unwrap();
        assert!(enable_vendor_at(&path, VendorId::Anthropic).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
    }

    fn blank_state(primary: VendorId) -> SettingsState {
        SettingsState {
            focus: Focus::Primary,
            primary_choices: VendorId::all().to_vec(),
            primary,
            keys: KEY_VENDORS.iter().map(|_| KeyInput::default()).collect(),
            vendors: VendorId::all()
                .iter()
                .map(|id| ProviderSwitch {
                    enabled: matches!(id.slug(), "anthropic" | "openai" | "zai" | "openrouter"),
                    dirty: false,
                })
                .collect(),
            notify_enabled: true,
            notify_enabled_dirty: false,
            notify_threshold: KeyInput::from_config(Some("97")),
            status: String::new(),
            scroll: 0,
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
    fn focus_cycles_through_primary_all_keys_notifications_and_save() {
        let mut f = Focus::Primary;
        let mut seen = vec![f];
        // Full cycle = Primary + N key rows + every provider switch + both
        // notification fields + Save.
        for _ in 0..(KEY_VENDORS.len() + PROVIDER_SWITCH_COUNT + 4) {
            f = f.next();
            seen.push(f);
        }
        // Primary, Key(0..n), Vendor(0..m), NotifyEnabled, NotifyThreshold,
        // Save, Primary.
        assert_eq!(seen.first(), Some(&Focus::Primary));
        assert_eq!(seen.last(), Some(&Focus::Primary));
        assert!(seen.contains(&Focus::Key(0)));
        assert!(seen.contains(&Focus::Key(KEY_VENDORS.len() - 1)));
        assert!(seen.contains(&Focus::Vendor(0)));
        assert!(seen.contains(&Focus::Vendor(PROVIDER_SWITCH_COUNT - 1)));
        assert!(seen.contains(&Focus::NotifyEnabled));
        assert!(seen.contains(&Focus::NotifyThreshold));
        assert!(seen.contains(&Focus::Save));
        // prev() is the inverse of next() at every joint of the ring (#244
        // inserted the provider rows between the credentials and alerts).
        assert_eq!(Focus::Primary.next().prev(), Focus::Primary);
        assert_eq!(Focus::Save.prev().next(), Focus::Save);
        assert_eq!(
            Focus::Key(KEY_VENDORS.len() - 1).next(),
            Focus::Vendor(0),
            "the first provider row follows the last credential"
        );
        assert_eq!(Focus::Vendor(0).prev(), Focus::Key(KEY_VENDORS.len() - 1));
        assert_eq!(
            Focus::Vendor(PROVIDER_SWITCH_COUNT - 1).next(),
            Focus::NotifyEnabled
        );
        assert_eq!(
            Focus::NotifyEnabled.prev(),
            Focus::Vendor(PROVIDER_SWITCH_COUNT - 1)
        );
        assert_eq!(Focus::NotifyThreshold.next(), Focus::Save);
        assert_eq!(Focus::Primary.prev(), Focus::Save);
    }

    /// The focus ring must track the real vendor enumeration, not a stale
    /// copy: a vendor added without this notice would make the last row
    /// unreachable.
    #[test]
    fn provider_switch_count_matches_the_vendor_enumeration() {
        assert_eq!(PROVIDER_SWITCH_COUNT, VendorId::all().len());
    }

    #[test]
    fn from_config_prefills_provider_switches_from_enabled_vendors() {
        let mut cfg = Config::default();
        cfg.grok.enabled = true;
        cfg.anthropic.enabled = false;
        let s = SettingsState::from_config_with(&cfg, |_| false);
        for (i, id) in VendorId::all().iter().enumerate() {
            assert_eq!(
                s.vendors[i],
                ProviderSwitch {
                    enabled: cfg.is_enabled(*id),
                    dirty: false
                },
                "{}",
                id.slug()
            );
        }
        assert!(!s.vendors[vendor_index(VendorId::Anthropic)].enabled);
        assert!(s.vendors[vendor_index(VendorId::Grok)].enabled);
    }

    fn vendor_index(id: VendorId) -> usize {
        VendorId::all().iter().position(|v| *v == id).unwrap()
    }

    #[test]
    fn provider_switch_flips_on_left_right_and_space() {
        let mut s = blank_state(VendorId::Anthropic);
        let grok = vendor_index(VendorId::Grok);
        s.focus = Focus::Vendor(grok);
        assert!(!s.vendors[grok].enabled);
        handle_key(&mut s, KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(
            s.vendors[grok],
            ProviderSwitch {
                enabled: true,
                dirty: true
            }
        );
        handle_key(&mut s, KeyCode::Char(' '), KeyModifiers::NONE);
        assert!(!s.vendors[grok].enabled);
        handle_key(&mut s, KeyCode::Left, KeyModifiers::NONE);
        assert!(s.vendors[grok].enabled);
        // Any other key leaves the switch (and its dirty flag) alone.
        handle_key(&mut s, KeyCode::Char('x'), KeyModifiers::NONE);
        assert!(s.vendors[grok].enabled);
        assert!(s.vendors[grok].dirty);
        // A control chord never toggles.
        handle_key(&mut s, KeyCode::Char(' '), KeyModifiers::CONTROL);
        assert!(s.vendors[grok].enabled);
        // Tab moves focus without touching the switch.
        let before = s.vendors[grok];
        handle_key(&mut s, KeyCode::Tab, KeyModifiers::NONE);
        assert_ne!(s.focus, Focus::Vendor(grok));
        assert_eq!(s.vendors[grok], before);
    }

    #[test]
    fn save_writes_only_toggled_providers_and_round_trips() {
        let (_dir, path) = temp_config(Some("# keep\n[anthropic]\nenabled = true\n"));
        let mut s = blank_state(VendorId::Anthropic);
        s.primary_choices = vec![VendorId::Anthropic];
        // Turn Grok on and Anthropic off; leave every other switch untouched.
        s.vendors[vendor_index(VendorId::Grok)].enabled = true;
        s.vendors[vendor_index(VendorId::Grok)].dirty = true;
        s.vendors[vendor_index(VendorId::Anthropic)].enabled = false;
        s.vendors[vendor_index(VendorId::Anthropic)].dirty = true;
        save_to_path(&s, &path).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.starts_with("# keep\n"), "{raw}");
        assert!(raw.contains("[anthropic]\nenabled = false"), "{raw}");
        assert!(raw.contains("[grok]\nenabled = true"), "{raw}");
        // A vendor nobody touched keeps whatever the file had: no new
        // `[zai] enabled = true` materializes, and a default-on vendor the
        // file never mentioned stays unwritten.
        assert!(!raw.contains("[zai]"), "{raw}");
        assert!(!raw.contains("[openrouter]"), "{raw}");

        let reloaded = Config::load_from(&path).unwrap();
        assert!(reloaded.is_enabled(VendorId::Grok));
        assert!(!reloaded.is_enabled(VendorId::Anthropic));
        assert!(
            reloaded.is_enabled(VendorId::Zai),
            "untouched default stays on"
        );
    }

    #[test]
    fn an_untouched_overlay_save_writes_no_provider_sections() {
        let (_dir, path) = temp_config(None);
        let mut s = blank_state(VendorId::Anthropic);
        s.primary_choices = vec![VendorId::Anthropic];
        save_to_path(&s, &path).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("enabled ="), "{raw}");
        assert!(!raw.contains("[grok]"), "{raw}");
    }

    /// The explicit switch is the user's word for it: pasting a credential
    /// auto-enables its vendor, and an off toggled in the same save wins.
    #[test]
    fn an_explicit_provider_off_wins_over_a_pasted_credential() {
        let (_dir, path) = temp_config(None);
        let mut s = blank_state(VendorId::Zai);
        s.primary_choices = vec![VendorId::Zai];
        s.keys[key_index(VendorId::Zai)] = KeyInput::from_config(Some("sk-zai"));
        s.keys[key_index(VendorId::Zai)].dirty = true;
        s.vendors[vendor_index(VendorId::Zai)].enabled = false;
        s.vendors[vendor_index(VendorId::Zai)].dirty = true;
        save_to_path(&s, &path).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("api_key = \"sk-zai\""), "{raw}");
        assert!(raw.contains("enabled = false"), "{raw}");
        assert!(!Config::load_from(&path).unwrap().is_enabled(VendorId::Zai));
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
    fn from_config_prefills_the_notification_fields() {
        let mut cfg = Config::default();
        cfg.notifications.enabled = false;
        cfg.notifications.threshold = 100;
        let s = SettingsState::from_config_with(&cfg, |_| false);
        assert!(!s.notify_enabled);
        assert!(!s.notify_enabled_dirty);
        assert_eq!(s.notify_threshold.buf, "100");
        assert!(!s.notify_threshold.dirty);
    }

    #[test]
    fn notification_toggle_flips_on_left_right_and_space() {
        let mut s = blank_state(VendorId::Anthropic);
        s.focus = Focus::NotifyEnabled;
        handle_key(&mut s, KeyCode::Right, KeyModifiers::NONE);
        assert!(!s.notify_enabled);
        assert!(s.notify_enabled_dirty);
        handle_key(&mut s, KeyCode::Char(' '), KeyModifiers::NONE);
        assert!(s.notify_enabled);
        handle_key(&mut s, KeyCode::Left, KeyModifiers::NONE);
        assert!(!s.notify_enabled);
        // Any other key leaves the toggle (and its dirty flag) alone.
        handle_key(&mut s, KeyCode::Up, KeyModifiers::NONE);
        assert!(!s.notify_enabled);
    }

    #[test]
    fn threshold_edits_accept_digits_and_reject_everything_else() {
        let mut s = blank_state(VendorId::Anthropic);
        s.focus = Focus::NotifyThreshold;
        // Start from an empty buffer to observe each accepted char.
        s.notify_threshold = KeyInput::default();
        for c in ['9', 'a', '.', '-', '→'] {
            handle_key(&mut s, KeyCode::Char(c), KeyModifiers::NONE);
        }
        assert_eq!(s.notify_threshold.buf, "9");
        assert!(s.notify_threshold.dirty);
        handle_key(&mut s, KeyCode::Char('8'), KeyModifiers::NONE);
        assert_eq!(s.notify_threshold.buf, "98");
        handle_key(&mut s, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(s.notify_threshold.buf, "9");
    }

    #[test]
    fn save_writes_the_notification_fields_and_round_trips() {
        let (_dir, path) = temp_config(Some("[ui]\nprimary = \"anthropic\"\n"));
        let mut s = blank_state(VendorId::Anthropic);
        s.notify_enabled = false;
        s.notify_enabled_dirty = true;
        // Start from an empty buffer so the written value is exactly "90".
        s.notify_threshold = KeyInput::default();
        for c in "90".chars() {
            s.notify_threshold.insert_char(c);
        }
        save_to_path(&s, &path).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("[notifications]"), "{raw}");
        assert!(raw.contains("enabled = false"), "{raw}");
        assert!(raw.contains("threshold = 90"), "{raw}");
        // The written file parses back through the same config path.
        let reloaded = Config::load_from(&path).unwrap();
        assert!(!reloaded.notifications.enabled);
        assert_eq!(reloaded.notifications.threshold, 90);
    }

    #[test]
    fn save_leaves_an_untouched_notification_section_alone() {
        let (_dir, path) = temp_config(None);
        let s = blank_state(VendorId::Anthropic);
        save_to_path(&s, &path).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("[notifications]"), "{raw}");
    }

    #[test]
    fn save_rejects_an_out_of_range_threshold_without_writing() {
        let (_dir, path) = temp_config(None);
        let mut s = blank_state(VendorId::Anthropic);
        for c in "300".chars() {
            s.notify_threshold.insert_char(c);
        }
        let err = save_to_path(&s, &path).unwrap_err().to_string();
        assert!(
            err.contains("[notifications] threshold must be a whole number between 1 and 100"),
            "{err}"
        );
        // A failed save must not leave a partial file behind.
        assert!(!path.exists());

        // An empty dirty buffer is the same refusal.
        let (_dir, path) = temp_config(None);
        let mut s = blank_state(VendorId::Anthropic);
        s.notify_threshold = KeyInput::default();
        s.notify_threshold.dirty = true;
        assert!(save_to_path(&s, &path).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn copilot_has_no_editable_credential_field() {
        assert!(
            !KEY_VENDORS
                .iter()
                .any(|vendor| vendor.id == VendorId::Copilot)
        );
    }

    /// The exported-env-var path is real behaviour and deserves a test of its
    /// own — just not one that reads the machine it runs on.
    #[test]
    fn a_key_vendor_with_its_env_var_exported_is_offered() {
        let cfg = Config::default();
        let env = cfg.api_key_env_for(VendorId::Ollama).to_string();

        let without = SettingsState::from_config_with(&cfg, |_| false);
        assert!(!without.primary_choices.contains(&VendorId::Ollama));

        let with = SettingsState::from_config_with(&cfg, |name| name == env);
        assert!(
            with.primary_choices.contains(&VendorId::Ollama),
            "a key vendor whose env var is exported must be selectable: {:?}",
            with.primary_choices
        );
    }

    #[test]
    fn from_config_offers_enabled_vendors_only() {
        let cfg = Config::default();
        // No ambient environment: this must not depend on whether the machine
        // running `cargo test` happens to export OLLAMA_API_KEY or friends.
        let s = SettingsState::from_config_with(&cfg, |_| false);
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

    // ─── Provider on/off switches (#244) ─────────────────────────────────────

    #[test]
    fn native_snapshot_lists_every_provider_with_its_enabled_state() {
        let mut cfg = Config::default();
        cfg.grok.enabled = true;
        let raw = settings_snapshot_json_with(&cfg, |_| false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();

        let vendors = parsed["vendors"].as_array().unwrap();
        assert_eq!(vendors.len(), VendorId::all().len());
        let by_id = |slug: &str| {
            vendors
                .iter()
                .find(|row| row["id"] == slug)
                .unwrap_or_else(|| panic!("no {slug} row: {vendors:?}"))
                .clone()
        };
        assert_eq!(by_id("anthropic")["enabled"], true);
        assert_eq!(by_id("grok")["enabled"], true);
        assert_eq!(by_id("kimi")["enabled"], false);
        // Labels come from the shared display-name source, not a second table.
        assert_eq!(by_id("anthropic")["label"], "Claude");
        assert_eq!(by_id("opencode-go")["label"], "OpenCode Go");
    }

    #[test]
    fn native_patch_toggles_providers_and_preserves_the_rest_of_the_file() {
        let (_dir, path) = temp_config(Some("# keep\n[zai]\nenabled = true\napi_key = \"k\"\n"));
        let cfg = Config::load_from(&path).unwrap();
        let request = serde_json::json!({
            "schema_version": 1,
            "vendors": {"grok": true, "zai": false}
        });

        apply_settings_json_to_path(&cfg, &request.to_string(), &path).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.starts_with("# keep\n"), "{raw}");
        assert!(raw.contains("api_key = \"k\""), "{raw}");
        let reloaded = Config::load_from(&path).unwrap();
        assert!(reloaded.is_enabled(VendorId::Grok));
        assert!(!reloaded.is_enabled(VendorId::Zai));
    }

    #[test]
    fn native_patch_refuses_an_unknown_provider_slug_without_writing() {
        let (_dir, path) = temp_config(Some("[zai]\nenabled = true\n"));
        let cfg = Config::load_from(&path).unwrap();
        for bad in ["", "mytool", "not-a-vendor"] {
            let request = serde_json::json!({
                "schema_version": 1,
                "vendors": {bad: true}
            });
            let error = state_from_apply_request(&cfg, &request.to_string())
                .unwrap_err()
                .to_string();
            assert!(error.contains("unknown provider"), "{bad}: {error}");
        }
        // A rejected patch leaves the file untouched.
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[zai]\nenabled = true\n"
        );
    }

    /// A vendor-toggle-only patch needs neither a primary nor a key change.
    #[test]
    fn native_vendor_toggle_alone_is_a_valid_patch() {
        let cfg = Config::default();
        let request = serde_json::json!({
            "schema_version": 1,
            "vendors": {"kimi": true}
        });
        let state = state_from_apply_request(&cfg, &request.to_string()).unwrap();
        let kimi = vendor_index(VendorId::Kimi);
        assert_eq!(
            state.vendors[kimi],
            ProviderSwitch {
                enabled: true,
                dirty: true
            }
        );
    }

    // ─── Overlay scroll (#244: the body outgrew small terminals) ────────────

    #[test]
    fn body_layout_positions_every_focusable_row_without_overlap() {
        let keys = 5;
        let vendors = 7;
        let layout = body_layout(keys, vendors);
        // Credentials: header at 3, rows 4..9, blank 9.
        assert_eq!(layout.primary, 1);
        assert_eq!(layout.first_key, 4);
        // Providers: blank after keys, header, then rows.
        assert_eq!(layout.first_vendor, layout.first_key + keys + 2);
        assert_eq!(layout.notify_enabled, layout.first_vendor + vendors + 2);
        assert_eq!(layout.notify_threshold, layout.notify_enabled + 1);
        assert_eq!(layout.save, layout.notify_threshold + 2);
        assert_eq!(layout.rows, layout.save + 1);
        // No two focusable rows share a line.
        let mut rows = vec![
            layout.primary,
            layout.notify_enabled,
            layout.notify_threshold,
            layout.save,
        ];
        rows.extend(layout.first_key..layout.first_key + keys);
        rows.extend(layout.first_vendor..layout.first_vendor + vendors);
        rows.sort_unstable();
        let mut unique = rows.clone();
        unique.dedup();
        assert_eq!(rows, unique);
        assert!(rows.iter().all(|row| *row < layout.rows));
    }

    #[test]
    fn focus_line_matches_the_body_layout_for_every_focus_variant() {
        let state = blank_state(VendorId::Anthropic);
        let layout = body_layout(KEY_VENDORS.len(), state.vendors.len());
        let cases = [
            (Focus::Primary, layout.primary),
            (Focus::Key(0), layout.first_key),
            (
                Focus::Key(KEY_VENDORS.len() - 1),
                layout.first_key + KEY_VENDORS.len() - 1,
            ),
            (Focus::Vendor(0), layout.first_vendor),
            (
                Focus::Vendor(state.vendors.len() - 1),
                layout.first_vendor + state.vendors.len() - 1,
            ),
            (Focus::NotifyEnabled, layout.notify_enabled),
            (Focus::NotifyThreshold, layout.notify_threshold),
            (Focus::Save, layout.save),
        ];
        for (focus, want) in cases {
            let mut s = state.clone();
            s.focus = focus;
            assert_eq!(focus_line(&s), want, "{focus:?}");
        }
    }

    #[test]
    fn follow_focus_scroll_sticks_jumps_and_clamps() {
        // Already visible: unchanged.
        assert_eq!(follow_focus_scroll(3, 50, 10, 0), 0);
        assert_eq!(follow_focus_scroll(9, 50, 10, 0), 0);
        // Below the viewport: pinned to the bottom edge.
        assert_eq!(follow_focus_scroll(10, 50, 10, 0), 1);
        assert_eq!(follow_focus_scroll(25, 50, 10, 5), 16);
        // Above the window: jumps up to the focused line.
        assert_eq!(follow_focus_scroll(2, 50, 10, 16), 2);
        // Never past the scrollable range.
        assert_eq!(follow_focus_scroll(49, 50, 10, 0), 40);
        assert_eq!(follow_focus_scroll(60, 50, 10, 0), 40);
        // Degenerate viewport/total: no scrolling.
        assert_eq!(follow_focus_scroll(5, 50, 0, 3), 0);
        assert_eq!(follow_focus_scroll(5, 0, 10, 3), 0);
    }

    /// End-to-end through the real renderer: on a terminal shorter than the
    /// overlay body, focusing a late provider switch keeps it and the Save
    /// button on screen — the reason the scroll exists.
    #[test]
    fn render_scrolls_the_body_so_a_late_row_and_save_stay_visible() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = crate::theme::Theme::default();
        let mut state = blank_state(VendorId::Anthropic);
        state.focus = Focus::Vendor(vendor_index(VendorId::Grok));

        terminal
            .draw(|f| render(f, f.area(), &mut state, &theme))
            .unwrap();

        let rendered = buffer_text(terminal.backend().buffer());
        assert!(
            rendered.iter().any(|line| line.contains("Grok")),
            "focused provider row must be visible: {rendered:?}"
        );
        // The Save control is the last thing focus moves to; keep it on
        // screen too (it renders after the provider rows).
        let mut state = blank_state(VendorId::Anthropic);
        state.focus = Focus::Save;
        terminal
            .draw(|f| render(f, f.area(), &mut state, &theme))
            .unwrap();
        let rendered = buffer_text(terminal.backend().buffer());
        assert!(
            rendered.iter().any(|line| line.contains("Save")),
            "Save must stay visible: {rendered:?}"
        );
    }

    /// Every line of a TestBackend buffer as a String, for substring checks.
    fn buffer_text(buffer: &ratatui::buffer::Buffer) -> Vec<String> {
        let area = buffer.area;
        let mut lines = Vec::new();
        for y in area.top()..area.bottom() {
            let mut line = String::new();
            for x in area.left()..area.right() {
                line.push(buffer[(x, y)].symbol().chars().next().unwrap_or(' '));
            }
            lines.push(line.trim_end().to_string());
        }
        lines
    }
}
