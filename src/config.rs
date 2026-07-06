//! Config file at `~/.config/ai-usagebar/config.toml`.
//!
//! Layout:
//! ```toml
//! [anthropic] enabled = true
//! [openai]    enabled = true   # Codex OAuth from ~/.codex/auth.json
//! [zai]       enabled = true
//! [openrouter] enabled = true
//! ```
//!
//! Every field is optional with sensible defaults — missing config file is
//! treated as "use defaults". API keys are read from env vars (the relevant
//! `*_api_key_env` field lets the user override which env var name).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::anthropic::creds::CredsTarget;
use crate::cache::Cache;
use crate::error::{AppError, Result};
use crate::vendor::VendorId;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub ui: UiConfig,
    pub anthropic: AnthropicConfig,
    pub openai: OpenAiConfig,
    pub zai: ZaiConfig,
    pub openrouter: OpenRouterConfig,
    pub deepseek: DeepseekConfig,
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
    /// Optional admin key env var name for the API-key-only fallback path.
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

/// Resolve an API key for a vendor: env var wins, then inline config, then
/// a clear error naming both fields. Used by Z.AI and OpenRouter vendors.
pub fn resolve_api_key(
    vendor_label: &str,
    env_var_name: &str,
    inline: Option<&str>,
) -> crate::error::Result<String> {
    if !env_var_name.is_empty()
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
    Err(crate::error::AppError::Credentials(format!(
        "{vendor_label}: no API key. Either export {env_var_name} or set \
         `api_key` under [{}] in {}.",
        vendor_label.to_lowercase(),
        config_path_hint()
    )))
}

impl Config {
    /// Load from `~/.config/ai-usagebar/config.toml`. Returns defaults if the
    /// file doesn't exist; errors only on actual parse failures.
    pub fn load() -> Result<Self> {
        let Some(path) = default_path() else {
            return Ok(Self::default());
        };
        Self::load_from(&path)
    }

    pub fn load_from(path: &std::path::Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(s) => Ok(toml::from_str(&s)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(AppError::io_at(path, e)),
        }
    }

    pub fn is_enabled(&self, id: VendorId) -> bool {
        match id {
            VendorId::Anthropic => self.anthropic.enabled,
            VendorId::Openai => self.openai.enabled,
            VendorId::Zai => self.zai.enabled,
            VendorId::Openrouter => self.openrouter.enabled,
            VendorId::Deepseek => self.deepseek.enabled,
        }
    }

    pub fn enabled_vendors(&self) -> Vec<VendorId> {
        VendorId::all()
            .iter()
            .copied()
            .filter(|id| self.is_enabled(*id))
            .collect()
    }
}

pub fn default_path() -> Option<PathBuf> {
    let proj = directories::ProjectDirs::from("", "", "ai-usagebar")?;
    Some(proj.config_dir().join("config.toml"))
}

/// Resolved `config.toml` path as a string for user-facing messages. Uses the
/// platform's config dir (`directories::ProjectDirs`), so it reads correctly on
/// Linux, macOS, and Windows instead of hard-coding the Unix `~/.config` path.
/// Falls back to the bare filename if the path can't be resolved.
pub fn config_path_hint() -> String {
    default_path()
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
    fn defaults_enable_all_vendors() {
        let c = Config::default();
        assert!(c.is_enabled(VendorId::Anthropic));
        assert!(c.is_enabled(VendorId::Openai));
        assert!(c.is_enabled(VendorId::Zai));
        assert!(c.is_enabled(VendorId::Openrouter));
        // DeepSeek requires an explicit API key, so it defaults to disabled.
        assert!(!c.is_enabled(VendorId::Deepseek));
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
                assert!(msg.contains(var), "error should name env var: {msg}");
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
        // DeepSeek is disabled by default (requires explicit API key config),
        // so it is absent from the enabled list unless the user enables it.
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
}
