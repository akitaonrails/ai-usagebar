//! TUI app state — vendors, tab selection, per-vendor snapshot cache.

use std::collections::HashSet;
use std::time::Duration;

use chrono::Utc;
use ratatui::layout::Rect;
use reqwest::Client;

use crate::cache::DEFAULT_TTL;
use crate::config::{Config, CustomProviderConfig};
use crate::error::{AppError, Result};
use crate::outcome::fetch_api_keyed;
use crate::theme::Theme;
use crate::tui::settings::SettingsRow;
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
    Error {
        message: String,
        /// Vendor plan already known from credentials when the quota fetch
        /// failed (Claude OAuth `subscriptionType`). Absent when the vendor
        /// has no plan without a snapshot.
        plan: Option<String>,
    },
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
    /// Bar-number settings this vendor was configured with — the tank size a
    /// prepaid balance is metered against, and which of the two numbers goes on
    /// the bar. Resolved from config at fetch time rather than stored in the
    /// snapshot, so editing config.toml takes effect on the next redraw instead
    /// of waiting for the cache to expire.
    pub display: crate::balance::DisplayPrefs,
}

/// Where a tab's usage comes from: a built-in vendor, or a user-declared
/// `[[custom]]` HTTP provider. A custom tab carries its own display names so
/// the TUI and the report never need the config again just to label it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TabSource {
    Builtin(VendorId),
    Custom {
        id: String,
        name: String,
        short_name: String,
    },
}

impl TabState {
    pub fn error(message: impl AsRef<str>) -> Self {
        Self::error_with_plan(message, None)
    }

    pub fn error_with_plan(message: impl AsRef<str>, plan: Option<String>) -> Self {
        let plan = plan.and_then(|plan| {
            let cleaned = crate::display::sanitize_untrusted_field(plan.trim());
            if cleaned.is_empty() || cleaned.eq_ignore_ascii_case("unknown") {
                None
            } else {
                Some(cleaned)
            }
        });
        Self::Error {
            message: message.as_ref().to_string(),
            plan,
        }
    }
}

/// Identity of one TUI tab. Usually a whole vendor; Claude and OpenRouter can
/// also name a configured account. `account: None` is a plain vendor tab or
/// that vendor's default account.
/// `desktop` marks an account whose usage comes from the Claude Desktop app's
/// own token rather than a `claude` CLI credential.
/// Custom providers never carry an account or a desktop flag.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TabId {
    pub source: TabSource,
    pub account: Option<String>,
    pub desktop: bool,
}

impl TabId {
    /// A plain vendor tab (default account for Anthropic).
    pub fn vendor(vendor: VendorId) -> Self {
        Self {
            source: TabSource::Builtin(vendor),
            account: None,
            desktop: false,
        }
    }

    /// A named Anthropic account tab (`[[anthropic.accounts]]` label).
    pub fn account(label: impl Into<String>) -> Self {
        Self::account_for(VendorId::Anthropic, label)
    }

    /// A named account for a vendor that supports account arrays.
    pub fn account_for(vendor: VendorId, label: impl Into<String>) -> Self {
        Self {
            source: TabSource::Builtin(vendor),
            account: Some(label.into()),
            desktop: false,
        }
    }

    /// An Anthropic account whose usage is read from the Claude Desktop app's
    /// own token store (a saved `~/.claude-acc/profiles/<label>` account).
    pub fn desktop_account(label: impl Into<String>) -> Self {
        Self {
            source: TabSource::Builtin(VendorId::Anthropic),
            account: Some(label.into()),
            desktop: true,
        }
    }

    /// A user-declared `[[custom]]` provider tab.
    pub fn custom(spec: &CustomProviderConfig) -> Self {
        Self {
            source: TabSource::Custom {
                id: spec.id.clone(),
                name: spec.name.clone(),
                short_name: spec.short_name.clone(),
            },
            account: None,
            desktop: false,
        }
    }

    /// The built-in vendor behind this tab; `None` for a custom provider.
    /// (Named `vendor_id` because `TabId::vendor` is the constructor.)
    pub fn vendor_id(&self) -> Option<VendorId> {
        match &self.source {
            TabSource::Builtin(vendor) => Some(*vendor),
            TabSource::Custom { .. } => None,
        }
    }
}

/// Expand enabled vendors into the tab list. Claude, OpenRouter, and Codex
/// (OpenAI) yield their default account followed by configured named accounts;
/// every other vendor is a single tab. With no extra accounts and
/// `overview_vendors` unset, the result equals `config.enabled_vendors()` in
/// canonical order. When the list is set, listed vendors lead and unlisted
/// enabled vendors follow in that same canonical order.
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

/// Enabled vendors in `[ui] overview_vendors` order. Unlisted vendors keep
/// the canonical order and follow the listed ones. Unset means canonical.
fn vendors_for_tabs(config: &Config) -> Vec<VendorId> {
    let enabled = config.enabled_vendors();
    let Some(wanted) = config.ui.overview_vendors.as_ref() else {
        return enabled;
    };
    let mut ordered = Vec::with_capacity(enabled.len());
    for vendor in wanted {
        if enabled.contains(vendor) && !ordered.contains(vendor) {
            ordered.push(*vendor);
        }
    }
    for vendor in enabled {
        if !ordered.contains(&vendor) {
            ordered.push(vendor);
        }
    }
    ordered
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
    for vendor in vendors_for_tabs(config) {
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
        } else if let Some(accounts) = config.api_key_accounts(vendor) {
            if config.show_default_api_key_account(vendor) || accounts.is_empty() {
                tabs.push(TabId::vendor(vendor));
            }
            for account in accounts {
                tabs.push(TabId::account_for(vendor, account.label.clone()));
            }
        } else if vendor == VendorId::Openai {
            if config.openai.show_default_account || config.openai.accounts.is_empty() {
                tabs.push(TabId::vendor(vendor));
            }
            for account in &config.openai.accounts {
                tabs.push(TabId::account_for(vendor, account.label.clone()));
            }
        } else if vendor == VendorId::Copilot {
            if config.copilot.show_default_account || config.copilot.accounts.is_empty() {
                tabs.push(TabId::vendor(vendor));
            }
            for account in &config.copilot.accounts {
                tabs.push(TabId::account_for(vendor, account.label.clone()));
            }
        } else if vendor == VendorId::Devin {
            if config.devin.show_default_account || config.devin.accounts.is_empty() {
                tabs.push(TabId::vendor(vendor));
            }
            for account in &config.devin.accounts {
                tabs.push(TabId::account_for(vendor, account.label.clone()));
            }
        } else {
            tabs.push(TabId::vendor(vendor));
        }
    }
    // Custom providers follow every built-in vendor, in config order.
    for spec in config.enabled_custom() {
        tabs.push(TabId::custom(spec));
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

/// Mouse hit-test surface recorded by the last draw and consumed by the event
/// loop. Draw is the single source of truth for where things are on screen, so
/// the render pass records the interactive rects here instead of the input
/// handler re-deriving layout.
#[derive(Debug, Default, Clone)]
pub struct HitTargets {
    /// Vendor navigation entries: Overview first, then each tab by index.
    pub nav_entries: Vec<(NavTarget, Rect)>,
    /// Footer actions with a mouse-accessible keyboard equivalent.
    pub footer_actions: Vec<(FooterAction, Rect)>,
    /// Settings overlay interactive rows: the primary selector, key fields,
    /// and save row.
    pub settings_rows: Vec<(SettingsRow, Rect)>,
}

/// What a click in the vendor navigation selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavTarget {
    Overview,
    Tab(usize),
}

/// What a click in the footer invokes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FooterAction {
    Refresh,
    RefreshAll,
    Settings,
    Quit,
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
    /// Interactive rects from the most recent draw, for mouse hit-testing.
    pub hit: std::rc::Rc<std::cell::RefCell<HitTargets>>,
}

impl App {
    pub fn new(tabs_meta: Vec<TabId>) -> Self {
        // Production: resolve the palette from the environment (Omarchy theme
        // if present, else One Dark).
        Self::with_theme(tabs_meta, Theme::default().merged_with_omarchy())
    }

    /// Like [`App::new`] but with an explicit theme. Lets tests build an `App`
    /// without reading the real Omarchy theme file
    /// (`~/.local/state/omarchy/current/theme/colors.toml`) — `new` resolves
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
            hit: std::rc::Rc::new(std::cell::RefCell::new(HitTargets::default())),
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
        self.tabs_meta.get(self.active).and_then(TabId::vendor_id)
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
            && let TabState::Error { message, .. } = state
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
            && let Some(idx) = self.tabs_meta.iter().position(|t| t.vendor_id() == Some(p))
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

    /// Select the Overview pane (mouse click on the first nav entry).
    pub fn select_overview(&mut self) {
        self.overview = true;
    }

    /// Select a vendor tab by index (mouse click on a nav entry).
    pub fn select_tab(&mut self, index: usize) {
        if index < self.tabs_meta.len() {
            self.active = index;
            self.overview = false;
        }
    }

    /// Apply a mouse click on a vendor-navigation entry.
    pub fn nav_from_target(&mut self, target: NavTarget) {
        match target {
            NavTarget::Overview => self.select_overview(),
            NavTarget::Tab(index) => self.select_tab(index),
        }
    }

    /// Tabs the Overview should list: `overview_vendors` filtered against the
    /// live tab set (preserving the config order), or all tabs when unset.
    /// The filter names built-in vendors only, so a configured list excludes
    /// every custom provider tab; the unset default includes them. A repeated
    /// vendor is listed once, at its first occurrence.
    pub fn overview_tabs(&self) -> Vec<usize> {
        match &self.overview_vendors {
            None => (0..self.tabs_meta.len()).collect(),
            Some(wanted) => {
                let mut seen = HashSet::new();
                wanted
                    .iter()
                    .filter(|vendor| seen.insert(**vendor))
                    .flat_map(|v| {
                        self.tabs_meta
                            .iter()
                            .enumerate()
                            .filter(move |(_, t)| t.vendor_id() == Some(*v))
                            .map(|(i, _)| i)
                    })
                    .collect()
            }
        }
    }
}

/// How fresh a tab's snapshot may be.
///
/// `Cached` is the catalog's ordinary read: a payload inside the default
/// TTL answers without the network. `Wire` is the key check's read, where
/// a payload left by another producer — a refresh that started under a
/// different key — is not an answer about the key just written, so the
/// TTL is zero and no cached payload is ever "fresh", even a same-millisecond
/// one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    Cached,
    Wire,
}

impl Freshness {
    /// The cache TTL this mode grants: zero for `Wire`, which the cache's
    /// pre-network hook treats as "never fresh" in every branch.
    pub fn ttl(self) -> Duration {
        match self {
            Freshness::Cached => DEFAULT_TTL,
            Freshness::Wire => Duration::ZERO,
        }
    }
}

/// Fetch and render one tab — returns a `TabState`.
pub async fn refresh_one(
    client: &Client,
    config: &Config,
    tab: &TabId,
    freshness: Freshness,
) -> TabState {
    match build_outcome(client, config, tab, freshness).await {
        Ok(outcome) => {
            // Resolve the cache age (a duration from "now" at fetch time) into an
            // absolute instant ONCE. Without this, sections_for would recompute
            // `Utc::now() - cache_age` on every draw and the displayed time would
            // tick upward in real time instead of holding at the last refresh.
            let now = Utc::now();
            let off_the_wire = outcome.off_the_wire();
            let fetched_at = outcome
                .cache_age
                .map(|age| now - chrono::Duration::from_std(age).unwrap_or_default());
            let state = TabState::Ready(Box::new(ReadyTab {
                snapshot: outcome.snapshot,
                stale: outcome.stale,
                last_error: outcome.last_error.map(|(code, message)| {
                    (code, crate::display::sanitize_untrusted_field(&message))
                }),
                fetched_at,
                display: match &tab.source {
                    TabSource::Builtin(vendor) => config.display_prefs(*vendor),
                    // A `[[custom]]` provider states its own percentages; it has
                    // no balance to meter and no headline to choose.
                    TabSource::Custom { .. } => crate::balance::DisplayPrefs::default(),
                },
            }));
            // The single notification hook every frontend shares: TUI, `usage`
            // report, tray, and the GNOME/KDE/Omarchy frontends all land here.
            // Wire-fresh outcomes only — never cached, stale, or failed ones —
            // and best-effort by construction: `run` returns nothing and cannot
            // change this function's result or the caller's exit code.
            if off_the_wire
                && config.notifications.enabled
                && let Some(input) = crate::notify::RefreshInput::from_tab(tab, &state, now)
            {
                let _ = crate::notify::run(input, config.notifications.threshold).await;
            }
            state
        }
        Err(e) => TabState::error_with_plan(
            crate::display::sanitize_untrusted_field(&e.user_message()),
            e.plan().map(str::to_string),
        ),
    }
}

/// Key and cache for an API-key vendor tab: a named account's key with its own
/// `<slug>/<label>` cache, or the default key with the vendor-root cache, whose
/// path never moves (issue #14).
fn api_key_and_cache(
    config: &Config,
    vendor: VendorId,
    account: Option<&str>,
) -> Result<(String, crate::cache::Cache)> {
    let api_key = config.resolve_account_api_key_for(vendor, account)?;
    let cache = match account {
        Some(label) => crate::cache::Cache::for_vendor_account(vendor.slug(), label)?,
        None => crate::cache::Cache::for_vendor(vendor.slug())?,
    }
    // The fetch goes out with this key and a provider's answer can quote it
    // back: the cache redacts it at the first point of writing, so the key
    // never reaches `.last_error` (nor the pair the fetch returns with it).
    .redacting(&api_key);
    Ok((api_key, cache))
}

// The key-based vendors' fetch wrap is `crate::outcome::fetch_api_keyed`,
// shared with the widget/CLI; the cache side is attached by `api_key_and_cache`.

async fn build_outcome(
    client: &Client,
    config: &Config,
    tab: &TabId,
    freshness: Freshness,
) -> Result<VendorOutcome> {
    let vendor = match &tab.source {
        TabSource::Builtin(vendor) => *vendor,
        TabSource::Custom { id, .. } => {
            // A custom provider owns its TTL and its cache slot; the cache is
            // keyed by the provider id under one shared `custom` vendor dir.
            let spec = config.custom_by_id(id).ok_or_else(|| {
                AppError::Other(format!("custom provider {id} is not configured"))
            })?;
            let api_key = spec.resolve_api_key()?;
            let cache = crate::cache::Cache::for_vendor_account("custom", id)?;
            let outcome =
                crate::custom::fetch_snapshot(client, spec, &api_key, &cache, spec.cache_ttl())
                    .await?;
            return Ok(outcome.into());
        }
    };
    // The built-in vendors share one TTL; a wire-only read (the key check)
    // grants none. A vendor that owns its TTL (the custom branch above) is
    // already out of this match.
    let ttl = freshness.ttl();
    match vendor {
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
                None => (
                    config.anthropic.default_creds_target(),
                    crate::cache::Cache::for_vendor("anthropic")?,
                ),
            };
            let endpoints = crate::anthropic::fetch::Endpoints::default();
            let outcome =
                crate::anthropic::fetch_snapshot(client, &creds_target, &cache, &endpoints, ttl)
                    .await?;
            Ok(outcome.map(crate::usage::VendorSnapshot::Anthropic))
        }
        VendorId::AnthropicApi => {
            let key = crate::config::resolve_api_key(
                "Anthropic_API",
                &config.anthropic_api.api_key_env,
                config.anthropic_api.api_key.as_deref(),
            )?;
            let cache = crate::cache::Cache::for_vendor("anthropic_api")?.redacting(&key);
            let endpoints = crate::anthropic_api::fetch::Endpoints::default();
            let outcome = fetch_api_keyed(
                async {
                    crate::anthropic_api::fetch_snapshot(
                        client,
                        &key,
                        &cache,
                        &endpoints,
                        ttl,
                        config.anthropic_api.monthly_limit,
                    )
                    .await
                    .map(Into::into)
                },
                &key,
            )
            .await?;
            Ok(outcome)
        }
        VendorId::Openrouter => {
            let (api_key, cache) = api_key_and_cache(config, vendor, tab.account.as_deref())?;
            let management_key = config.openrouter_management_key(tab.account.as_deref());
            let endpoints = crate::openrouter::fetch::Endpoints::default();
            let outcome = fetch_api_keyed(
                async {
                    crate::openrouter::fetch_snapshot(
                        client,
                        &api_key,
                        management_key.as_deref(),
                        &cache,
                        &endpoints,
                        ttl,
                    )
                    .await
                    .map(Into::into)
                },
                &api_key,
            )
            .await?;
            Ok(outcome)
        }
        VendorId::Zai => {
            let (api_key, cache) = api_key_and_cache(config, vendor, tab.account.as_deref())?;
            let endpoints = crate::zai::fetch::Endpoints::default();
            let outcome = fetch_api_keyed(
                async {
                    crate::zai::fetch_snapshot(
                        client,
                        &api_key,
                        &cache,
                        &endpoints,
                        ttl,
                        config.zai.plan_tier.as_deref(),
                    )
                    .await
                    .map(Into::into)
                },
                &api_key,
            )
            .await?;
            Ok(outcome)
        }
        VendorId::Openai => {
            let label = tab.account.as_deref();
            let cache = match label {
                Some(label) => crate::cache::Cache::for_vendor_account("openai", label)?,
                None => crate::cache::Cache::for_vendor("openai")?,
            };
            let route = || config.openai.fetch_auth_path(label);
            let endpoints = crate::openai::fetch::Endpoints::default();
            let outcome =
                crate::openai::fetch_snapshot_routed(client, route, &cache, &endpoints, ttl)
                    .await?;
            Ok(outcome.into())
        }
        VendorId::Copilot => {
            let label = tab.account.as_deref();
            let token = config.copilot.resolve_token_async(label).await?;
            let cache = match label {
                Some(label) => crate::cache::Cache::for_vendor_account("copilot", label)?,
                None => crate::cache::Cache::for_vendor("copilot")?,
            };
            let endpoints = crate::copilot::fetch::Endpoints::default();
            let outcome =
                crate::copilot::fetch_snapshot(client, &token, &cache, &endpoints, ttl).await?;
            Ok(outcome.into())
        }
        VendorId::Deepseek => {
            let (api_key, cache) = api_key_and_cache(config, vendor, tab.account.as_deref())?;
            let endpoints = crate::deepseek::fetch::Endpoints::default();
            let outcome = fetch_api_keyed(
                async {
                    crate::deepseek::fetch_snapshot(client, &api_key, &cache, &endpoints, ttl)
                        .await
                        .map(Into::into)
                },
                &api_key,
            )
            .await?;
            Ok(outcome)
        }
        VendorId::Deepinfra => {
            let (api_key, cache) = api_key_and_cache(config, vendor, tab.account.as_deref())?;
            let endpoints = crate::deepinfra::fetch::Endpoints::default();
            let outcome = fetch_api_keyed(
                async {
                    crate::deepinfra::fetch::fetch_snapshot(
                        client, &api_key, &cache, &endpoints, ttl,
                    )
                    .await
                    .map(Into::into)
                },
                &api_key,
            )
            .await?;
            Ok(outcome)
        }
        VendorId::Kimi => {
            let (auth, endpoints) = crate::kimi::resolve_auth(&config.kimi)?;
            // The API key is the value the user entered and the only one a
            // provider answer could echo back. The Kimi Code CLI's OAuth
            // token is not exposed by `Auth` — `bearer_token` resolves it at
            // fetch time and refreshes rotate it — so that variant carries
            // an empty (no-op) secret.
            let secret = match &auth {
                crate::kimi::fetch::Auth::ApiKey(key) => key.clone(),
                crate::kimi::fetch::Auth::KimiCode(_) => String::new(),
            };
            let cache = crate::cache::Cache::for_vendor("kimi")?.redacting(&secret);
            let outcome = fetch_api_keyed(
                async {
                    crate::kimi::fetch::fetch_snapshot_with_auth(
                        client, &auth, &cache, &endpoints, ttl,
                    )
                    .await
                    .map(Into::into)
                },
                &secret,
            )
            .await?;
            Ok(outcome)
        }
        VendorId::Kilo => {
            let (api_key, cache) = api_key_and_cache(config, vendor, tab.account.as_deref())?;
            let endpoints = crate::kilo::fetch::Endpoints::default();
            let outcome = fetch_api_keyed(
                async {
                    crate::kilo::fetch_snapshot(
                        client,
                        &api_key,
                        &cache,
                        &endpoints,
                        ttl,
                        config.kilo.organization_id.as_deref(),
                    )
                    .await
                    .map(Into::into)
                },
                &api_key,
            )
            .await?;
            Ok(outcome)
        }
        VendorId::Novita => {
            let (api_key, cache) = api_key_and_cache(config, vendor, tab.account.as_deref())?;
            let endpoints = crate::novita::fetch::Endpoints::default();
            let outcome = fetch_api_keyed(
                async {
                    crate::novita::fetch_snapshot(client, &api_key, &cache, &endpoints, ttl)
                        .await
                        .map(Into::into)
                },
                &api_key,
            )
            .await?;
            Ok(outcome)
        }
        VendorId::Synthetic => {
            let (api_key, cache) = api_key_and_cache(config, vendor, tab.account.as_deref())?;
            let endpoints = crate::synthetic::fetch::Endpoints::default();
            let outcome = fetch_api_keyed(
                async {
                    crate::synthetic::fetch_snapshot(
                        client,
                        &api_key,
                        &cache,
                        &endpoints,
                        ttl,
                        config.synthetic.plan_tier.as_deref(),
                    )
                    .await
                    .map(Into::into)
                },
                &api_key,
            )
            .await?;
            Ok(outcome)
        }
        VendorId::DeepRelay => {
            let (api_key, cache) = api_key_and_cache(config, vendor, tab.account.as_deref())?;
            let endpoints = crate::deeprelay::fetch::Endpoints::default();
            let outcome = fetch_api_keyed(
                async {
                    crate::deeprelay::fetch_snapshot(client, &api_key, &cache, &endpoints, ttl)
                        .await
                        .map(Into::into)
                },
                &api_key,
            )
            .await?;
            Ok(outcome)
        }
        VendorId::Moonshot => {
            let (api_key, cache) = api_key_and_cache(config, vendor, tab.account.as_deref())?;
            let (endpoints, currency) =
                crate::moonshot::fetch::Endpoints::for_region(&config.moonshot.region);
            let outcome = fetch_api_keyed(
                async {
                    crate::moonshot::fetch_snapshot(
                        client, &api_key, &cache, &endpoints, ttl, currency,
                    )
                    .await
                    .map(Into::into)
                },
                &api_key,
            )
            .await?;
            Ok(outcome)
        }
        VendorId::Grok => {
            let (key, cache) = api_key_and_cache(config, vendor, tab.account.as_deref())?;
            let endpoints = crate::grok::fetch::Endpoints::default();
            let outcome = fetch_api_keyed(
                async {
                    crate::grok::fetch_snapshot(
                        client,
                        &key,
                        &cache,
                        &endpoints,
                        ttl,
                        config.grok.team_id.as_deref(),
                    )
                    .await
                    .map(Into::into)
                },
                &key,
            )
            .await?;
            Ok(outcome)
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
                ttl,
            )
            .await?;
            Ok(outcome.into())
        }
        VendorId::Antigravity => {
            // No API key: the local Antigravity server is the source, and the
            // saved Google session stands in while nothing is running.
            let cache = crate::cache::Cache::for_vendor("antigravity")?;
            let oauth = crate::antigravity::cloud::OauthClient::from_config(
                config.antigravity.oauth_client_id.as_deref(),
                config.antigravity.oauth_client_secret.as_deref(),
            );
            let outcome = crate::antigravity::fetch_snapshot(
                client,
                &cache,
                ttl,
                oauth.as_ref(),
                config.antigravity.agy_binary.as_deref(),
                config.antigravity.ide_language_server,
            )
            .await?;
            Ok(outcome.into())
        }
        VendorId::Grokbot => {
            // The desktop app's own session is the login; refreshed pairs
            // persist only inside the vendor cache.
            let creds = crate::grokbot::resolve_credentials(&config.grokbot)?;
            let cache = crate::cache::Cache::for_vendor("grokbot")?;
            let endpoints = crate::grokbot::fetch::Endpoints::default();
            let outcome =
                crate::grokbot::fetch::fetch_snapshot_with(client, &creds, &cache, &endpoints, ttl)
                    .await?;
            Ok(outcome.into())
        }
        VendorId::ModelStudio => {
            // The bl CLI's own console session is the login; its region/site
            // pair picks the gateway, and only a token fingerprint persists.
            let creds = crate::modelstudio::resolve_credentials(&config.modelstudio)?;
            let cache = crate::cache::Cache::for_vendor("modelstudio")?;
            let endpoints =
                crate::modelstudio::fetch::Endpoints::for_gateway(creds.region, creds.site);
            let outcome =
                crate::modelstudio::fetch_snapshot_with(client, &creds, &cache, &endpoints, ttl)
                    .await?;
            Ok(outcome.into())
        }
        VendorId::MuseCode => {
            let cache = crate::cache::Cache::for_vendor("muse-code")?;
            Ok(crate::muse::fetch::fetch_snapshot(&config.muse, &cache)
                .await?
                .into())
        }
        VendorId::Devin => {
            let (creds_cfg, cache) = match tab.account.as_deref() {
                Some(label) => {
                    let creds_path = config.devin.resolve_credentials_path(Some(label))?;
                    let mut acct_cfg = config.devin.clone();
                    acct_cfg.credentials_path = Some(creds_path);
                    let cache = crate::cache::Cache::for_vendor_account("devin", label)?;
                    (acct_cfg, cache)
                }
                None => (
                    config.devin.clone(),
                    crate::cache::Cache::for_vendor("devin")?,
                ),
            };
            let outcome = crate::devin::fetch::fetch_snapshot(&creds_cfg, &cache, ttl).await?;
            Ok(outcome.into())
        }
        VendorId::Droid => {
            let cache = crate::cache::Cache::for_vendor("droid")?;
            let outcome = crate::droid::fetch::fetch_snapshot(&config.droid, &cache, ttl).await?;
            Ok(outcome.into())
        }
        VendorId::Minimax => {
            let (api_key, cache) = api_key_and_cache(config, vendor, tab.account.as_deref())?;
            let endpoints = crate::minimax::fetch::Endpoints::for_region(&config.minimax.region);
            let outcome = fetch_api_keyed(
                async {
                    crate::minimax::fetch_snapshot(client, &api_key, &cache, &endpoints, ttl)
                        .await
                        .map(Into::into)
                },
                &api_key,
            )
            .await?;
            Ok(outcome)
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
                ttl,
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
            let outcome = crate::kiro::fetch_snapshot(client, &db_path, &cache, ttl).await?;
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
            // Nous keeps no cache of its own, so every read is a live one.
            Ok(crate::outcome::Outcome::fresh(
                crate::usage::VendorSnapshot::NousResearch(account),
            ))
        }
        VendorId::OpenCodeGo => {
            let api_key = crate::opencode_go::creds::resolve_key(
                &config.opencode_go.api_key_env,
                config.opencode_go.api_key.as_deref(),
                config.opencode_go.auth_paths.as_deref(),
            )?;
            let cache = crate::cache::Cache::for_vendor("opencode-go")?.redacting(&api_key);
            let endpoints = crate::opencode_go::fetch::Endpoints::default();
            let outcome = fetch_api_keyed(
                async {
                    crate::opencode_go::fetch::fetch_snapshot(
                        client, &api_key, &cache, &endpoints, ttl,
                    )
                    .await
                    .map(Into::into)
                },
                &api_key,
            )
            .await?;
            Ok(outcome)
        }
        VendorId::Cline => {
            let credential = crate::cline::creds::resolve(
                config.cline.auth_paths.as_deref(),
                &config.cline.api_key_env,
                config.cline.api_key.as_deref(),
            )?;
            let cache = crate::cache::Cache::for_vendor("cline")?;
            let endpoints = crate::cline::fetch::Endpoints::default();
            let outcome = crate::cline::fetch::fetch_snapshot(
                client,
                &credential,
                &cache,
                &endpoints,
                ttl,
                Utc::now(),
            )
            .await?;
            Ok(outcome.into())
        }
        VendorId::CommandCode => {
            let credential =
                crate::commandcode::creds::resolve(config.commandcode.auth_paths.as_deref())?;
            let cache = crate::cache::Cache::for_vendor("commandcode")?;
            let endpoints = crate::commandcode::fetch::Endpoints::default();
            let outcome = crate::commandcode::fetch::fetch_snapshot(
                client,
                &credential.token,
                &cache,
                &endpoints,
                ttl,
            )
            .await?;
            Ok(outcome.into())
        }
        VendorId::Ollama => {
            let api_key = crate::config::resolve_api_key(
                "Ollama",
                &config.ollama.api_key_env,
                config.ollama.api_key.as_deref(),
            )?;
            let cache = crate::cache::Cache::for_vendor("ollama")?.redacting(&api_key);
            let endpoints = crate::ollama::fetch::Endpoints::default();
            let outcome = fetch_api_keyed(
                async {
                    crate::ollama::fetch_snapshot(
                        client,
                        &api_key,
                        &config.ollama.plan,
                        &cache,
                        &endpoints,
                        ttl,
                    )
                    .await
                    .map(Into::into)
                },
                &api_key,
            )
            .await?;
            Ok(outcome)
        }
        VendorId::OrcaRouter => {
            let (api_key, cache) = api_key_and_cache(config, vendor, tab.account.as_deref())?;
            let endpoints = crate::orcarouter::fetch::Endpoints::default();
            let outcome = fetch_api_keyed(
                async {
                    crate::orcarouter::fetch_snapshot(client, &api_key, &cache, &endpoints, ttl)
                        .await
                        .map(Into::into)
                },
                &api_key,
            )
            .await?;
            Ok(outcome)
        }
        VendorId::Lyceum => {
            let (api_key, cache) = api_key_and_cache(config, vendor, tab.account.as_deref())?;
            let outcome = fetch_api_keyed(
                async {
                    crate::lyceum::fetch::fetch_snapshot(client, &api_key, &cache, ttl)
                        .await
                        .map(Into::into)
                },
                &api_key,
            )
            .await?;
            Ok(outcome)
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
            if tab.vendor_id() == Some(VendorId::Anthropic) {
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
    fn mouse_nav_targets_select_overview_and_tabs() {
        let mut app = App::with_theme(
            vec![
                TabId::vendor(VendorId::Anthropic),
                TabId::vendor(VendorId::Openai),
            ],
            Theme::default(),
        );
        app.overview = true;
        app.nav_from_target(NavTarget::Tab(1));
        assert!(!app.overview);
        assert_eq!(app.active, 1);
        app.nav_from_target(NavTarget::Overview);
        assert!(app.overview);
        // Out-of-range tab indices are ignored.
        app.nav_from_target(NavTarget::Tab(99));
        assert!(app.overview);
    }

    #[test]
    fn hit_targets_default_to_empty() {
        let app = App::with_theme(Vec::new(), Theme::default());
        assert!(app.hit.borrow().nav_entries.is_empty());
        assert!(app.hit.borrow().footer_actions.is_empty());
        assert!(app.hit.borrow().settings_rows.is_empty());
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

        // A repeated vendor is listed once, at its first occurrence.
        app.overview_vendors = Some(vec![VendorId::Anthropic, VendorId::Anthropic]);
        assert_eq!(app.overview_tabs(), vec![0]);
    }

    fn config_with_accounts(labels: &[&str]) -> Config {
        let mut config = Config::default();
        // Keep only Anthropic enabled so the test asserts on account expansion,
        // not on the full default vendor set.
        config.openai.enabled = false;
        config.zai.enabled = false;
        config.openrouter.enabled = false;
        config.commandcode.enabled = false;
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
        empty.commandcode.enabled = false;
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
    fn tabs_follow_overview_vendors_and_keep_the_rest() {
        let mut config = Config::default();
        config.zai.enabled = false;
        config.openrouter.enabled = false;
        config.commandcode.enabled = false;
        config.supergrok.enabled = true;
        config.antigravity.enabled = true;
        config.opencode_go.enabled = true;
        config.copilot.enabled = true;
        config.ui.overview_vendors = Some(vec![
            VendorId::Anthropic,
            VendorId::Zai,
            VendorId::Openai,
            VendorId::Anthropic,
            VendorId::Supergrok,
            VendorId::OpenCodeGo,
            VendorId::Antigravity,
        ]);
        let vendors: Vec<VendorId> = tabs_from_config(&config)
            .iter()
            .filter_map(TabId::vendor_id)
            .collect();
        assert_eq!(
            vendors,
            vec![
                VendorId::Anthropic,
                VendorId::Openai,
                VendorId::Supergrok,
                VendorId::OpenCodeGo,
                VendorId::Antigravity,
                VendorId::Copilot,
            ]
        );
    }

    #[test]
    fn tabs_without_accounts_are_just_enabled_vendors() {
        // No [[anthropic.accounts]] → one tab per enabled vendor, unchanged.
        let config = Config::default();
        let tabs = tabs_from_config(&config);
        let vendors: Vec<VendorId> = tabs.iter().filter_map(TabId::vendor_id).collect();
        assert_eq!(vendors, config.enabled_vendors());
        assert!(tabs.iter().all(|t| t.account.is_none()));
    }

    #[test]
    fn tabs_expand_openrouter_accounts_without_changing_other_vendors() {
        let mut config = Config::default();
        config.anthropic.enabled = false;
        config.openai.enabled = false;
        config.zai.enabled = false;
        config.commandcode.enabled = false;
        config.openrouter.accounts = vec![
            crate::config::ApiKeyAccount {
                label: "work".into(),
                api_key_env: Some("OPENROUTER_WORK_API_KEY".into()),
                api_key: None,
                management_api_key_env: None,
            },
            crate::config::ApiKeyAccount {
                label: "personal".into(),
                api_key_env: None,
                api_key: Some("personal-key".into()),
                management_api_key_env: None,
            },
        ];
        assert_eq!(
            tabs_from_config(&config),
            vec![
                TabId::vendor(VendorId::Openrouter),
                TabId::account_for(VendorId::Openrouter, "work"),
                TabId::account_for(VendorId::Openrouter, "personal"),
            ]
        );
    }

    #[test]
    fn openai_named_accounts_get_their_own_tabs_after_the_default() {
        let mut config = Config::default();
        config.anthropic.enabled = false;
        config.zai.enabled = false;
        config.openrouter.enabled = false;
        config.commandcode.enabled = false;
        config.openai.accounts.push(crate::config::OpenAiAccount {
            label: "work".into(),
            codex_auth_path: "/tmp/codex-work/auth.json".into(),
        });
        assert_eq!(
            tabs_from_config(&config),
            vec![
                TabId::vendor(VendorId::Openai),
                TabId::account_for(VendorId::Openai, "work"),
            ]
        );
    }

    #[test]
    fn copilot_named_accounts_get_their_own_tabs_and_can_hide_the_default() {
        let base = || {
            let mut config = Config::default();
            config.anthropic.enabled = false;
            config.openai.enabled = false;
            config.zai.enabled = false;
            config.openrouter.enabled = false;
            config.commandcode.enabled = false;
            config.copilot.enabled = true;
            config
        };

        // No accounts: exactly the pre-#378 single tab.
        assert_eq!(
            tabs_from_config(&base()),
            vec![TabId::vendor(VendorId::Copilot)]
        );

        // Named accounts follow the default, in config order.
        let mut config = base();
        config.copilot.accounts.push(crate::config::CopilotAccount {
            label: "work".into(),
            user: "octocat-work".into(),
        });
        config.copilot.accounts.push(crate::config::CopilotAccount {
            label: "personal".into(),
            user: "octocat".into(),
        });
        assert_eq!(
            tabs_from_config(&config),
            vec![
                TabId::vendor(VendorId::Copilot),
                TabId::account_for(VendorId::Copilot, "work"),
                TabId::account_for(VendorId::Copilot, "personal"),
            ]
        );

        // The ambient active-gh tab is suppressible once every account is named.
        config.copilot.show_default_account = false;
        assert_eq!(
            tabs_from_config(&config),
            vec![
                TabId::account_for(VendorId::Copilot, "work"),
                TabId::account_for(VendorId::Copilot, "personal"),
            ]
        );

        // But never when it would leave Copilot with no tab at all.
        let mut empty = base();
        empty.copilot.show_default_account = false;
        assert_eq!(
            tabs_from_config(&empty),
            vec![TabId::vendor(VendorId::Copilot)]
        );
    }

    #[test]
    fn devin_named_accounts_get_their_own_tabs_and_can_hide_the_default() {
        let base = || {
            let mut config = Config::default();
            config.anthropic.enabled = false;
            config.openai.enabled = false;
            config.zai.enabled = false;
            config.openrouter.enabled = false;
            config.commandcode.enabled = false;
            config.devin.enabled = true;
            config
        };

        // No accounts: exactly the single default tab.
        assert_eq!(
            tabs_from_config(&base()),
            vec![TabId::vendor(VendorId::Devin)]
        );

        // Named accounts follow the default, in config order.
        let mut config = base();
        config.devin.accounts.push(crate::config::DevinAccount {
            label: "work".into(),
            credentials_path: "/tmp/devin-work/credentials.toml".into(),
        });
        config.devin.accounts.push(crate::config::DevinAccount {
            label: "personal".into(),
            credentials_path: "/tmp/devin-personal/credentials.toml".into(),
        });
        assert_eq!(
            tabs_from_config(&config),
            vec![
                TabId::vendor(VendorId::Devin),
                TabId::account_for(VendorId::Devin, "work"),
                TabId::account_for(VendorId::Devin, "personal"),
            ]
        );

        // Hiding the default tab leaves only the named ones.
        config.devin.show_default_account = false;
        assert_eq!(
            tabs_from_config(&config),
            vec![
                TabId::account_for(VendorId::Devin, "work"),
                TabId::account_for(VendorId::Devin, "personal"),
            ]
        );

        // But never when it would leave Devin with no tab at all.
        let mut empty = base();
        empty.devin.show_default_account = false;
        assert_eq!(
            tabs_from_config(&empty),
            vec![TabId::vendor(VendorId::Devin)]
        );
    }

    #[test]
    fn openrouter_can_hide_default_only_when_named_accounts_exist() {
        let mut config = Config::default();
        config.anthropic.enabled = false;
        config.openai.enabled = false;
        config.zai.enabled = false;
        config.commandcode.enabled = false;
        config.openrouter.show_default_account = false;
        assert_eq!(
            tabs_from_config(&config),
            vec![TabId::vendor(VendorId::Openrouter)]
        );

        config
            .openrouter
            .accounts
            .push(crate::config::ApiKeyAccount {
                label: "work".into(),
                api_key_env: Some("OPENROUTER_WORK_API_KEY".into()),
                api_key: None,
                management_api_key_env: None,
            });
        assert_eq!(
            tabs_from_config(&config),
            vec![TabId::account_for(VendorId::Openrouter, "work")]
        );
    }

    #[test]
    fn every_api_key_account_vendor_fans_out_like_openrouter() {
        let config_for = |vendor: VendorId, show_default: bool| -> Config {
            let section = vendor.config_section();
            // Switch off the vendors that are on by default, so the tab list
            // is this vendor's alone.
            let mut text: String = ["anthropic", "openai", "commandcode", "zai", "openrouter"]
                .into_iter()
                .filter(|other| *other != section)
                .map(|other| format!("[{other}]\nenabled = false\n"))
                .collect();
            text.push_str(&format!(
                "[{section}]\nenabled = true\nshow_default_account = {show_default}\n\
                 [[{section}.accounts]]\nlabel = \"work\"\napi_key = \"k1\"\n\
                 [[{section}.accounts]]\nlabel = \"personal\"\napi_key = \"k2\"\n"
            ));
            let config: Config = toml::from_str(&text).unwrap();
            config.validate().unwrap();
            config
        };
        for vendor in Config::API_KEY_ACCOUNT_VENDORS {
            let named = vec![
                TabId::account_for(vendor, "work"),
                TabId::account_for(vendor, "personal"),
            ];
            let mut with_default = vec![TabId::vendor(vendor)];
            with_default.extend(named.clone());
            assert_eq!(
                tabs_from_config(&config_for(vendor, true)),
                with_default,
                "{vendor:?}"
            );
            assert_eq!(
                tabs_from_config(&config_for(vendor, false)),
                named,
                "{vendor:?}"
            );
        }
    }

    #[test]
    fn codex_can_hide_default_only_when_named_accounts_exist() {
        let mut config = Config::default();
        config.anthropic.enabled = false;
        config.zai.enabled = false;
        config.openrouter.enabled = false;
        config.commandcode.enabled = false;
        config.openai.show_default_account = false;
        assert_eq!(
            tabs_from_config(&config),
            vec![TabId::vendor(VendorId::Openai)]
        );

        config.openai.accounts.push(crate::config::OpenAiAccount {
            label: "work".into(),
            codex_auth_path: "/tmp/codex-work/auth.json".into(),
        });
        assert_eq!(
            tabs_from_config(&config),
            vec![TabId::account_for(VendorId::Openai, "work")]
        );
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
        config.commandcode.enabled = false;
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
        app.tabs[0] = TabState::error("old");
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
            TabState::error("old result"),
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
            TabState::error("wrong tab"),
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
        assert!(app.apply_refresh(generation, &anthropic, TabState::error("ready")));
        assert!(matches!(app.tabs[0], TabState::Loading));
        assert!(matches!(&app.tabs[1], TabState::Error { message, .. } if message == "ready"));
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
                recent_models: Vec::new(),
            }),
            stale: false,
            last_error: None,
            fetched_at: Some(fetched_at),
            display: Default::default(),
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

        assert!(app.apply_refresh(app.tab_generation, &tab, TabState::error("not signed in"),));
        assert!(!app.is_refreshing(&tab));
        assert!(
            matches!(&app.tabs[0], TabState::Error { message, .. } if message == "not signed in")
        );
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
        assert!(app.apply_refresh(app.tab_generation, &tab, TabState::error("refresh failed"),));
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

        assert!(!app.apply_refresh(old_generation, &tab, TabState::error("old result"),));
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

    fn custom_spec(id: &str, enabled: bool) -> CustomProviderConfig {
        CustomProviderConfig {
            id: id.into(),
            name: "My Tool".into(),
            short_name: "myt".into(),
            enabled,
            ..Default::default()
        }
    }

    #[test]
    fn custom_providers_get_tabs_after_every_builtin() {
        let config = Config {
            custom: vec![custom_spec("mytool", true), custom_spec("other", true)],
            ..Default::default()
        };
        let tabs = tabs_from_config(&config);
        let builtin_count = config.enabled_vendors().len();
        assert_eq!(tabs.len(), builtin_count + 2);
        assert!(
            tabs[..builtin_count]
                .iter()
                .all(|t| matches!(t.source, TabSource::Builtin(_)))
        );
        assert_eq!(tabs[builtin_count], TabId::custom(&config.custom[0]));
        assert_eq!(tabs[builtin_count + 1], TabId::custom(&config.custom[1]));
        assert_eq!(
            tabs[builtin_count].source,
            TabSource::Custom {
                id: "mytool".into(),
                name: "My Tool".into(),
                short_name: "myt".into(),
            }
        );
        assert!(tabs[builtin_count].account.is_none());
        assert!(!tabs[builtin_count].desktop);
        assert_eq!(tabs[builtin_count].vendor_id(), None);
    }

    #[test]
    fn disabled_custom_provider_has_no_tab() {
        let config = Config {
            custom: vec![custom_spec("mytool", false)],
            ..Default::default()
        };
        let tabs = tabs_from_config(&config);
        assert!(
            tabs.iter()
                .all(|t| matches!(t.source, TabSource::Builtin(_)))
        );
        assert_eq!(tabs.len(), config.enabled_vendors().len());
    }

    #[test]
    fn custom_tabs_are_listed_by_the_default_overview_but_not_by_a_vendor_filter() {
        let spec = custom_spec("mytool", true);
        let mut app = App::with_theme(
            vec![
                TabId::vendor(VendorId::Anthropic),
                TabId::custom(&spec),
                TabId::vendor(VendorId::Openai),
            ],
            Theme::default(),
        );
        assert_eq!(app.overview_tabs(), vec![0, 1, 2]);

        // `overview_vendors` names VendorIds only, so the custom tab drops out.
        app.overview_vendors = Some(vec![VendorId::Openai, VendorId::Anthropic]);
        assert_eq!(app.overview_tabs(), vec![2, 0]);
    }

    #[test]
    fn a_custom_active_tab_has_no_vendor_and_never_staggers() {
        let spec = custom_spec("mytool", true);
        let tabs = vec![
            TabId::vendor(VendorId::Anthropic),
            TabId::custom(&spec),
            TabId::account("work"),
        ];
        let mut app = App::with_theme(tabs.clone(), Theme::default());
        app.active = 1;
        assert_eq!(app.active_vendor(), None);
        assert_eq!(app.active_tab_id(), Some(&TabId::custom(&spec)));

        // select_primary never lands on a custom tab.
        app.select_primary(Some(VendorId::Anthropic));
        assert_eq!(app.active, 0);

        let step = Duration::from_millis(800);
        assert_eq!(
            refresh_stagger(&tabs, step),
            vec![Duration::ZERO, Duration::ZERO, step]
        );
    }
}
