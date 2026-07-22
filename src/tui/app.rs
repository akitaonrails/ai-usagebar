//! TUI app state — vendors, tab selection, per-vendor snapshot cache.

use std::time::Duration;

use chrono::Utc;
use reqwest::Client;

use crate::cache::DEFAULT_TTL;
use crate::config::Config;
use crate::error::Result;
use crate::theme::Theme;
use crate::vendor::{VendorId, VendorOutcome};

/// What we display per vendor — raw snapshot + fetch metadata for native
/// panel rendering, or an error message when the fetch failed.
///
/// `Ready` is boxed because the snapshot is much larger than the other two
/// variants (silences `clippy::large_enum_variant`).
#[derive(Debug, Clone)]
pub enum TabState {
    Loading,
    Ready(Box<ReadyTab>),
    Error(String),
}

#[derive(Debug, Clone)]
pub struct ReadyTab {
    pub snapshot: crate::usage::VendorSnapshot,
    pub stale: bool,
    pub last_error: Option<(u16, String)>,
    /// Absolute moment the cache was written (i.e. the API response landed).
    /// Snapshotted once at TabState build time so the rendered "Updated …"
    /// timestamp stays stable across redraws instead of drifting with the
    /// passing wall clock.
    pub fetched_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Identity of one TUI tab. Usually a whole vendor; for Anthropic it can also
/// name a specific configured account (issues #14 / #17). `account: None` is a
/// plain vendor tab — the default Claude account, or any non-Anthropic vendor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabId {
    pub vendor: VendorId,
    pub account: Option<String>,
}

impl TabId {
    /// A plain vendor tab (default account for Anthropic).
    pub fn vendor(vendor: VendorId) -> Self {
        Self {
            vendor,
            account: None,
        }
    }

    /// A named Anthropic account tab (`[[anthropic.accounts]]` label).
    pub fn account(label: impl Into<String>) -> Self {
        Self {
            vendor: VendorId::Anthropic,
            account: Some(label.into()),
        }
    }
}

/// Expand enabled vendors into the tab list. Anthropic yields its default
/// account tab followed by one tab per `[[anthropic.accounts]]` entry, in
/// config order; every other vendor is a single tab. With no extra accounts
/// configured the result equals `config.enabled_vendors()` — identical tab set
/// and order to before (issue #14/#17 back-compat).
pub fn tabs_from_config(config: &Config) -> Vec<TabId> {
    let mut tabs = Vec::new();
    for vendor in config.enabled_vendors() {
        tabs.push(TabId::vendor(vendor));
        if vendor == VendorId::Anthropic {
            for acct in &config.anthropic.accounts {
                tabs.push(TabId::account(acct.label.clone()));
            }
        }
    }
    tabs
}

#[derive(Debug)]
pub struct App {
    pub tabs_meta: Vec<TabId>,
    pub active: usize,
    pub tabs: Vec<TabState>,
    /// Monotonically increasing identity for a complete tab-set replacement.
    /// Background fetches carry this with their tab identity so results from a
    /// previous Settings reload cannot land in a new tab at the old index.
    pub tab_generation: u64,
    pub theme: Theme,
    pub quit: bool,
    /// When `Some`, the Settings overlay is open and consuming key events.
    pub settings: Option<crate::tui::settings::SettingsState>,
    /// Local context monitoring is separately opt-in and never changes the
    /// vendor tab set.
    pub context_enabled: bool,
    /// Monotonic across overlay close/reopen cycles so an old detached scan
    /// can never share the new overlay's first generation number.
    pub context_generation: u64,
    /// When `Some`, the local Claude Code context overlay owns keyboard input.
    pub context: Option<crate::tui::context::ContextState>,
}

impl App {
    pub fn new(tabs_meta: Vec<TabId>) -> Self {
        // Production: resolve the palette from the environment (Omarchy theme
        // if present, else One Dark).
        Self::with_theme(tabs_meta, Theme::default().merged_with_omarchy())
    }

    /// Like [`App::new`] but with an explicit theme. Lets tests build an `App`
    /// without reading the real Omarchy theme file
    /// (`$HOME/.config/omarchy/current/theme/colors.toml`) — `new` resolves
    /// that path and the `$HOME` env var via `merged_with_omarchy`, which is
    /// not hermetic. Production code uses `new`/`new_with_primary`.
    pub fn with_theme(tabs_meta: Vec<TabId>, theme: Theme) -> Self {
        let n = tabs_meta.len();
        Self {
            tabs_meta,
            active: 0,
            tabs: vec![TabState::Loading; n],
            tab_generation: 0,
            theme,
            quit: false,
            settings: None,
            context_enabled: false,
            context_generation: 0,
            context: None,
        }
    }

    /// Construct with an initial active tab — usually `[ui] primary` from
    /// config. Silently falls through to index 0 if the requested vendor
    /// isn't present (e.g. it was disabled).
    pub fn new_with_primary(tabs_meta: Vec<TabId>, primary: Option<VendorId>) -> Self {
        let mut app = Self::new(tabs_meta);
        app.select_primary(primary);
        app
    }

    pub fn active_tab_id(&self) -> Option<&TabId> {
        self.tabs_meta.get(self.active)
    }

    pub fn active_vendor(&self) -> Option<VendorId> {
        self.tabs_meta.get(self.active).map(|t| t.vendor)
    }

    /// Replace the tab set — used after a Settings save reloads config, so
    /// tabs added or removed in `config.toml` while the TUI is open (e.g. a
    /// new `[[anthropic.accounts]]` entry) appear without a restart. Every
    /// tab resets to `Loading` (the caller re-spawns fetches) and the
    /// selection is clamped in case the list shrank.
    pub fn set_tabs(&mut self, tabs_meta: Vec<TabId>) {
        self.tab_generation = self.tab_generation.wrapping_add(1);
        self.active = self.active.min(tabs_meta.len().saturating_sub(1));
        self.tabs = vec![TabState::Loading; tabs_meta.len()];
        self.tabs_meta = tabs_meta;
    }

    /// Apply an asynchronous refresh only when it still belongs to this tab
    /// generation and the captured tab identity still exists. Lookup by
    /// identity, rather than the old positional index, also makes a reordered
    /// tab list safe.
    pub fn apply_refresh(&mut self, generation: u64, tab: &TabId, state: TabState) -> bool {
        if generation != self.tab_generation {
            return false;
        }
        let Some(index) = self.tabs_meta.iter().position(|current| current == tab) else {
            return false;
        };
        self.tabs[index] = state;
        true
    }

    /// Move to the first tab of `primary`'s vendor (the default account tab,
    /// since it precedes any of that vendor's account tabs).
    pub fn select_primary(&mut self, primary: Option<VendorId>) {
        if let Some(p) = primary
            && let Some(idx) = self.tabs_meta.iter().position(|t| t.vendor == p)
        {
            self.active = idx;
        }
    }

    pub fn next_tab(&mut self) {
        if !self.tabs_meta.is_empty() {
            self.active = (self.active + 1) % self.tabs_meta.len();
        }
    }

    pub fn prev_tab(&mut self) {
        if !self.tabs_meta.is_empty() {
            self.active = (self.active + self.tabs_meta.len() - 1) % self.tabs_meta.len();
        }
    }
}

/// Fetch and render one tab — returns a `TabState`.
pub async fn refresh_one(client: &Client, config: &Config, tab: &TabId) -> TabState {
    match build_outcome(client, config, tab).await {
        Ok(outcome) => {
            // Resolve the cache age (a duration from "now" at fetch time) into an
            // absolute instant ONCE. Without this, sections_for would recompute
            // `Utc::now() - cache_age` on every draw and the displayed time would
            // tick upward in real time instead of holding at the last refresh.
            let now = Utc::now();
            let fetched_at = outcome
                .cache_age
                .map(|age| now - chrono::Duration::from_std(age).unwrap_or_default());
            TabState::Ready(Box::new(ReadyTab {
                snapshot: outcome.snapshot,
                stale: outcome.stale,
                last_error: outcome.last_error,
                fetched_at,
            }))
        }
        Err(e) => TabState::Error(e.to_string()),
    }
}

async fn build_outcome(client: &Client, config: &Config, tab: &TabId) -> Result<VendorOutcome> {
    match tab.vendor {
        VendorId::Anthropic => {
            // A named account resolves to its own file + `anthropic/<label>`
            // cache, shared with the widget via `account_target` (#14/#17).
            // The default tab keeps the pre-existing resolution: config
            // `credentials_path` is an explicit strict read, and only the
            // platform default gets the macOS Keychain fallback.
            let (creds_target, cache) = match tab.account.as_deref() {
                Some(label) => config.anthropic.account_target(label)?,
                None => {
                    let target = match config.anthropic.credentials_path.clone() {
                        Some(p) => crate::anthropic::creds::CredsTarget::Explicit(p),
                        None => crate::anthropic::creds::CredsTarget::Default(
                            crate::anthropic::creds::default_path().unwrap_or_default(),
                        ),
                    };
                    (target, crate::cache::Cache::for_vendor("anthropic")?)
                }
            };
            let endpoints = crate::anthropic::fetch::Endpoints::default();
            let outcome = crate::anthropic::fetch_snapshot(
                client,
                &creds_target,
                &cache,
                &endpoints,
                DEFAULT_TTL,
            )
            .await?;
            Ok(crate::vendor::VendorOutcome {
                snapshot: crate::usage::VendorSnapshot::Anthropic(outcome.snapshot),
                stale: outcome.stale,
                last_error: outcome.last_error,
                cache_age: outcome.cache_age,
            })
        }
        VendorId::AnthropicApi => {
            let key = crate::config::resolve_api_key(
                "Anthropic_API",
                &config.anthropic_api.api_key_env,
                config.anthropic_api.api_key.as_deref(),
            )?;
            let cache = crate::cache::Cache::for_vendor("anthropic_api")?;
            let endpoints = crate::anthropic_api::fetch::Endpoints::default();
            let outcome = crate::anthropic_api::fetch_snapshot(
                client,
                &key,
                &cache,
                &endpoints,
                DEFAULT_TTL,
                config.anthropic_api.monthly_limit,
            )
            .await?;
            Ok(outcome.into())
        }
        VendorId::Openrouter => {
            let api_key = crate::config::resolve_api_key(
                "OpenRouter",
                &config.openrouter.api_key_env,
                config.openrouter.api_key.as_deref(),
            )?;
            let cache = crate::cache::Cache::for_vendor("openrouter")?;
            let endpoints = crate::openrouter::fetch::Endpoints::default();
            let outcome = crate::openrouter::fetch_snapshot(
                client,
                &api_key,
                &cache,
                &endpoints,
                DEFAULT_TTL,
            )
            .await?;
            Ok(outcome.into())
        }
        VendorId::Zai => {
            let api_key = crate::config::resolve_api_key(
                "Zai",
                &config.zai.api_key_env,
                config.zai.api_key.as_deref(),
            )?;
            let cache = crate::cache::Cache::for_vendor("zai")?;
            let endpoints = crate::zai::fetch::Endpoints::default();
            let outcome = crate::zai::fetch_snapshot(
                client,
                &api_key,
                &cache,
                &endpoints,
                DEFAULT_TTL,
                config.zai.plan_tier.as_deref(),
            )
            .await?;
            Ok(outcome.into())
        }
        VendorId::Openai => {
            let cache = crate::cache::Cache::for_vendor("openai")?;
            let creds_path = config
                .openai
                .codex_auth_path
                .clone()
                .unwrap_or_else(|| crate::openai::creds::default_path().unwrap_or_default());
            let endpoints = crate::openai::fetch::Endpoints::default();
            let outcome =
                crate::openai::fetch_snapshot(client, &creds_path, &cache, &endpoints, DEFAULT_TTL)
                    .await?;
            Ok(outcome.into())
        }
        VendorId::Deepseek => {
            let api_key = crate::config::resolve_api_key(
                "DeepSeek",
                &config.deepseek.api_key_env,
                config.deepseek.api_key.as_deref(),
            )?;
            let cache = crate::cache::Cache::for_vendor("deepseek")?;
            let endpoints = crate::deepseek::fetch::Endpoints::default();
            let outcome =
                crate::deepseek::fetch_snapshot(client, &api_key, &cache, &endpoints, DEFAULT_TTL)
                    .await?;
            Ok(outcome.into())
        }
        VendorId::Kimi => {
            let api_key = crate::config::resolve_api_key(
                "Kimi",
                &config.kimi.api_key_env,
                config.kimi.api_key.as_deref(),
            )?;
            let cache = crate::cache::Cache::for_vendor("kimi")?;
            let endpoints = crate::kimi::fetch::Endpoints::default();
            let outcome =
                crate::kimi::fetch_snapshot(client, &api_key, &cache, &endpoints, DEFAULT_TTL)
                    .await?;
            Ok(outcome.into())
        }
        VendorId::Kilo => {
            let api_key = crate::config::resolve_api_key(
                "Kilo",
                &config.kilo.api_key_env,
                config.kilo.api_key.as_deref(),
            )?;
            let cache = crate::cache::Cache::for_vendor("kilo")?;
            let endpoints = crate::kilo::fetch::Endpoints::default();
            let outcome = crate::kilo::fetch_snapshot(
                client,
                &api_key,
                &cache,
                &endpoints,
                DEFAULT_TTL,
                config.kilo.organization_id.as_deref(),
            )
            .await?;
            Ok(outcome.into())
        }
        VendorId::Novita => {
            let api_key = crate::config::resolve_api_key(
                "Novita",
                &config.novita.api_key_env,
                config.novita.api_key.as_deref(),
            )?;
            let cache = crate::cache::Cache::for_vendor("novita")?;
            let endpoints = crate::novita::fetch::Endpoints::default();
            let outcome =
                crate::novita::fetch_snapshot(client, &api_key, &cache, &endpoints, DEFAULT_TTL)
                    .await?;
            Ok(outcome.into())
        }
        VendorId::Moonshot => {
            let api_key = crate::config::resolve_api_key(
                "Moonshot",
                &config.moonshot.api_key_env,
                config.moonshot.api_key.as_deref(),
            )?;
            let cache = crate::cache::Cache::for_vendor("moonshot")?;
            let (endpoints, currency) =
                crate::moonshot::fetch::Endpoints::for_region(&config.moonshot.region);
            let outcome = crate::moonshot::fetch_snapshot(
                client,
                &api_key,
                &cache,
                &endpoints,
                DEFAULT_TTL,
                currency,
            )
            .await?;
            Ok(outcome.into())
        }
        VendorId::Grok => {
            let key = crate::config::resolve_api_key(
                "Grok",
                &config.grok.api_key_env,
                config.grok.api_key.as_deref(),
            )?;
            let cache = crate::cache::Cache::for_vendor("grok")?;
            let endpoints = crate::grok::fetch::Endpoints::default();
            let outcome = crate::grok::fetch_snapshot(
                client,
                &key,
                &cache,
                &endpoints,
                DEFAULT_TTL,
                config.grok.team_id.as_deref(),
            )
            .await?;
            Ok(outcome.into())
        }
        VendorId::Antigravity => {
            // No credentials: the local Antigravity server is the source.
            let cache = crate::cache::Cache::for_vendor("antigravity")?;
            let outcome = crate::antigravity::fetch_snapshot(client, &cache, DEFAULT_TTL).await?;
            Ok(outcome.into())
        }
    }
}

/// Convenience for the watch-driven binary: how long to wait between
/// automatic refreshes.
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(60);

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    // Use `App::with_theme(.., Theme::default())` rather than `App::new`, which
    // would read the real Omarchy theme file + `$HOME`. The tab-selection logic
    // under test is theme-agnostic.
    #[test]
    fn select_primary_moves_to_enabled_vendor() {
        let mut app = App::with_theme(
            vec![
                TabId::vendor(VendorId::Anthropic),
                TabId::vendor(VendorId::Openrouter),
            ],
            Theme::default(),
        );
        app.select_primary(Some(VendorId::Openrouter));
        assert_eq!(app.active_vendor(), Some(VendorId::Openrouter));
    }

    #[test]
    fn select_primary_ignores_disabled_vendor() {
        let mut app = App::with_theme(vec![TabId::vendor(VendorId::Anthropic)], Theme::default());
        app.select_primary(Some(VendorId::Openai));
        assert_eq!(app.active_vendor(), Some(VendorId::Anthropic));
    }

    fn config_with_accounts(labels: &[&str]) -> Config {
        let mut config = Config::default();
        // Keep only Anthropic enabled so the test asserts on account expansion,
        // not on the full default vendor set.
        config.openai.enabled = false;
        config.zai.enabled = false;
        config.openrouter.enabled = false;
        config.anthropic.accounts = labels
            .iter()
            .map(|l| crate::config::AnthropicAccount {
                label: (*l).to_string(),
                credentials_path: format!("/creds/{l}.json").into(),
            })
            .collect();
        config
    }

    #[test]
    fn tabs_expand_anthropic_accounts_after_default() {
        // Default Claude tab first, then each account in config order.
        let tabs = tabs_from_config(&config_with_accounts(&["work", "personal"]));
        assert_eq!(
            tabs,
            vec![
                TabId::vendor(VendorId::Anthropic),
                TabId::account("work"),
                TabId::account("personal"),
            ]
        );
    }

    #[test]
    fn tabs_without_accounts_are_just_enabled_vendors() {
        // No [[anthropic.accounts]] → one tab per enabled vendor, unchanged.
        let config = Config::default();
        let tabs = tabs_from_config(&config);
        let vendors: Vec<VendorId> = tabs.iter().map(|t| t.vendor).collect();
        assert_eq!(vendors, config.enabled_vendors());
        assert!(tabs.iter().all(|t| t.account.is_none()));
    }

    #[test]
    fn set_tabs_resets_states_and_clamps_selection() {
        // Simulates a Settings save that shrank the tab list: the selection
        // must clamp into range and every tab must reset to Loading so the
        // caller's spawn_all repopulates against the new config.
        let mut app = App::with_theme(
            tabs_from_config(&config_with_accounts(&["work", "personal"])),
            Theme::default(),
        );
        app.active = 2; // "personal"
        app.tabs[0] = TabState::Error("old".into());

        app.set_tabs(tabs_from_config(&config_with_accounts(&[])));
        assert_eq!(app.tabs_meta, vec![TabId::vendor(VendorId::Anthropic)]);
        assert_eq!(app.active, 0, "selection clamped after shrink");
        assert!(matches!(app.tabs[0], TabState::Loading));
    }

    #[test]
    fn refresh_from_old_generation_is_discarded() {
        let mut app = App::with_theme(vec![TabId::vendor(VendorId::Anthropic)], Theme::default());
        let old_generation = app.tab_generation;
        app.set_tabs(vec![TabId::vendor(VendorId::Openai)]);

        assert!(!app.apply_refresh(
            old_generation,
            &TabId::vendor(VendorId::Anthropic),
            TabState::Error("old result".into()),
        ));
        assert!(matches!(app.tabs[0], TabState::Loading));
    }

    #[test]
    fn refresh_identity_mismatch_is_discarded() {
        let mut app = App::with_theme(vec![TabId::vendor(VendorId::Anthropic)], Theme::default());
        let generation = app.tab_generation;

        assert!(!app.apply_refresh(
            generation,
            &TabId::vendor(VendorId::Openai),
            TabState::Error("wrong tab".into()),
        ));
        assert!(matches!(app.tabs[0], TabState::Loading));
    }

    #[test]
    fn refresh_identity_lands_at_new_index_after_same_generation_reorder() {
        let anthropic = TabId::vendor(VendorId::Anthropic);
        let openai = TabId::vendor(VendorId::Openai);
        let mut app = App::with_theme(vec![anthropic.clone(), openai.clone()], Theme::default());
        let generation = app.tab_generation;

        // A reorder is safe because delivery resolves the captured identity,
        // not a stale positional index.
        app.tabs_meta.swap(0, 1);
        app.tabs.swap(0, 1);
        assert!(app.apply_refresh(generation, &anthropic, TabState::Error("ready".into())));
        assert!(matches!(app.tabs[0], TabState::Loading));
        assert!(matches!(&app.tabs[1], TabState::Error(message) if message == "ready"));
    }

    fn ready_at(fetched_at: chrono::DateTime<Utc>) -> TabState {
        TabState::Ready(Box::new(ReadyTab {
            snapshot: crate::usage::VendorSnapshot::Openrouter(crate::usage::OpenRouterSnapshot {
                label: "test".into(),
                total_credits: 0.0,
                total_usage: 0.0,
                usage_daily: 0.0,
                usage_weekly: 0.0,
                usage_monthly: 0.0,
                is_free_tier: false,
                limit: None,
                limit_remaining: None,
            }),
            stale: false,
            last_error: None,
            fetched_at: Some(fetched_at),
        }))
    }

    #[test]
    fn apply_refresh_stamps_fetched_at_on_only_the_matching_tab() {
        // Pins the per-tab `fetched_at` the header now reads: a landed Anthropic
        // response leaves the still-loading OpenAI tab with no time of its own.
        // Dropping the global `last_refresh` clock is not observable from here
        // (it was write-only) — that is asserted against the rendered header in
        // `view::tests::header_refresh_*`.
        let anthropic = TabId::vendor(VendorId::Anthropic);
        let openai = TabId::vendor(VendorId::Openai);
        let mut app = App::with_theme(vec![anthropic.clone(), openai], Theme::default());
        let generation = app.tab_generation;
        let fetched_at = Utc.with_ymd_and_hms(2026, 5, 23, 12, 0, 0).unwrap();

        assert!(app.apply_refresh(generation, &anthropic, ready_at(fetched_at)));
        match &app.tabs[0] {
            TabState::Ready(ready) => assert_eq!(ready.fetched_at, Some(fetched_at)),
            other => panic!("expected Anthropic tab Ready, got {other:?}"),
        }
        assert!(matches!(app.tabs[1], TabState::Loading));
    }

    #[test]
    fn select_primary_lands_on_default_account_tab() {
        // With account tabs present, `primary = anthropic` selects the default
        // Claude tab (index 0), not one of its account tabs.
        let app = {
            let tabs = tabs_from_config(&config_with_accounts(&["work"]));
            let mut a = App::with_theme(tabs, Theme::default());
            a.select_primary(Some(VendorId::Anthropic));
            a
        };
        assert_eq!(app.active, 0);
        assert_eq!(
            app.active_tab_id(),
            Some(&TabId::vendor(VendorId::Anthropic))
        );
    }
}
