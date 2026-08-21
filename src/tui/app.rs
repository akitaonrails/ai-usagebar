//! TUI app state — vendors, tab selection, per-vendor snapshot cache.

use std::collections::HashSet;
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
/// `desktop` marks an account whose usage comes from the Claude Desktop app's
/// own token rather than a `claude` CLI credential.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TabId {
    pub vendor: VendorId,
    pub account: Option<String>,
    pub desktop: bool,
}

impl TabId {
    /// A plain vendor tab (default account for Anthropic).
    pub fn vendor(vendor: VendorId) -> Self {
        Self {
            vendor,
            account: None,
            desktop: false,
        }
    }

    /// A named Anthropic account tab (`[[anthropic.accounts]]` label).
    pub fn account(label: impl Into<String>) -> Self {
        Self {
            vendor: VendorId::Anthropic,
            account: Some(label.into()),
            desktop: false,
        }
    }

    /// An Anthropic account whose usage is read from the Claude Desktop app's
    /// own token store (a saved `~/.claude-acc/profiles/<label>` account).
    pub fn desktop_account(label: impl Into<String>) -> Self {
        Self {
            vendor: VendorId::Anthropic,
            account: Some(label.into()),
            desktop: true,
        }
    }
}

/// Expand enabled vendors into the tab list. Anthropic yields its default
/// account tab followed by one tab per `[[anthropic.accounts]]` entry, in
/// config order; every other vendor is a single tab. With no extra accounts
/// configured the result equals `config.enabled_vendors()` — identical tab set
/// and order to before (issue #14/#17 back-compat).
///
/// Config-only and pure — no Desktop profiles. Production uses
/// [`tabs_with_desktop`]; this stays for the hermetic unit tests and any caller
/// that only wants configured accounts.
pub fn tabs_from_config(config: &Config) -> Vec<TabId> {
    build_tabs(config, &[])
}

/// The production aggregate-view tab list: configured accounts plus every saved
/// Claude Desktop profile that has usable credentials. Desktop discovery is
/// best-effort and macOS-only; anywhere else this equals [`tabs_from_config`].
pub fn tabs_with_desktop(config: &Config) -> Vec<TabId> {
    build_tabs(config, &desktop_profile_labels(config))
}

/// Core expansion, parameterized on the Desktop account labels so it stays pure
/// and unit-testable. Desktop accounts follow the CLI accounts and count toward
/// "Anthropic has accounts" for the default-tab suppression.
///
/// In aggregate views, a label present in both a `[[anthropic.accounts]]` CLI
/// entry and a Desktop profile is sourced from **Desktop**, and the CLI entry is
/// dropped. The same account in two stores means two of them refreshing one
/// rotating refresh token — each rotation invalidates the other's copy — and
/// the CLI copy can even refresh to a stale/wrong identity that still
/// authenticates but reports another account's (often zero) usage, which no
/// credential-health check can catch. The app-maintained Desktop token is the
/// one source that avoids both the rotation war and that silent misattribution.
fn build_tabs(config: &Config, desktop_labels: &[String]) -> Vec<TabId> {
    let desktop_set: HashSet<&str> = desktop_labels.iter().map(String::as_str).collect();
    let mut tabs = Vec::new();
    for vendor in config.enabled_vendors() {
        if vendor == VendorId::Anthropic {
            let accounts: Vec<_> = config
                .anthropic
                .all_accounts()
                .into_iter()
                .filter(|a| !desktop_set.contains(a.label.as_str()))
                .collect();
            // The default (unnamed) Claude tab is suppressible once every
            // account is named — but never when it would leave Anthropic with
            // no tab at all. Desktop accounts count as named accounts here.
            if config.anthropic.show_default_account
                || (accounts.is_empty() && desktop_labels.is_empty())
            {
                tabs.push(TabId::vendor(vendor));
            }
            for acct in accounts {
                tabs.push(TabId::account(acct.label));
            }
            for label in desktop_labels {
                tabs.push(TabId::desktop_account(label.clone()));
            }
        } else if vendor == VendorId::Copilot && !config.copilot.accounts.is_empty() {
            // Multiple Copilot accounts: create one tab per account
            for acct in &config.copilot.accounts {
                tabs.push(TabId {
                    vendor: VendorId::Copilot,
                    account: Some(acct.label.clone()),
                    desktop: false,
                });
            }
        } else {
            tabs.push(TabId::vendor(vendor));
        }
    }
    tabs
}

/// Labels of saved Claude Desktop profiles with usable credentials. macOS-only
/// (elsewhere there is no Desktop app); best-effort, so an unreadable profile
/// store just yields none rather than failing the whole tab list.
#[cfg(target_os = "macos")]
fn desktop_profile_labels(config: &Config) -> Vec<String> {
    let Ok(paths) = crate::claude_desktop::Paths::resolve(&config.anthropic) else {
        return Vec::new();
    };
    if !paths.available() {
        return Vec::new();
    }
    crate::claude_desktop::load_profiles(&paths.profiles_dir)
        .into_iter()
        .filter(|p| p.has_credentials)
        .map(|p| p.label)
        .collect()
}

#[cfg(not(target_os = "macos"))]
fn desktop_profile_labels(_config: &Config) -> Vec<String> {
    Vec::new()
}

#[derive(Debug)]
pub struct App {
    pub tabs_meta: Vec<TabId>,
    pub active: usize,
    pub tabs: Vec<TabState>,
    /// Tab identities with a request currently in flight. Kept separate from
    /// `tabs` so a successful snapshot remains visible while it is refreshed.
    refreshing_tabs: HashSet<TabId>,
    /// Monotonically increasing identity for a complete tab-set replacement.
    /// Background fetches carry this with their tab identity so results from a
    /// previous Settings reload cannot land in a new tab at the old index.
    pub tab_generation: u64,
    /// When `true`, the Overview pane is selected (the virtual first tab that
    /// summarizes every vendor at once) instead of a per-vendor detail tab.
    pub overview: bool,
    /// Which vendors the Overview lists (`[ui] overview_vendors`); `None` = all.
    pub overview_vendors: Option<Vec<VendorId>>,
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
    /// Presentation style for the vendor navigation box (`[ui] vendor_box`).
    pub vendor_box: crate::config::VendorBoxStyle,
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
            refreshing_tabs: HashSet::new(),
            tab_generation: 0,
            overview: false,
            overview_vendors: None,
            theme,
            quit: false,
            settings: None,
            context_enabled: false,
            context_generation: 0,
            context: None,
            vendor_box: crate::config::VendorBoxStyle::Sidebar,
        }
    }

    /// Construct with an initial active tab — usually `[ui] primary` from
    /// config. Silently falls through to index 0 if the requested vendor
    /// isn't present (e.g. it was disabled).
    pub fn new_with_primary(tabs_meta: Vec<TabId>, primary: Option<VendorId>) -> Self {
        let mut app = Self::new(tabs_meta);
        // Default landing is the Overview (show everything at once). An explicit
        // `[ui] primary` opts into opening on that vendor's tab instead.
        if primary.is_some() {
            app.select_primary(primary);
        } else {
            app.overview = true;
        }
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
    /// tab resets to `Loading` (the caller re-spawns fetches). The selected tab
    /// is preserved by identity when possible; otherwise its old position is
    /// clamped in case the list shrank.
    pub fn set_tabs(&mut self, tabs_meta: Vec<TabId>) {
        let selected = self.active_tab_id().cloned();
        let fallback = self.active.min(tabs_meta.len().saturating_sub(1));
        self.tab_generation = self.tab_generation.wrapping_add(1);
        self.active = selected
            .as_ref()
            .and_then(|tab| tabs_meta.iter().position(|candidate| candidate == tab))
            .unwrap_or(fallback);
        self.tabs = vec![TabState::Loading; tabs_meta.len()];
        self.tabs_meta = tabs_meta;
        self.refreshing_tabs.clear();
    }

    /// Mark one tab as in flight. A ready snapshot stays in place; tabs that
    /// have never succeeded still use the full `Loading` state. Returning
    /// `false` suppresses duplicate requests for the same tab.
    pub fn begin_refresh(&mut self, tab: &TabId) -> bool {
        let Some(index) = self.tabs_meta.iter().position(|current| current == tab) else {
            return false;
        };
        if !self.refreshing_tabs.insert(tab.clone()) {
            return false;
        }
        if !matches!(self.tabs[index], TabState::Ready(_)) {
            self.tabs[index] = TabState::Loading;
        }
        true
    }

    pub fn is_refreshing(&self, tab: &TabId) -> bool {
        self.refreshing_tabs.contains(tab)
    }

    pub fn tab_is_refreshing(&self, index: usize) -> bool {
        self.tabs_meta
            .get(index)
            .is_some_and(|tab| self.is_refreshing(tab))
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
        let was_refreshing = self.refreshing_tabs.remove(tab);
        // If revalidation fails after a successful snapshot, preserve the
        // useful data but make the failure explicit. Initial failures still
        // become the normal Error state because there is no data to preserve.
        if was_refreshing
            && let TabState::Ready(ready) = &mut self.tabs[index]
            && let TabState::Error(message) = state
        {
            ready.stale = true;
            ready.last_error = Some((0, message));
        } else {
            self.tabs[index] = state;
        }
        true
    }

    /// Move to the first tab of `primary`'s vendor (the default account tab,
    /// since it precedes any of that vendor's account tabs).
    pub fn select_primary(&mut self, primary: Option<VendorId>) {
        if let Some(p) = primary
            && let Some(idx) = self.tabs_meta.iter().position(|t| t.vendor == p)
        {
            self.active = idx;
            self.overview = false;
        }
    }

    /// The selectable ring is `[Overview, tab0, tab1, …]`. `next_tab`/`prev_tab`
    /// walk it, wrapping through the Overview at the ends.
    pub fn next_tab(&mut self) {
        if self.overview {
            if !self.tabs_meta.is_empty() {
                self.overview = false;
                self.active = 0;
            }
        } else if self.active + 1 < self.tabs_meta.len() {
            self.active += 1;
        } else {
            self.overview = true;
        }
    }

    pub fn prev_tab(&mut self) {
        if self.overview {
            if !self.tabs_meta.is_empty() {
                self.overview = false;
                self.active = self.tabs_meta.len() - 1;
            }
        } else if self.active > 0 {
            self.active -= 1;
        } else {
            self.overview = true;
        }
    }

    /// Tabs the Overview should list: `overview_vendors` filtered against the
    /// live tab set (preserving the config order), or all tabs when unset.
    pub fn overview_tabs(&self) -> Vec<usize> {
        match &self.overview_vendors {
            None => (0..self.tabs_meta.len()).collect(),
            Some(wanted) => wanted
                .iter()
                .flat_map(|v| {
                    self.tabs_meta
                        .iter()
                        .enumerate()
                        .filter(move |(_, t)| t.vendor == *v)
                        .map(|(i, _)| i)
                })
                .collect(),
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
                last_error: outcome.last_error.map(|(code, message)| {
                    (code, crate::display::sanitize_untrusted_field(&message))
                }),
                fetched_at,
            }))
        }
        Err(e) => TabState::Error(crate::display::sanitize_untrusted_field(&e.user_message())),
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
                Some(label) if tab.desktop => {
                    crate::anthropic::desktop_creds::account_target(config, label)?
                }
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
        VendorId::Supergrok => {
            let cache = crate::cache::Cache::for_vendor("supergrok")?;
            let scope_paths = crate::supergrok::scope::ScopePaths::with_overrides(
                config.supergrok.auth_path.as_deref(),
                config.supergrok.config_path.as_deref(),
            )?;
            let outcome = crate::supergrok::fetch_snapshot(
                &config.supergrok.grok_binary,
                &scope_paths,
                &cache,
                DEFAULT_TTL,
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
        VendorId::Minimax => {
            let api_key = crate::config::resolve_api_key(
                "MiniMax",
                &config.minimax.api_key_env,
                config.minimax.api_key.as_deref(),
            )?;
            let cache = crate::cache::Cache::for_vendor("minimax")?;
            let endpoints = crate::minimax::fetch::Endpoints::for_region(&config.minimax.region);
            let outcome =
                crate::minimax::fetch_snapshot(client, &api_key, &cache, &endpoints, DEFAULT_TTL)
                    .await?;
            Ok(outcome.into())
        }
        VendorId::Cursor => {
            let cache = crate::cache::Cache::for_vendor("cursor")?;
            let db_path = config
                .cursor
                .db_path
                .clone()
                .map(Ok)
                .unwrap_or_else(crate::cursor::db::default_db_path)?;
            let agent_auth_path = config
                .cursor
                .agent_auth_path
                .clone()
                .map(Ok)
                .unwrap_or_else(crate::cursor::db::default_agent_auth_path)?;
            let endpoints = crate::cursor::fetch::Endpoints::default();
            let outcome = crate::cursor::fetch_snapshot(
                client,
                &db_path,
                &agent_auth_path,
                &cache,
                &endpoints,
                DEFAULT_TTL,
            )
            .await?;
            Ok(outcome.into())
        }
        VendorId::Copilot => {
            let cache = if let Some(label) = tab.account.as_deref() {
                crate::cache::Cache::for_vendor(&format!("copilot@{}", label))?
            } else {
                crate::cache::Cache::for_vendor("copilot")?
            };
            let endpoints = crate::copilot::fetch::Endpoints::from_config(&config.copilot);
            let outcome = crate::copilot::fetch_snapshot_with_account(
                client,
                &config.copilot,
                &cache,
                &endpoints,
                DEFAULT_TTL,
                tab.account.as_deref(),
            )
            .await?;
            Ok(outcome.into())
        }
        VendorId::Kiro => {
            let cache = crate::cache::Cache::for_vendor("kiro")?;
            let db_path = config
                .kiro
                .db_path
                .clone()
                .map(Ok)
                .unwrap_or_else(crate::kiro::db::default_db_path)?;
            let outcome =
                crate::kiro::fetch_snapshot(client, &db_path, &cache, DEFAULT_TTL).await?;
            Ok(outcome.into())
        }
        VendorId::NousResearch => {
            let store = crate::nous::credentials::CredentialStore::default();
            let endpoints = crate::nous::fetch::Endpoints::default();
            let account = crate::nous::fetch::fetch_account_with_refresh(
                client,
                &store,
                &endpoints,
                Utc::now(),
            )
            .await?;
            Ok(crate::vendor::VendorOutcome {
                snapshot: crate::usage::VendorSnapshot::NousResearch(account),
                stale: false,
                last_error: None,
                cache_age: Some(Duration::ZERO),
            })
        }
        VendorId::OpenCodeGo => {
            let api_key = crate::config::resolve_api_key(
                "OpenCode Go",
                &config.opencode_go.api_key_env,
                config.opencode_go.api_key.as_deref(),
            )?;
            let cache = crate::cache::Cache::for_vendor("opencode-go")?;
            let endpoints = crate::opencode_go::fetch::Endpoints::default();
            let outcome = crate::opencode_go::fetch::fetch_snapshot(
                client,
                &api_key,
                &cache,
                &endpoints,
                DEFAULT_TTL,
            )
            .await?;
            Ok(outcome.into())
        }
    }
}

/// Convenience for the watch-driven binary: how long to wait between
/// automatic refreshes.
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(60);

/// Gap between successive Anthropic fetches at refresh time. Every Anthropic
/// tab (the default account and each named/discovered account) hits the same
/// `/api/oauth/usage` + token-refresh endpoints, which rate-limit a burst of
/// simultaneous requests from one client — so with several accounts the TUI
/// would fire them all at once and some would come back `429`. Spacing them
/// out keeps every account refreshing politely.
pub const ANTHROPIC_REFRESH_STAGGER: Duration = Duration::from_millis(800);

/// Per-tab startup delay for one `spawn_all` pass. Only Anthropic tabs are
/// staggered (they share the rate-limited endpoint and multiply with accounts);
/// every other vendor hits its own endpoint and starts immediately. The first
/// Anthropic tab also starts immediately; each subsequent one waits one more
/// `step`. Pure and position-based so it is unit-testable.
pub fn refresh_stagger(tabs: &[TabId], step: Duration) -> Vec<Duration> {
    let mut anthropic_seen: u32 = 0;
    tabs.iter()
        .map(|tab| {
            if tab.vendor == VendorId::Anthropic {
                let delay = step * anthropic_seen;
                anthropic_seen += 1;
                delay
            } else {
                Duration::ZERO
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    // Use `App::with_theme(.., Theme::default())` rather than `App::new`, which
    // would read the real Omarchy theme file + `$HOME`. The tab-selection logic
    // under test is theme-agnostic.
    #[test]
    fn refresh_stagger_spaces_out_anthropic_tabs_only() {
        let step = Duration::from_millis(800);
        let tabs = vec![
            TabId::vendor(VendorId::Anthropic), // default account
            TabId::account("work"),
            TabId::account("personal"),
            TabId::vendor(VendorId::Openai),
            TabId::vendor(VendorId::Zai),
        ];
        let delays = refresh_stagger(&tabs, step);
        assert_eq!(
            delays,
            vec![
                Duration::ZERO, // 1st anthropic — immediate
                step,           // 2nd anthropic
                step * 2,       // 3rd anthropic
                Duration::ZERO, // openai — own endpoint, immediate
                Duration::ZERO, // zai — own endpoint, immediate
            ]
        );
    }

    #[test]
    fn refresh_stagger_is_a_noop_without_anthropic_accounts() {
        // A single Anthropic tab (or none) never waits.
        let tabs = vec![
            TabId::vendor(VendorId::Anthropic),
            TabId::vendor(VendorId::Openrouter),
        ];
        assert!(
            refresh_stagger(&tabs, Duration::from_millis(800))
                .iter()
                .all(|d| d.is_zero())
        );
    }

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

    #[test]
    fn nav_ring_wraps_through_the_overview_at_both_ends() {
        let mut app = App::with_theme(
            vec![
                TabId::vendor(VendorId::Anthropic),
                TabId::vendor(VendorId::Openai),
            ],
            Theme::default(),
        );
        app.overview = true;

        app.next_tab(); // Overview -> first vendor
        assert!(!app.overview);
        assert_eq!(app.active, 0);
        app.next_tab();
        assert_eq!(app.active, 1);
        app.next_tab(); // last vendor -> Overview
        assert!(app.overview);

        app.prev_tab(); // Overview -> last vendor
        assert!(!app.overview);
        assert_eq!(app.active, 1);
        app.prev_tab();
        assert_eq!(app.active, 0);
        app.prev_tab(); // first vendor -> Overview
        assert!(app.overview);
    }

    #[test]
    fn overview_tabs_defaults_to_all_and_honors_the_config_filter() {
        let mut app = App::with_theme(
            vec![
                TabId::vendor(VendorId::Anthropic),
                TabId::vendor(VendorId::Openai),
                TabId::vendor(VendorId::Zai),
            ],
            Theme::default(),
        );
        assert_eq!(app.overview_tabs(), vec![0, 1, 2]);

        // Subset in the given order.
        app.overview_vendors = Some(vec![VendorId::Zai, VendorId::Anthropic]);
        assert_eq!(app.overview_tabs(), vec![2, 0]);

        // A listed-but-absent vendor is simply skipped.
        app.overview_vendors = Some(vec![VendorId::Grok, VendorId::Openai]);
        assert_eq!(app.overview_tabs(), vec![1]);
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
    fn show_default_account_false_hides_the_unnamed_claude_tab() {
        // With named accounts and show_default_account=false, only the named
        // tabs appear — no redundant default "Claude" tab.
        let mut config = config_with_accounts(&["work", "personal"]);
        config.anthropic.show_default_account = false;
        assert_eq!(
            tabs_from_config(&config),
            vec![TabId::account("work"), TabId::account("personal")]
        );

        // But with no named accounts it is kept, so Anthropic never loses its
        // only tab.
        let mut empty = Config::default();
        empty.openai.enabled = false;
        empty.zai.enabled = false;
        empty.openrouter.enabled = false;
        empty.anthropic.show_default_account = false;
        assert_eq!(
            tabs_from_config(&empty),
            vec![TabId::vendor(VendorId::Anthropic)]
        );
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
    fn tabs_include_accounts_auto_discovered_from_accounts_dir() {
        // A CLAUDE_CONFIG_DIR-style directory becomes account tabs with no
        // explicit [[anthropic.accounts]] entry. Hermetic: real TempDir.
        let td = tempfile::tempdir().unwrap();
        for label in ["work", "personal"] {
            let dir = td.path().join(label);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(".credentials.json"), "{}").unwrap();
        }
        let mut config = Config::default();
        config.openai.enabled = false;
        config.zai.enabled = false;
        config.openrouter.enabled = false;
        config.anthropic.accounts_dir = Some(td.path().to_path_buf());

        let tabs = tabs_from_config(&config);
        assert_eq!(
            tabs,
            vec![
                TabId::vendor(VendorId::Anthropic),
                TabId::account("personal"), // sorted by label
                TabId::account("work"),
            ]
        );
    }

    #[test]
    fn desktop_labels_become_account_tabs_after_cli_accounts() {
        // Pure core: desktop accounts follow CLI accounts, in the order given.
        let config = config_with_accounts(&["work"]);
        let tabs = build_tabs(&config, &["gmail".into(), "hotmail".into()]);
        assert_eq!(
            tabs,
            vec![
                TabId::vendor(VendorId::Anthropic),
                TabId::account("work"),
                TabId::desktop_account("gmail"),
                TabId::desktop_account("hotmail"),
            ]
        );
    }

    #[test]
    fn a_desktop_profile_wins_a_label_collision_with_a_cli_account() {
        // One tab per label; the Desktop source wins so the label is never fed
        // from two stores refreshing one rotating token (which invalidate each
        // other and can silently show a wrong account's usage). The CLI entry is
        // dropped; a CLI-only label (work) is untouched.
        let config = config_with_accounts(&["gmail", "work"]);
        let tabs = build_tabs(&config, &["gmail".into(), "hotmail".into()]);
        assert_eq!(
            tabs,
            vec![
                TabId::vendor(VendorId::Anthropic),
                TabId::account("work"),
                TabId::desktop_account("gmail"),
                TabId::desktop_account("hotmail"),
            ]
        );
    }

    #[test]
    fn desktop_accounts_suppress_the_default_tab_like_named_ones() {
        // show_default_account=false + only Desktop accounts => no default tab,
        // exactly as if they were [[anthropic.accounts]] (the Desktop-only user).
        let mut config = config_with_accounts(&[]);
        config.cursor.enabled = false;
        config.anthropic.show_default_account = false;

        // No accounts of either kind: the default tab survives (never leave
        // Anthropic tab-less).
        assert_eq!(
            build_tabs(&config, &[]),
            vec![TabId::vendor(VendorId::Anthropic)]
        );
        // A Desktop account is present: default suppressed, only the account.
        assert_eq!(
            build_tabs(&config, &["gmail".into()]),
            vec![TabId::desktop_account("gmail")]
        );
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
        let old_tab = app.tabs_meta[0].clone();
        assert!(app.begin_refresh(&old_tab));

        app.set_tabs(tabs_from_config(&config_with_accounts(&[])));
        assert_eq!(app.tabs_meta, vec![TabId::vendor(VendorId::Anthropic)]);
        assert_eq!(app.active, 0, "selection clamped after shrink");
        assert!(matches!(app.tabs[0], TabState::Loading));
        assert!(!app.is_refreshing(&old_tab));
    }

    #[test]
    fn set_tabs_preserves_selected_identity_when_entries_are_inserted() {
        let mut app = App::with_theme(
            vec![
                TabId::vendor(VendorId::Anthropic),
                TabId::vendor(VendorId::Openai),
            ],
            Theme::default(),
        );
        app.active = 1;

        app.set_tabs(vec![
            TabId::vendor(VendorId::Anthropic),
            TabId::account("work"),
            TabId::vendor(VendorId::Openai),
        ]);

        assert_eq!(app.active, 2);
        assert_eq!(app.active_tab_id(), Some(&TabId::vendor(VendorId::Openai)));
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
        assert!(app.begin_refresh(&anthropic));

        // A reorder is safe because delivery resolves the captured identity,
        // not a stale positional index.
        app.tabs_meta.swap(0, 1);
        app.tabs.swap(0, 1);
        assert!(app.apply_refresh(generation, &anthropic, TabState::Error("ready".into())));
        assert!(matches!(app.tabs[0], TabState::Loading));
        assert!(matches!(&app.tabs[1], TabState::Error(message) if message == "ready"));
        assert!(!app.is_refreshing(&anthropic));
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
    fn refresh_keeps_ready_snapshot_visible_and_suppresses_duplicates() {
        let tab = TabId::vendor(VendorId::Openrouter);
        let fetched_at = Utc.with_ymd_and_hms(2026, 5, 23, 12, 0, 0).unwrap();
        let mut app = App::with_theme(vec![tab.clone()], Theme::default());
        app.tabs[0] = ready_at(fetched_at);

        assert!(app.begin_refresh(&tab));
        assert!(
            !app.begin_refresh(&tab),
            "duplicate request must be suppressed"
        );
        assert!(app.is_refreshing(&tab));
        match &app.tabs[0] {
            TabState::Ready(ready) => assert_eq!(ready.fetched_at, Some(fetched_at)),
            other => panic!("ready snapshot disappeared during refresh: {other:?}"),
        }
    }

    #[test]
    fn first_refresh_still_uses_loading_until_data_arrives() {
        let tab = TabId::vendor(VendorId::Openrouter);
        let mut app = App::with_theme(vec![tab.clone()], Theme::default());

        assert!(app.begin_refresh(&tab));
        assert!(app.is_refreshing(&tab));
        assert!(matches!(app.tabs[0], TabState::Loading));

        assert!(app.apply_refresh(
            app.tab_generation,
            &tab,
            TabState::Error("not signed in".into()),
        ));
        assert!(!app.is_refreshing(&tab));
        assert!(matches!(&app.tabs[0], TabState::Error(message) if message == "not signed in"));
    }

    #[test]
    fn successful_revalidation_replaces_snapshot_and_clears_indicator() {
        let tab = TabId::vendor(VendorId::Openrouter);
        let old_at = Utc.with_ymd_and_hms(2026, 5, 23, 12, 0, 0).unwrap();
        let new_at = Utc.with_ymd_and_hms(2026, 5, 23, 12, 1, 0).unwrap();
        let mut app = App::with_theme(vec![tab.clone()], Theme::default());
        app.tabs[0] = ready_at(old_at);

        assert!(app.begin_refresh(&tab));
        assert!(app.apply_refresh(app.tab_generation, &tab, ready_at(new_at)));
        assert!(!app.is_refreshing(&tab));
        match &app.tabs[0] {
            TabState::Ready(ready) => assert_eq!(ready.fetched_at, Some(new_at)),
            other => panic!("expected replacement snapshot, got {other:?}"),
        }
    }

    #[test]
    fn failed_revalidation_preserves_snapshot_with_visible_warning() {
        let tab = TabId::vendor(VendorId::Openrouter);
        let fetched_at = Utc.with_ymd_and_hms(2026, 5, 23, 12, 0, 0).unwrap();
        let mut app = App::with_theme(vec![tab.clone()], Theme::default());
        app.tabs[0] = ready_at(fetched_at);

        assert!(app.begin_refresh(&tab));
        assert!(app.apply_refresh(
            app.tab_generation,
            &tab,
            TabState::Error("refresh failed".into()),
        ));
        assert!(!app.is_refreshing(&tab));
        match &app.tabs[0] {
            TabState::Ready(ready) => {
                assert_eq!(ready.fetched_at, Some(fetched_at));
                assert!(ready.stale);
                assert_eq!(ready.last_error, Some((0, "refresh failed".into())));
            }
            other => panic!("last successful snapshot was lost: {other:?}"),
        }
        let sections = crate::tui::panels::sections_for(&app.tabs[0], Utc::now(), 5);
        assert!(sections.iter().any(|section| matches!(
            section,
            crate::tui::panels::Section::Text { label, value }
                if label == "Warning" && value == "refresh failed"
        )));
    }

    #[test]
    fn old_generation_result_does_not_clear_current_refresh() {
        let tab = TabId::vendor(VendorId::Openrouter);
        let mut app = App::with_theme(vec![tab.clone()], Theme::default());
        let old_generation = app.tab_generation;
        app.set_tabs(vec![tab.clone()]);
        assert!(app.begin_refresh(&tab));

        assert!(!app.apply_refresh(old_generation, &tab, TabState::Error("old result".into()),));
        assert!(app.is_refreshing(&tab));
        assert!(matches!(app.tabs[0], TabState::Loading));
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

    fn config_with_copilot_accounts(labels: &[&str]) -> Config {
        let mut config = Config::default();
        // Keep only Copilot enabled to test account expansion.
        config.anthropic.enabled = false;
        config.openai.enabled = false;
        config.zai.enabled = false;
        config.openrouter.enabled = false;
        config.copilot.enabled = true;
        config.copilot.accounts = labels
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let hostname = if i == 0 {
                    "github.com".to_string()
                } else {
                    "work.ghe.com".to_string()
                };
                crate::config::CopilotAccountConfig {
                    label: (*l).to_string(),
                    hostname: Some(hostname),
                    token: None,
                    token_env: None,
                }
            })
            .collect();
        config
    }

    #[test]
    fn copilot_multi_account_creates_separate_tabs_per_account() {
        // With multiple Copilot accounts, each account gets its own tab
        // with the format "copilot · <label>".
        let tabs = tabs_from_config(&config_with_copilot_accounts(&["personal", "work"]));
        assert_eq!(tabs.len(), 2);
        
        // First tab: personal account
        assert_eq!(tabs[0].vendor, VendorId::Copilot);
        assert_eq!(tabs[0].account, Some("personal".to_string()));
        assert!(!tabs[0].desktop);
        
        // Second tab: work account
        assert_eq!(tabs[1].vendor, VendorId::Copilot);
        assert_eq!(tabs[1].account, Some("work".to_string()));
        assert!(!tabs[1].desktop);
    }

    #[test]
    fn copilot_single_account_creates_single_vendor_tab() {
        // In legacy single-account mode (no accounts array), Copilot gets
        // a single vendor tab with no account label.
        let mut config = Config::default();
        config.anthropic.enabled = false;
        config.openai.enabled = false;
        config.zai.enabled = false;
        config.openrouter.enabled = false;
        config.copilot.enabled = true;
        config.copilot.hostname = Some("github.com".to_string());
        config.copilot.accounts.clear(); // No accounts array = legacy mode
        
        let tabs = tabs_from_config(&config);
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0].vendor, VendorId::Copilot);
        assert_eq!(tabs[0].account, None); // No account label in legacy mode
    }
}

