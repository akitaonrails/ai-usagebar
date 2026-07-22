//! Config file at `~/.config/ai-usagebar/config.toml`.
//!
//! Layout:
//! ```toml
//! [anthropic]  enabled = true
//! [openai]     enabled = true   # Codex OAuth from ~/.codex/auth.json
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
use std::path::PathBuf;

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
    pub zai: ZaiConfig,
    pub openrouter: OpenRouterConfig,
    pub deepseek: DeepseekConfig,
    pub kimi: KimiConfig,
    pub kilo: KiloConfig,
    pub novita: NovitaConfig,
    pub moonshot: MoonshotConfig,
    pub grok: GrokConfig,
    pub antigravity: AntigravityConfig,
}

/// UI / dispatch preferences. Currently just `primary` — which vendor the
/// widget shows when `--vendor` is omitted, and which TUI tab is selected
/// at startup.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct UiConfig {
    /// `None` → fall back to anthropic for backward compatibility.
    pub primary: Option<VendorId>,
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
}

impl Default for AnthropicConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            credentials_path: None,
            accounts: Vec::new(),
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
/// credentials_path = "~/.config/ai-usagebar/accounts/work.json"
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

impl AnthropicConfig {
    /// Find a configured extra account by label, or error listing the known
    /// labels so a typo fails loudly instead of silently hitting the default.
    pub fn account(&self, label: &str) -> Result<&AnthropicAccount> {
        validate_account_label(label)?;
        self.accounts
            .iter()
            .find(|a| a.label == label)
            .ok_or_else(|| {
                let known: Vec<&str> = self.accounts.iter().map(|a| a.label.as_str()).collect();
                AppError::Credentials(format!(
                    "anthropic account {label:?} not found in [[anthropic.accounts]]; \
                     known labels: {known:?}"
                ))
            })
    }

    /// Resolve a named account to the credentials target + isolated cache it
    /// fetches through: a strict [`CredsTarget::Explicit`] on the account's file
    /// (never the Keychain — issue #15) and an `anthropic/<label>` cache subdir.
    /// Shared by the widget (`--account`) and the TUI's per-account tab (#14,
    /// #17) so both resolve accounts identically; the widget layers its
    /// `--cache-dir` override on top of the cache returned here.
    pub fn account_target(&self, label: &str) -> Result<(CredsTarget, Cache)> {
        let account = self.account(label)?;
        Ok((
            CredsTarget::Explicit(account.credentials_path.clone()),
            Cache::for_vendor_account("anthropic", label)?,
        ))
    }
}

/// The label doubles as a cache subdirectory name
/// (`~/.cache/ai-usagebar/anthropic/<label>/`), which nests inside the default
/// account's cache dir — so path separators or dot-dirs would escape or
/// collide with the cache layout (`usage.json`, `.stale`, …). Reject anything
/// that isn't a plain single-segment name.
fn validate_account_label(label: &str) -> Result<()> {
    let bad = label.is_empty()
        || label == "."
        || label == ".."
        || label.contains(['/', '\\'])
        || label == "usage.json";
    if bad {
        return Err(AppError::Credentials(format!(
            "invalid anthropic account label {label:?}: must be a non-empty name \
             without path separators (it becomes a cache subdirectory)"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct OpenAiConfig {
    pub enabled: bool,
    /// Override the Codex auth file path (defaults to `~/.codex/auth.json`).
    pub codex_auth_path: Option<PathBuf>,
    /// Reserved, and inert: names the env var an API-key-only path *would*
    /// read (admin key → `/v1/organization/costs`). Nothing consumes it —
    /// OpenAI usage comes solely from Codex OAuth. Kept because that path is
    /// still intended, not for back-compat: `[openai]` doesn't deny unknown
    /// fields, so an existing `admin_key_env` would load either way. See
    /// `config.example.toml`, which ships it commented out so nobody sets it
    /// expecting an effect.
    pub admin_key_env: String,
}

impl Default for OpenAiConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            codex_auth_path: None,
            admin_key_env: "OPENAI_ADMIN_KEY".to_string(),
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
    pub api_key_env: String,
    pub api_key: Option<String>,
}

impl Default for OpenRouterConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            api_key_env: "OPENROUTER_API_KEY".to_string(),
            api_key: None,
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
    pub api_key: Option<String>,
}

impl Default for KimiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_key_env: "KIMI_API_KEY".to_string(),
            api_key: None,
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

/// Antigravity reads its quota from whichever local Antigravity product is
/// running, so it needs no credentials — only an on/off switch.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct AntigravityConfig {
    pub enabled: bool,
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
    let valid_env_name = is_valid_env_var_name(env_var_name);
    if valid_env_name
        && let Ok(v) = std::env::var(env_var_name)
        && !v.is_empty()
    {
        return Ok(v);
    }
    if let Some(v) = inline
        && !v.is_empty()
    {
        return Ok(v.to_string());
    }
    let advice = if valid_env_name {
        "set an API key in a valid environment variable or set `api_key`"
    } else {
        "fix the invalid `api_key_env` with a valid environment variable name or set `api_key`"
    };
    Err(crate::error::AppError::Credentials(format!(
        "{vendor_label}: no API key. Either {advice} under [{}] in {}.",
        vendor_label.to_lowercase(),
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
                Ok(config)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(AppError::io_at(path, e)),
        }
    }

    fn expand_paths(&mut self) {
        expand_tilde_opt(&mut self.context.projects_path);
        expand_tilde_opt(&mut self.anthropic.credentials_path);
        expand_tilde_opt(&mut self.openai.codex_auth_path);
        for account in &mut self.anthropic.accounts {
            account.credentials_path = expand_tilde(&account.credentials_path);
        }
    }

    pub fn is_enabled(&self, id: VendorId) -> bool {
        match id {
            VendorId::Anthropic => self.anthropic.enabled,
            VendorId::AnthropicApi => self.anthropic_api.enabled,
            VendorId::Openai => self.openai.enabled,
            VendorId::Zai => self.zai.enabled,
            VendorId::Openrouter => self.openrouter.enabled,
            VendorId::Deepseek => self.deepseek.enabled,
            VendorId::Kimi => self.kimi.enabled,
            VendorId::Kilo => self.kilo.enabled,
            VendorId::Novita => self.novita.enabled,
            VendorId::Moonshot => self.moonshot.enabled,
            VendorId::Grok => self.grok.enabled,
            VendorId::Antigravity => self.antigravity.enabled,
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
        Ok(())
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

    fn write_toml(s: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(s.as_bytes()).unwrap();
        f.flush().unwrap();
        f
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
            VendorId::Deepseek,
            VendorId::Kimi,
            VendorId::Kilo,
            VendorId::Novita,
            VendorId::Moonshot,
            VendorId::Grok,
        ] {
            assert!(!c.is_enabled(opt_in), "{opt_in:?}");
        }
        assert_eq!(c.enabled_vendors().len(), 4);
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
        for bad in ["", ".", "..", "a/b", r"a\b", "usage.json"] {
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
    }
}
