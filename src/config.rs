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

fn is_valid_env_var_name(name: &str) -> bool {
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
        .any(|key| key.is_some_and(|key| !key.is_empty()))
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
/// [`default_path`] stays canonical, but on macOS a file at the documented
/// `~/.config/ai-usagebar/config.toml` is honored when the canonical one does
/// not exist — otherwise everyone who followed the README (and both desktop
/// integrations, which read that path) silently got defaults. The legacy file
/// is never moved or rewritten: it may hold API keys, and relocating a secret
/// behind the user's back is not this tool's business.
pub fn resolved_path() -> Option<PathBuf> {
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

    #[test]
    fn config_path_hint_ends_with_config_toml() {
        // Platform-resolved (Linux/macOS/Windows), but always ends in the
        // config filename — the trailing segment is what messages rely on.
        assert!(config_path_hint().ends_with("config.toml"));
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
}
