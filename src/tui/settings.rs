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
    Config, edit_config_document, is_valid_env_var_name, read_config_document, set_bool, set_value,
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
        id: VendorId::Deepinfra,
        label: "DeepInfra",
        section: VendorId::Deepinfra.config_section(),
        config_key: "api_key",
        secret_label: "API key",
        note: "billing balance and monthly spend",
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
        id: VendorId::Synthetic,
        label: "Synthetic",
        section: VendorId::Synthetic.config_section(),
        config_key: "api_key",
        secret_label: "API key",
        note: "weekly credit quota",
    },
    KeyVendor {
        id: VendorId::DeepRelay,
        label: "DeepRelay",
        section: VendorId::DeepRelay.config_section(),
        config_key: "api_key",
        secret_label: "API key",
        note: "key needs the billing:read scope",
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
    KeyVendor {
        id: VendorId::Lyceum,
        label: "Lyceum",
        section: VendorId::Lyceum.config_section(),
        config_key: "api_key",
        secret_label: "API key",
        note: "billing credits",
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

/// An interactive row recorded for mouse hit-testing during the settings draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsRow {
    /// A focusable control (primary picker, a key field, or the save row).
    Focus(Focus),
    /// A value cell that focuses its row and sends the synthetic key through
    /// [`handle_key`]: space toggles provider / quota-alerts switches,
    /// `←`/`→` step the primary-vendor radio over its ◀ ▶ arrows.
    Switch(Focus, KeyCode, KeyModifiers),
    /// The primary vendor's name cell: clicking opens (or the popup is open
    /// and a click outside closes) the picker popup.
    OpenPicker,
    /// A primary-vendor choice row in the open picker popup; clicking selects
    /// that vendor.
    Pick(usize),
    /// A hint-footer "link": clicking sends the synthetic key through
    /// [`handle_key`], so save/close/toggle behave exactly as if pressed.
    HintKey(KeyCode, KeyModifiers),
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

/// Open state of the primary-vendor picker popup: `cursor` indexes
/// [`SettingsState::primary_choices`], `scroll` is the list's scroll offset.
#[derive(Debug, Clone, Copy, Default)]
pub struct PrimaryPicker {
    pub cursor: usize,
    pub scroll: u16,
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
    /// Open picker popup for the primary-vendor radio; `None` while closed.
    pub picker: Option<PrimaryPicker>,
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
        // Copilot and the key vendors above are offered while disabled, but
        // only as an explicit pick: shown as the current primary, an untouched
        // save would write their `enabled = true` back.
        let primary = cfg
            .ui
            .primary
            .filter(|vendor| primary_choices.contains(vendor) && cfg.is_enabled(*vendor))
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
            picker: None,
        }
    }

    /// The focused key input, if a key row is focused.
    fn focused_key_mut(&mut self) -> Option<&mut KeyInput> {
        match self.focus {
            Focus::Key(i) => self.keys.get_mut(i),
            _ => None,
        }
    }

    /// Move focus through the ring. [`Focus::next`]/[`Focus::prev`] own the
    /// order; these wrappers keep the call sites reading as state methods.
    fn next_focus(&self) -> Focus {
        self.focus.next()
    }

    fn prev_focus(&self) -> Focus {
        self.focus.prev()
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
    // The vendor picker consumes every key while open (Ctrl-C stays global),
    // so Esc closes the popup before the modal's Esc arm can.
    if state.picker.is_some()
        && !(matches!(code, KeyCode::Char('c')) && mods.contains(KeyModifiers::CONTROL))
    {
        return handle_picker_key(state, code);
    }
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
            state.focus = state.next_focus();
            return Action::Continue;
        }
        KeyCode::BackTab | KeyCode::Up => {
            state.focus = state.prev_focus();
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

/// Key handling while the primary-vendor picker is open: ↑/↓ move the cursor
/// (wrapping), Enter/space select and close, Esc closes without selecting.
fn handle_picker_key(state: &mut SettingsState, code: KeyCode) -> Action {
    let count = state.primary_choices.len();
    if count == 0 {
        state.picker = None;
        return Action::Continue;
    }
    let Some(picker) = state.picker.as_mut() else {
        return Action::Continue;
    };
    match code {
        KeyCode::Up => picker.cursor = (picker.cursor + count - 1) % count,
        KeyCode::Down => picker.cursor = (picker.cursor + 1) % count,
        KeyCode::Enter | KeyCode::Char(' ') => {
            state.primary = state.primary_choices[picker.cursor];
            state.picker = None;
        }
        KeyCode::Esc => state.picker = None,
        _ => {}
    }
    Action::Continue
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
    // One locked read-modify-write: a key check's candidate write and this
    // save queue on the same lock instead of overwriting each other.
    edit_config_document(path, |doc| {
        // Remove fields written by the short-lived inline Copilot-token design.
        // GitHub CLI owns the OAuth credential now; retaining a secret this app
        // neither reads nor supports would be misleading and unsafe.
        if let Some(table) = doc
            .get_mut("copilot")
            .and_then(toml_edit::Item::as_table_like_mut)
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
            set_string(doc, "ui", "primary", state.primary.slug())?;
            if state.primary == VendorId::Copilot
                || KEY_VENDORS.iter().any(|kv| kv.id == state.primary)
            {
                set_bool(doc, state.primary.config_section(), "enabled", true)?;
            }
        }

        for (i, kv) in KEY_VENDORS.iter().enumerate() {
            let Some(input) = state.keys.get(i) else {
                continue;
            };
            update_key(doc, kv, input)?;
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
                crate::config::set_vendor_enabled_in_doc(doc, id.slug(), switch.enabled)?;
            }
        }

        // [notifications]: each field is written only when the user touched it,
        // so an untouched overlay save leaves an absent/commented section alone.
        if state.notify_enabled_dirty {
            set_bool(doc, "notifications", "enabled", state.notify_enabled)?;
        }
        if state.notify_threshold.dirty {
            let raw = state.notify_threshold.buf.trim();
            let threshold = raw.parse::<u8>();
            match threshold {
                Ok(threshold) if (1..=100).contains(&threshold) => {
                    set_value(
                        doc,
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

        Ok(())
    })
}

/// The credential rules every apply path enforces: the TUI's `settings
/// apply` and the tray's `set-api-key` check must refuse the same keys, so
/// the rule lives here instead of in each caller.
fn reject_invalid_key(secret_label: &str, vendor_slug: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(AppError::Other(format!(
            "{secret_label} for {vendor_slug:?} is empty; use the clear action to remove it"
        )));
    }
    if value.len() > MAX_API_KEY_BYTES {
        return Err(AppError::Other(format!(
            "{secret_label} for {vendor_slug:?} exceeds {MAX_API_KEY_BYTES} bytes"
        )));
    }
    if value.chars().any(char::is_control) {
        return Err(AppError::Other(format!(
            "{secret_label} for {vendor_slug:?} contains control characters"
        )));
    }
    Ok(())
}

/// The key vendor a credential request names, by `VendorId::slug()`. The
/// tray's `set-api-key` sends the slug its provider row carries; anything
/// else — including the OAuth vendors' slugs — has no credential field.
pub(crate) fn key_vendor_for_slug(slug: &str) -> Option<&'static KeyVendor> {
    KEY_VENDORS.iter().find(|vendor| vendor.id.slug() == slug)
}

/// Validate one credential the way [`state_from_apply_request`] does, so
/// the tray's `set-api-key` check and the TUI's save refuse the same keys.
pub(crate) fn validate_key_value(vendor: &KeyVendor, value: &str) -> Result<()> {
    reject_invalid_key(vendor.secret_label, vendor.id.slug(), value)
}

/// One credential field as found in the config: the value with the
/// decoration `toml_edit` attached to it. Cloned out of the parsed
/// document, so a restore puts it back byte for byte — comments and all.
/// No `Debug`: the value is a secret, and a backtrace of the snapshot must
/// not print it.
pub(crate) struct KeyItemSnapshot {
    key_decor: toml_edit::Decor,
    value: toml_edit::Value,
}

/// The config state the `set-api-key` check must be able to give back when
/// the provider refuses the new key: whether the file existed, whether the
/// vendor's section did, and the `api_key` and `enabled` fields exactly as
/// found. A restore touches only those two fields — everything else in the
/// document is left as the document holds it at restore time.
pub(crate) struct KeySnapshot {
    /// Recorded for the tests and for the snapshot's own contract; the
    /// restore's file-removal step now keys off the document being left
    /// empty by [`crate::config::edit_config_document`], not off this flag.
    #[cfg_attr(not(test), allow(dead_code))]
    pub file_existed: bool,
    pub section_existed: bool,
    pub key: Option<KeyItemSnapshot>,
    pub enabled: Option<KeyItemSnapshot>,
}

/// Capture one credential field — its value and its key decoration — from
/// the vendor's section, when it holds a value. A field that holds a table
/// is not a credential: it captures as absent, and the restore removes it.
fn capture_key_item(table: &dyn toml_edit::TableLike, name: &str) -> Option<KeyItemSnapshot> {
    let (key, item) = table.get_key_value(name)?;
    let value = item.as_value()?.clone();
    Some(KeyItemSnapshot {
        key_decor: key.leaf_decor().clone(),
        value,
    })
}

/// Snapshot the credential state the `set-api-key` check must be able to
/// give back: read the config with `toml_edit` (comments intact) and clone
/// the vendor's `api_key` and `enabled` fields out of it. A missing file
/// is an empty document, so the snapshot simply records that nothing was
/// there; an unparseable config is an error rather than a guess.
pub(crate) fn snapshot_inline_key(path: &Path, vendor: &KeyVendor) -> Result<KeySnapshot> {
    let file_existed = path.exists();
    let doc = read_config_document(path)?;
    Ok(key_snapshot_of_doc(&doc, file_existed, vendor))
}

/// [`snapshot_inline_key`] over a document already in hand — the capture
/// the locked check write runs inside [`edit_config_document`], where the
/// file's state is settled under the lock.
fn key_snapshot_of_doc(
    doc: &toml_edit::DocumentMut,
    file_existed: bool,
    vendor: &KeyVendor,
) -> KeySnapshot {
    match doc
        .get(vendor.section)
        .and_then(toml_edit::Item::as_table_like)
    {
        Some(table) => KeySnapshot {
            file_existed,
            section_existed: true,
            key: capture_key_item(table, vendor.config_key),
            enabled: capture_key_item(table, "enabled"),
        },
        None => KeySnapshot {
            file_existed,
            section_existed: false,
            key: None,
            enabled: None,
        },
    }
}

/// The verdict the key check publishes when a non-empty key is already
/// saved: the dialog only exists for a provider without an inline
/// credential, and a saved key is replaced in the TUI settings, not
/// checked here.
pub(crate) const KEY_ALREADY_SAVED_MESSAGE: &str =
    "A key is already saved in config.toml; replace it in the TUI settings.";

/// The vendor's saved inline key by the catalog's rule — the same one
/// `Config::inline_api_key` applies: an absent field, or a field that holds
/// the empty string, is no credential. An `api_key = ''` left by a cleared
/// field must not block the dialog the catalog offers for it.
pub(crate) fn saved_inline_key(snapshot: &KeySnapshot) -> Option<String> {
    snapshot
        .key
        .as_ref()
        .and_then(|item| item.value.as_str())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Write one inline credential the way the overlay's save does: `toml_edit`,
/// the vendor's own section, `chmod 600` through the locked
/// [`edit_config_document`]. A non-empty credential also opts the vendor in,
/// like `update_key`: a key that stays disabled is a key the fetch never
/// sees. The value is validated first, so a refused key never touches the
/// file. The key check itself uses
/// [`snapshot_and_write_inline_key_if_absent`], which re-checks the
/// "no key yet" precondition under the same lock; this unconditional write
/// is what a user-driven save does, and the tests stand in for it.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn write_inline_key(path: &Path, vendor: &KeyVendor, value: &str) -> Result<()> {
    validate_key_value(vendor, value)?;
    edit_config_document(path, |doc| {
        set_string(doc, vendor.section, vendor.config_key, value)?;
        set_bool(doc, vendor.section, "enabled", true)
    })
}

/// The key check's whole read-modify-write in one locked step: snapshot the
/// credential state, and only while no key is still saved, write `value` —
/// plus the vendor's opt-in — and return the snapshot the later restore
/// compares against. The snapshot, the "no key yet" condition, and the
/// write happen under the same config lock the TUI's save takes, so a TUI
/// save that lands between any two of the steps is either fully before
/// (the condition sees its key and the check refuses with
/// [`KEY_ALREADY_SAVED_MESSAGE`], leaving the file alone) or fully after
/// (its write is the later decision and wins) — the candidate can no longer
/// overwrite a key the TUI saved in the window between deciding and
/// writing. An empty value counts as absent, by the catalog's rule; the
/// value is validated first, so a refused key never touches the file.
pub(crate) fn snapshot_and_write_inline_key_if_absent(
    path: &Path,
    vendor: &KeyVendor,
    value: &str,
) -> Result<KeySnapshot> {
    validate_key_value(vendor, value)?;
    edit_config_document(path, |doc| {
        let snapshot = key_snapshot_of_doc(doc, path.exists(), vendor);
        if saved_inline_key(&snapshot).is_some() {
            return Err(AppError::Other(KEY_ALREADY_SAVED_MESSAGE.to_string()));
        }
        set_string(doc, vendor.section, vendor.config_key, value)?;
        set_bool(doc, vendor.section, "enabled", true)?;
        Ok(snapshot)
    })
}

/// Give the config back the state [`snapshot_inline_key`] captured,
/// compare-and-restore: the check wrote `candidate` as the key and `true`
/// as the vendor's `enabled` switch, so each field is put back — with its
/// captured value and decoration, or removed when it was absent — only
/// while it still holds exactly what the check wrote. A later decision on
/// the same field, made while the check waited for the provider, belongs
/// to the user now and survives. No other field in the file is touched,
/// so edits made to it in the meantime survive the restore. A section the
/// check created is removed when nothing else is left in it, and a file
/// the check created is removed when the document is left empty —
/// [`edit_config_document`] owns that step. No validation runs: the
/// captured values already lived in the user's config. The whole
/// compare-and-restore is one locked read-modify-write under the same lock
/// the TUI's save takes, so a save made while the check waited is seen
/// here, not clobbered by it.
pub(crate) fn restore_inline_key_snapshot(
    path: &Path,
    vendor: &KeyVendor,
    snapshot: &KeySnapshot,
    candidate: &str,
) -> Result<()> {
    edit_config_document(path, |doc| {
        let table = doc
            .entry(vendor.section)
            .or_insert_with(toml_edit::table)
            .as_table_mut()
            .ok_or_else(|| {
                AppError::Other(format!("config.toml: [{}] is not a table", vendor.section))
            })?;
        // Compare-and-restore: the check wrote `candidate` as the key and
        // `true` as the vendor's `enabled` switch, so each field goes back —
        // with its captured value and decoration, or out when it was absent —
        // only while it still holds exactly what the check wrote. A later
        // decision on the same field, made while the check waited for the
        // provider (a key saved, the vendor switched off), belongs to the user
        // now and survives.
        let current_key = table
            .get(vendor.config_key)
            .and_then(toml_edit::Item::as_str);
        match &snapshot.key {
            Some(captured) if current_key == Some(candidate) => {
                let key = toml_edit::Key::new(vendor.config_key)
                    .with_leaf_decor(captured.key_decor.clone());
                table.insert_formatted(&key, toml_edit::Item::Value(captured.value.clone()));
            }
            None if current_key == Some(candidate) => {
                table.remove(vendor.config_key);
            }
            // The field holds someone else's value: leave it.
            _ => {}
        }
        let current_enabled = table.get("enabled").and_then(toml_edit::Item::as_bool);
        match &snapshot.enabled {
            Some(captured) if current_enabled == Some(true) => {
                let key =
                    toml_edit::Key::new("enabled").with_leaf_decor(captured.key_decor.clone());
                table.insert_formatted(&key, toml_edit::Item::Value(captured.value.clone()));
            }
            None if current_enabled == Some(true) => {
                table.remove("enabled");
            }
            // The user switched the vendor: leave their decision.
            _ => {}
        }
        // A section the check created must not survive the restore as an
        // empty husk — but one that holds anything the check did not write
        // does.
        if !snapshot.section_existed
            && doc
                .get(vendor.section)
                .and_then(toml_edit::Item::as_table_like)
                .is_some_and(|table| table.is_empty())
        {
            doc.remove(vendor.section);
        }
        Ok(())
    })
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
            .and_then(toml_edit::Item::as_table_like_mut)
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
        .as_table_like_mut()
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
/// `pub(crate)`: the tray's `set-api-key` check enforces the same rule.
pub(crate) const MAX_API_KEY_BYTES: usize = 16 * 1024;

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
                reject_invalid_key(KEY_VENDORS[index].secret_label, &id, &value)?;
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
    edit_config_document(path, |doc| {
        set_bool(doc, vendor.config_section(), "enabled", true)
    })
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
pub fn render(
    f: &mut Frame,
    area: Rect,
    state: &mut SettingsState,
    theme: &Theme,
    hits: &mut Vec<(SettingsRow, Rect)>,
) {
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

    // The body long outgrew every terminal once each provider got a row, so
    // the view follows the focus ring. Compute the offset up front: hit rects
    // must be recorded at the *rendered* row (unscrolled position minus the
    // scroll offset), or a click selects the control `scroll` rows away from
    // the one the user sees.
    let layout = body_layout(KEY_VENDORS.len(), state.vendors.len());
    let total = layout.rows + usize::from(!state.status.is_empty());
    let scroll = follow_focus_scroll(focus_line(state), total, chunks[0].height, state.scroll);
    state.scroll = scroll;

    // A row's on-screen y is the body top plus its line index minus the scroll
    // offset (each rendered line is exactly one row). Recorded alongside each
    // interactive row so a mouse click maps back to a focus target. Rows
    // scrolled past either edge of the body are not rendered and get no rect.
    let row_at = |line: usize| {
        let top = chunks[0].y;
        let bottom = top.saturating_add(chunks[0].height);
        let y = top.saturating_add(line as u16).checked_sub(scroll)?;
        (y >= top && y < bottom).then_some(Rect::new(inner.x, y, inner.width, 1))
    };

    // — Primary vendor + API keys header —
    let mut lines: Vec<Line> = vec![
        section_header("Primary vendor", "shown first on the bar / TUI", &bubble),
        primary_line(state, &bubble),
        Line::from(""),
        section_header("API keys", "all key providers", &bubble),
    ];
    if let Some(r) = row_at(1) {
        // The focused radio renders "◀ name ▶" over a name padded to the
        // widest choice, so the arrows sit at fixed columns however long the
        // current pick is. The arrow cells click-step the radio; the name
        // cell opens the picker popup. Unfocused rows show no arrows — the
        // first click only focuses.
        if state.focus == Focus::Primary {
            let pad = primary_name_pad(state);
            hits.push((
                SettingsRow::Switch(Focus::Primary, KeyCode::Left, KeyModifiers::NONE),
                Rect::new(r.x.saturating_add(5), r.y, 2, 1),
            ));
            hits.push((
                SettingsRow::Switch(Focus::Primary, KeyCode::Right, KeyModifiers::NONE),
                Rect::new(r.x.saturating_add(9 + pad), r.y, 2, 1),
            ));
            hits.push((
                SettingsRow::OpenPicker,
                Rect::new(r.x.saturating_add(7), r.y, pad + 2, 1),
            ));
        }
        hits.push((SettingsRow::Focus(Focus::Primary), r));
    }

    for (i, kv) in KEY_VENDORS.iter().enumerate() {
        let focused = state.focus == Focus::Key(i);
        if let Some(r) = row_at(lines.len()) {
            hits.push((SettingsRow::Focus(Focus::Key(i)), r));
        }
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
        let focused = state.focus == Focus::Vendor(i);
        if let Some(r) = row_at(lines.len()) {
            // The switch cell is pushed first so a click on the on/off value
            // toggles the row instead of only focusing it. Its rect covers
            // exactly the rendered value segment: not the name tail (a name
            // past the 11-column padding pushes the value right) and not the
            // empty space right of the row.
            let (value_x, value_w) =
                switch_cell(id.display_name(), switch_value(switch.enabled), focused);
            hits.push((
                SettingsRow::Switch(Focus::Vendor(i), KeyCode::Char(' '), KeyModifiers::NONE),
                Rect::new(r.x.saturating_add(value_x), r.y, value_w, 1),
            ));
            hits.push((SettingsRow::Focus(Focus::Vendor(i)), r));
        }
        lines.push(provider_row(id, switch, focused, &bubble));
    }
    lines.push(Line::from(""));

    // — Notifications —
    lines.push(section_header(
        "Notifications",
        "desktop alert when a quota window crosses the threshold",
        &bubble,
    ));
    if let Some(r) = row_at(lines.len()) {
        let (value_x, value_w) = switch_cell(
            "Quota alerts",
            switch_value(state.notify_enabled),
            state.focus == Focus::NotifyEnabled,
        );
        hits.push((
            SettingsRow::Switch(Focus::NotifyEnabled, KeyCode::Char(' '), KeyModifiers::NONE),
            Rect::new(r.x.saturating_add(value_x), r.y, value_w, 1),
        ));
        hits.push((SettingsRow::Focus(Focus::NotifyEnabled), r));
    }
    lines.push(notify_enabled_line(state, &bubble));
    if let Some(r) = row_at(lines.len()) {
        hits.push((SettingsRow::Focus(Focus::NotifyThreshold), r));
    }
    lines.push(notify_threshold_line(state, &bubble));
    lines.push(Line::from(""));

    // — Save + status —
    if let Some(r) = row_at(lines.len()) {
        hits.push((SettingsRow::Focus(Focus::Save), r));
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

    // The view follows the focus ring; `scroll` was computed (and persisted
    // into the state) before the hit rects were recorded.
    f.render_widget(Paragraph::new(lines).scroll((scroll, 0)), chunks[0]);

    // Context-aware hint footer.
    /// One hint segment: the key label, its description, and the synthetic key a
    /// click sends through [`handle_key`] (`None` for pure key hints).
    type HintSegment<'a> = (&'a str, &'a str, Option<(KeyCode, KeyModifiers)>);

    // Context-aware hint footer. Each segment is (key, description, click):
    // segments with a key payload are actionable "links" — clicking one sends
    // that synthetic key through `handle_key`, so save/close/toggle stay in
    // one place. Pure key hints (move/type/digits) get no click rect.
    let segments: &[HintSegment] = match state.focus {
        Focus::Primary => &[
            ("↑↓/tab", "move", None),
            (
                "←→",
                "change vendor",
                Some((KeyCode::Right, KeyModifiers::NONE)),
            ),
            (
                "^S",
                "save",
                Some((KeyCode::Char('s'), KeyModifiers::CONTROL)),
            ),
            ("esc", "close", Some((KeyCode::Esc, KeyModifiers::NONE))),
        ],
        Focus::Key(_) => &[
            ("↑↓/tab", "move", None),
            ("type", "edit key", None),
            (
                "^V",
                "reveal",
                Some((KeyCode::Char('v'), KeyModifiers::CONTROL)),
            ),
            (
                "^S",
                "save",
                Some((KeyCode::Char('s'), KeyModifiers::CONTROL)),
            ),
            ("esc", "close", Some((KeyCode::Esc, KeyModifiers::NONE))),
        ],
        Focus::Vendor(_) | Focus::NotifyEnabled => &[
            ("↑↓/tab", "move", None),
            (
                "←→/space",
                "toggle",
                Some((KeyCode::Char(' '), KeyModifiers::NONE)),
            ),
            (
                "^S",
                "save",
                Some((KeyCode::Char('s'), KeyModifiers::CONTROL)),
            ),
            ("esc", "close", Some((KeyCode::Esc, KeyModifiers::NONE))),
        ],
        Focus::NotifyThreshold => &[
            ("↑↓/tab", "move", None),
            ("type", "digits 1-100", None),
            (
                "^S",
                "save",
                Some((KeyCode::Char('s'), KeyModifiers::CONTROL)),
            ),
            ("esc", "close", Some((KeyCode::Esc, KeyModifiers::NONE))),
        ],
        Focus::Save => &[
            ("↑↓/tab", "move", None),
            (
                "enter/^S",
                "save",
                Some((KeyCode::Enter, KeyModifiers::NONE)),
            ),
            ("esc", "close", Some((KeyCode::Esc, KeyModifiers::NONE))),
        ],
    };
    let hint = bubble.help_line(segments.iter().map(|(key, desc, _)| (*key, *desc)));
    f.render_widget(Paragraph::new(hint), chunks[1]);

    // Help draws each segment as "key description" joined by " • " (3 cells,
    // same separator the main footer replicates for its click targets), left
    // aligned from the row's first cell. Record the same cells for segments
    // with a click payload, clipped to the visible row width.
    let right = chunks[1].x.saturating_add(chunks[1].width);
    let mut x = chunks[1].x;
    for (key, desc, action) in segments {
        let width = (crate::display::text_width(key) as u16)
            .saturating_add(1)
            .saturating_add(crate::display::text_width(desc) as u16);
        if let Some((code, mods)) = action
            && x < right
        {
            let visible = width.min(right.saturating_sub(x));
            if visible > 0 {
                hits.push((
                    SettingsRow::HintKey(*code, *mods),
                    Rect::new(x, chunks[1].y, visible, 1),
                ));
            }
        }
        x = x.saturating_add(width).saturating_add(3);
    }

    // The primary-vendor picker floats over the body when open, drawn last so
    // it lands on top of everything already rendered.
    if state.picker.is_some() {
        render_primary_picker(f, state, &bubble, chunks[0], hits);
    }
}

/// The primary-vendor dropdown: anchored under the radio row, listing
/// [`SettingsState::primary_choices`] with the cursor scrolled into view.
/// Rows are recorded as [`SettingsRow::Pick`] click targets.
fn render_primary_picker(
    f: &mut Frame,
    state: &mut SettingsState,
    theme: &BubbleTheme,
    body: Rect,
    hits: &mut Vec<(SettingsRow, Rect)>,
) {
    let count = state.primary_choices.len();
    if count == 0 {
        return;
    }
    let pad = primary_name_pad(state) as usize;
    let width = u16::try_from(pad + 6).unwrap_or(u16::MAX).min(body.width);
    if width < 4 {
        return;
    }
    // Anchor under the radio row (body line 1), capped to the body bottom.
    let height = (count as u16 + 2)
        .min(body.bottom().saturating_sub(body.y + 2))
        .max(3);
    let area = Rect::new(
        body.x.saturating_add(2),
        body.y.saturating_add(2),
        width,
        height,
    );

    f.render_widget(Clear, area);
    let block = theme.titled_modal_block(" Primary vendor ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(picker) = state.picker.as_mut() else {
        return;
    };
    let viewport = inner.height;
    picker.scroll = follow_focus_scroll(picker.cursor, count, viewport, picker.scroll);
    for (index, vendor) in state
        .primary_choices
        .iter()
        .enumerate()
        .skip(picker.scroll as usize)
        .take(viewport as usize)
    {
        let y = inner.y + u16::try_from(index - picker.scroll as usize).unwrap_or(u16::MAX);
        let cursor = picker.cursor == index;
        let current = *vendor == state.primary;
        let style = if cursor {
            theme.accent.add_modifier(Modifier::BOLD)
        } else if current {
            theme.title
        } else {
            theme.text
        };
        let line = Line::from(vec![
            theme.span(if cursor { "▸ " } else { "  " }),
            Span::styled(format!("{:<pad$}", vendor.display_name(), pad = pad), style),
            theme.span(if current { "  ✓" } else { "" }),
        ]);
        f.render_widget(line, Rect::new(inner.x, y, inner.width, 1));
        hits.push((
            SettingsRow::Pick(index),
            Rect::new(inner.x, y, inner.width, 1),
        ));
    }
}

fn section_header(title: &str, sub: &str, theme: &BubbleTheme) -> Line<'static> {
    Line::from(vec![
        theme.span(" "),
        Span::styled(title.to_string(), theme.title.add_modifier(Modifier::BOLD)),
        theme.muted(format!("   — {sub}")),
    ])
}

/// Display width the primary radio pads its name to: the widest choice, so
/// the ◀/▶ arrows sit at fixed columns however long the current pick is.
fn primary_name_pad(state: &SettingsState) -> u16 {
    state
        .primary_choices
        .iter()
        .map(|v| crate::display::text_width(v.display_name()) as u16)
        .max()
        .unwrap_or(1)
}

fn primary_line(state: &SettingsState, theme: &BubbleTheme) -> Line<'static> {
    let focused = state.focus == Focus::Primary;
    let name = state.primary.display_name().to_string();
    if focused {
        let pad = primary_name_pad(state) as usize;
        Line::from(vec![
            theme.span("   "),
            Span::styled("▸ ", theme.accent.add_modifier(Modifier::BOLD)),
            Span::styled("◀ ", theme.accent),
            Span::styled(
                format!(" {:<pad$} ", name, pad = pad),
                theme
                    .selected
                    .add_modifier(Modifier::REVERSED | Modifier::BOLD),
            ),
            Span::styled(" ▶", theme.accent),
            theme.muted("    ← → or click to change"),
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

/// Labels in provider / notification rows are padded to at least 11 columns;
/// the renderers and the mouse hit-test share this so a click lands where the
/// value actually renders.
fn padded_label(label: &str) -> String {
    format!("{:<11}", label)
}

/// The on/off text a provider / quota switch renders.
fn switch_value(enabled: bool) -> &'static str {
    if enabled { "on" } else { "off" }
}

/// The value cell's offset and rendered width. Mirrors provider_row /
/// notify_enabled_line: a 5-cell focus prefix, the padded label, then either
/// `" ◀ on ▶ "` when focused or `"  on"` when unfocused. Names past 11
/// columns push the cell right — never assume a fixed column.
fn switch_cell(label: &str, value: &str, focused: bool) -> (u16, u16) {
    let value_x = 5 + crate::display::text_width(&padded_label(label)) as u16;
    let decoration_w = if focused { 6 } else { 2 };
    let value_w = decoration_w + crate::display::text_width(value) as u16;
    (value_x, value_w)
}

/// One `[<vendor>] enabled` row (#244): the shared display name plus an
/// on/off value styled like the notification toggle.
fn provider_row(
    id: &VendorId,
    switch: ProviderSwitch,
    focused: bool,
    theme: &BubbleTheme,
) -> Line<'static> {
    let label = padded_label(id.display_name());
    let value = switch_value(switch.enabled);
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
    let label = padded_label("Quota alerts");
    let value = switch_value(state.notify_enabled);
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
            picker: None,
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
            VendorId::Deepinfra,
            VendorId::Kilo,
            VendorId::Novita,
            VendorId::Synthetic,
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

    /// Switching off the vendor that is still the primary is a state a save
    /// can produce. A key vendor that keeps its key stays on offer, so the
    /// overlay must not reopen with it selected: the next save, whatever it
    /// changed, would take that as the user's pick and switch it back on.
    #[test]
    fn an_unrelated_save_leaves_a_disabled_primary_switched_off() {
        let (_dir, path) = temp_config(Some(
            "[ui]\nprimary = \"openrouter\"\n[openrouter]\nenabled = false\napi_key = \"test\"\n",
        ));
        let cfg = Config::load_from(&path).unwrap();
        let mut s = SettingsState::from_config_with(&cfg, |_| false);
        assert!(s.primary_choices.contains(&VendorId::Openrouter));
        assert_ne!(s.primary, VendorId::Openrouter);

        s.notify_enabled = !s.notify_enabled;
        s.notify_enabled_dirty = true;
        save_to_path(&s, &path).unwrap();

        let saved = Config::load_from(&path).unwrap();
        assert!(!saved.is_enabled(VendorId::Openrouter));
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
    fn key_vendor_for_slug_finds_only_the_key_table() {
        // Every key vendor resolves by slug...
        for vendor in KEY_VENDORS {
            assert_eq!(
                key_vendor_for_slug(vendor.id.slug()).map(|kv| kv.id),
                Some(vendor.id),
                "{} must resolve",
                vendor.id.slug()
            );
        }
        // ...and no vendor outside the table does: the OAuth vendors' slugs
        // and the custom ids have no credential field to fill.
        for id in VendorId::all() {
            if KEY_VENDORS.iter().any(|kv| kv.id == *id) {
                continue;
            }
            assert!(key_vendor_for_slug(id.slug()).is_none(), "{:?}", id);
        }
        assert!(key_vendor_for_slug("no-such-vendor").is_none());
    }

    #[test]
    fn validate_key_value_refuses_empty_long_and_controlled() {
        let vendor = key_vendor_for_slug(VendorId::Zai.slug()).unwrap();
        assert!(validate_key_value(vendor, "test-key-value").is_ok());
        let empty = validate_key_value(vendor, "");
        assert!(empty.is_err());
        assert!(empty.unwrap_err().to_string().contains("is empty"));
        let long = "x".repeat(MAX_API_KEY_BYTES + 1);
        let over = validate_key_value(vendor, &long);
        assert!(over.is_err());
        assert!(
            over.unwrap_err()
                .to_string()
                .contains(&format!("exceeds {MAX_API_KEY_BYTES}"))
        );
        let controlled = validate_key_value(vendor, "a\u{1b}[31mb");
        assert!(controlled.is_err());
        assert!(
            controlled
                .unwrap_err()
                .to_string()
                .contains("control characters")
        );
        // The apply request enforces the same rule through the same code:
        // one accepted by the helper must be accepted by the request, and
        // one refused by the helper must be refused there.
        let cfg = crate::config::Config::default();
        let bad = serde_json::json!({
            "schema_version": 1,
            "keys": { VendorId::Zai.slug(): { "action": "set", "value": "a\u{1b}[31mb" } }
        })
        .to_string();
        assert!(state_from_apply_request(&cfg, &bad).is_err());
        let good = serde_json::json!({
            "schema_version": 1,
            "keys": { VendorId::Zai.slug(): { "action": "set", "value": "test-key-value" } }
        })
        .to_string();
        assert!(state_from_apply_request(&cfg, &good).is_ok());
    }

    #[test]
    fn inline_key_write_leaves_the_value_in_the_right_section() {
        let vendor = key_vendor_for_slug(VendorId::Zai.slug()).unwrap();
        let (_dir, path) = temp_config(Some("# keep me\n[ui]\nprimary = \"zai\"\n"));
        write_inline_key(&path, vendor, "test-key-value").unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("[zai]"), "{raw}");
        assert!(raw.contains(r#"api_key = "test-key-value""#), "{raw}");
        // A non-empty credential opts the vendor in, as the overlay does.
        assert!(raw.contains("enabled = true"), "{raw}");
        // And everything else of the config survives.
        assert!(raw.contains("# keep me"), "{raw}");
        assert!(raw.contains(r#"primary = "zai""#), "{raw}");
        // The file is back to a parseable config with the value captured.
        let snapshot = snapshot_inline_key(&path, vendor).unwrap();
        assert!(snapshot.key.is_some());
        assert!(snapshot.enabled.is_some());
    }

    #[test]
    #[cfg(unix)]
    fn inline_key_write_chmods_the_config_to_600() {
        use std::os::unix::fs::PermissionsExt;
        let vendor = key_vendor_for_slug(VendorId::Openrouter.slug()).unwrap();
        let (_dir, path) = temp_config(None);
        write_inline_key(&path, vendor, "test-key-value").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn inline_key_write_refuses_invalid_values_without_touching_the_file() {
        let vendor = key_vendor_for_slug(VendorId::Zai.slug()).unwrap();
        let original = "[zai]\napi_key = \"old-key-value\"\n";
        let (_dir, path) = temp_config(Some(original));
        let long = "x".repeat(MAX_API_KEY_BYTES + 1);
        for bad in ["", long.as_str(), "a\u{1b}[31mb"] {
            assert!(write_inline_key(&path, vendor, bad).is_err(), "{bad:?}");
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                original,
                "the file must not move for {bad:?}"
            );
        }
        // A missing config is created only by a valid value.
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("config.toml");
        assert!(write_inline_key(&empty, vendor, "").is_err());
        assert!(!empty.exists());
    }

    #[test]
    fn inline_key_snapshot_restores_the_exact_previous_state() {
        let vendor = key_vendor_for_slug(VendorId::Zai.slug()).unwrap();
        // An old key with the vendor off, and a comment on the key's line:
        // the restore must give all of it back, byte for byte.
        let original = "[ui]\nprimary = \"zai\"\n[zai]\n# my old key\napi_key = \"old-key-value\"\nenabled = false\n";
        let (_dir, path) = temp_config(Some(original));
        let snapshot = snapshot_inline_key(&path, vendor).unwrap();
        assert!(snapshot.file_existed);
        assert!(snapshot.section_existed);
        write_inline_key(&path, vendor, "test-key-value").unwrap();
        assert_ne!(std::fs::read_to_string(&path).unwrap(), original);
        restore_inline_key_snapshot(&path, vendor, &snapshot, "test-key-value").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn inline_key_snapshot_restore_keeps_a_later_user_decision() {
        let vendor = key_vendor_for_slug(VendorId::Zai.slug()).unwrap();
        let original = "[ui]\nprimary = \"zai\"\n[zai]\napi_key = \"old-key-value\"\n";
        let (_dir, path) = temp_config(Some(original));
        let snapshot = snapshot_inline_key(&path, vendor).unwrap();
        write_inline_key(&path, vendor, "test-key-value").unwrap();
        // While the check waits: the user saves another key and switches
        // the vendor off, with the real writers.
        write_inline_key(&path, vendor, "another-key-value").unwrap();
        crate::config::set_vendor_enabled(&path, vendor.id.slug(), false).unwrap();
        restore_inline_key_snapshot(&path, vendor, &snapshot, "test-key-value").unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        // Both fields hold the user's later decisions, not the snapshot.
        assert!(raw.contains(r#"api_key = "another-key-value""#), "{raw}");
        assert!(raw.contains("enabled = false"), "{raw}");
        assert!(!raw.contains("test-key-value"), "{raw}");
        assert!(!raw.contains("old-key-value"), "{raw}");
    }

    #[test]
    fn saved_inline_key_counts_an_empty_field_as_absent() {
        let vendor = key_vendor_for_slug(VendorId::Zai.slug()).unwrap();
        // The catalog's rule: `Config::inline_api_key` filters the empty
        // string, so a cleared field offers the key dialog again.
        let (_dir, path) = temp_config(Some("[zai]\napi_key = ''\n"));
        assert!(saved_inline_key(&snapshot_inline_key(&path, vendor).unwrap()).is_none());
        let (_dir, path) = temp_config(Some("[zai]\napi_key = \"k\"\n"));
        let snapshot = snapshot_inline_key(&path, vendor).unwrap();
        assert_eq!(saved_inline_key(&snapshot).as_deref(), Some("k"));
    }

    #[test]
    fn check_write_refuses_a_saved_nonempty_key() {
        let vendor = key_vendor_for_slug(VendorId::Zai.slug()).unwrap();
        let original = "[ui]\nprimary = \"zai\"\n[zai]\napi_key = \"old-key-value\"\n";
        let (_dir, path) = temp_config(Some(original));
        let error = match snapshot_and_write_inline_key_if_absent(&path, vendor, "test-key-value") {
            Ok(_) => panic!("a saved key must not be replaced by the check's write"),
            Err(error) => error,
        };
        assert_eq!(error.user_message(), KEY_ALREADY_SAVED_MESSAGE);
        // The refusal is inside the locked read-modify-write: nothing was
        // written.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn check_write_accepts_an_empty_field_and_returns_the_pre_state() {
        let vendor = key_vendor_for_slug(VendorId::Zai.slug()).unwrap();
        let (_dir, path) = temp_config(Some("[zai]\napi_key = ''\nenabled = false\n"));
        // The catalog offers the dialog for this file, so the conditional
        // write must too: the empty value counts as absent.
        let snapshot =
            snapshot_and_write_inline_key_if_absent(&path, vendor, "test-key-value").unwrap();
        assert!(snapshot.file_existed);
        assert!(snapshot.section_existed);
        // The snapshot is the state before the write: the empty field, and
        // the vendor still off — what a later restore compares against.
        assert_eq!(
            saved_inline_key(&snapshot),
            None,
            "an empty field is no credential, even when captured"
        );
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains(r#"api_key = "test-key-value""#), "{raw}");
        assert!(raw.contains("enabled = true"), "{raw}");
    }

    /// F6's scenario under the lock: the TUI's save and the check's
    /// conditional write both wait on the lock the test holds from the
    /// outside, and once it is released they run one after the other — in
    /// either order the key the TUI saved is never overwritten by the
    /// candidate.
    #[test]
    fn tui_save_and_check_write_serialize_on_the_config_lock() {
        use std::sync::mpsc;
        let (dir, path) = temp_config(Some("[ui]\nprimary = \"zai\"\n"));
        let _ = dir;
        let lock_path = crate::config::config_lock_path(&path);

        // Both real writers, queued on the lock the test holds: the TUI's
        // `save_to_path` and the check's locked snapshot—write.
        let waiters =
            |path: &std::path::Path, tx: mpsc::Sender<()>, tx_check: mpsc::Sender<Result<()>>| {
                let tui_path = path.to_path_buf();
                let tui = std::thread::spawn(move || {
                    let state = state_with("tui-key-value", "openrouter-key-value", VendorId::Zai);
                    tx.send(()).unwrap();
                    save_to_path(&state, &tui_path).unwrap();
                });
                let check_path = path.to_path_buf();
                let check = std::thread::spawn(move || {
                    let vendor = key_vendor_for_slug("zai").unwrap();
                    tx_check
                        .send(
                            snapshot_and_write_inline_key_if_absent(
                                &check_path,
                                vendor,
                                "candidate-key-value",
                            )
                            .map(|_| ()),
                        )
                        .unwrap();
                });
                (tui, check)
            };

        // Round 1: no key is saved yet. Both writers wait, the lock is
        // released, and whatever order the lock grants, the TUI's key is
        // what is left — the candidate either refused or got replaced by
        // the TUI's later save.
        let held = crate::cache::acquire_lock(&lock_path, std::time::Duration::from_secs(30))
            .expect("the test takes the config lock");
        let (tx, rx) = mpsc::channel();
        let (tx_check, rx_check) = mpsc::channel();
        let (tui, check) = waiters(&path, tx, tx_check);
        rx.recv().unwrap();
        drop(held);
        let check_result = rx_check.recv().unwrap();
        tui.join().unwrap();
        check.join().unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(
            raw.contains(r#"api_key = "tui-key-value""#),
            "the TUI's key must survive: {raw}"
        );
        assert!(
            !raw.contains("candidate-key-value"),
            "the candidate must not overwrite the TUI's key: {raw}"
        );
        if let Err(error) = check_result {
            // The TUI's save ran first: the check saw its key and refused
            // with the existing saved-key message.
            assert_eq!(error.user_message(), KEY_ALREADY_SAVED_MESSAGE);
        }

        // Round 2: the TUI's key is saved now. The same two writers wait
        // again; whatever order the lock grants, the check must refuse and
        // the file must not change.
        let before = std::fs::read_to_string(&path).unwrap();
        let held = crate::cache::acquire_lock(&lock_path, std::time::Duration::from_secs(30))
            .expect("the test takes the config lock again");
        let (tx, rx) = mpsc::channel();
        let (tx_check, rx_check) = mpsc::channel();
        let (tui, check) = waiters(&path, tx, tx_check);
        rx.recv().unwrap();
        drop(held);
        let error = rx_check
            .recv()
            .unwrap()
            .expect_err("a saved key must refuse the candidate");
        tui.join().unwrap();
        check.join().unwrap();
        assert_eq!(error.user_message(), KEY_ALREADY_SAVED_MESSAGE);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }
    #[test]
    fn inline_key_snapshot_removes_the_key_when_there_was_none() {
        let vendor = key_vendor_for_slug(VendorId::Deepseek.slug()).unwrap();
        let original = "[ui]\nprimary = \"deepseek\"\n";
        let (_dir, path) = temp_config(Some(original));
        let snapshot = snapshot_inline_key(&path, vendor).unwrap();
        assert!(!snapshot.section_existed);
        write_inline_key(&path, vendor, "test-key-value").unwrap();
        restore_inline_key_snapshot(&path, vendor, &snapshot, "test-key-value").unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        // The key is gone, the section the check created is gone, and
        // everything else of the file is intact.
        assert!(!raw.contains("api_key"), "the key must be gone: {raw}");
        assert!(
            !raw.contains("[deepseek]"),
            "the section must be gone: {raw}"
        );
        assert!(raw.contains(r#"primary = "deepseek""#), "{raw}");
    }

    #[test]
    fn inline_key_snapshot_of_a_missing_file_removes_it_on_restore() {
        let vendor = key_vendor_for_slug(VendorId::Kimi.slug()).unwrap();
        let (dir, path) = temp_config(None);
        let _ = dir;
        let snapshot = snapshot_inline_key(&path, vendor).unwrap();
        assert!(!snapshot.file_existed);
        assert!(!snapshot.section_existed);
        assert!(snapshot.key.is_none());
        assert!(snapshot.enabled.is_none());
        write_inline_key(&path, vendor, "test-key-value").unwrap();
        restore_inline_key_snapshot(&path, vendor, &snapshot, "test-key-value").unwrap();
        assert!(!path.exists(), "the file the check created must be gone");
    }

    #[test]
    fn inline_key_snapshot_keeps_edits_made_elsewhere_in_the_meantime() {
        let vendor = key_vendor_for_slug(VendorId::Zai.slug()).unwrap();
        let original = "[ui]\nprimary = \"zai\"\n[zai]\napi_key = \"old-key-value\"\n";
        let (_dir, path) = temp_config(Some(original));
        let snapshot = snapshot_inline_key(&path, vendor).unwrap();
        write_inline_key(&path, vendor, "test-key-value").unwrap();
        // An edit the check did not make: a new field in the same section.
        let mut doc = crate::config::read_config_document(&path).unwrap();
        set_string(&mut doc, "zai", "api_key_env", "SOME_ENV_VAR").unwrap();
        crate::config::write_config_document(&path, &doc).unwrap();
        restore_inline_key_snapshot(&path, vendor, &snapshot, "test-key-value").unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        // The snapshot's state is back...
        assert!(raw.contains(r#"api_key = "old-key-value""#), "{raw}");
        // ...and the foreign edit survived, untouched by the snapshot.
        assert!(raw.contains(r#"api_key_env = "SOME_ENV_VAR""#), "{raw}");
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
    fn save_clears_a_key_from_a_toml_inline_table() {
        let (_dir, path) = temp_config(Some(
            "zai = { enabled = false, api_key = \"remove-me\", plan_tier = \"pro\" } # keep me\n\
             [ui]\nprimary = \"anthropic\"\n",
        ));
        let cfg = Config::load_from(&path).unwrap();
        let mut state = SettingsState::from_config_with(&cfg, |_| false);
        let input = &mut state.keys[key_index(VendorId::Zai)];
        *input = KeyInput::default();
        input.dirty = true;

        save_to_path(&state, &path).unwrap();

        let reloaded = Config::load_from(&path).unwrap();
        assert!(reloaded.zai.api_key.is_none());
        assert!(!reloaded.zai.enabled);
        assert_eq!(reloaded.zai.plan_tier.as_deref(), Some("pro"));
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("remove-me"));
        assert!(raw.contains("zai = {"));
        assert!(raw.contains("# keep me"));
    }

    #[test]
    fn save_updates_a_key_and_enables_a_toml_inline_table() {
        let (_dir, path) = temp_config(Some(
            "zai = { enabled = false, api_key = \"old-secret\", plan_tier = \"pro\" } # keep me\n\
             [ui]\nprimary = \"anthropic\"\n",
        ));
        let cfg = Config::load_from(&path).unwrap();
        let mut state = SettingsState::from_config_with(&cfg, |_| false);
        let input = &mut state.keys[key_index(VendorId::Zai)];
        input.buf = "new-secret".into();
        input.dirty = true;

        save_to_path(&state, &path).unwrap();

        let reloaded = Config::load_from(&path).unwrap();
        assert_eq!(reloaded.zai.api_key.as_deref(), Some("new-secret"));
        assert!(reloaded.zai.enabled);
        assert_eq!(reloaded.zai.plan_tier.as_deref(), Some("pro"));
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("old-secret"));
        assert!(raw.contains("new-secret"));
        assert!(raw.contains("zai = {"));
        assert!(raw.contains("# keep me"));
    }

    #[test]
    fn enable_vendor_at_enables_a_toml_inline_table() {
        let (_dir, path) = temp_config(Some(
            "zai = { enabled = false, plan_tier = \"pro\" } # keep me\n\
             [ui]\nprimary = \"anthropic\"\n",
        ));
        enable_vendor_at(&path, VendorId::Zai).unwrap();

        let reloaded = Config::load_from(&path).unwrap();
        assert!(reloaded.zai.enabled);
        assert_eq!(reloaded.zai.plan_tier.as_deref(), Some("pro"));
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("zai = {"));
        assert!(raw.contains("# keep me"));
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

    /// Actionable hint segments record `HintKey` click rects on the hint row;
    /// pure key hints (move/type/digits) never do.
    #[test]
    fn render_records_clickable_hint_links_for_the_focused_control() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let theme = crate::theme::Theme::default();
        let mut state = blank_state(VendorId::Anthropic);
        state.focus = Focus::Vendor(vendor_index(VendorId::Grok));
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        let mut hits = Vec::new();
        terminal
            .draw(|f| render(f, f.area(), &mut state, &theme, &mut hits))
            .unwrap();

        let hint_keys: Vec<(KeyCode, KeyModifiers)> = hits
            .iter()
            .filter_map(|(row, _)| match row {
                SettingsRow::HintKey(code, mods) => Some((*code, *mods)),
                _ => None,
            })
            .collect();
        // Toggle (space), save (^S) and close (esc) are links, recorded in
        // segment order on a Vendor row.
        assert_eq!(
            hint_keys,
            vec![
                (KeyCode::Char(' '), KeyModifiers::NONE),
                (KeyCode::Char('s'), KeyModifiers::CONTROL),
                (KeyCode::Esc, KeyModifiers::NONE),
            ]
        );
        // Every link lives on the single hint row, below the body.
        let ys: Vec<u16> = hits
            .iter()
            .filter(|(row, _)| matches!(row, SettingsRow::HintKey(..)))
            .map(|(_, rect)| rect.y)
            .collect();
        assert!(ys.windows(2).all(|pair| pair[0] == pair[1]));

        // On the Primary row the change-vendor link is a Right key instead of
        // the toggle, and pure hints never become clickable anywhere.
        let mut state = blank_state(VendorId::Anthropic);
        state.focus = Focus::Primary;
        let mut hits = Vec::new();
        terminal
            .draw(|f| render(f, f.area(), &mut state, &theme, &mut hits))
            .unwrap();
        let hint_keys: Vec<(KeyCode, KeyModifiers)> = hits
            .iter()
            .filter_map(|(row, _)| match row {
                SettingsRow::HintKey(code, mods) => Some((*code, *mods)),
                _ => None,
            })
            .collect();
        assert!(hint_keys.contains(&(KeyCode::Right, KeyModifiers::NONE)));
        assert!(!hint_keys.contains(&(KeyCode::Char(' '), KeyModifiers::NONE)));
    }

    /// Every provider row records a switch-cell click target right of the
    /// label columns, on the same row as its focus target; the quota-alerts
    /// row records one too.
    #[test]
    fn render_records_switch_cells_right_of_the_labels() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let theme = crate::theme::Theme::default();
        let mut state = blank_state(VendorId::Anthropic);
        state.focus = Focus::Vendor(0);
        let mut terminal = Terminal::new(TestBackend::new(100, 70)).unwrap();
        let mut hits = Vec::new();
        terminal
            .draw(|f| render(f, f.area(), &mut state, &theme, &mut hits))
            .unwrap();

        let focus_of = |i: usize| {
            hits.iter().find_map(|(row, rect)| match row {
                SettingsRow::Focus(Focus::Vendor(v)) if *v == i => Some(*rect),
                _ => None,
            })
        };
        for (i, id) in VendorId::all().iter().enumerate() {
            let focus_rect = focus_of(i).unwrap_or_else(|| panic!("vendor {i} has no focus row"));
            let switch = hits
                .iter()
                .find_map(|(row, rect)| match row {
                    SettingsRow::Switch(Focus::Vendor(v), KeyCode::Char(' '), _) if *v == i => {
                        Some(*rect)
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("vendor {i} has no switch cell"));
            // The value cell starts after the 5-cell prefix + the padded
            // label (names past 11 columns push it right) and covers only
            // the rendered value — never the name tail or the trailing
            // empty row space.
            let expected_x =
                5 + crate::display::text_width(&format!("{:<11}", id.display_name())) as u16;
            let value_len = if state.vendors[i].enabled { 2 } else { 3 }; // "on" / "off"
            let focused = state.focus == Focus::Vendor(i);
            assert_eq!(switch.y, focus_rect.y);
            assert_eq!(switch.x, focus_rect.x + expected_x);
            assert_eq!(
                switch.width,
                (if focused { 6 } else { 2 }) + value_len,
                "rendered value segment"
            );
            assert!(switch.x + switch.width <= focus_rect.x + focus_rect.width);
        }
        // The quota-alerts row has a switch cell as well.
        assert!(hits.iter().any(|(row, _)| matches!(
            row,
            SettingsRow::Switch(Focus::NotifyEnabled, KeyCode::Char(' '), _)
        )));
    }

    /// The focused primary radio renders ◀ name ▶: those arrow cells become
    /// click targets that step the vendor radio both ways.
    #[test]
    fn render_records_primary_arrow_cells_when_focused() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let theme = crate::theme::Theme::default();
        let mut state = blank_state(VendorId::Anthropic);
        state.focus = Focus::Primary;
        let mut terminal = Terminal::new(TestBackend::new(100, 70)).unwrap();
        let mut hits = Vec::new();
        terminal
            .draw(|f| render(f, f.area(), &mut state, &theme, &mut hits))
            .unwrap();

        let focus_rect = hits
            .iter()
            .find_map(|(row, rect)| match row {
                SettingsRow::Focus(Focus::Primary) => Some(*rect),
                _ => None,
            })
            .expect("primary row has a focus target");
        let prev = hits.iter().find_map(|(row, rect)| match row {
            SettingsRow::Switch(Focus::Primary, KeyCode::Left, _) => Some(*rect),
            _ => None,
        });
        let next = hits.iter().find_map(|(row, rect)| match row {
            SettingsRow::Switch(Focus::Primary, KeyCode::Right, _) => Some(*rect),
            _ => None,
        });
        let prev = prev.expect("focused primary has a ◀ cell");
        let next = next.expect("focused primary has a ▶ cell");
        assert_eq!(prev.x, focus_rect.x + 5);
        assert_eq!(prev.width, 2);
        assert!(next.x > prev.x + 2, "▶ sits right of the vendor name");
        assert_eq!(next.width, 2);
        assert_eq!(prev.y, next.y);

        // Unfocused (arrows only render on the focused radio), the row shows
        // no switch cells: only the focus target exists.
        let mut state = blank_state(VendorId::Anthropic);
        state.focus = Focus::Vendor(0);
        let mut unfocused_hits = Vec::new();
        terminal
            .draw(|f| render(f, f.area(), &mut state, &theme, &mut unfocused_hits))
            .unwrap();
        assert!(
            !unfocused_hits
                .iter()
                .any(|(row, _)| matches!(row, SettingsRow::Switch(Focus::Primary, _, _)))
        );
    }

    /// Regression: ▶ used to sit at 9 + the current name's width, so it
    /// moved on every cycle. The name pads to the widest choice now — the
    /// arrow columns are identical whichever vendor is picked.
    #[test]
    fn primary_arrow_cells_sit_at_fixed_columns() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let theme = crate::theme::Theme::default();
        let mut terminal = Terminal::new(TestBackend::new(100, 70)).unwrap();
        let mut arrow_x = |primary: VendorId| {
            let mut state = blank_state(primary);
            state.focus = Focus::Primary;
            let mut hits = Vec::new();
            terminal
                .draw(|f| render(f, f.area(), &mut state, &theme, &mut hits))
                .unwrap();
            hits.iter()
                .find_map(|(row, rect)| match row {
                    SettingsRow::Switch(Focus::Primary, KeyCode::Right, _) => Some(rect.x),
                    _ => None,
                })
                .expect("focused primary has a ▶ cell")
        };
        assert_eq!(arrow_x(VendorId::Anthropic), arrow_x(VendorId::Copilot));
    }

    /// The picker consumes keys while open: ↑/↓ move the cursor (wrapping),
    /// Enter/space select and close, Esc closes without selecting — and with
    /// the picker closed, Esc closes the modal again.
    #[test]
    fn picker_key_handling_selects_and_closes() {
        let mut s = blank_state(VendorId::Anthropic);
        s.focus = Focus::Primary;
        s.picker = Some(PrimaryPicker {
            cursor: 0,
            scroll: 0,
        });
        let count = s.primary_choices.len();
        assert!(count > 1);

        assert_eq!(
            handle_key(&mut s, KeyCode::Down, KeyModifiers::NONE),
            Action::Continue
        );
        assert_eq!(s.picker.as_ref().unwrap().cursor, 1);
        // Wraps at the top.
        handle_key(&mut s, KeyCode::Up, KeyModifiers::NONE);
        handle_key(&mut s, KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(s.picker.as_ref().unwrap().cursor, count - 1);

        // Enter selects the cursor's choice and closes the popup.
        let expected = s.primary_choices[s.picker.as_ref().unwrap().cursor];
        assert_eq!(
            handle_key(&mut s, KeyCode::Enter, KeyModifiers::NONE),
            Action::Continue
        );
        assert_eq!(s.primary, expected);
        assert!(s.picker.is_none());

        // Esc closes the picker without selecting; the next Esc closes the
        // modal.
        s.picker = Some(PrimaryPicker {
            cursor: 2,
            scroll: 0,
        });
        assert_eq!(
            handle_key(&mut s, KeyCode::Esc, KeyModifiers::NONE),
            Action::Continue
        );
        assert!(s.picker.is_none());
        assert_eq!(
            handle_key(&mut s, KeyCode::Esc, KeyModifiers::NONE),
            Action::Close
        );
    }

    /// The open picker records one click row per choice.
    #[test]
    fn open_picker_records_one_click_row_per_choice() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let theme = crate::theme::Theme::default();
        let mut state = blank_state(VendorId::Anthropic);
        state.focus = Focus::Primary;
        state.picker = Some(PrimaryPicker {
            cursor: 0,
            scroll: 0,
        });
        let mut terminal = Terminal::new(TestBackend::new(100, 70)).unwrap();
        let mut hits = Vec::new();
        terminal
            .draw(|f| render(f, f.area(), &mut state, &theme, &mut hits))
            .unwrap();

        let picks: Vec<usize> = hits
            .iter()
            .filter_map(|(row, _)| match row {
                SettingsRow::Pick(i) => Some(*i),
                _ => None,
            })
            .collect();
        assert_eq!(picks.len(), state.primary_choices.len());
        assert_eq!(picks.first().copied(), Some(0));
        assert_eq!(picks.last().copied(), Some(state.primary_choices.len() - 1));
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
        let mut hits = Vec::new();

        terminal
            .draw(|f| render(f, f.area(), &mut state, &theme, &mut hits))
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
        let mut hits = Vec::new();
        terminal
            .draw(|f| render(f, f.area(), &mut state, &theme, &mut hits))
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
