//! Config file at `~/.config/ai-usagebar/config.toml`.
//!
//! Layout:
//! ```toml
//! [anthropic]  enabled = true
//! [openai]     enabled = true   # Codex OAuth from ~/.codex/auth.json
//! [copilot]    enabled = false  # GitHub CLI OAuth, or an explicit env override
//! [zai]        enabled = true
//! [openrouter] enabled = true
//! [deepseek]   enabled = false
//! [kimi]       enabled = false
//! [[custom]]   id = "mytool"   # user-defined HTTP provider, static token
//! ```
//!
//! Every field is optional with sensible defaults — missing config file is
//! treated as "use defaults". API keys are read from env vars (the relevant
//! `*_api_key_env` field lets the user override which env var name).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

use serde::{Deserialize, Serialize};

use crate::anthropic::creds::CredsTarget;
use crate::cache::Cache;
use crate::error::{AppError, Result};
use crate::vendor::VendorId;

/// A misspelled section name is silently ignored without this: `[openrouer]`
/// leaves OpenRouter on its defaults and the user sees the wrong vendor set
/// with no diagnostic. Denying unknown keys is deliberately applied at the
/// *section* level only — the set of sections is small and stable, whereas
/// denying unknown keys inside every section would hard-fail configs that
/// carry a field from a future or removed version.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub ui: UiConfig,
    pub tray: TrayConfig,
    pub context: ContextConfig,
    pub anthropic: AnthropicConfig,
    pub anthropic_api: AnthropicApiConfig,
    pub openai: OpenAiConfig,
    pub copilot: CopilotConfig,
    pub zai: ZaiConfig,
    pub openrouter: OpenRouterConfig,
    pub deepseek: DeepseekConfig,
    pub kimi: KimiConfig,
    pub kilo: KiloConfig,
    pub novita: NovitaConfig,
    pub moonshot: MoonshotConfig,
    pub grok: GrokConfig,
    pub supergrok: SuperGrokConfig,
    pub antigravity: AntigravityConfig,
    pub cursor: CursorConfig,
    pub minimax: MinimaxConfig,
    pub kiro: KiroConfig,
    pub nous: NousConfig,
    #[serde(rename = "opencode-go")]
    pub opencode_go: OpenCodeGoConfig,
    pub commandcode: CommandCodeConfig,
    /// User-defined providers, one `[[custom]]` table each.
    pub custom: Vec<CustomProviderConfig>,
}

/// UI / dispatch preferences. Currently just `primary` — which vendor the
/// widget shows when `--vendor` is omitted, and which TUI tab is selected
/// at startup.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct UiConfig {
    /// `None` → fall back to anthropic for backward compatibility.
    pub primary: Option<VendorId>,
    /// Which vendors the Overview shows (the TUI's first tab and the macOS
    /// menu-bar's top section), in this order. `None` → every enabled vendor,
    /// in the canonical order.
    pub overview_vendors: Option<Vec<VendorId>>,
    /// Layout style for vendor navigation in the TUI: sidebar | navbar | none.
    pub vendor_box: Option<VendorBoxStyle>,
}

impl UiConfig {
    pub fn vendor_box(&self) -> VendorBoxStyle {
        self.vendor_box.unwrap_or_default()
    }
}

/// Windows tray popover preferences the host process needs before the
/// WebView is up: the global shortcut it registers, how often it polls and
/// how it treats new releases. Screen-only preferences (theme, density, time
/// format) live in the popover's own storage instead.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct TrayConfig {
    /// Global shortcut that toggles the popover, in the canonical
    /// "Ctrl+Shift+U" spelling. `None` → no shortcut registered.
    pub shortcut: Option<String>,
    /// How often the tray re-reads every provider, in minutes: 1, 5 or 10.
    /// The footer's Refresh is always immediate. `None` → 5.
    pub refresh_minutes: Option<u64>,
    /// What the tray does when a newer release is published.
    pub updates: Option<UpdateMode>,
}

/// Poll intervals the tray offers, in minutes. The provider cache TTL is
/// 60 s regardless; this only decides how often the tray asks.
pub const TRAY_REFRESH_MINUTES: [u64; 3] = [1, 5, 10];
const DEFAULT_TRAY_REFRESH_MINUTES: u64 = 5;

impl TrayConfig {
    pub fn refresh_minutes(&self) -> u64 {
        self.refresh_minutes.unwrap_or(DEFAULT_TRAY_REFRESH_MINUTES)
    }

    pub fn updates(&self) -> UpdateMode {
        self.updates.unwrap_or_default()
    }
}

/// How the tray handles a newer release: install it unattended, show a
/// banner with an Install button, or never check in the background.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateMode {
    Auto,
    #[default]
    Notify,
    Off,
}

impl UpdateMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Notify => "notify",
            Self::Off => "off",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "notify" => Some(Self::Notify),
            "off" => Some(Self::Off),
            _ => None,
        }
    }
}

/// Presentation style of the TUI vendor navigation box.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VendorBoxStyle {
    /// Vertical sidebar box on wide terminals; falls back to top navbar on narrow terminals.
    #[default]
    Sidebar,
    /// Horizontal navbar strip above the dashboard detail panel.
    Navbar,
    /// Completely hide vendor navigation (dashboards expand to fill full width).
    None,
}

/// Where the context view docks in the dashboard body. `v` cycles it while the
/// overlay is open; the config value is what it opens with.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextLayout {
    /// Takes the whole body, the way a vendor panel does.
    #[default]
    Full,
    /// Beside the dashboard.
    Split,
    /// Below the dashboard.
    Bottom,
}

impl ContextLayout {
    pub fn next(self) -> Self {
        match self {
            ContextLayout::Full => ContextLayout::Split,
            ContextLayout::Split => ContextLayout::Bottom,
            ContextLayout::Bottom => ContextLayout::Full,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ContextLayout::Full => "full",
            ContextLayout::Split => "split",
            ContextLayout::Bottom => "bottom",
        }
    }
}

/// Optional local Claude Code context-window monitor. This is deliberately
/// separate from vendors: sessions are discovered from local transcripts and
/// change while the TUI is running, whereas vendor tabs are config-declared
/// account identities.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ContextConfig {
    /// Keep the filesystem scanner completely dormant unless explicitly
    /// enabled. The `c` key and its footer hint are hidden while disabled.
    pub enabled: bool,
    /// Override Claude Code's normal `~/.claude/projects` transcript root.
    pub projects_path: Option<PathBuf>,
    /// Optional fallback denominator. When absent, sessions without an exact
    /// model override show their input-token count without inventing a %.
    pub context_window_tokens: Option<u64>,
    /// Exact Claude model id -> context-window size. This takes precedence
    /// over `context_window_tokens`, which keeps mixed 200K/1M histories safe.
    pub model_context_window_tokens: BTreeMap<String, u64>,
    /// Where the view opens: full | split | bottom.
    pub layout: ContextLayout,
}

impl ContextConfig {
    pub fn window_tokens_for(&self, model: Option<&str>) -> Option<u64> {
        model
            .and_then(|model| self.model_context_window_tokens.get(model).copied())
            .filter(|tokens| *tokens > 0)
            .or_else(|| self.context_window_tokens.filter(|tokens| *tokens > 0))
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct AnthropicConfig {
    pub enabled: bool,
    /// Override the credentials file path (defaults to `~/.claude/.credentials.json`).
    /// This is the *default* account; extra subscriptions go in `accounts`.
    pub credentials_path: Option<PathBuf>,
    /// Extra Anthropic accounts beyond the default, each selected on the CLI
    /// with `--account <label>` (issue #14). Empty by default, so existing
    /// single-account configs are byte-for-byte unchanged.
    pub accounts: Vec<AnthropicAccount>,
    /// Directory to auto-discover extra accounts from, in Claude Code's own
    /// `CLAUDE_CONFIG_DIR` layout: each immediate subdirectory becomes an
    /// account labeled by the subdirectory name. The credentials may live in
    /// that directory's `.credentials.json` or in the macOS Keychain, so
    /// discovery intentionally does not probe for the credentials file.
    /// Merged with `accounts` (explicit wins on a label clash); each is
    /// refreshed independently.
    pub accounts_dir: Option<PathBuf>,
    /// Whether the default (unnamed) Claude account gets its own tab. Defaults
    /// to `true` for back-compat. Set `false` when every account is managed
    /// explicitly (via `accounts`/`accounts_dir`) so the ambient
    /// Keychain/`~/.claude` login doesn't add a redundant "Claude" tab. Ignored
    /// when there are no named accounts, so Anthropic never loses its only tab.
    pub show_default_account: bool,
    /// Where the Claude **Desktop app**'s saved account profiles live. Defaults
    /// to `~/.claude-acc/profiles`, the store claude-acc
    /// (<https://github.com/ohmaseclaro/claude-acc>) creates — `account switch`
    /// reads and writes that layout so the two tools stay interchangeable.
    /// Unrelated to `accounts_dir`, which is the `claude` CLI's own accounts.
    pub desktop_profiles_dir: Option<PathBuf>,
}

impl Default for AnthropicConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            credentials_path: None,
            accounts: Vec::new(),
            accounts_dir: None,
            show_default_account: true,
            desktop_profiles_dir: None,
        }
    }
}

/// One extra Anthropic account beyond the default (issue #14). The default
/// account stays the singular `[anthropic] credentials_path`; each entry here
/// is an additional subscription selected on the CLI with `--account <label>`.
///
/// ```toml
/// [[anthropic.accounts]]
/// label = "work"
/// credentials_path = "~/.config/ai-usagebar/accounts/work/.credentials.json"
/// ```
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct AnthropicAccount {
    /// Stable name used on the CLI (`--account <label>`) and as the cache
    /// subdir (`~/.cache/ai-usagebar/anthropic/<label>`).
    pub label: String,
    /// OAuth credentials file for this account (same JSON shape Claude Code
    /// writes). Token refreshes are written back here, so each account keeps
    /// itself alive independently.
    pub credentials_path: PathBuf,
}

impl AnthropicAccount {
    /// The `CLAUDE_CONFIG_DIR` this account occupies — the credential file's
    /// own directory. Claude Code hashes exactly this path for the account's
    /// Keychain item, so it is also the account's identity for
    /// [`crate::anthropic::keychain`].
    pub fn config_dir(&self) -> PathBuf {
        self.credentials_path
            .parent()
            .map_or_else(|| self.credentials_path.clone(), Path::to_path_buf)
    }
}

impl AnthropicConfig {
    /// Every extra account: the explicit `[[anthropic.accounts]]` entries plus
    /// any auto-discovered under [`accounts_dir`](AnthropicConfig::accounts_dir).
    /// Explicit entries take precedence on a label clash. This is what tabs and
    /// `--account` enumerate, so a discovered account behaves exactly like a
    /// hand-written one (own cache subdir, independent refresh).
    pub fn all_accounts(&self) -> Vec<AnthropicAccount> {
        let mut out = self.accounts.clone();
        if let Some(dir) = &self.accounts_dir {
            for acct in discover_accounts(dir) {
                if !out.iter().any(|a| a.label == acct.label) {
                    out.push(acct);
                }
            }
        }
        out
    }

    /// Find an extra account by label (explicit or discovered), or error listing
    /// the known labels so a typo fails loudly instead of silently hitting the
    /// default. Returns an owned account because discovered entries are
    /// synthesized, not stored.
    pub fn account(&self, label: &str) -> Result<AnthropicAccount> {
        validate_account_label(label)?;
        let all = self.all_accounts();
        all.iter().find(|a| a.label == label).cloned().ok_or_else(|| {
            let known: Vec<&str> = all.iter().map(|a| a.label.as_str()).collect();
            AppError::Credentials(format!(
                "anthropic account {label:?} not found in [[anthropic.accounts]] or accounts_dir; \
                 known labels: {known:?}"
            ))
        })
    }

    /// Resolve a named account to the credentials target + isolated cache it
    /// fetches through: [`CredsTarget::Named`], which on macOS prefers the
    /// Keychain item scoped to the file's own directory (that is where
    /// `CLAUDE_CONFIG_DIR=<dir> claude` actually writes) and falls back to
    /// the file elsewhere — never a *different* account's item, since the
    /// hash is per-directory, so issue #15's cross-account concern doesn't
    /// apply. Plus an `anthropic/<label>` cache subdir. Shared by the widget
    /// (`--account`) and the TUI's per-account tab (#14, #17) so both resolve
    /// accounts identically; the widget layers its `--cache-dir` override on
    /// top of the cache returned here.
    pub fn account_target(&self, label: &str) -> Result<(CredsTarget, Cache)> {
        let active = crate::anthropic::cli_account::home_claude_json()
            .ok()
            .and_then(|path| {
                crate::anthropic::cli_account::resolve_active_label(&path, &self.all_accounts())
            });
        self.account_target_with(label, active.as_deref())
    }

    /// The pure half of [`account_target`](AnthropicConfig::account_target),
    /// with "which account the `claude` CLI is signed into" injected — the same
    /// shape as `Cli::resolve_vendor_with`.
    ///
    /// When `label` *is* the live CLI login, its credential has been moved into
    /// the default slot and removed from its named slot. Reading the default
    /// one keeps exactly one live lineage, so a refresh here cannot invalidate
    /// the credential `claude` is using (or the other way round). The cache directory
    /// is unchanged either way, so the tab keeps its identity and its cached
    /// usage across a switch.
    pub fn account_target_with(
        &self,
        label: &str,
        cli_active: Option<&str>,
    ) -> Result<(CredsTarget, Cache)> {
        let account = self.account(label)?;
        let cache = Cache::for_vendor_account("anthropic", label)?;
        if cli_active == Some(label) {
            return Ok((
                CredsTarget::Default(crate::anthropic::creds::default_path()?),
                cache,
            ));
        }
        Ok((
            CredsTarget::Named {
                config_dir: account.config_dir(),
                path: account.credentials_path,
            },
            cache,
        ))
    }
}

/// The label doubles as a cache subdirectory name
/// (`~/.cache/ai-usagebar/anthropic/<label>/`), which nests inside the default
/// account's cache dir — so path separators, control characters, or reserved
/// cache sidecar names would escape, spoof terminal output, or collide with the
/// cache layout (`usage.json`, `.stale`, …).
pub fn validate_account_label(label: &str) -> Result<()> {
    validate_account_label_for("anthropic", label)
}

fn validate_account_label_for(vendor: &str, label: &str) -> Result<()> {
    const RESERVED: [&str; 4] = ["usage.json", ".stale", ".last_error", ".fetch.lock"];
    let bad = label.is_empty()
        || label == "."
        || label == ".."
        || label.contains(['/', '\\'])
        || label.contains(':')
        || label.chars().any(char::is_control)
        || RESERVED.contains(&label);
    if bad {
        return Err(AppError::Credentials(format!(
            "invalid {vendor} account label {label:?}: must be a non-empty name \
             without path separators, drive prefixes, control characters, or reserved cache names"
        )));
    }
    Ok(())
}

/// Discover accounts under `accounts_dir` in the `CLAUDE_CONFIG_DIR` layout:
/// each immediate subdirectory becomes an account labeled by the subdirectory
/// name. Best-effort: an unreadable directory or unusable label is skipped
/// silently rather than failing the whole config — discovery is convenience,
/// while an explicit `[[anthropic.accounts]]` entry stays authoritative. The
/// fetch path resolves credentials from either `.credentials.json` or the macOS
/// Keychain. Sorted by label so the tab order is stable across runs.
fn discover_accounts(accounts_dir: &std::path::Path) -> Vec<AnthropicAccount> {
    let Ok(entries) = std::fs::read_dir(accounts_dir) else {
        return Vec::new();
    };
    let mut found: Vec<AnthropicAccount> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_dir() {
                return None;
            }
            let label = path.file_name()?.to_str()?.to_string();
            validate_account_label(&label).ok()?;
            Some(AnthropicAccount {
                label,
                credentials_path: path.join(".credentials.json"),
            })
        })
        .collect();
    found.sort_by(|a, b| a.label.cmp(&b.label));
    found
}

/// Render a path with `$HOME` collapsed back to `~`, matching the style the docs
/// and existing `[[anthropic.accounts]]` entries use. Pure so it's testable;
/// paths outside home are returned verbatim.
pub fn tildify(path: &Path, home: &Path) -> String {
    path.strip_prefix(home)
        .map(|rest| {
            let rendered = rest.display().to_string();
            // Config paths use the same portable `~/...` spelling on every
            // platform. A Windows `~\...` would not be expanded by the loader.
            #[cfg(windows)]
            let rendered = rendered.replace('\\', "/");
            format!("~/{rendered}")
        })
        .unwrap_or_else(|_| path.display().to_string())
}

/// Where a newly-registered account's credentials file lives by default: next
/// to `config.toml`, under `accounts/<label>/.credentials.json`. Returns the
/// absolute path (for `mkdir`) — tilde-render it with [`tildify`] for display
/// and for the value written into config.
pub fn default_account_credentials_path(config_path: &Path, label: &str) -> PathBuf {
    let base = config_path.parent().unwrap_or_else(|| Path::new("."));
    base.join("accounts").join(label).join(".credentials.json")
}

/// Append a `[[anthropic.accounts]]` entry to a parsed config document, in
/// place. Pure over a `toml_edit` document so the validation, duplicate check,
/// and formatting are testable without disk. Preserves the rest of the file
/// (comments, key order, other sections) — only the new array-of-tables entry
/// is added. Errors on an invalid label or a label that already exists.
pub fn add_anthropic_account_to_doc(
    doc: &mut toml_edit::DocumentMut,
    label: &str,
    credentials_path: &str,
) -> Result<()> {
    use toml_edit::{Item, Table, value};

    validate_account_label(label)?;

    let anthropic = doc
        .entry("anthropic")
        .or_insert_with(|| Item::Table(Table::new()));
    let anthropic = anthropic
        .as_table_mut()
        .ok_or_else(|| AppError::Other("[anthropic] in config.toml is not a table".into()))?;

    let accounts = anthropic
        .entry("accounts")
        .or_insert_with(|| Item::ArrayOfTables(toml_edit::ArrayOfTables::new()));
    let accounts = accounts.as_array_of_tables_mut().ok_or_else(|| {
        AppError::Other("[[anthropic.accounts]] in config.toml is not an array of tables".into())
    })?;

    let exists = accounts
        .iter()
        .any(|t| t.get("label").and_then(Item::as_str) == Some(label));
    if exists {
        return Err(AppError::Credentials(format!(
            "anthropic account {label:?} already exists in config.toml"
        )));
    }

    let mut table = Table::new();
    table["label"] = value(label);
    table["credentials_path"] = value(credentials_path);
    accounts.push(table);
    Ok(())
}

/// Set or update a boolean field in a TOML section, preserving comments and
/// formatting of unaffected nodes. Shared by the Settings overlay and
/// [`enable_vendors_in`] so both writers shape `enabled = true` identically.
pub(crate) fn set_bool(
    doc: &mut toml_edit::DocumentMut,
    section: &str,
    key: &str,
    new_value: bool,
) -> Result<()> {
    let table = doc
        .entry(section)
        .or_insert_with(toml_edit::table)
        .as_table_mut()
        .ok_or_else(|| AppError::Other(format!("config.toml: [{section}] is not a table")))?;

    if let Some(item) = table.get_mut(key)
        && let Some(v) = item.as_value_mut()
    {
        // Keep a trailing `# comment` on the line being rewritten: the value
        // is the only thing that changed, and the note beside it is the
        // user's.
        let suffix = v.decor().suffix().cloned();
        *v = toml_edit::Value::from(new_value);
        v.decor_mut().set_prefix(" ");
        if let Some(suffix) = suffix {
            v.decor_mut().set_suffix(suffix);
        }
        return Ok(());
    }
    table.insert(key, toml_edit::value(new_value));
    Ok(())
}

/// Set, replace or remove a scalar field in a TOML section, preserving
/// comments and formatting of unaffected nodes. `None` removes the key so a
/// cleared preference does not linger as an empty string. The value keeps
/// its own TOML type on disk — an integer preference such as
/// `refresh_minutes` must not be quoted, or `Config::load_from` rejects it.
pub(crate) fn set_value(
    doc: &mut toml_edit::DocumentMut,
    section: &str,
    key: &str,
    new_value: Option<toml_edit::Value>,
) -> Result<()> {
    let table = doc
        .entry(section)
        .or_insert_with(toml_edit::table)
        .as_table_mut()
        .ok_or_else(|| AppError::Other(format!("config.toml: [{section}] is not a table")))?;

    let Some(mut new_value) = new_value else {
        table.remove(key);
        return Ok(());
    };
    if let Some(item) = table.get_mut(key)
        && let Some(v) = item.as_value_mut()
    {
        let suffix = v.decor().suffix().cloned();
        new_value.decor_mut().set_prefix(" ");
        if let Some(suffix) = suffix {
            new_value.decor_mut().set_suffix(suffix);
        }
        *v = new_value;
        return Ok(());
    }
    table.insert(key, toml_edit::Item::Value(new_value));
    Ok(())
}

/// Write one `[tray]` preference into the config at `path`, creating the
/// file when it doesn't exist and leaving every other line as it was.
/// `None` removes the key. The value keeps the TOML type it is given
/// (`"notify"` stays a string, `5` stays an integer). The tray host is the
/// only writer.
pub fn set_tray_value(path: &Path, key: &str, value: Option<toml_edit::Value>) -> Result<()> {
    let mut doc = read_config_document(path)?;
    let before = doc.to_string();
    set_value(&mut doc, "tray", key, value)?;
    if doc.to_string() == before {
        return Ok(());
    }
    write_config_document(path, &doc)
}

/// Read `path` into a `toml_edit` document with comments intact. A missing
/// file is an empty document, so a writer can create the config from nothing;
/// any other I/O failure or a parse error is reported rather than clobbered.
pub(crate) fn read_config_document(path: &Path) -> Result<toml_edit::DocumentMut> {
    let original = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(AppError::io_at(path, error)),
    };
    if original.trim().is_empty() {
        return Ok(toml_edit::DocumentMut::new());
    }
    original.parse().map_err(|e: toml_edit::TomlError| {
        AppError::Other(format!("config.toml not parseable: {e}"))
    })
}

/// Persist an edited config document: parent dir created, atomic
/// tempfile-and-rename write, and `chmod 600` on Unix because the file may
/// carry inline credentials. The one write path for every config editor.
pub(crate) fn write_config_document(path: &Path, doc: &toml_edit::DocumentMut) -> Result<()> {
    let bytes = doc.to_string();
    crate::cache::atomic_write(path, bytes.as_bytes())?;

    #[cfg(unix)]
    {
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o600);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
    Ok(())
}

/// Flip `enabled = true` for each vendor's section in the config at `path`,
/// creating the file when it doesn't exist and leaving every other line —
/// comments, keys, unrelated sections — exactly as it was. Never writes
/// `false`: `config.toml` stays the user's source of truth and this only ever
/// widens it. A document that comes out textually unchanged (every vendor
/// already enabled) is not rewritten, so an idempotent call doesn't touch the
/// file's mtime or race a concurrent editor.
pub fn enable_vendors_in(path: &Path, vendors: &[VendorId]) -> Result<Vec<VendorId>> {
    let mut doc = read_config_document(path)?;
    let before = doc.to_string();
    let written: Vec<VendorId> = vendors
        .iter()
        .copied()
        .filter(|vendor| !is_explicitly_disabled(&doc, *vendor))
        .collect();
    for vendor in &written {
        set_bool(&mut doc, vendor.config_section(), "enabled", true)?;
    }
    if doc.to_string() == before {
        return Ok(written);
    }
    write_config_document(path, &doc)?;
    Ok(written)
}

/// Whether the config *says* `enabled = false` for this vendor, as opposed to
/// not mentioning it.
///
/// This is the durable record of a user having turned a vendor off. `detect`
/// also keeps a set of vendors it has already considered, but that lives in the
/// cache directory, which is by convention safe to delete — so it cannot be the
/// only thing standing between "the user opted out" and re-enabling a provider
/// (and resuming requests to it) behind their back. The config file is the one
/// place that outlives a cache wipe, so the explicit `false` is honored here,
/// at the write, where no caller can route around it.
fn is_explicitly_disabled(doc: &toml_edit::DocumentMut, vendor: VendorId) -> bool {
    doc.get(vendor.config_section())
        .and_then(|section| section.get("enabled"))
        .and_then(|enabled| enabled.as_bool())
        == Some(false)
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct OpenAiConfig {
    pub enabled: bool,
    /// Override the Codex auth file path (defaults to `~/.codex/auth.json`).
    pub codex_auth_path: Option<PathBuf>,
    /// Extra Codex logins, each its own `auth.json`. Same shape as
    /// [`AnthropicAccount`] and for the same reason: Codex is an OAuth vendor,
    /// so an account *is* a credential file, and `openai::creds::write_back`
    /// refreshes into whichever one it read.
    #[serde(default)]
    pub accounts: Vec<OpenAiAccount>,
    /// Reserved, and inert: names the env var an API-key-only path *would*
    /// read (admin key → `/v1/organization/costs`). Nothing consumes it —
    /// OpenAI usage comes solely from Codex OAuth. Kept because that path is
    /// still intended, not for back-compat: `[openai]` doesn't deny unknown
    /// fields, so an existing `admin_key_env` would load either way. See
    /// `config.example.toml`, which ships it commented out so nobody sets it
    /// expecting an effect.
    pub admin_key_env: String,
}

/// One extra Codex login.
///
/// ```toml
/// [[openai.accounts]]
/// label = "work"
/// codex_auth_path = "~/.config/ai-usagebar/accounts/work-codex/auth.json"
/// ```
///
/// A second login is made with `CODEX_HOME=~/.codex-work codex login`; point
/// `codex_auth_path` at the `auth.json` it writes.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct OpenAiAccount {
    /// Stable name used on the CLI (`--account <label>`) and as the cache
    /// subdir (`~/.cache/ai-usagebar/openai/<label>`).
    pub label: String,
    /// Codex OAuth file for this account. Refreshed tokens are written back
    /// here, so each account keeps itself alive independently.
    pub codex_auth_path: PathBuf,
}

impl OpenAiConfig {
    /// The auth file for `label`, or the singular/default one when `label` is
    /// `None`. An unknown label is an error rather than a silent fall back to
    /// the default account, which would report the wrong login's usage.
    pub fn resolve_auth_path(&self, label: Option<&str>) -> Result<PathBuf> {
        let Some(label) = label else {
            return match &self.codex_auth_path {
                Some(path) => Ok(path.clone()),
                None => crate::openai::creds::default_path(),
            };
        };
        self.accounts
            .iter()
            .find(|account| account.label == label)
            .map(|account| account.codex_auth_path.clone())
            .ok_or_else(|| {
                AppError::Credentials(format!(
                    "no OpenAI account named {label:?}. Add it under \
                     [[openai.accounts]], or drop --account to use the default login."
                ))
            })
    }
}

impl Default for OpenAiConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            codex_auth_path: None,
            accounts: Vec::new(),
            admin_key_env: "OPENAI_ADMIN_KEY".to_string(),
        }
    }
}

/// GitHub Copilot quota from the private endpoint used by VS Code. The token
/// comes from an explicit environment override or the official GitHub CLI;
/// this app never reads, copies, or writes GitHub credential stores.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct CopilotConfig {
    pub enabled: bool,
    /// Path to the official GitHub CLI. Unset looks `gh` up on `PATH`, which
    /// is how `gh` is normally installed; set it to pin the executable.
    pub gh_binary: Option<PathBuf>,
}

impl CopilotConfig {
    pub fn resolve_token(&self) -> Result<String> {
        self.resolve_token_with(
            |name| std::env::var_os(name),
            &crate::copilot::credentials::SystemGhAuthTokenRunner,
        )
    }

    fn resolve_token_with(
        &self,
        environment: impl Fn(&str) -> Option<std::ffi::OsString>,
        runner: &impl crate::copilot::credentials::GhAuthTokenRunner,
    ) -> Result<String> {
        if let Some(value) = environment("GITHUB_COPILOT_TOKEN") {
            let token = value.into_string().map_err(|_| {
                AppError::Credentials(
                    "GitHub Copilot: GITHUB_COPILOT_TOKEN is not valid UTF-8.".into(),
                )
            })?;
            if !token.is_empty() {
                return Ok(token);
            }
        }
        crate::copilot::credentials::resolve_with(runner, self.gh_binary.as_deref())
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct NousConfig {
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct OpenCodeGoConfig {
    pub enabled: bool,
    pub api_key_env: String,
    pub api_key: Option<String>,
}

/// Command Code reads the OAuth credential from the official CLI or pi, so it
/// has no API key of its own. `auth_paths` overrides that search list for a
/// non-standard install.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct CommandCodeConfig {
    pub enabled: bool,
    pub auth_paths: Option<Vec<PathBuf>>,
}

impl Default for OpenCodeGoConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_key_env: "OPENCODE_GO_API_KEY".to_string(),
            api_key: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ZaiConfig {
    pub enabled: bool,
    /// Env var name to read the key from (env wins over `api_key`).
    pub api_key_env: String,
    /// Inline key (fallback when the env var is unset). Chmod 600 your
    /// config file if you put a real key here.
    pub api_key: Option<String>,
    /// Optional plan tier label (lite/pro/max) — display-only.
    pub plan_tier: Option<String>,
}

impl Default for ZaiConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            api_key_env: "ZAI_API_KEY".to_string(),
            api_key: None,
            plan_tier: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct OpenRouterConfig {
    pub enabled: bool,
    /// Extra OpenRouter accounts beyond the default key. Each account gets a
    /// separate aggregate-view entry and cache directory.
    pub accounts: Vec<OpenRouterAccount>,
    /// Whether aggregate views include the default (unnamed) key when named
    /// accounts exist. Ignored when `accounts` is empty so OpenRouter never
    /// loses its only tab.
    pub show_default_account: bool,
    pub api_key_env: String,
    pub api_key: Option<String>,
}

impl Default for OpenRouterConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            accounts: Vec::new(),
            show_default_account: true,
            api_key_env: "OPENROUTER_API_KEY".to_string(),
            api_key: None,
        }
    }
}

/// One named OpenRouter account. The default account continues to use the
/// singular `api_key_env` / `api_key` fields under `[openrouter]`.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct OpenRouterAccount {
    /// Stable CLI/report label and account-scoped cache subdirectory.
    pub label: String,
    /// Optional environment variable containing this account's key.
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// Inline fallback when the account environment variable is unset.
    #[serde(default)]
    pub api_key: Option<String>,
}

impl OpenRouterConfig {
    /// Find a named account or fail loudly instead of falling back to the
    /// default key (which would show the wrong account's usage).
    pub fn account(&self, label: &str) -> Result<&OpenRouterAccount> {
        validate_account_label_for("openrouter", label)?;
        self.accounts
            .iter()
            .find(|account| account.label == label)
            .ok_or_else(|| {
                let known: Vec<&str> = self
                    .accounts
                    .iter()
                    .map(|account| account.label.as_str())
                    .collect();
                AppError::Credentials(format!(
                    "openrouter account {label:?} not found in [[openrouter.accounts]]; \
                     known labels: {known:?}"
                ))
            })
    }

    /// Resolve either the backward-compatible default key or one named
    /// account. Configured values are never included in an error message.
    pub fn resolve_api_key(&self, label: Option<&str>) -> Result<String> {
        match label {
            None => resolve_api_key("OpenRouter", &self.api_key_env, self.api_key.as_deref()),
            Some(label) => {
                let account = self.account(label)?;
                resolve_api_key_in_section(
                    &format!("OpenRouter account {label:?}"),
                    "[[openrouter.accounts]]",
                    account.api_key_env.as_deref().unwrap_or(""),
                    account.api_key.as_deref(),
                )
            }
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct DeepseekConfig {
    pub enabled: bool,
    pub api_key_env: String,
    pub api_key: Option<String>,
}

impl Default for DeepseekConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_key_env: "DEEPSEEK_API_KEY".to_string(),
            api_key: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct KimiConfig {
    pub enabled: bool,
    pub api_key_env: String,
    /// Optional: with no key set, the vendor falls back to the Kimi Code CLI's
    /// own OAuth login, which is what a subscriber already has locally.
    pub api_key: Option<String>,
    /// Override for kimi-code's credential file (default
    /// `~/.kimi-code/credentials/kimi-code.json`), mirroring `[cursor] db_path`
    /// and `[kiro] db_path`. Useful with a relocated `KIMI_CODE_HOME`.
    pub credentials_path: Option<PathBuf>,
    /// `"auto"` follows kimi-code's own install marker (`~/.kimi-code/region`);
    /// `"cn"` pins `api.kimi.com` / `auth.kimi.com`, `"global"` pins
    /// `api.kimi.ai` / `auth.kimi.ai`. A token minted by one deployment means
    /// nothing to the other, so this picks the instance, not a currency.
    pub region: String,
}

impl Default for KimiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_key_env: "KIMI_API_KEY".to_string(),
            api_key: None,
            credentials_path: None,
            region: "auto".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct KiloConfig {
    pub enabled: bool,
    pub api_key_env: String,
    pub api_key: Option<String>,
    /// Optional Kilo organization id — scopes the balance to a team via the
    /// `x-kilocode-organizationid` header. Omit for the personal balance.
    pub organization_id: Option<String>,
}

impl Default for KiloConfig {
    fn default() -> Self {
        // Opt-in like DeepSeek: requires an explicit API key, so it defaults to
        // disabled and never affects existing installs.
        Self {
            enabled: false,
            api_key_env: "KILO_API_KEY".to_string(),
            api_key: None,
            organization_id: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct NovitaConfig {
    pub enabled: bool,
    pub api_key_env: String,
    pub api_key: Option<String>,
}

impl Default for NovitaConfig {
    fn default() -> Self {
        // Opt-in like DeepSeek/Kilo: needs an explicit API key.
        Self {
            enabled: false,
            api_key_env: "NOVITA_API_KEY".to_string(),
            api_key: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct MinimaxConfig {
    pub enabled: bool,
    pub api_key_env: String,
    pub api_key: Option<String>,
    /// `"global"` → api.minimax.io; `"cn"` → api.minimaxi.com. Unlike
    /// Moonshot's, this does not change the unit — MiniMax reports quota as a
    /// percentage either way. It picks the *instance*: a key issued for one
    /// host is rejected by the other (`status_code 2049`), so pointing this at
    /// the wrong region reads as an invalid key rather than an empty plan.
    pub region: String,
}

impl Default for MinimaxConfig {
    fn default() -> Self {
        // Opt-in like the other API-key vendors: needs an explicit key.
        Self {
            enabled: false,
            api_key_env: "MINIMAX_API_KEY".to_string(),
            api_key: None,
            region: "global".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct MoonshotConfig {
    pub enabled: bool,
    pub api_key_env: String,
    pub api_key: Option<String>,
    /// `"global"` → api.moonshot.ai (USD); `"cn"` → api.moonshot.cn (CNY).
    pub region: String,
}

impl Default for MoonshotConfig {
    fn default() -> Self {
        // Opt-in like DeepSeek/Kilo/Novita: needs an explicit API key.
        Self {
            enabled: false,
            api_key_env: "MOONSHOT_API_KEY".to_string(),
            api_key: None,
            region: "global".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct GrokConfig {
    pub enabled: bool,
    /// Env var for the xAI **Management** key (distinct from the inference key).
    pub api_key_env: String,
    pub api_key: Option<String>,
    /// Optional team id. When absent, it's auto-resolved from the management
    /// key via `/auth/management-keys/validation`.
    pub team_id: Option<String>,
}

impl Default for GrokConfig {
    fn default() -> Self {
        // Opt-in: needs a management key (and, for prepaid, a team).
        Self {
            enabled: false,
            api_key_env: "XAI_MANAGEMENT_KEY".to_string(),
            api_key: None,
            team_id: None,
        }
    }
}

/// SuperGrok subscription auth — no API key of its own. Billing and banked
/// resets use the `key` already in Grok Build's `auth.json` (read-only).
/// Login, issuer, proxy, and token rotation stay inside Grok Build.
///
/// Opt-in like Cursor/Kiro (`enabled` defaults to `false`): it requires a
/// separate official executable and signed-in session, so it stays off until
/// the user explicitly turns it on.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct SuperGrokConfig {
    pub enabled: bool,
    /// Trusted official Grok Build executable. Defaults to its canonical
    /// `$GROK_HOME/bin/grok` (or `~/.grok/bin/grok`) installation path instead
    /// of searching PATH, where unrelated programs can share the name.
    pub grok_binary: PathBuf,
    /// Opaque auth/config files used only to fingerprint the active cache
    /// scope. Their contents are never parsed or copied to the cache.
    pub auth_path: Option<PathBuf>,
    pub config_path: Option<PathBuf>,
}

impl Default for SuperGrokConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            grok_binary: default_grok_binary(),
            auth_path: None,
            config_path: None,
        }
    }
}

fn default_grok_binary() -> PathBuf {
    let executable = if cfg!(windows) { "grok.exe" } else { "grok" };
    let grok_home = std::env::var_os("GROK_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| crate::cache::home_dir().ok().map(|home| home.join(".grok")));
    grok_home
        .map(|home| home.join("bin").join(executable))
        .unwrap_or_else(|| PathBuf::from(executable))
}

/// Antigravity reads its quota from whichever local Antigravity product is
/// running, so it needs no credentials — only an on/off switch.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct AntigravityConfig {
    pub enabled: bool,
}

/// Cursor reads its quota through a session token the Cursor IDE already
/// wrote to its local `state.vscdb` — no API key, but (unlike Antigravity)
/// there is a real on-disk path that can need overriding (e.g. a portable or
/// non-default Cursor install), mirroring `openai.codex_auth_path`.
///
/// Opt-in like DeepSeek/Kilo/etc (`enabled` defaults to `false`, matching
/// `bool::default()`): reads an undocumented endpoint via a session token
/// scraped from a local IDE file, so it stays off until the user explicitly
/// turns it on.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct CursorConfig {
    pub enabled: bool,
    /// Override Cursor's local state database path (defaults to the
    /// platform-standard `.../User/globalStorage/state.vscdb` — see
    /// `cursor::db::default_db_path`).
    pub db_path: Option<PathBuf>,
    /// Override the headless `cursor-agent` CLI's own login file (defaults to
    /// `.../cursor/auth.json` — see `cursor::db::default_agent_auth_path`).
    /// Used as a fallback when `db_path` doesn't exist, so a text-only
    /// machine that never runs the desktop IDE still gets usage.
    pub agent_auth_path: Option<PathBuf>,
}

/// Kiro CLI reads its quota through the AWS SSO OIDC session kiro-cli already
/// wrote to its own local `data.sqlite3` — no API key, but (like Cursor) a
/// real on-disk path that can need overriding.
///
/// Opt-in like Cursor/DeepSeek/Kilo/etc (`enabled` defaults to `false`):
/// calls a reverse-engineered CodeWhisperer endpoint via a session token
/// scraped from a local CLI database, so it stays off until the user
/// explicitly turns it on.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct KiroConfig {
    pub enabled: bool,
    /// Override kiro-cli's local database path (defaults to the
    /// platform-standard `.../kiro-cli/data.sqlite3` — see
    /// `kiro::db::default_db_path`).
    pub db_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct AnthropicApiConfig {
    pub enabled: bool,
    /// Env var for the Console **Admin key** (`sk-ant-admin01-…`), distinct from
    /// an inference key and from the Claude Code OAuth login.
    pub api_key_env: String,
    pub api_key: Option<String>,
    /// Monthly USD spend limit, used only for the spend-vs-limit % display. The
    /// API exposes neither this limit nor the remaining prepaid balance.
    pub monthly_limit: Option<f64>,
}

impl Default for AnthropicApiConfig {
    fn default() -> Self {
        // Opt-in: needs an explicit Admin key.
        Self {
            enabled: false,
            api_key_env: "ANTHROPIC_ADMIN_KEY".to_string(),
            api_key: None,
            monthly_limit: None,
        }
    }
}

/// A user-defined HTTP provider: one GET with a static token, projected onto
/// the shared report shape through RFC 6901 JSON Pointers.
///
/// Everything a built-in vendor hard-codes is a field here, which is why this
/// type validates so much more than the others: a typo in `[deepseek]` hits a
/// fixed endpoint and fails loudly, while a typo here quietly sends the user's
/// key to the wrong host. The `id` doubles as the cache directory name and the
/// `--vendor` selector, so it is held to the character class of the built-in
/// slugs.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, remote = "Self")]
pub struct CustomProviderConfig {
    /// `[a-z0-9][a-z0-9_-]{0,31}`; unique, and never a built-in vendor's slug.
    pub id: String,
    /// Display name, 1 to 48 characters. Defaults to `id`.
    pub name: String,
    /// Exactly three lowercase ASCII letters, unique across built-in vendors
    /// and other custom providers — it is the `{vendor_short}` bar tag.
    pub short_name: String,
    pub enabled: bool,
    /// `https://` unless `allow_http`; never carries `user:pass@`.
    pub url: String,
    pub allow_http: bool,
    /// Env var read first; `""` means the inline `api_key` is the only source.
    pub api_key_env: String,
    pub api_key: Option<String>,
    /// The header that carries the key.
    pub auth_header: String,
    /// Sent as `"<scheme> <key>"`; `""` sends the bare key.
    pub auth_scheme: String,
    /// Extra non-secret headers.
    pub headers: BTreeMap<String, String>,
    /// Literal plan label.
    pub plan: Option<String>,
    /// Pointer to the plan label in the response; wins over `plan`.
    pub plan_path: Option<String>,
    /// Must be within `10..=3600`.
    pub cache_ttl_secs: u64,
    pub metrics: Vec<CustomMetricSpec>,
    pub texts: Vec<CustomTextSpec>,
}

impl Default for CustomProviderConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            short_name: String::new(),
            enabled: false,
            url: String::new(),
            allow_http: false,
            api_key_env: String::new(),
            api_key: None,
            auth_header: "Authorization".to_string(),
            auth_scheme: "Bearer".to_string(),
            headers: BTreeMap::new(),
            plan: None,
            plan_path: None,
            cache_ttl_secs: 60,
            metrics: Vec::new(),
            texts: Vec::new(),
        }
    }
}

/// `name` defaults to `id`, which a per-field serde default cannot express (a
/// default sees no sibling field). The derive is routed through
/// `remote = "Self"` so the fill-in happens here, on every parse path, rather
/// than only in `Config::load_from`.
impl<'de> Deserialize<'de> for CustomProviderConfig {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let mut this = Self::deserialize(deserializer)?;
        if this.name.is_empty() {
            this.name = this.id.clone();
        }
        Ok(this)
    }
}

impl Serialize for CustomProviderConfig {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        Self::serialize(self, serializer)
    }
}

/// One percentage row. Either `percent` alone, or `used` and `limit`
/// together — never a mix, so a row cannot show a percentage from one field
/// and a footnote from another.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct CustomMetricSpec {
    pub label: String,
    pub used: Option<String>,
    pub limit: Option<String>,
    pub percent: Option<String>,
    /// Pointer to an RFC 3339 string or a Unix epoch (seconds or milliseconds).
    pub resets_at: Option<String>,
    /// Window length for pacing, at least 60.
    pub window_secs: Option<u64>,
}

/// One free-text row: a string, number, or boolean at `value`.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct CustomTextSpec {
    pub label: String,
    pub value: String,
}

impl CustomProviderConfig {
    /// The TOML locator for error messages: `[[custom]] id = "mytool"`.
    pub fn section_label(&self) -> String {
        format!("[[custom]] id = {:?}", self.id)
    }

    /// Env var (when `api_key_env` is set) → inline `api_key` → a
    /// `Credentials` error that names the section and never the key.
    pub fn resolve_api_key(&self) -> Result<String> {
        if let Some(key) = optional_api_key(&self.api_key_env, self.api_key.as_deref()) {
            return Ok(key);
        }
        let advice = if self.api_key_env.is_empty() {
            "set `api_key`, or name an environment variable in `api_key_env`".to_string()
        } else {
            format!("export {} or set `api_key`", self.api_key_env)
        };
        Err(AppError::Credentials(format!(
            "custom {}: no API key. Either {advice} under {} in {}.",
            self.id,
            self.section_label(),
            config_path_hint()
        )))
    }

    pub fn cache_ttl(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.cache_ttl_secs)
    }

    /// Every rule that serde cannot express, each naming the section. Runs
    /// for disabled entries too: a broken entry is a broken config, and the
    /// day it is enabled is the wrong day to find out.
    fn validate(&self, index: usize) -> Result<()> {
        if !is_valid_custom_id(&self.id) {
            return Err(AppError::Other(format!(
                "[[custom]] entry #{}: id {:?} must match [a-z0-9][a-z0-9_-]{{0,31}}",
                index + 1,
                self.id
            )));
        }
        let section = self.section_label();
        let bad = |msg: String| AppError::Other(format!("{section}: {msg}"));

        if VendorId::all().iter().any(|v| v.slug() == self.id) {
            return Err(bad(format!("id {:?} is a built-in vendor", self.id)));
        }
        let name_len = self.name.chars().count();
        if name_len == 0 || name_len > 48 || self.name.chars().any(char::is_control) {
            return Err(bad(
                "name must be 1 to 48 characters without control characters".into(),
            ));
        }
        if self.short_name.len() != 3 || !self.short_name.bytes().all(|b| b.is_ascii_lowercase()) {
            return Err(bad(format!(
                "short_name {:?} must be exactly 3 lowercase ASCII letters",
                self.short_name
            )));
        }
        let url = reqwest::Url::parse(&self.url)
            .map_err(|_| bad(format!("url {:?} is not a valid URL", self.url)))?;
        match url.scheme() {
            "https" => {}
            "http" if self.allow_http => {}
            "http" => {
                return Err(bad(
                    "url must use https:// (set allow_http = true to permit http://)".into(),
                ));
            }
            other => return Err(bad(format!("url scheme {other:?} is not http or https"))),
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(bad("url must not carry credentials (user:pass@)".into()));
        }
        if url.host_str().is_none() {
            return Err(bad("url has no host".into()));
        }
        if !self.api_key_env.is_empty() && !is_valid_env_var_name(&self.api_key_env) {
            return Err(bad(format!(
                "api_key_env {:?} is not a valid environment variable name",
                self.api_key_env
            )));
        }
        validate_header_name(&section, "auth_header", &self.auth_header)?;
        if reqwest::header::HeaderValue::from_str(&format!("{} k", self.auth_scheme)).is_err() {
            return Err(bad(
                "auth_scheme contains characters that are not valid in an HTTP header".into(),
            ));
        }
        for (name, value) in &self.headers {
            validate_header_name(&section, "headers", name)?;
            if name.eq_ignore_ascii_case(&self.auth_header) {
                return Err(bad(format!(
                    "headers must not repeat auth_header {:?}",
                    self.auth_header
                )));
            }
            if reqwest::header::HeaderValue::from_str(value).is_err() {
                return Err(bad(format!(
                    "header {name:?} has a value that is not valid in an HTTP header"
                )));
            }
        }
        if let Some(plan) = &self.plan {
            validate_custom_label(&section, "plan", plan)?;
        }
        if let Some(pointer) = &self.plan_path {
            validate_pointer(&section, "plan_path", pointer)?;
        }
        if !(10..=3600).contains(&self.cache_ttl_secs) {
            return Err(bad(format!(
                "cache_ttl_secs must be between 10 and 3600, got {}",
                self.cache_ttl_secs
            )));
        }
        if self.metrics.is_empty() && self.texts.is_empty() {
            return Err(bad(
                "needs at least one [[custom.metrics]] or [[custom.texts]] entry".into(),
            ));
        }
        let mut metric_labels = HashSet::new();
        for metric in &self.metrics {
            validate_custom_label(&section, "metric label", &metric.label)?;
            if !metric_labels.insert(metric.label.as_str()) {
                return Err(bad(format!("duplicate metric label {:?}", metric.label)));
            }
            let pair = (metric.used.is_some(), metric.limit.is_some());
            let well_formed = if metric.percent.is_some() {
                pair == (false, false)
            } else {
                pair == (true, true)
            };
            if !well_formed {
                return Err(bad(format!(
                    "metric {:?} must set `percent`, or both `used` and `limit` (not a mix)",
                    metric.label
                )));
            }
            for (field, pointer) in [
                ("used", &metric.used),
                ("limit", &metric.limit),
                ("percent", &metric.percent),
                ("resets_at", &metric.resets_at),
            ] {
                if let Some(pointer) = pointer {
                    validate_pointer(&section, field, pointer)?;
                }
            }
            if let Some(secs) = metric.window_secs
                && secs < 60
            {
                return Err(bad(format!(
                    "metric {:?} window_secs must be at least 60, got {secs}",
                    metric.label
                )));
            }
        }
        let mut text_labels = HashSet::new();
        for text in &self.texts {
            validate_custom_label(&section, "text label", &text.label)?;
            if !text_labels.insert(text.label.as_str()) {
                return Err(bad(format!("duplicate text label {:?}", text.label)));
            }
            validate_pointer(&section, "value", &text.value)?;
        }
        Ok(())
    }
}

fn is_valid_custom_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    let Some(&first) = bytes.first() else {
        return false;
    };
    bytes.len() <= 32
        && (first.is_ascii_lowercase() || first.is_ascii_digit())
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-'))
}

fn validate_pointer(section: &str, field: &str, pointer: &str) -> Result<()> {
    if !pointer.starts_with('/') || pointer.chars().any(char::is_control) {
        return Err(AppError::Other(format!(
            "{section}: {field} {pointer:?} must be an RFC 6901 JSON Pointer starting with '/'"
        )));
    }
    Ok(())
}

fn validate_custom_label(section: &str, field: &str, label: &str) -> Result<()> {
    let len = label.chars().count();
    if len == 0 || len > 64 || label.chars().any(char::is_control) {
        return Err(AppError::Other(format!(
            "{section}: {field} {label:?} must be 1 to 64 characters without control characters"
        )));
    }
    Ok(())
}

fn validate_header_name(section: &str, field: &str, name: &str) -> Result<()> {
    if name.is_empty() || reqwest::header::HeaderName::from_bytes(name.as_bytes()).is_err() {
        return Err(AppError::Other(format!(
            "{section}: {field} {name:?} is not a valid HTTP header name"
        )));
    }
    Ok(())
}

/// Resolve an API key for a vendor: a valid env-var name wins, then inline
/// config, then a clear error naming both fields. Used by every API-key vendor.
pub fn resolve_api_key(
    vendor_label: &str,
    env_var_name: &str,
    inline: Option<&str>,
) -> crate::error::Result<String> {
    let section = match vendor_label {
        "OpenCode Go" => "[opencode-go]".to_string(),
        _ => format!("[{}]", vendor_label.to_lowercase()),
    };
    resolve_api_key_in_section(vendor_label, &section, env_var_name, inline)
}

/// The env-then-inline lookup without the "or fail" ending, for vendors where
/// an absent API key is a legitimate state rather than an error — Kimi accepts
/// a Kimi Code CLI subscription login instead.
pub fn optional_api_key(env_var_name: &str, inline: Option<&str>) -> Option<String> {
    if is_valid_env_var_name(env_var_name)
        && let Ok(v) = std::env::var(env_var_name)
        && !v.is_empty()
    {
        return Some(v);
    }
    inline.filter(|v| !v.is_empty()).map(str::to_string)
}

fn resolve_api_key_in_section(
    vendor_label: &str,
    section: &str,
    env_var_name: &str,
    inline: Option<&str>,
) -> crate::error::Result<String> {
    if let Some(key) = optional_api_key(env_var_name, inline) {
        return Ok(key);
    }
    let valid_env_name = is_valid_env_var_name(env_var_name);
    let advice = if valid_env_name {
        "set an API key in a valid environment variable or set `api_key`"
    } else {
        "fix the invalid `api_key_env` with a valid environment variable name or set `api_key`"
    };
    Err(crate::error::AppError::Credentials(format!(
        "{vendor_label}: no API key. Either {advice} under {section} in {}.",
        config_path_hint()
    )))
}

pub(crate) fn is_valid_env_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

impl Config {
    /// Load from `~/.config/ai-usagebar/config.toml`. Returns defaults if the
    /// file doesn't exist; errors only on actual parse failures.
    pub fn load() -> Result<Self> {
        let Some(path) = resolved_path() else {
            return Ok(Self::default());
        };
        Self::load_from(&path)
    }

    pub fn load_from(path: &std::path::Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(s) => {
                let mut config: Self = toml::from_str(&s)?;
                // `~` is shell syntax, not path syntax: `PathBuf` keeps it
                // literally, so a documented `credentials_path = "~/..."`
                // silently pointed at a directory named `~`.
                config.expand_paths();
                config.validate()?;
                #[cfg(unix)]
                config.protect_inline_secrets(path)?;
                // A custom provider's token variable is as secret as any
                // built-in one; subprocesses (`gh`, `grok`, `claude`) must
                // not inherit it.
                crate::vendor::register_secret_env_vars(&config.custom_secret_env_vars());
                Ok(config)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(AppError::io_at(path, e)),
        }
    }

    fn expand_paths(&mut self) {
        expand_tilde_opt(&mut self.context.projects_path);
        expand_tilde_opt(&mut self.anthropic.credentials_path);
        expand_tilde_opt(&mut self.anthropic.accounts_dir);
        expand_tilde_opt(&mut self.anthropic.desktop_profiles_dir);
        expand_tilde_opt(&mut self.openai.codex_auth_path);
        expand_tilde_opt(&mut self.cursor.db_path);
        expand_tilde_opt(&mut self.cursor.agent_auth_path);
        expand_tilde_opt(&mut self.kiro.db_path);
        expand_tilde_opt(&mut self.kimi.credentials_path);
        self.supergrok.grok_binary = expand_tilde(&self.supergrok.grok_binary);
        expand_tilde_opt(&mut self.supergrok.auth_path);
        expand_tilde_opt(&mut self.supergrok.config_path);
        for account in &mut self.anthropic.accounts {
            account.credentials_path = expand_tilde(&account.credentials_path);
        }
        for account in &mut self.openai.accounts {
            account.codex_auth_path = expand_tilde(&account.codex_auth_path);
        }
    }

    /// Explicitly enumerate every inline credential field. Adding a new
    /// credential vendor must add it here so its config receives the same
    /// protection.
    #[cfg(unix)]
    fn has_inline_secrets(&self) -> bool {
        [
            self.zai.api_key.as_deref(),
            self.openrouter.api_key.as_deref(),
            self.deepseek.api_key.as_deref(),
            self.kimi.api_key.as_deref(),
            self.kilo.api_key.as_deref(),
            self.novita.api_key.as_deref(),
            self.minimax.api_key.as_deref(),
            self.moonshot.api_key.as_deref(),
            self.grok.api_key.as_deref(),
            self.anthropic_api.api_key.as_deref(),
            self.opencode_go.api_key.as_deref(),
        ]
        .into_iter()
        .chain(
            self.openrouter
                .accounts
                .iter()
                .map(|account| account.api_key.as_deref()),
        )
        .chain(self.custom.iter().map(|c| c.api_key.as_deref()))
        .any(|key| key.is_some_and(|key| !key.is_empty()))
    }

    fn custom_secret_env_vars(&self) -> Vec<String> {
        self.custom
            .iter()
            .filter(|c| !c.api_key_env.is_empty())
            .map(|c| c.api_key_env.clone())
            .collect()
    }

    /// The `[[custom]]` providers that are switched on, in config order.
    pub fn enabled_custom(&self) -> impl Iterator<Item = &CustomProviderConfig> {
        self.custom.iter().filter(|c| c.enabled)
    }

    /// A `[[custom]]` provider by `id`, enabled or not.
    pub fn custom_by_id(&self, id: &str) -> Option<&CustomProviderConfig> {
        self.custom.iter().find(|c| c.id == id)
    }

    #[cfg(unix)]
    fn protect_inline_secrets(&self, path: &Path) -> Result<()> {
        if !self.has_inline_secrets() {
            return Ok(());
        }

        let metadata = std::fs::metadata(path).map_err(|_| {
            AppError::Credentials(format!(
                "config at {} contains inline credentials but its permissions could not be checked; fix permissions or move credentials to environment variables",
                path.display()
            ))
        })?;
        if inline_key_permission_decision(metadata.mode()) == InlineKeyPermissionDecision::Tighten {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|_| {
                AppError::Credentials(format!(
                    "config at {} contains inline credentials but is group/other-readable and could not be tightened to 0600; fix permissions or move credentials to environment variables",
                    path.display()
                ))
            })?;
        }
        Ok(())
    }

    pub fn is_enabled(&self, id: VendorId) -> bool {
        match id {
            VendorId::Anthropic => self.anthropic.enabled,
            VendorId::AnthropicApi => self.anthropic_api.enabled,
            VendorId::Openai => self.openai.enabled,
            VendorId::Copilot => self.copilot.enabled,
            VendorId::Zai => self.zai.enabled,
            VendorId::Openrouter => self.openrouter.enabled,
            VendorId::Deepseek => self.deepseek.enabled,
            VendorId::Kimi => self.kimi.enabled,
            VendorId::Kilo => self.kilo.enabled,
            VendorId::Novita => self.novita.enabled,
            VendorId::Moonshot => self.moonshot.enabled,
            VendorId::Grok => self.grok.enabled,
            VendorId::Supergrok => self.supergrok.enabled,
            VendorId::Antigravity => self.antigravity.enabled,
            VendorId::Cursor => self.cursor.enabled,
            VendorId::Minimax => self.minimax.enabled,
            VendorId::Kiro => self.kiro.enabled,
            VendorId::NousResearch => self.nous.enabled,
            VendorId::OpenCodeGo => self.opencode_go.enabled,
            VendorId::CommandCode => self.commandcode.enabled,
        }
    }

    /// The environment variable this provider's API key is read from, honoring
    /// a per-vendor `api_key_env` override; `""` for a provider that takes no
    /// key. Matching on [`VendorId`] rather than on a section name is
    /// deliberate: a new key vendor that nobody adds here fails to compile,
    /// where a `_ =>` arm over `&str` sections would silently hand back the
    /// wrong default and report the provider as unconfigured for ever.
    pub fn api_key_env_for(&self, id: VendorId) -> &str {
        match id {
            VendorId::AnthropicApi => &self.anthropic_api.api_key_env,
            VendorId::Zai => &self.zai.api_key_env,
            VendorId::Openrouter => &self.openrouter.api_key_env,
            VendorId::Deepseek => &self.deepseek.api_key_env,
            VendorId::Kimi => &self.kimi.api_key_env,
            VendorId::Kilo => &self.kilo.api_key_env,
            VendorId::Novita => &self.novita.api_key_env,
            VendorId::Moonshot => &self.moonshot.api_key_env,
            VendorId::Grok => &self.grok.api_key_env,
            VendorId::Minimax => &self.minimax.api_key_env,
            VendorId::OpenCodeGo => &self.opencode_go.api_key_env,
            // Fixed names: OAuth-first providers whose environment override is
            // not user-renameable, and the providers with no key at all.
            VendorId::Anthropic
            | VendorId::Openai
            | VendorId::Copilot
            | VendorId::Supergrok
            | VendorId::Antigravity
            | VendorId::Cursor
            | VendorId::Kiro
            | VendorId::NousResearch
            | VendorId::CommandCode => id.api_key_env(),
        }
    }

    /// A non-empty inline `api_key` from this provider's config section. An
    /// empty string counts as unset, the same way the vendors' own
    /// `resolve_api_key` treats it.
    pub fn inline_api_key(&self, id: VendorId) -> Option<&str> {
        let raw = match id {
            VendorId::AnthropicApi => self.anthropic_api.api_key.as_deref(),
            VendorId::Zai => self.zai.api_key.as_deref(),
            VendorId::Openrouter => self.openrouter.api_key.as_deref(),
            VendorId::Deepseek => self.deepseek.api_key.as_deref(),
            VendorId::Kimi => self.kimi.api_key.as_deref(),
            VendorId::Kilo => self.kilo.api_key.as_deref(),
            VendorId::Novita => self.novita.api_key.as_deref(),
            VendorId::Moonshot => self.moonshot.api_key.as_deref(),
            VendorId::Grok => self.grok.api_key.as_deref(),
            VendorId::Minimax => self.minimax.api_key.as_deref(),
            VendorId::OpenCodeGo => self.opencode_go.api_key.as_deref(),
            VendorId::Anthropic
            | VendorId::Openai
            | VendorId::Copilot
            | VendorId::Supergrok
            | VendorId::Antigravity
            | VendorId::Cursor
            | VendorId::Kiro
            | VendorId::NousResearch
            | VendorId::CommandCode => None,
        };
        raw.filter(|key| !key.is_empty())
    }

    pub fn enabled_vendors(&self) -> Vec<VendorId> {
        VendorId::all()
            .iter()
            .copied()
            .filter(|id| self.is_enabled(*id))
            .collect()
    }

    /// Validate cross-entry constraints that serde cannot express. Account
    /// labels are both CLI selectors and TUI tab identities, so duplicates
    /// would make either destination ambiguous.
    pub fn validate(&self) -> Result<()> {
        if let Some(minutes) = self.tray.refresh_minutes
            && !TRAY_REFRESH_MINUTES.contains(&minutes)
        {
            return Err(AppError::Other(format!(
                "[tray] refresh_minutes must be one of 1, 5 or 10, got {minutes}"
            )));
        }
        if self.context.context_window_tokens == Some(0) {
            return Err(AppError::Other(
                "[context] context_window_tokens must be greater than zero".into(),
            ));
        }
        for (model, tokens) in &self.context.model_context_window_tokens {
            if model.trim().is_empty() {
                return Err(AppError::Other(
                    "[context] model_context_window_tokens keys must not be empty".into(),
                ));
            }
            if *tokens == 0 {
                return Err(AppError::Other(format!(
                    "[context] model_context_window_tokens entry {model:?} must be greater than zero"
                )));
            }
        }
        if let Some(limit) = self.anthropic_api.monthly_limit
            && (!limit.is_finite() || limit <= 0.0)
        {
            return Err(AppError::Other(
                "[anthropic_api] monthly_limit must be finite and greater than zero; \
                 remove it to show spend without a limit"
                    .into(),
            ));
        }
        if crate::kimi::oauth::Region::parse(&self.kimi.region).is_none()
            && !self.kimi.region.eq_ignore_ascii_case("auto")
        {
            return Err(AppError::Other(format!(
                "[kimi] region must be \"auto\", \"cn\", or \"global\", got {:?}",
                self.kimi.region
            )));
        }
        if !self.minimax.region.eq_ignore_ascii_case("global")
            && !self.minimax.region.eq_ignore_ascii_case("cn")
        {
            return Err(AppError::Other(format!(
                "[minimax] region must be \"global\" or \"cn\", got {:?}",
                self.minimax.region
            )));
        }
        if self.supergrok.grok_binary.as_os_str().is_empty() {
            return Err(AppError::Other(
                "[supergrok] grok_binary must not be empty".into(),
            ));
        }
        let mut labels = HashSet::new();
        for account in &self.anthropic.accounts {
            validate_account_label(&account.label)?;
            if !labels.insert(&account.label) {
                return Err(AppError::Credentials(format!(
                    "duplicate anthropic account label {:?}",
                    account.label
                )));
            }
        }
        let mut openai_labels = HashSet::new();
        for account in &self.openai.accounts {
            validate_account_label_for("openai", &account.label)?;
            if !openai_labels.insert(&account.label) {
                return Err(AppError::Credentials(format!(
                    "duplicate openai account label {:?}",
                    account.label
                )));
            }
        }
        let mut openrouter_labels = HashSet::new();
        for account in &self.openrouter.accounts {
            validate_account_label_for("openrouter", &account.label)?;
            if !openrouter_labels.insert(&account.label) {
                return Err(AppError::Credentials(format!(
                    "duplicate openrouter account label {:?}",
                    account.label
                )));
            }
            let has_env = account
                .api_key_env
                .as_deref()
                .is_some_and(|name| !name.is_empty());
            let has_inline = account
                .api_key
                .as_deref()
                .is_some_and(|key| !key.is_empty());
            if !has_env && !has_inline {
                return Err(AppError::Credentials(format!(
                    "openrouter account {:?} must set api_key_env or api_key",
                    account.label
                )));
            }
        }
        self.validate_custom()
    }

    /// Per-entry rules live on `CustomProviderConfig`; the cross-entry ones —
    /// `id` and `short_name` uniqueness, including against the built-in
    /// vendors — need the whole list and live here.
    fn validate_custom(&self) -> Result<()> {
        let mut ids = HashSet::new();
        let mut short_names: HashSet<&str> =
            VendorId::all().iter().map(|v| v.short_name()).collect();
        for (index, custom) in self.custom.iter().enumerate() {
            custom.validate(index)?;
            if !ids.insert(custom.id.as_str()) {
                return Err(AppError::Other(format!(
                    "{}: duplicate id",
                    custom.section_label()
                )));
            }
            if !short_names.insert(custom.short_name.as_str()) {
                return Err(AppError::Other(format!(
                    "{}: short_name {:?} is already used by a built-in vendor or another [[custom]] entry",
                    custom.section_label(),
                    custom.short_name
                )));
            }
        }
        Ok(())
    }
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InlineKeyPermissionDecision {
    Ok,
    Tighten,
}

#[cfg(unix)]
fn inline_key_permission_decision(mode: u32) -> InlineKeyPermissionDecision {
    if mode & 0o077 == 0 {
        InlineKeyPermissionDecision::Ok
    } else {
        InlineKeyPermissionDecision::Tighten
    }
}

pub fn default_path() -> Option<PathBuf> {
    let proj = directories::ProjectDirs::from("", "", "ai-usagebar")?;
    Some(proj.config_dir().join("config.toml"))
}

/// The Unix-conventional location, which is what every doc, the config
/// example, and both desktop integrations have always pointed at. On Linux it
/// *is* [`default_path`]; on macOS `ProjectDirs` resolves to
/// `~/Library/Application Support/…` instead, so the two diverge.
fn legacy_xdg_path() -> Option<PathBuf> {
    let home = crate::cache::home_dir().ok()?;
    Some(home.join(".config").join("ai-usagebar").join("config.toml"))
}

/// The config file actually in effect.
///
/// A `--config` override (see [`set_override_path`]) wins outright so a test
/// run never touches the real file. Otherwise [`default_path`] stays
/// canonical, but on macOS a file at the documented
/// `~/.config/ai-usagebar/config.toml` is honored when the canonical one does
/// not exist — otherwise everyone who followed the README (and both desktop
/// integrations, which read that path) silently got defaults. The legacy file
/// is never moved or rewritten: it may hold API keys, and relocating a secret
/// behind the user's back is not this tool's business.
pub fn resolved_path() -> Option<PathBuf> {
    if let Some(path) = override_path() {
        return Some(path);
    }
    let canonical = default_path();
    if let Some(p) = &canonical
        && p.exists()
    {
        return canonical;
    }
    if let Some(legacy) = legacy_xdg_path()
        && legacy.exists()
    {
        return Some(legacy);
    }
    canonical
}

static PATH_OVERRIDE: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// Point every config load, save, and hint at one explicit file — the
/// `--config` flag. Takes precedence over the canonical and legacy locations.
/// The file does not have to exist yet: loads treat it as defaults while
/// Settings saves create it. Process-wide, so call it once at startup before
/// any config is read.
pub fn set_override_path(path: &std::path::Path) {
    if let Ok(mut slot) = PATH_OVERRIDE.lock() {
        *slot = Some(path.to_path_buf());
    }
}

/// Drop the override again. Used only by tests so they can restore the
/// process-wide state they changed.
#[doc(hidden)]
pub fn clear_override_path() {
    if let Ok(mut slot) = PATH_OVERRIDE.lock() {
        *slot = None;
    }
}

fn override_path() -> Option<PathBuf> {
    PATH_OVERRIDE.lock().ok().and_then(|slot| slot.clone())
}

/// Value of a `--config=PATH` argument, split at the OS-string level so a
/// path with bytes Windows/Unix can store but UTF-8 cannot represent (an
/// undecodable filename on Unix, a lone surrogate on Windows) survives
/// intact instead of being mangled by `to_string_lossy`. `None` when the
/// argument is not in that form. Used by both binaries' argv pre-parsers.
#[doc(hidden)]
pub fn config_flag_value(arg: &std::ffi::OsStr) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        let rest = arg.as_bytes().strip_prefix(b"--config=")?;
        Some(std::ffi::OsString::from_vec(rest.to_vec()).into())
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        const PREFIX: &[u16] = &[
            b'-' as u16,
            b'-' as u16,
            b'c' as u16,
            b'o' as u16,
            b'n' as u16,
            b'f' as u16,
            b'i' as u16,
            b'g' as u16,
            b'=' as u16,
        ];
        let wide: Vec<u16> = arg.encode_wide().collect();
        let rest = wide.strip_prefix(PREFIX)?;
        Some(std::ffi::OsString::from_wide(rest).into())
    }
    #[cfg(not(any(unix, windows)))]
    {
        Some(PathBuf::from(arg.to_str()?.strip_prefix("--config=")?))
    }
}

/// Expand a leading `~` (or `~/`) against the user's home directory. Anything
/// else — including `~user` — is left untouched.
fn expand_tilde(p: &std::path::Path) -> PathBuf {
    let Some(s) = p.to_str() else {
        return p.to_path_buf();
    };
    let rest = if s == "~" {
        ""
    } else if let Some(r) = s.strip_prefix("~/") {
        r
    } else {
        return p.to_path_buf();
    };
    match crate::cache::home_dir() {
        Ok(home) if rest.is_empty() => home,
        Ok(home) => home.join(rest),
        Err(_) => p.to_path_buf(),
    }
}

fn expand_tilde_opt(p: &mut Option<PathBuf>) {
    if let Some(inner) = p.as_ref() {
        *p = Some(expand_tilde(inner));
    }
}

/// Resolved `config.toml` path as a string for user-facing messages. Uses the
/// platform's config dir (`directories::ProjectDirs`), so it reads correctly on
/// Linux, macOS, and Windows instead of hard-coding the Unix `~/.config` path.
/// Falls back to the bare filename if the path can't be resolved.
pub fn config_path_hint() -> String {
    resolved_path()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "config.toml".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[cfg(unix)]
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    fn write_toml(s: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(s.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    /// The back-compat guarantee #134 asks for: a config with no
    /// `[[openai.accounts]]` resolves exactly what it resolved before, whether
    /// it sets `codex_auth_path` or leaves it to the default.
    #[test]
    fn openai_without_accounts_resolves_the_singular_path() {
        let explicit = OpenAiConfig {
            codex_auth_path: Some(PathBuf::from("/tmp/codex/auth.json")),
            ..OpenAiConfig::default()
        };
        assert_eq!(
            explicit.resolve_auth_path(None).unwrap(),
            PathBuf::from("/tmp/codex/auth.json")
        );

        let bare = OpenAiConfig::default();
        assert_eq!(
            bare.resolve_auth_path(None).unwrap(),
            crate::openai::creds::default_path().unwrap(),
            "no codex_auth_path must still mean ~/.codex/auth.json"
        );
    }

    /// Each named account resolves its own file, and the default login is still
    /// reachable alongside them.
    #[test]
    fn openai_named_accounts_resolve_their_own_auth_file() {
        let config: Config = toml::from_str(
            r#"
            [openai]
            codex_auth_path = "/tmp/personal/auth.json"
            [[openai.accounts]]
            label = "work"
            codex_auth_path = "/tmp/work/auth.json"
            "#,
        )
        .unwrap();

        assert_eq!(
            config.openai.resolve_auth_path(Some("work")).unwrap(),
            PathBuf::from("/tmp/work/auth.json")
        );
        assert_eq!(
            config.openai.resolve_auth_path(None).unwrap(),
            PathBuf::from("/tmp/personal/auth.json")
        );
    }

    /// An unknown label must fail rather than quietly fall back to the default
    /// login — reporting the wrong subscription's usage is worse than an error.
    #[test]
    fn an_unknown_openai_account_is_an_error_not_a_fallback() {
        let config = OpenAiConfig {
            codex_auth_path: Some(PathBuf::from("/tmp/personal/auth.json")),
            accounts: vec![OpenAiAccount {
                label: "work".into(),
                codex_auth_path: PathBuf::from("/tmp/work/auth.json"),
            }],
            ..OpenAiConfig::default()
        };
        let err = config
            .resolve_auth_path(Some("nope"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("nope"), "{err}");
        assert!(err.contains("[[openai.accounts]]"), "{err}");
    }

    #[test]
    fn defaults_enable_only_the_four_core_vendors() {
        let c = Config::default();
        assert!(c.is_enabled(VendorId::Anthropic));
        assert!(c.is_enabled(VendorId::Openai));
        assert!(c.is_enabled(VendorId::Zai));
        assert!(c.is_enabled(VendorId::Openrouter));
        for opt_in in [
            VendorId::AnthropicApi,
            VendorId::Copilot,
            VendorId::Deepseek,
            VendorId::Kimi,
            VendorId::Kilo,
            VendorId::Novita,
            VendorId::Moonshot,
            VendorId::Grok,
            VendorId::Supergrok,
            VendorId::Cursor,
            VendorId::Minimax,
            VendorId::Kiro,
        ] {
            assert!(!c.is_enabled(opt_in), "{opt_in:?}");
        }
        assert_eq!(c.enabled_vendors().len(), 4);
    }

    #[test]
    fn new_provider_defaults_are_opt_in_and_use_exact_auth_contracts() {
        let config = Config::default();
        assert!(!config.is_enabled(VendorId::NousResearch));
        assert!(!config.is_enabled(VendorId::OpenCodeGo));
        assert_eq!(config.opencode_go.api_key_env, "OPENCODE_GO_API_KEY");
        assert!(config.opencode_go.api_key.is_none());
        assert!(!config.is_enabled(VendorId::Copilot));
    }

    #[cfg(unix)]
    #[test]
    fn inline_credentials_are_protected() {
        let mut config = Config::default();
        config.opencode_go.api_key = Some("<redacted>".to_string());
        assert!(config.has_inline_secrets());
    }

    #[cfg(unix)]
    #[test]
    fn openrouter_named_inline_keys_receive_config_file_protection() {
        let mut config = Config::default();
        config.openrouter.accounts.push(OpenRouterAccount {
            label: "work".into(),
            api_key_env: None,
            api_key: Some("<redacted>".into()),
        });
        assert!(config.has_inline_secrets());
    }

    #[test]
    fn missing_file_uses_defaults() {
        let path = std::path::Path::new("/tmp/does-not-exist-ai-usagebar-test");
        let c = Config::load_from(path).unwrap();
        assert!(c.is_enabled(VendorId::Anthropic));
    }

    #[test]
    fn parses_full_config() {
        let f = write_toml(
            r#"
            [anthropic]
            enabled = true

            [openai]
            enabled = false
            admin_key_env = "MY_ADMIN_KEY"

            [zai]
            enabled = true
            api_key_env = "MY_ZAI"
            plan_tier = "pro"

            [openrouter]
            enabled = false
            "#,
        );
        let c = Config::load_from(f.path()).unwrap();
        assert!(c.is_enabled(VendorId::Anthropic));
        assert!(!c.is_enabled(VendorId::Openai));
        assert!(c.is_enabled(VendorId::Zai));
        assert!(!c.is_enabled(VendorId::Openrouter));
        assert_eq!(c.openai.admin_key_env, "MY_ADMIN_KEY");
        assert_eq!(c.zai.api_key_env, "MY_ZAI");
        assert_eq!(c.zai.plan_tier.as_deref(), Some("pro"));
        assert!(c.openrouter.accounts.is_empty());
        assert!(c.openrouter.show_default_account);
    }

    #[test]
    fn partial_config_falls_back_to_defaults() {
        let f = write_toml(
            r#"[openai]
enabled = false
"#,
        );
        let c = Config::load_from(f.path()).unwrap();
        assert!(!c.is_enabled(VendorId::Openai));
        // Other vendors keep their defaults.
        assert!(c.is_enabled(VendorId::Anthropic));
        assert_eq!(c.openai.admin_key_env, "OPENAI_ADMIN_KEY");
    }

    #[test]
    fn malformed_toml_returns_error() {
        let f = write_toml("this is not = = valid");
        assert!(Config::load_from(f.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn load_from_tightens_world_readable_config_with_inline_api_key() {
        let file = write_toml("[zai]\napi_key = \"test-inline-key\"\n");
        std::fs::set_permissions(file.path(), std::fs::Permissions::from_mode(0o644)).unwrap();

        Config::load_from(file.path()).unwrap();

        assert_eq!(
            std::fs::metadata(file.path()).unwrap().mode() & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn load_from_leaves_world_readable_config_without_inline_api_keys_unchanged() {
        let file = write_toml("[zai]\napi_key_env = \"TEST_ZAI_API_KEY\"\n");
        std::fs::set_permissions(file.path(), std::fs::Permissions::from_mode(0o644)).unwrap();

        Config::load_from(file.path()).unwrap();

        assert_eq!(
            std::fs::metadata(file.path()).unwrap().mode() & 0o777,
            0o644
        );
    }

    #[cfg(unix)]
    #[test]
    fn inline_key_permission_decision_requires_tightening_for_group_or_other_bits() {
        assert_eq!(
            inline_key_permission_decision(0o600),
            InlineKeyPermissionDecision::Ok
        );
        assert_eq!(
            inline_key_permission_decision(0o640),
            InlineKeyPermissionDecision::Tighten
        );
        assert_eq!(
            inline_key_permission_decision(0o604),
            InlineKeyPermissionDecision::Tighten
        );
    }

    #[test]
    fn anthropic_api_monthly_limit_must_be_positive_and_finite() {
        for value in ["0", "-1", "inf", "nan"] {
            let file = write_toml(&format!("[anthropic_api]\nmonthly_limit = {value}\n"));
            let error = Config::load_from(file.path()).unwrap_err().to_string();
            assert!(error.contains("monthly_limit"), "value {value}: {error}");
        }

        let file = write_toml("[anthropic_api]\nmonthly_limit = 1000\n");
        assert_eq!(
            Config::load_from(file.path())
                .unwrap()
                .anthropic_api
                .monthly_limit,
            Some(1000.0)
        );
    }

    #[test]
    fn minimax_region_accepts_only_known_instances() {
        for region in ["global", "GLOBAL", "cn", "CN"] {
            let file = write_toml(&format!("[minimax]\nregion = {region:?}\n"));
            assert_eq!(
                Config::load_from(file.path()).unwrap().minimax.region,
                region
            );
        }

        for region in ["", "china", "us"] {
            let file = write_toml(&format!("[minimax]\nregion = {region:?}\n"));
            let error = Config::load_from(file.path()).unwrap_err().to_string();
            assert!(error.contains("[minimax] region"), "{error}");
        }
    }

    #[test]
    fn kimi_region_accepts_auto_and_both_deployments() {
        for region in ["auto", "AUTO", "cn", "mainland-cn", "global"] {
            let file = write_toml(&format!("[kimi]\nregion = {region:?}\n"));
            assert_eq!(Config::load_from(file.path()).unwrap().kimi.region, region);
        }

        for region in ["", "us", "oversea"] {
            let file = write_toml(&format!("[kimi]\nregion = {region:?}\n"));
            let error = Config::load_from(file.path()).unwrap_err().to_string();
            assert!(error.contains("[kimi] region"), "{error}");
        }
    }

    #[test]
    fn kimi_defaults_to_auto_region_and_no_credential_override() {
        let defaults = KimiConfig::default();
        assert_eq!(defaults.region, "auto");
        assert_eq!(defaults.credentials_path, None);
        assert!(!defaults.enabled);
    }

    #[test]
    fn kimi_credentials_path_expands_a_tilde() {
        let file = write_toml("[kimi]\ncredentials_path = \"~/kimi/creds.json\"\n");
        let path = Config::load_from(file.path())
            .unwrap()
            .kimi
            .credentials_path
            .unwrap();
        assert!(!path.starts_with("~"), "{}", path.display());
        assert!(path.ends_with("kimi/creds.json"), "{}", path.display());
    }

    #[test]
    fn optional_api_key_reports_absence_instead_of_failing() {
        assert_eq!(
            optional_api_key("KIMI_API_KEY_DEFINITELY_UNSET", Some("inline")),
            Some("inline".to_string())
        );
        assert_eq!(
            optional_api_key("KIMI_API_KEY_DEFINITELY_UNSET", None),
            None
        );
        assert_eq!(optional_api_key("KIMI_API_KEY_UNSET", Some("")), None);
        // An unusable `api_key_env` still lets an inline key through, exactly
        // as `resolve_api_key` does.
        assert_eq!(
            optional_api_key("9INVALID", Some("inline")),
            Some("inline".to_string())
        );
    }

    #[test]
    fn context_monitor_is_opt_in_and_window_sizes_are_explicit() {
        let defaults = Config::default();
        assert!(!defaults.context.enabled);
        assert_eq!(
            defaults.context.window_tokens_for(Some("claude-test")),
            None
        );

        let file = write_toml(
            r#"
            [context]
            enabled = true
            context_window_tokens = 200000

            [context.model_context_window_tokens]
            claude-opus-1m = 1000000
            "claude exact id" = 300000
            "#,
        );
        let config = Config::load_from(file.path()).unwrap();
        assert!(config.context.enabled);
        assert_eq!(
            config.context.window_tokens_for(Some("claude-opus-1m")),
            Some(1_000_000)
        );
        assert_eq!(
            config.context.window_tokens_for(Some("claude exact id")),
            Some(300_000)
        );
        assert_eq!(
            config.context.window_tokens_for(Some("another-model")),
            Some(200_000)
        );
    }

    #[test]
    fn context_layout_defaults_to_full_and_parses_each_variant() {
        assert_eq!(Config::default().context.layout, ContextLayout::Full);
        for (text, want) in [
            ("full", ContextLayout::Full),
            ("split", ContextLayout::Split),
            ("bottom", ContextLayout::Bottom),
        ] {
            let file = write_toml(&format!("[context]\nlayout = \"{text}\"\n"));
            assert_eq!(Config::load_from(file.path()).unwrap().context.layout, want);
        }
        let file = write_toml("[context]\nlayout = \"floating\"\n");
        assert!(
            Config::load_from(file.path()).is_err(),
            "an unknown layout must be rejected, not silently defaulted"
        );
    }

    #[test]
    fn vendor_box_defaults_to_sidebar_and_parses_each_variant() {
        assert_eq!(Config::default().ui.vendor_box(), VendorBoxStyle::Sidebar);
        for (text, want) in [
            ("sidebar", VendorBoxStyle::Sidebar),
            ("navbar", VendorBoxStyle::Navbar),
            ("none", VendorBoxStyle::None),
        ] {
            let file = write_toml(&format!("[ui]\nvendor_box = \"{text}\"\n"));
            assert_eq!(
                Config::load_from(file.path()).unwrap().ui.vendor_box(),
                want
            );
        }
        let file = write_toml("[ui]\nvendor_box = \"floating\"\n");
        assert!(
            Config::load_from(file.path()).is_err(),
            "an unknown vendor_box style must be rejected, not silently defaulted"
        );
    }

    #[test]
    fn context_window_sizes_must_be_nonzero_and_model_ids_nonempty() {
        for source in [
            "[context]\ncontext_window_tokens = 0\n",
            "[context.model_context_window_tokens]\nclaude = 0\n",
            "[context.model_context_window_tokens]\n\" \" = 200000\n",
        ] {
            let file = write_toml(source);
            let error = Config::load_from(file.path()).unwrap_err().to_string();
            assert!(error.contains("context"), "{error}");
        }
    }

    // serial guard for env-var manipulation tests so they don't race
    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        static M: std::sync::Mutex<()> = std::sync::Mutex::new(());
        M.lock().unwrap_or_else(|p| p.into_inner())
    }

    #[test]
    fn resolve_api_key_prefers_env_over_inline() {
        let _g = env_guard();
        // Use a unique env var name so we don't clobber test parallelism.
        let var = "AI_USAGEBAR_TEST_ENV_WINS";
        // SAFETY: tests are single-threaded under env_guard.
        unsafe { std::env::set_var(var, "from-env") };
        let got = resolve_api_key("Zai", var, Some("from-inline")).unwrap();
        unsafe { std::env::remove_var(var) };
        assert_eq!(got, "from-env");
    }

    #[test]
    fn resolve_api_key_falls_back_to_inline() {
        let _g = env_guard();
        let var = "AI_USAGEBAR_TEST_INLINE_FALLBACK";
        unsafe { std::env::remove_var(var) };
        let got = resolve_api_key("Zai", var, Some("inline-key")).unwrap();
        assert_eq!(got, "inline-key");
    }

    #[test]
    fn copilot_token_prefers_explicit_environment_over_gh_cli() {
        struct NeverRun;
        impl crate::copilot::credentials::GhAuthTokenRunner for NeverRun {
            fn run(
                &self,
                _: &crate::copilot::credentials::GhAuthTokenCommand,
            ) -> std::io::Result<crate::copilot::credentials::GhAuthTokenOutput> {
                panic!("environment override must not invoke gh")
            }
        }

        let token = CopilotConfig::default()
            .resolve_token_with(
                |name| (name == "GITHUB_COPILOT_TOKEN").then(|| "from-environment".into()),
                &NeverRun,
            )
            .unwrap();
        assert_eq!(token, "from-environment");
    }

    #[test]
    fn copilot_token_uses_injected_gh_cli_and_hides_failure_output() {
        struct FailedGh;
        impl crate::copilot::credentials::GhAuthTokenRunner for FailedGh {
            fn run(
                &self,
                _: &crate::copilot::credentials::GhAuthTokenCommand,
            ) -> std::io::Result<crate::copilot::credentials::GhAuthTokenOutput> {
                Ok(crate::copilot::credentials::GhAuthTokenOutput {
                    success: false,
                    stdout: b"never-echo-gh-output".to_vec(),
                })
            }
        }
        let error = CopilotConfig::default()
            .resolve_token_with(|_| None, &FailedGh)
            .unwrap_err()
            .to_string();
        assert!(error.contains("gh auth login --web"));
        assert!(!error.contains("never-echo-gh-output"));
    }

    #[test]
    fn resolve_api_key_errors_when_both_missing() {
        let _g = env_guard();
        let var = "AI_USAGEBAR_TEST_BOTH_MISSING";
        unsafe { std::env::remove_var(var) };
        let err = resolve_api_key("Zai", var, None).unwrap_err();
        match err {
            crate::error::AppError::Credentials(msg) => {
                assert!(
                    msg.contains("api_key"),
                    "error should suggest config field: {msg}"
                );
            }
            other => panic!("expected Credentials error, got {other:?}"),
        }
    }

    #[test]
    fn resolve_api_key_uses_exact_opencode_go_section_name() {
        let _g = env_guard();
        unsafe { std::env::remove_var("OPENCODE_GO_API_KEY") };
        let err = resolve_api_key("OpenCode Go", "OPENCODE_GO_API_KEY", None).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("[opencode-go]"),
            "wrong section hint: {message}"
        );
        assert!(
            !message.contains("[opencode go]"),
            "wrong section hint: {message}"
        );
    }

    fn path_override_guard() -> std::sync::MutexGuard<'static, ()> {
        static M: std::sync::Mutex<()> = std::sync::Mutex::new(());
        M.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Serializes the override tests *and* guarantees the process-wide
    /// override is dropped when the test ends — including via a panic, which
    /// a bare set/clear pair does not survive. A leaked override makes every
    /// later test in this process resolve a deleted temp file, turning one
    /// failure into a cascade of confusing sibling failures.
    struct ScopedPathOverride {
        _serial: std::sync::MutexGuard<'static, ()>,
    }

    impl Drop for ScopedPathOverride {
        fn drop(&mut self) {
            clear_override_path();
        }
    }

    fn scoped_path_override() -> ScopedPathOverride {
        ScopedPathOverride {
            _serial: path_override_guard(),
        }
    }

    #[test]
    fn override_path_wins_over_canonical_and_legacy() {
        let _scoped = scoped_path_override();
        let file = NamedTempFile::new().unwrap();
        set_override_path(file.path());
        assert_eq!(resolved_path().as_deref(), Some(file.path()));
        assert_eq!(config_path_hint(), file.path().display().to_string());
        clear_override_path();
        // The usual locations decide again once the override is gone.
        let p = resolved_path().expect("a config path must resolve");
        assert!(p.ends_with("config.toml"));
    }

    #[test]
    fn scoped_override_guard_clears_the_override_on_panic() {
        // Silence the simulated failure's hook output; the assertion below is
        // the real report.
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _scoped = scoped_path_override();
            set_override_path(std::path::Path::new("panicked-override.toml"));
            panic!("simulated mid-test failure");
        }))
        .is_err();
        std::panic::set_hook(hook);
        assert!(panicked, "the simulated failure must run");
        let _serial = path_override_guard();
        assert!(
            override_path().is_none(),
            "a panicking test must not leak the override into siblings"
        );
    }

    #[test]
    fn config_path_hint_ends_with_config_toml() {
        let _g = path_override_guard();
        // Platform-resolved (Linux/macOS/Windows), but always ends in the
        // config filename — the trailing segment is what messages rely on.
        assert!(config_path_hint().ends_with("config.toml"));
    }

    #[test]
    fn config_flag_value_splits_the_equals_form() {
        use std::ffi::OsStr;
        assert_eq!(
            config_flag_value(OsStr::new("--config=work.toml")).as_deref(),
            Some(std::path::Path::new("work.toml"))
        );
        assert_eq!(
            config_flag_value(OsStr::new("--config=")).as_deref(),
            Some(std::path::Path::new(""))
        );
        assert_eq!(config_flag_value(OsStr::new("--config")), None);
        assert_eq!(config_flag_value(OsStr::new("--config-file")), None);
        assert_eq!(config_flag_value(OsStr::new("account")), None);
    }

    /// The `--config=PATH` form must preserve a path the platform can store
    /// but UTF-8 cannot represent — `to_string_lossy` would replace the bad
    /// bytes with U+FFFD and produce a false "config file not found".
    #[cfg(unix)]
    #[test]
    fn config_flag_value_keeps_undecodable_bytes_intact() {
        use std::ffi::OsString;
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        let raw = OsString::from_vec(b"--config=caf\xe9.toml".to_vec());
        let value = config_flag_value(&raw).expect("prefix matches");
        assert_eq!(value.as_os_str().as_bytes(), b"caf\xe9.toml");
    }

    #[cfg(windows)]
    #[test]
    fn config_flag_value_keeps_lone_surrogates_intact() {
        use std::ffi::OsString;
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        let mut wide: Vec<u16> = "--config=".encode_utf16().collect();
        wide.push(0xDC00); // lone low surrogate: not valid Unicode
        wide.extend("x.toml".encode_utf16());
        let raw = OsString::from_wide(&wide);
        let value = config_flag_value(&raw).expect("prefix matches");
        let mut expected = vec![0xDC00u16];
        expected.extend("x.toml".encode_utf16());
        assert_eq!(
            value.as_os_str().encode_wide().collect::<Vec<_>>(),
            expected
        );
    }

    #[test]
    fn resolve_api_key_treats_empty_env_as_unset() {
        let _g = env_guard();
        let var = "AI_USAGEBAR_TEST_EMPTY_ENV";
        unsafe { std::env::set_var(var, "") };
        let got = resolve_api_key("OpenRouter", var, Some("inline")).unwrap();
        unsafe { std::env::remove_var(var) };
        assert_eq!(got, "inline");
    }

    #[test]
    fn resolve_api_key_rejects_invalid_env_var_name_without_leaking_it() {
        let _g = env_guard();
        // Simulates a user accidentally pasting the key into api_key_env.
        let bad = "sk-kimi-very-real-looking-pasted-secret";
        let err = resolve_api_key("Kimi", bad, None).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("invalid") && msg.contains("api_key_env"),
            "error should explain misconfiguration: {msg}"
        );
        assert!(
            !msg.contains(bad),
            "error must not echo the misconfigured value: {msg}"
        );
        assert!(msg.contains("valid environment variable name"));
        assert!(
            msg.contains("[kimi]"),
            "error should point at the lowercase TOML section: {msg}"
        );
    }

    #[test]
    fn resolve_api_key_invalid_env_name_falls_back_to_inline() {
        let _g = env_guard();
        let got = resolve_api_key("Kimi", "sk-pasted-secret", Some("inline-key")).unwrap();
        assert_eq!(got, "inline-key");
    }

    #[test]
    fn resolve_api_key_never_leaks_valid_looking_configured_env_name() {
        let _g = env_guard();
        // This is syntactically a valid environment variable name, but could
        // be a pasted secret and must not be reflected in the error.
        let pasted_secret = "sk_pasted_secret";
        unsafe { std::env::remove_var(pasted_secret) };
        let err = resolve_api_key("Kimi", pasted_secret, None).unwrap_err();
        assert!(
            !err.to_string().contains(pasted_secret),
            "error must not echo configured api_key_env values"
        );
    }

    #[test]
    fn is_valid_env_var_name_rules() {
        // Valid: alphabetic or underscore first, then alnum/underscore.
        for valid in ["KIMI_API_KEY", "_PRIVATE", "a", "Z9", "MY_ZAI_2"] {
            assert!(is_valid_env_var_name(valid), "{valid} should be valid");
        }
        // Invalid: empty, digit-first, or shell-illegal characters.
        for invalid in ["", "9LIVES", "sk-kimi", "MY KEY", "A.B", "sk/k"] {
            assert!(
                !is_valid_env_var_name(invalid),
                "{invalid} should be invalid"
            );
        }
    }

    #[test]
    fn config_parses_with_inline_api_key_and_primary() {
        let f = write_toml(
            r#"
            [ui]
            primary = "openrouter"

            [zai]
            enabled = true
            api_key_env = "MY_ZAI"
            api_key = "sk-zai-inline"

            [openrouter]
            enabled = true
            api_key = "sk-or-inline"
            "#,
        );
        let c = Config::load_from(f.path()).unwrap();
        assert_eq!(c.ui.primary, Some(VendorId::Openrouter));
        assert_eq!(c.zai.api_key.as_deref(), Some("sk-zai-inline"));
        assert_eq!(c.openrouter.api_key.as_deref(), Some("sk-or-inline"));
    }

    #[test]
    fn openrouter_named_accounts_preserve_the_default_contract() {
        let f = write_toml(
            r#"
            [openrouter]
            enabled = true
            api_key_env = "AI_USAGEBAR_TEST_OR_DEFAULT"
            api_key = "default-inline"
            show_default_account = false

            [[openrouter.accounts]]
            label = "work"
            api_key_env = "OPENROUTER_WORK_API_KEY"

            [[openrouter.accounts]]
            label = "personal"
            api_key = "personal-inline"
            "#,
        );
        let _g = env_guard();
        unsafe { std::env::remove_var("AI_USAGEBAR_TEST_OR_DEFAULT") };
        let config = Config::load_from(f.path()).unwrap();
        assert!(!config.openrouter.show_default_account);
        assert_eq!(config.openrouter.accounts.len(), 2);
        assert_eq!(
            config.openrouter.resolve_api_key(None).unwrap(),
            "default-inline"
        );
        assert_eq!(
            config.openrouter.resolve_api_key(Some("personal")).unwrap(),
            "personal-inline"
        );
    }

    #[test]
    fn openrouter_named_accounts_reject_ambiguous_or_unsafe_labels() {
        for source in [
            r#"
            [[openrouter.accounts]]
            label = "work"
            api_key = "one"
            [[openrouter.accounts]]
            label = "work"
            api_key = "two"
            "#,
            r#"
            [[openrouter.accounts]]
            label = "../work"
            api_key = "one"
            "#,
            r#"
            [[openrouter.accounts]]
            label = "work"
            "#,
        ] {
            let f = write_toml(source);
            assert!(Config::load_from(f.path()).is_err(), "accepted {source}");
        }
    }

    #[test]
    fn openrouter_unknown_account_never_falls_back_to_default_key() {
        let mut config = OpenRouterConfig {
            api_key: Some("default-secret".into()),
            ..OpenRouterConfig::default()
        };
        config.accounts.push(OpenRouterAccount {
            label: "work".into(),
            api_key_env: None,
            api_key: Some("work-secret".into()),
        });
        let message = config
            .resolve_api_key(Some("missing"))
            .unwrap_err()
            .to_string();
        assert!(message.contains("missing") && message.contains("work"));
        assert!(!message.contains("default-secret"));
        assert!(!message.contains("work-secret"));
    }

    #[test]
    fn openrouter_account_key_errors_do_not_echo_configured_values() {
        let config = OpenRouterConfig {
            accounts: vec![OpenRouterAccount {
                label: "work".into(),
                api_key_env: Some("sk_pasted_secret".into()),
                api_key: None,
            }],
            ..OpenRouterConfig::default()
        };
        let _g = env_guard();
        unsafe { std::env::remove_var("sk_pasted_secret") };
        let message = config
            .resolve_api_key(Some("work"))
            .unwrap_err()
            .to_string();
        assert!(message.contains("[[openrouter.accounts]]"));
        assert!(!message.contains("sk_pasted_secret"));
    }

    #[test]
    fn enabled_vendors_preserves_canonical_order() {
        // DeepSeek and Kimi are disabled by default (require explicit API key
        // config), so they are absent from the enabled list unless enabled.
        let c = Config::default();
        assert_eq!(
            c.enabled_vendors(),
            vec![
                VendorId::Anthropic,
                VendorId::Openai,
                VendorId::Zai,
                VendorId::Openrouter,
            ]
        );
    }

    #[test]
    fn deepseek_appears_when_enabled() {
        let f = write_toml(
            r#"
            [deepseek]
            enabled = true
            api_key = "sk-test"
            "#,
        );
        let c = Config::load_from(f.path()).unwrap();
        assert!(c.is_enabled(VendorId::Deepseek));
        assert!(c.enabled_vendors().contains(&VendorId::Deepseek));
        assert_eq!(c.deepseek.api_key.as_deref(), Some("sk-test"));
    }

    #[test]
    fn tilde_paths_are_expanded_on_load() {
        // `PathBuf` keeps `~` literally, so the documented
        // `credentials_path = "~/..."` used to resolve to a directory named
        // `~` relative to the process's cwd.
        let f = write_toml(
            r#"
            [context]
            projects_path = "~/.claude/projects"

            [anthropic]
            credentials_path = "~/.claude/.credentials.json"

            [[anthropic.accounts]]
            label = "work"
            credentials_path = "~/work.json"
            "#,
        );
        let c = Config::load_from(f.path()).unwrap();
        let home = crate::cache::home_dir().unwrap();

        assert_eq!(c.context.projects_path, Some(home.join(".claude/projects")));
        let got = c.anthropic.credentials_path.unwrap();
        assert_eq!(got, home.join(".claude/.credentials.json"));
        assert!(!got.to_string_lossy().contains('~'));
        assert_eq!(
            c.anthropic.accounts[0].credentials_path,
            home.join("work.json")
        );
    }

    #[test]
    fn absolute_and_relative_paths_are_left_alone() {
        let f = write_toml(
            r#"
            [anthropic]
            credentials_path = "/etc/creds.json"
            "#,
        );
        let c = Config::load_from(f.path()).unwrap();
        assert_eq!(
            c.anthropic.credentials_path.unwrap(),
            std::path::Path::new("/etc/creds.json")
        );

        // `~user` is not ours to interpret.
        let f2 = write_toml(
            r#"
            [anthropic]
            credentials_path = "~someone/creds.json"
            "#,
        );
        let c2 = Config::load_from(f2.path()).unwrap();
        assert_eq!(
            c2.anthropic.credentials_path.unwrap(),
            std::path::Path::new("~someone/creds.json")
        );
    }

    #[test]
    fn resolved_path_is_the_canonical_one_and_names_the_config_file() {
        let _g = path_override_guard();
        // Hermetic: only asserts the shape, never which file happens to exist
        // on the machine running the tests.
        let p = resolved_path().expect("a config path must resolve");
        assert!(p.ends_with("config.toml"));
        let canonical = default_path().unwrap();
        let legacy = legacy_xdg_path().unwrap();
        assert!(
            p == canonical || p == legacy,
            "resolved to an unexpected location: {}",
            p.display()
        );
    }

    #[test]
    fn misspelled_section_is_rejected_not_ignored() {
        // The regression this guards: `[openrouer]` used to parse fine, leave
        // OpenRouter on its defaults, and give the user no hint at all.
        let f = write_toml(
            r#"
            [openrouer]
            enabled = true
            api_key = "sk-or-v1-typo"
            "#,
        );
        let err = Config::load_from(f.path()).unwrap_err().to_string();
        assert!(
            err.contains("openrouer"),
            "error should name the typo: {err}"
        );
    }

    #[test]
    fn invalid_toml_is_an_error_not_silent_defaults() {
        let f = write_toml("[zai\nenabled = true\n");
        assert!(Config::load_from(f.path()).is_err());
    }

    #[test]
    fn a_missing_file_is_still_just_defaults() {
        // Absence stays the legitimate "use defaults" case — only real parse
        // and I/O failures are errors.
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope").join("config.toml");
        let c = Config::load_from(&missing).unwrap();
        assert!(c.is_enabled(VendorId::Anthropic));
    }

    #[test]
    fn kimi_appears_when_enabled() {
        let f = write_toml(
            r#"
            [kimi]
            enabled = true
            api_key = "sk-test"
            "#,
        );
        let c = Config::load_from(f.path()).unwrap();
        assert!(c.is_enabled(VendorId::Kimi));
        assert!(c.enabled_vendors().contains(&VendorId::Kimi));
        assert_eq!(c.kimi.api_key.as_deref(), Some("sk-test"));
    }

    #[test]
    fn enabled_deepseek_and_kimi_appear_in_canonical_order_ending_with_them() {
        let f = write_toml(
            r#"
            [deepseek]
            enabled = true
            api_key = "sk-ds"

            [kimi]
            enabled = true
            api_key = "sk-kimi"
            "#,
        );
        let c = Config::load_from(f.path()).unwrap();
        assert_eq!(
            c.enabled_vendors(),
            vec![
                VendorId::Anthropic,
                VendorId::Openai,
                VendorId::Zai,
                VendorId::Openrouter,
                VendorId::Deepseek,
                VendorId::Kimi,
            ]
        );
    }

    #[test]
    fn parses_anthropic_accounts_and_looks_them_up() {
        let f = write_toml(
            r#"
            [anthropic]
            enabled = true

            [[anthropic.accounts]]
            label = "personal"
            credentials_path = "/creds/personal.json"

            [[anthropic.accounts]]
            label = "work"
            credentials_path = "/creds/work.json"
            "#,
        );
        let c = Config::load_from(f.path()).unwrap();
        assert_eq!(c.anthropic.accounts.len(), 2);
        let work = c.anthropic.account("work").unwrap();
        assert_eq!(work.credentials_path, PathBuf::from("/creds/work.json"));
        // A typo names the offending label and lists the known ones.
        let err = format!("{:?}", c.anthropic.account("missing").unwrap_err());
        assert!(err.contains("missing") && err.contains("work"), "{err}");
    }

    #[test]
    fn duplicate_anthropic_account_labels_are_rejected_on_load() {
        let f = write_toml(
            r#"
            [[anthropic.accounts]]
            label = "work"
            credentials_path = "/creds/work-one.json"

            [[anthropic.accounts]]
            label = "work"
            credentials_path = "/creds/work-two.json"
            "#,
        );
        let err = Config::load_from(f.path()).unwrap_err().to_string();
        assert!(
            err.contains("duplicate anthropic account label \"work\""),
            "{err}"
        );
    }

    #[test]
    fn account_label_rejects_path_like_names() {
        let cfg = AnthropicConfig::default();
        for bad in [
            "",
            ".",
            "..",
            "a/b",
            r"a\b",
            "C:work",
            "line\nbreak",
            "tab\tname",
            "usage.json",
            ".stale",
            ".last_error",
            ".fetch.lock",
        ] {
            let err = cfg.account(bad).unwrap_err();
            assert!(
                format!("{err:?}").contains("invalid anthropic account label"),
                "{bad:?} should be rejected as a label"
            );
        }
    }

    #[test]
    fn anthropic_accounts_default_to_empty() {
        // No [[anthropic.accounts]] → the single default account, empty list,
        // nothing to migrate (issue #14, back-compat rule 1).
        assert!(Config::default().anthropic.accounts.is_empty());
        assert!(Config::default().anthropic.accounts_dir.is_none());
    }

    // --- accounts_dir: CLAUDE_CONFIG_DIR-style auto-discovery ----------------
    // All hermetic: discovery reads a TempDir, never the user's real config.

    /// Create `<root>/<label>/.credentials.json` (contents irrelevant here —
    /// discovery keys on the file existing, the fetch path parses it).
    fn seed_account_dir(root: &std::path::Path, label: &str) {
        let dir = root.join(label);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".credentials.json"), "{}").unwrap();
    }

    #[test]
    fn discovers_account_dirs_in_claude_config_dir_layout() {
        let td = tempfile::tempdir().unwrap();
        seed_account_dir(td.path(), "work");
        seed_account_dir(td.path(), "personal");
        // Keychain-backed macOS logins may not write .credentials.json; their
        // config directories are still account entries.
        std::fs::create_dir_all(td.path().join("keychain-only")).unwrap();
        // A loose file (not a dir) is ignored.
        std::fs::write(td.path().join("stray.json"), "{}").unwrap();

        let cfg = AnthropicConfig {
            accounts_dir: Some(td.path().to_path_buf()),
            ..Default::default()
        };
        let all = cfg.all_accounts();
        let labels: Vec<&str> = all.iter().map(|a| a.label.as_str()).collect();
        assert_eq!(labels, vec!["keychain-only", "personal", "work"]);
        assert_eq!(
            all[2].credentials_path,
            td.path().join("work").join(".credentials.json")
        );
    }

    #[test]
    fn explicit_account_wins_over_a_discovered_one_with_the_same_label() {
        let td = tempfile::tempdir().unwrap();
        seed_account_dir(td.path(), "work");
        let cfg = AnthropicConfig {
            accounts: vec![AnthropicAccount {
                label: "work".into(),
                credentials_path: "/explicit/work.json".into(),
            }],
            accounts_dir: Some(td.path().to_path_buf()),
            ..Default::default()
        };
        let all = cfg.all_accounts();
        assert_eq!(all.len(), 1, "no duplicate label");
        assert_eq!(
            all[0].credentials_path,
            std::path::Path::new("/explicit/work.json"),
            "explicit entry wins"
        );
        // A discovered account is still reachable through `account()`.
        seed_account_dir(td.path(), "other");
        assert_eq!(cfg.account("other").unwrap().label, "other");
    }

    #[test]
    fn missing_accounts_dir_is_silently_empty_not_an_error() {
        let cfg = AnthropicConfig {
            accounts_dir: Some("/nonexistent/ai-usagebar-accounts".into()),
            ..Default::default()
        };
        assert!(cfg.all_accounts().is_empty());
    }

    #[test]
    fn openai_account_auth_paths_are_tilde_expanded_on_load() {
        let f = write_toml(
            r#"
            [[openai.accounts]]
            label = "work"
            codex_auth_path = "~/.codex-work/auth.json"
            "#,
        );
        let c = Config::load_from(f.path()).unwrap();
        let home = crate::cache::home_dir().unwrap();
        assert_eq!(
            c.openai.accounts[0].codex_auth_path,
            home.join(".codex-work/auth.json")
        );
    }

    #[test]
    fn accounts_dir_is_tilde_expanded_on_load() {
        let f = write_toml(
            r#"
            [anthropic]
            accounts_dir = "~/.config/ai-usagebar/accounts"
            "#,
        );
        let c = Config::load_from(f.path()).unwrap();
        let home = crate::cache::home_dir().unwrap();
        assert_eq!(
            c.anthropic.accounts_dir,
            Some(home.join(".config/ai-usagebar/accounts"))
        );
    }

    #[test]
    fn desktop_profiles_dir_is_tilde_expanded_on_load() {
        let f = write_toml(
            r#"
            [anthropic]
            desktop_profiles_dir = "~/.claude-acc/profiles"
            "#,
        );
        let c = Config::load_from(f.path()).unwrap();
        let home = crate::cache::home_dir().unwrap();
        assert_eq!(
            c.anthropic.desktop_profiles_dir,
            Some(home.join(".claude-acc/profiles"))
        );
    }

    #[test]
    fn the_live_cli_account_is_read_from_the_default_credential_slot() {
        let cfg = AnthropicConfig {
            accounts: vec![
                AnthropicAccount {
                    label: "work".into(),
                    credentials_path: "/tmp/accounts/work/.credentials.json".into(),
                },
                AnthropicAccount {
                    label: "personal".into(),
                    credentials_path: "/tmp/accounts/personal/.credentials.json".into(),
                },
            ],
            ..Default::default()
        };

        let (idle, idle_cache) = cfg.account_target_with("work", Some("personal")).unwrap();
        assert!(
            matches!(&idle, CredsTarget::Named { config_dir, .. }
                if config_dir == std::path::Path::new("/tmp/accounts/work")),
            "{idle:?}"
        );

        // Same label, but it is the login `claude` itself is using: one lineage.
        let (live, live_cache) = cfg.account_target_with("work", Some("work")).unwrap();
        assert!(matches!(live, CredsTarget::Default(_)), "{live:?}");

        // The cache must not move, or a switch would silently orphan the tab's
        // usage history and show "Loading…" until the next fetch.
        assert_eq!(idle_cache.dir(), live_cache.dir());
    }

    #[test]
    fn no_live_cli_account_keeps_every_account_on_its_own_slot() {
        let cfg = AnthropicConfig {
            accounts: vec![AnthropicAccount {
                label: "work".into(),
                credentials_path: "/tmp/accounts/work/.credentials.json".into(),
            }],
            ..Default::default()
        };
        let (target, _) = cfg.account_target_with("work", None).unwrap();
        assert!(matches!(target, CredsTarget::Named { .. }), "{target:?}");
    }

    /// The shipped example, which `make install` puts in
    /// `share/ai-usagebar/config.example.toml`. Repo-relative, so this stays
    /// hermetic — it never touches the user's real config.
    fn config_example() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config.example.toml")
    }

    #[test]
    fn shipped_example_parses_as_a_real_config() {
        // The example is documentation users copy verbatim, but nothing used
        // to parse it — so a renamed section or field could rot there
        // unnoticed, and `deny_unknown_fields` would reject the copy on the
        // user's machine instead of in CI.
        let c = Config::load_from(&config_example()).unwrap();
        assert!(!c.context.enabled);
        assert!(c.is_enabled(VendorId::Anthropic));
        assert!(c.is_enabled(VendorId::Openai));
        assert!(!c.is_enabled(VendorId::AnthropicApi));
        assert!(!c.is_enabled(VendorId::Deepseek));
        assert!(!c.is_enabled(VendorId::Kimi));
        assert!(!c.is_enabled(VendorId::Kilo));
        assert!(!c.is_enabled(VendorId::Novita));
        assert!(!c.is_enabled(VendorId::Moonshot));
        assert!(!c.is_enabled(VendorId::Grok));
        assert!(!c.is_enabled(VendorId::Cursor));
        assert!(!c.is_enabled(VendorId::Minimax));
    }

    #[test]
    fn shipped_example_does_not_advertise_admin_key_env_as_working() {
        // The regression: the example shipped an *uncommented*
        // `admin_key_env = "OPENAI_ADMIN_KEY"`, indistinguishable from a live
        // setting. Nothing reads it, so a user could set it, skip
        // `codex login`, and wait for usage that never arrives.
        let text = std::fs::read_to_string(config_example()).unwrap();
        let live: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|l| l.contains("admin_key_env") && !l.starts_with('#'))
            .collect();
        assert!(
            live.is_empty(),
            "admin_key_env must stay commented out while it is inert: {live:?}"
        );
        // Still documented, though — silently dropping it would leave users
        // who already set it with no explanation of why it does nothing.
        assert!(
            text.contains("admin_key_env") && text.contains("RESERVED"),
            "the example should keep describing admin_key_env as reserved"
        );
    }

    #[test]
    fn admin_key_env_is_accepted_but_changes_nothing() {
        // The field survives because the API-key-only path is still intended.
        // What has to hold today is narrower: setting it loads without error
        // and moves nothing the code actually acts on.
        let f = write_toml(
            r#"
            [openai]
            admin_key_env = "SOME_ADMIN_KEY"
            "#,
        );
        let c = Config::load_from(f.path()).unwrap();
        assert_eq!(c.openai.admin_key_env, "SOME_ADMIN_KEY");
        // Nothing else moved: OpenAI still resolves through Codex OAuth only.
        let default = OpenAiConfig::default();
        assert_eq!(c.openai.enabled, default.enabled);
        assert_eq!(c.openai.codex_auth_path, default.codex_auth_path);
        assert_eq!(c.enabled_vendors(), Config::default().enabled_vendors());
    }

    #[test]
    fn config_example_documents_every_vendor_without_secrets() {
        let raw = std::fs::read_to_string(config_example()).unwrap();
        let cfg = Config::load_from(&config_example()).unwrap();
        // Every vendor the binary can dispatch needs a documented section, or
        // users have no way to discover how to turn it on.
        for id in VendorId::all() {
            let section = id.slug();
            assert!(
                raw.contains(&format!("[{section}]")),
                "config.example.toml has no [{section}] section"
            );
        }

        // The example must not ship anything enabled-by-key-only, and must not
        // carry a real secret.
        assert!(!cfg.anthropic_api.enabled && cfg.anthropic_api.api_key.is_none());
        assert!(!cfg.kilo.enabled && cfg.kilo.api_key.is_none());
        assert!(!cfg.novita.enabled && cfg.novita.api_key.is_none());
        assert!(!cfg.moonshot.enabled && cfg.moonshot.api_key.is_none());
        assert!(!cfg.grok.enabled && cfg.grok.api_key.is_none());
        assert!(!cfg.supergrok.enabled);
        assert_eq!(cfg.supergrok.grok_binary, default_grok_binary());
        assert_eq!(
            cfg.supergrok
                .grok_binary
                .file_name()
                .and_then(|p| p.to_str()),
            Some(if cfg!(windows) { "grok.exe" } else { "grok" })
        );
        assert!(cfg.supergrok.auth_path.is_none());
        assert!(cfg.supergrok.config_path.is_none());
        assert!(!cfg.cursor.enabled && cfg.cursor.db_path.is_none());
        assert!(!cfg.kiro.enabled && cfg.kiro.db_path.is_none());
    }

    #[test]
    fn supergrok_binary_must_not_be_empty() {
        let file = write_toml(
            r#"
            [supergrok]
            enabled = true
            grok_binary = ""
            "#,
        );
        let error = Config::load_from(file.path()).unwrap_err().to_string();
        assert!(error.contains("grok_binary must not be empty"));
    }

    #[test]
    fn supergrok_paths_are_tilde_expanded() {
        let file = write_toml(
            r#"
            [supergrok]
            grok_binary = "~/bin/grok"
            auth_path = "~/.grok/auth.json"
            config_path = "~/.grok/config.toml"
            "#,
        );
        let config = Config::load_from(file.path()).unwrap();
        let home = crate::cache::home_dir().unwrap();
        assert_eq!(config.supergrok.grok_binary, home.join("bin/grok"));
        assert_eq!(
            config.supergrok.auth_path,
            Some(home.join(".grok/auth.json"))
        );
        assert_eq!(
            config.supergrok.config_path,
            Some(home.join(".grok/config.toml"))
        );
    }

    #[test]
    fn kiro_db_path_is_tilde_expanded() {
        let f = write_toml(
            r#"
            [kiro]
            db_path = "~/kiro-data.sqlite3"
            "#,
        );
        let c = Config::load_from(f.path()).unwrap();
        let home = crate::cache::home_dir().unwrap();
        assert_eq!(c.kiro.db_path, Some(home.join("kiro-data.sqlite3")));
    }

    #[test]
    fn kiro_appears_when_enabled() {
        let f = write_toml(
            r#"
            [kiro]
            enabled = true
            "#,
        );
        let c = Config::load_from(f.path()).unwrap();
        assert!(c.is_enabled(VendorId::Kiro));
        assert!(c.enabled_vendors().contains(&VendorId::Kiro));
    }

    #[test]
    fn cursor_db_path_is_tilde_expanded() {
        let f = write_toml(
            r#"
            [cursor]
            db_path = "~/cursor-state.vscdb"
            "#,
        );
        let c = Config::load_from(f.path()).unwrap();
        let home = crate::cache::home_dir().unwrap();
        assert_eq!(c.cursor.db_path, Some(home.join("cursor-state.vscdb")));
    }

    #[test]
    fn cursor_agent_auth_path_is_tilde_expanded() {
        let f = write_toml(
            r#"
            [cursor]
            agent_auth_path = "~/cursor-agent-auth.json"
            "#,
        );
        let c = Config::load_from(f.path()).unwrap();
        let home = crate::cache::home_dir().unwrap();
        assert_eq!(
            c.cursor.agent_auth_path,
            Some(home.join("cursor-agent-auth.json"))
        );
    }

    #[test]
    fn cursor_appears_when_enabled() {
        let f = write_toml(
            r#"
            [cursor]
            enabled = true
            "#,
        );
        let c = Config::load_from(f.path()).unwrap();
        assert!(c.is_enabled(VendorId::Cursor));
        assert!(c.enabled_vendors().contains(&VendorId::Cursor));
    }

    #[test]
    fn add_account_appends_and_preserves_existing() {
        let mut doc: toml_edit::DocumentMut = r#"
# keep me
[anthropic]
enabled = true

[[anthropic.accounts]]
label = "personal"
credentials_path = "~/.config/ai-usagebar/accounts/personal/.credentials.json"
"#
        .parse()
        .unwrap();
        add_anthropic_account_to_doc(
            &mut doc,
            "work",
            "~/.config/ai-usagebar/accounts/work/.credentials.json",
        )
        .unwrap();
        let rendered = doc.to_string();
        assert!(rendered.contains("# keep me"), "comment must survive");
        // Round-trips through the real loader with both accounts intact and ordered.
        let f = write_toml(&rendered);
        let c = Config::load_from(f.path()).unwrap();
        let labels: Vec<&str> = c
            .anthropic
            .accounts
            .iter()
            .map(|a| a.label.as_str())
            .collect();
        assert_eq!(labels, vec!["personal", "work"]);
    }

    #[test]
    fn add_account_to_empty_doc_is_loadable() {
        let mut doc = toml_edit::DocumentMut::new();
        add_anthropic_account_to_doc(&mut doc, "solo", "~/x/.credentials.json").unwrap();
        let f = write_toml(&doc.to_string());
        let c = Config::load_from(f.path()).unwrap();
        assert_eq!(c.anthropic.accounts.len(), 1);
        assert_eq!(c.anthropic.accounts[0].label, "solo");
    }

    #[test]
    fn add_account_rejects_duplicate_label() {
        let mut doc: toml_edit::DocumentMut = r#"
[[anthropic.accounts]]
label = "work"
credentials_path = "~/w/.credentials.json"
"#
        .parse()
        .unwrap();
        assert!(
            add_anthropic_account_to_doc(&mut doc, "work", "~/other/.credentials.json").is_err(),
            "a duplicate label must be rejected, not appended"
        );
    }

    #[test]
    fn add_account_rejects_bad_label() {
        let mut doc = toml_edit::DocumentMut::new();
        assert!(add_anthropic_account_to_doc(&mut doc, "a/b", "~/x/.credentials.json").is_err());
        assert!(add_anthropic_account_to_doc(&mut doc, "", "~/x/.credentials.json").is_err());
    }

    #[test]
    fn tildify_collapses_home_only() {
        let home = Path::new("/Users/me");
        assert_eq!(tildify(&home.join("a/b"), home), "~/a/b");
        assert_eq!(tildify(Path::new("/etc/hosts"), home), "/etc/hosts");
    }

    #[test]
    fn default_account_credentials_path_nests_under_config_dir() {
        let cfg = Path::new("/home/u/.config/ai-usagebar/config.toml");
        assert_eq!(
            default_account_credentials_path(cfg, "work"),
            Path::new("/home/u/.config/ai-usagebar/accounts/work/.credentials.json"),
        );
    }
    // ----- [[custom]] providers -----

    const CUSTOM_BLOCK: &str = r#"
[[custom]]
id = "mytool"
name = "My Tool"
short_name = "myt"
enabled = true
url = "https://api.example.test/v1/usage"
api_key_env = "MYTOOL_API_KEY"
auth_header = "Authorization"
auth_scheme = "Bearer"
plan = "Pro"
cache_ttl_secs = 120
[custom.headers]
X-Org = "org_1"
[[custom.metrics]]
label = "Requests"
used = "/requests/used"
limit = "/requests/limit"
resets_at = "/requests/reset"
window_secs = 3600
[[custom.texts]]
label = "Tier"
value = "/tier"
"#;

    fn custom_with(from: &str, to: &str) -> String {
        assert!(CUSTOM_BLOCK.contains(from), "fixture has no {from:?}");
        CUSTOM_BLOCK.replace(from, to)
    }

    fn custom_error(toml: &str) -> String {
        Config::load_from(write_toml(toml).path())
            .unwrap_err()
            .to_string()
    }

    fn assert_custom_rejected(toml: &str, needle: &str) {
        let msg = custom_error(toml);
        assert!(msg.contains(needle), "expected {needle:?} in: {msg}");
        assert!(
            msg.contains("[[custom]]"),
            "the error must locate the section: {msg}"
        );
    }

    #[test]
    fn custom_block_parses_every_field() {
        let config = Config::load_from(write_toml(CUSTOM_BLOCK).path()).unwrap();
        assert_eq!(config.custom.len(), 1);
        let c = &config.custom[0];
        assert_eq!(c.id, "mytool");
        assert_eq!(c.name, "My Tool");
        assert_eq!(c.short_name, "myt");
        assert!(c.enabled);
        assert_eq!(c.url, "https://api.example.test/v1/usage");
        assert!(!c.allow_http);
        assert_eq!(c.api_key_env, "MYTOOL_API_KEY");
        assert_eq!(c.api_key, None);
        assert_eq!(c.auth_header, "Authorization");
        assert_eq!(c.auth_scheme, "Bearer");
        assert_eq!(c.headers.get("X-Org").map(String::as_str), Some("org_1"));
        assert_eq!(c.plan.as_deref(), Some("Pro"));
        assert_eq!(c.plan_path, None);
        assert_eq!(c.cache_ttl(), std::time::Duration::from_secs(120));
        assert_eq!(c.metrics.len(), 1);
        assert_eq!(c.metrics[0].label, "Requests");
        assert_eq!(c.metrics[0].used.as_deref(), Some("/requests/used"));
        assert_eq!(c.metrics[0].limit.as_deref(), Some("/requests/limit"));
        assert_eq!(c.metrics[0].percent, None);
        assert_eq!(c.metrics[0].resets_at.as_deref(), Some("/requests/reset"));
        assert_eq!(c.metrics[0].window_secs, Some(3600));
        assert_eq!(c.texts.len(), 1);
        assert_eq!(c.texts[0].label, "Tier");
        assert_eq!(c.texts[0].value, "/tier");
        assert_eq!(c.section_label(), r#"[[custom]] id = "mytool""#);
    }

    #[test]
    fn custom_defaults_are_the_documented_ones_and_name_falls_back_to_id() {
        let config: Config = toml::from_str(
            r#"
            [[custom]]
            id = "bare"
            short_name = "bre"
            url = "https://example.test/u"
            [[custom.metrics]]
            label = "Q"
            percent = "/pct"
            "#,
        )
        .unwrap();
        let c = &config.custom[0];
        assert_eq!(c.name, "bare", "name must default to id on a plain parse");
        assert!(!c.enabled);
        assert!(!c.allow_http);
        assert_eq!(c.api_key_env, "");
        assert_eq!(c.auth_header, "Authorization");
        assert_eq!(c.auth_scheme, "Bearer");
        assert_eq!(c.cache_ttl_secs, 60);
        assert!(config.validate().is_ok());
        assert!(Config::default().custom.is_empty());
    }

    #[test]
    fn custom_rejects_a_malformed_id() {
        let long = "a".repeat(33);
        for id in ["", "My Tool", "-lead", "UPPER", long.as_str()] {
            let msg = custom_error(&custom_with(r#"id = "mytool""#, &format!("id = {id:?}")));
            assert!(msg.contains("[[custom]] entry #1"), "{id:?}: {msg}");
            assert!(msg.contains("must match"), "{id:?}: {msg}");
        }
    }

    #[test]
    fn custom_rejects_a_builtin_slug_as_id() {
        assert_custom_rejected(
            &custom_with(r#"id = "mytool""#, r#"id = "deepseek""#),
            "is a built-in vendor",
        );
        assert_custom_rejected(
            &custom_with(r#"id = "mytool""#, r#"id = "opencode-go""#),
            "is a built-in vendor",
        );
    }

    #[test]
    fn custom_rejects_duplicate_ids() {
        let twice = format!(
            "{}{}",
            CUSTOM_BLOCK,
            custom_with(r#"short_name = "myt""#, r#"short_name = "myu""#)
        );
        assert_custom_rejected(&twice, "duplicate id");
    }

    #[test]
    fn custom_rejects_a_name_over_48_chars() {
        let long = "n".repeat(49);
        assert_custom_rejected(
            &custom_with(r#"name = "My Tool""#, &format!("name = {long:?}")),
            "name must be 1 to 48 characters",
        );
    }

    #[test]
    fn custom_rejects_a_short_name_that_is_not_three_lowercase_letters() {
        for short in ["my", "myto", "MYT", "m1t"] {
            assert_custom_rejected(
                &custom_with(r#"short_name = "myt""#, &format!("short_name = {short:?}")),
                "exactly 3 lowercase ASCII letters",
            );
        }
    }

    #[test]
    fn custom_rejects_a_short_name_taken_by_a_builtin_or_another_entry() {
        assert_custom_rejected(
            &custom_with(r#"short_name = "myt""#, r#"short_name = "dsk""#),
            "already used by a built-in vendor",
        );
        let twice = format!(
            "{}{}",
            CUSTOM_BLOCK,
            custom_with(r#"id = "mytool""#, r#"id = "othertool""#)
        );
        assert_custom_rejected(&twice, "already used by a built-in vendor");
    }

    #[test]
    fn custom_rejects_http_unless_allowed() {
        let plain = custom_with(
            r#"url = "https://api.example.test/v1/usage""#,
            r#"url = "http://localhost:8080/usage""#,
        );
        assert_custom_rejected(&plain, "url must use https://");
        let allowed = plain.replace(
            r#"url = "http://localhost:8080/usage""#,
            "url = \"http://localhost:8080/usage\"\nallow_http = true",
        );
        assert!(
            Config::load_from(write_toml(&allowed).path()).is_ok(),
            "allow_http must permit http://"
        );
    }

    #[test]
    fn custom_rejects_a_url_with_userinfo_or_a_bad_scheme_or_garbage() {
        assert_custom_rejected(
            &custom_with(
                r#"url = "https://api.example.test/v1/usage""#,
                r#"url = "https://user:pw@api.example.test/v1/usage""#,
            ),
            "must not carry credentials",
        );
        assert_custom_rejected(
            &custom_with(
                r#"url = "https://api.example.test/v1/usage""#,
                r#"url = "not a url""#,
            ),
            "is not a valid URL",
        );
        assert_custom_rejected(
            &custom_with(
                r#"url = "https://api.example.test/v1/usage""#,
                r#"url = "ftp://api.example.test/v1/usage""#,
            ),
            "is not http or https",
        );
    }

    #[test]
    fn custom_rejects_an_invalid_api_key_env() {
        assert_custom_rejected(
            &custom_with(
                r#"api_key_env = "MYTOOL_API_KEY""#,
                r#"api_key_env = "1BAD-NAME""#,
            ),
            "is not a valid environment variable name",
        );
        let none = custom_with(r#"api_key_env = "MYTOOL_API_KEY""#, r#"api_key_env = """#);
        assert!(
            Config::load_from(write_toml(&none).path()).is_ok(),
            "an empty api_key_env means inline-only and is valid"
        );
    }

    #[test]
    fn custom_rejects_an_invalid_auth_header_name() {
        assert_custom_rejected(
            &custom_with(
                r#"auth_header = "Authorization""#,
                r#"auth_header = "X Api Key""#,
            ),
            "auth_header \"X Api Key\" is not a valid HTTP header name",
        );
    }

    #[test]
    fn custom_rejects_a_control_char_in_auth_scheme() {
        assert_custom_rejected(
            &custom_with(
                r#"auth_scheme = "Bearer""#,
                "auth_scheme = \"Bearer\\u0007\"",
            ),
            "auth_scheme contains characters that are not valid",
        );
        let bare = custom_with(r#"auth_scheme = "Bearer""#, r#"auth_scheme = """#);
        assert!(
            Config::load_from(write_toml(&bare).path()).is_ok(),
            "an empty scheme (bare key) is valid"
        );
    }

    #[test]
    fn custom_rejects_a_bad_extra_header() {
        assert_custom_rejected(
            &custom_with(r#"X-Org = "org_1""#, r#"authorization = "Bearer other""#),
            "headers must not repeat auth_header",
        );
        assert_custom_rejected(
            &custom_with(r#"X-Org = "org_1""#, r#""X Org" = "org_1""#),
            "is not a valid HTTP header name",
        );
        assert_custom_rejected(
            &custom_with(r#"X-Org = "org_1""#, "X-Org = \"org\\u0001\""),
            "has a value that is not valid in an HTTP header",
        );
    }

    #[test]
    fn custom_rejects_cache_ttl_outside_10_to_3600() {
        for ttl in ["9", "3601"] {
            assert_custom_rejected(
                &custom_with("cache_ttl_secs = 120", &format!("cache_ttl_secs = {ttl}")),
                "cache_ttl_secs must be between 10 and 3600",
            );
        }
    }

    #[test]
    fn custom_rejects_an_entry_with_no_metrics_or_texts() {
        let toml = r#"
[[custom]]
id = "empty"
short_name = "emp"
url = "https://example.test/u"
"#;
        assert_custom_rejected(toml, "at least one [[custom.metrics]] or [[custom.texts]]");
    }

    #[test]
    fn custom_rejects_a_metric_mixing_percent_with_used_or_limit() {
        assert_custom_rejected(
            &custom_with(
                r#"limit = "/requests/limit""#,
                "limit = \"/requests/limit\"\npercent = \"/requests/pct\"",
            ),
            "must set `percent`, or both `used` and `limit`",
        );
        assert_custom_rejected(
            &custom_with("limit = \"/requests/limit\"\n", ""),
            "must set `percent`, or both `used` and `limit`",
        );
    }

    #[test]
    fn custom_rejects_a_pointer_without_a_leading_slash() {
        assert_custom_rejected(
            &custom_with(r#"used = "/requests/used""#, r#"used = "requests.used""#),
            "used \"requests.used\" must be an RFC 6901 JSON Pointer",
        );
        assert_custom_rejected(
            &custom_with(r#"value = "/tier""#, r#"value = "tier""#),
            "value \"tier\" must be an RFC 6901 JSON Pointer",
        );
        assert_custom_rejected(
            &custom_with(r#"plan = "Pro""#, r#"plan_path = "plan""#),
            "plan_path \"plan\" must be an RFC 6901 JSON Pointer",
        );
        assert_custom_rejected(
            &custom_with(
                r#"resets_at = "/requests/reset""#,
                "resets_at = \"/re\\u001bset\"",
            ),
            "resets_at",
        );
    }

    #[test]
    fn custom_rejects_a_label_outside_1_to_64_chars() {
        let long = "l".repeat(65);
        assert_custom_rejected(
            &custom_with(r#"label = "Requests""#, &format!("label = {long:?}")),
            "metric label",
        );
        assert_custom_rejected(
            &custom_with(r#"label = "Tier""#, r#"label = """#),
            "text label \"\" must be 1 to 64 characters",
        );
    }

    #[test]
    fn custom_rejects_window_secs_under_60() {
        assert_custom_rejected(
            &custom_with("window_secs = 3600", "window_secs = 59"),
            "window_secs must be at least 60",
        );
    }

    #[test]
    fn custom_rejects_duplicate_metric_and_text_labels() {
        let metric_twice = custom_with(
            "window_secs = 3600\n",
            "window_secs = 3600\n[[custom.metrics]]\nlabel = \"Requests\"\npercent = \"/pct\"\n",
        );
        assert_custom_rejected(&metric_twice, "duplicate metric label \"Requests\"");
        let text_twice =
            format!("{CUSTOM_BLOCK}[[custom.texts]]\nlabel = \"Tier\"\nvalue = \"/other\"\n");
        assert_custom_rejected(&text_twice, "duplicate text label \"Tier\"");
    }

    #[test]
    fn enabled_custom_and_custom_by_id_select_entries() {
        let two = format!(
            "{}{}",
            CUSTOM_BLOCK,
            custom_with(r#"id = "mytool""#, r#"id = "off""#)
                .replace(r#"short_name = "myt""#, r#"short_name = "off""#)
                .replace("enabled = true", "enabled = false")
        );
        let config = Config::load_from(write_toml(&two).path()).unwrap();
        let enabled: Vec<&str> = config.enabled_custom().map(|c| c.id.as_str()).collect();
        assert_eq!(enabled, ["mytool"]);
        assert_eq!(
            config.custom_by_id("off").map(|c| c.name.as_str()),
            Some("My Tool")
        );
        assert!(config.custom_by_id("nope").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn has_inline_secrets_sees_a_custom_inline_key() {
        let without: Config = toml::from_str(CUSTOM_BLOCK).unwrap();
        assert!(!without.has_inline_secrets());
        let with: Config = toml::from_str(&custom_with(
            r#"api_key_env = "MYTOOL_API_KEY""#,
            "api_key_env = \"MYTOOL_API_KEY\"\napi_key = \"sk-inline\"",
        ))
        .unwrap();
        assert!(with.has_inline_secrets());
    }

    #[test]
    fn custom_resolve_api_key_prefers_env_then_inline_then_errors_without_the_key() {
        let var = "AI_USAGEBAR_CUSTOM_TEST_KEY_51C2";
        let mut spec = CustomProviderConfig {
            id: "mytool".into(),
            api_key_env: var.into(),
            api_key: Some("sk-inline-secret".into()),
            ..CustomProviderConfig::default()
        };
        unsafe { std::env::set_var(var, "sk-env-secret") };
        let from_env = spec.resolve_api_key();
        unsafe { std::env::remove_var(var) };
        assert_eq!(from_env.unwrap(), "sk-env-secret");

        assert_eq!(spec.resolve_api_key().unwrap(), "sk-inline-secret");

        spec.api_key = Some(String::new());
        let err = spec.resolve_api_key().unwrap_err();
        assert!(matches!(err, AppError::Credentials(_)), "{err:?}");
        let msg = err.to_string();
        assert!(msg.contains(r#"[[custom]] id = "mytool""#), "{msg}");
        assert!(msg.contains(var), "{msg}");
        assert!(!msg.contains("secret"), "{msg}");

        spec.api_key_env = String::new();
        let msg = spec.resolve_api_key().unwrap_err().to_string();
        assert!(msg.contains("set `api_key`"), "{msg}");
    }

    #[test]
    fn loading_a_config_registers_custom_env_vars_for_scrubbing() {
        let var = "AI_USAGEBAR_CUSTOM_SCRUB_TEST_9B1D";
        assert!(!crate::vendor::vendor_secret_env_vars_to_remove(&[]).contains(&var));
        let file = write_toml(&custom_with("MYTOOL_API_KEY", var));
        Config::load_from(file.path()).unwrap();
        assert!(
            crate::vendor::vendor_secret_env_vars_to_remove(&[]).contains(&var),
            "a custom provider's env var must be scrubbed from subprocesses"
        );
    }

    /// `VendorId::config_section` is what every by-name config writer uses;
    /// this proves each section name is one the parser actually recognizes
    /// (the `deny_unknown_fields` on `Config` makes a misspelling fail loudly)
    /// and lands on that vendor's `enabled` switch.
    #[test]
    fn every_config_section_parses_to_its_vendors_enabled_switch() {
        for vendor in VendorId::all() {
            let text = format!(
                "[{}]
enabled = true
",
                vendor.config_section()
            );
            let config: Config = toml::from_str(&text)
                .unwrap_or_else(|e| panic!("{}: {e}", vendor.config_section()));
            assert!(config.is_enabled(*vendor), "{}", vendor.config_section());
            let others = VendorId::all()
                .iter()
                .filter(|other| *other != vendor && config.is_enabled(**other))
                .count();
            assert_eq!(
                others,
                Config::default().enabled_vendors().len()
                    - usize::from(Config::default().is_enabled(*vendor)),
                "[{}] enabled a different vendor",
                vendor.config_section()
            );
        }
    }

    #[test]
    fn tray_section_parses_and_defaults_to_notify() {
        let file = write_toml("[tray]\nshortcut = \"Ctrl+Shift+U\"\nupdates = \"auto\"\n");
        let config = Config::load_from(file.path()).unwrap();
        assert_eq!(config.tray.shortcut.as_deref(), Some("Ctrl+Shift+U"));
        assert_eq!(config.tray.updates(), UpdateMode::Auto);

        let empty = Config::load_from(write_toml("[ui]\n").path()).unwrap();
        assert_eq!(empty.tray, TrayConfig::default());
        assert_eq!(empty.tray.updates(), UpdateMode::Notify);
        assert_eq!(UpdateMode::parse(" Off "), Some(UpdateMode::Off));
        assert_eq!(UpdateMode::parse("weekly"), None);
        assert_eq!(UpdateMode::Auto.as_str(), "auto");
    }

    #[test]
    fn tray_section_rejects_a_misspelled_mode() {
        let file = write_toml("[tray]\nupdates = \"sometimes\"\n");
        assert!(Config::load_from(file.path()).is_err());
    }

    #[test]
    fn tray_refresh_minutes_defaults_to_five_and_parses() {
        let empty = Config::load_from(write_toml("[ui]\n").path()).unwrap();
        assert_eq!(empty.tray.refresh_minutes, None);
        assert_eq!(empty.tray.refresh_minutes(), 5);

        let file = write_toml("[tray]\nrefresh_minutes = 10\n");
        let config = Config::load_from(file.path()).unwrap();
        assert_eq!(config.tray.refresh_minutes(), 10);
    }

    #[test]
    fn tray_refresh_minutes_rejects_values_outside_the_menu() {
        for minutes in ["3", "0"] {
            let file = write_toml(&format!("[tray]\nrefresh_minutes = {minutes}\n"));
            let error = Config::load_from(file.path()).unwrap_err().to_string();
            assert!(error.contains("[tray] refresh_minutes"), "{error}");
            assert!(error.contains("1, 5 or 10"), "{error}");
        }
    }

    #[test]
    fn set_tray_value_writes_refresh_minutes_as_an_integer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[tray]\nrefresh_minutes = 5 # mine\n").unwrap();

        set_tray_value(&path, "refresh_minutes", Some(10i64.into())).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, "[tray]\nrefresh_minutes = 10 # mine\n");
        assert_eq!(Config::load_from(&path).unwrap().tray.refresh_minutes(), 10);

        set_tray_value(&path, "refresh_minutes", None).unwrap();
        assert_eq!(Config::load_from(&path).unwrap().tray.refresh_minutes(), 5);
    }

    #[test]
    fn set_tray_value_creates_replaces_and_removes_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\n# primary = \"anthropic\"\n").unwrap();

        set_tray_value(&path, "shortcut", Some("Ctrl+Shift+U".into())).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# primary = \"anthropic\""), "{text}");
        assert!(
            text.contains("[tray]\nshortcut = \"Ctrl+Shift+U\""),
            "{text}"
        );

        set_tray_value(&path, "shortcut", Some("Alt+F5".into())).unwrap();
        set_tray_value(&path, "updates", Some("off".into())).unwrap();
        let config = Config::load_from(&path).unwrap();
        assert_eq!(config.tray.shortcut.as_deref(), Some("Alt+F5"));
        assert_eq!(config.tray.updates(), UpdateMode::Off);

        set_tray_value(&path, "shortcut", None).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("shortcut"), "{text}");
        assert!(text.contains("updates = \"off\""), "{text}");

        // Idempotent removal does not rewrite the file.
        let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
        set_tray_value(&path, "shortcut", None).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), mtime);
    }

    #[test]
    fn set_value_keeps_the_trailing_comment_when_replacing() {
        let mut doc: toml_edit::DocumentMut =
            "[tray]\nshortcut = \"Ctrl+U\" # mine\n".parse().unwrap();
        set_value(&mut doc, "tray", "shortcut", Some("Alt+U".into())).unwrap();
        assert_eq!(doc.to_string(), "[tray]\nshortcut = \"Alt+U\" # mine\n");
    }

    #[test]
    fn enable_vendors_in_creates_a_missing_config() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("sub").join("config.toml");

        enable_vendors_in(&path, &[VendorId::Grok]).unwrap();

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[grok]
enabled = true
"
        );
        assert!(Config::load_from(&path).unwrap().is_enabled(VendorId::Grok));
    }

    #[test]
    fn enable_vendors_in_keeps_comments_and_appends_the_new_section() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let original = "# my settings
[zai]
api_key = \"x\" # keep
enabled = false
";
        std::fs::write(&path, original).unwrap();

        enable_vendors_in(&path, &[VendorId::Grok, VendorId::OpenCodeGo]).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.starts_with(
                "# my settings
"
            ),
            "{text}"
        );
        assert!(
            text.contains(
                "api_key = \"x\" # keep
"
            ),
            "{text}"
        );
        assert!(
            text.contains(
                "[grok]
enabled = true
"
            ),
            "{text}"
        );
        assert!(
            text.contains(
                "[opencode-go]
enabled = true
"
            ),
            "{text}"
        );
        let config = Config::load_from(&path).unwrap();
        assert!(
            !config.is_enabled(VendorId::Zai),
            "never widens to false, never flips others"
        );
        assert!(config.is_enabled(VendorId::Grok));
        assert!(config.is_enabled(VendorId::OpenCodeGo));
    }

    #[test]
    fn enable_vendors_in_leaves_an_explicit_false_alone() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let original = "[grok]
enabled = false # off
api_key = \"k\"
";
        std::fs::write(&path, original).unwrap();

        // `enabled = false` in the file is the user having said no. Only the
        // automatic path goes through here — the Settings overlay writes with
        // `set_bool` — so nothing a person does by hand is blocked by this.
        let written = enable_vendors_in(&path, &[VendorId::Grok]).unwrap();

        assert!(written.is_empty(), "{written:?}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            original,
            "the file must not be rewritten at all"
        );
    }

    #[test]
    fn enable_vendors_in_adds_the_switch_when_the_config_never_mentioned_it() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[grok]\napi_key = \"k\"\n").unwrap();

        let written = enable_vendors_in(&path, &[VendorId::Grok]).unwrap();

        assert_eq!(written, vec![VendorId::Grok]);
        assert!(Config::load_from(&path).unwrap().is_enabled(VendorId::Grok));
    }

    #[test]
    fn enable_vendors_in_is_textually_idempotent() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        let original = "[grok]
enabled = true

# trailing
";
        std::fs::write(&path, original).unwrap();
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();

        enable_vendors_in(&path, &[VendorId::Grok]).unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            before,
            "an unchanged document must not be rewritten"
        );
    }

    #[test]
    fn enable_vendors_in_with_nothing_to_enable_leaves_a_missing_file_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");

        enable_vendors_in(&path, &[]).unwrap();

        assert!(!path.exists());
    }
}
