//! TUI app state — vendors, tab selection, per-vendor snapshot cache.

use std::time::Duration;

use chrono::Utc;
use reqwest::Client;

use crate::cache::{Cache, DEFAULT_TTL};
use crate::config::Config;
use crate::error::Result;
use crate::theme::Theme;
use crate::vendor::{RenderOpts, VendorId, VendorOutcome};
use crate::waybar::WaybarOutput;
use crate::{anthropic, openai, openrouter, zai};

/// What we display per vendor — either a successfully rendered tooltip, or
/// the error string from a failed fetch.
#[derive(Debug, Clone)]
pub enum TabState {
    Loading,
    Ready { tooltip_pango: String, bar_text_pango: String },
    Error(String),
}

#[derive(Debug)]
pub struct App {
    pub vendors: Vec<VendorId>,
    pub active: usize,
    pub tabs: Vec<TabState>,
    pub theme: Theme,
    pub last_refresh: chrono::DateTime<chrono::Utc>,
    pub quit: bool,
}

impl App {
    pub fn new(vendors: Vec<VendorId>) -> Self {
        let n = vendors.len();
        Self {
            vendors,
            active: 0,
            tabs: vec![TabState::Loading; n],
            theme: Theme::default().merged_with_omarchy(),
            last_refresh: Utc::now(),
            quit: false,
        }
    }

    pub fn active_vendor(&self) -> Option<VendorId> {
        self.vendors.get(self.active).copied()
    }

    pub fn next_tab(&mut self) {
        if !self.vendors.is_empty() {
            self.active = (self.active + 1) % self.vendors.len();
        }
    }

    pub fn prev_tab(&mut self) {
        if !self.vendors.is_empty() {
            self.active = (self.active + self.vendors.len() - 1) % self.vendors.len();
        }
    }
}

/// Fetch and render one vendor — returns a `TabState`.
pub async fn refresh_one(
    client: &Client,
    config: &Config,
    theme: &Theme,
    vendor: VendorId,
) -> TabState {
    match fetch_and_render(client, config, theme, vendor).await {
        Ok(out) => TabState::Ready {
            tooltip_pango: out.tooltip,
            bar_text_pango: out.text,
        },
        Err(e) => TabState::Error(e.to_string()),
    }
}

async fn fetch_and_render(
    client: &Client,
    config: &Config,
    theme: &Theme,
    vendor: VendorId,
) -> Result<WaybarOutput> {
    let opts = RenderOpts {
        format: None,
        tooltip_format: None,
        icon: None,
        pace_tolerance: 5,
        format_pace_color: false,
        tooltip_pace_pts: true, // TUI always shows elapsed-position markers
    };
    let now = Utc::now();
    match vendor {
        VendorId::Anthropic => {
            let cache = Cache::for_vendor("anthropic")?;
            let creds_path = config
                .anthropic
                .credentials_path
                .clone()
                .unwrap_or_else(|| anthropic::creds::default_path().unwrap_or_default());
            let endpoints = anthropic::fetch::Endpoints::default();
            let outcome = anthropic::fetch_snapshot(
                client,
                &creds_path,
                &cache,
                &endpoints,
                DEFAULT_TTL,
            )
            .await?;
            let render = crate::widget::render::RenderInput {
                outcome: &outcome,
                theme,
                format: crate::widget::render::DEFAULT_FORMAT,
                tooltip_format: None,
                icon: None,
                pace_tolerance: opts.pace_tolerance,
                format_pace_color: false,
                tooltip_pace_pts: opts.tooltip_pace_pts,
                now,
            };
            Ok(crate::widget::render::render_anthropic(&render))
        }
        VendorId::Openrouter => {
            let api_key = std::env::var(&config.openrouter.api_key_env).map_err(|_| {
                crate::error::AppError::Credentials(format!(
                    "{} not set",
                    config.openrouter.api_key_env
                ))
            })?;
            let cache = Cache::for_vendor("openrouter")?;
            let endpoints = openrouter::fetch::Endpoints::default();
            let outcome = openrouter::fetch_snapshot(client, &api_key, &cache, &endpoints, DEFAULT_TTL).await?;
            let snap = outcome.snapshot.clone();
            let vo: VendorOutcome = outcome.into();
            Ok(openrouter::vendor::render(&vo, &snap, theme, &opts, now))
        }
        VendorId::Zai => {
            let api_key = std::env::var(&config.zai.api_key_env).map_err(|_| {
                crate::error::AppError::Credentials(format!("{} not set", config.zai.api_key_env))
            })?;
            let cache = Cache::for_vendor("zai")?;
            let endpoints = zai::fetch::Endpoints::default();
            let outcome = zai::fetch_snapshot(
                client,
                &api_key,
                &cache,
                &endpoints,
                DEFAULT_TTL,
                config.zai.plan_tier.as_deref(),
            )
            .await?;
            let snap = outcome.snapshot.clone();
            let vo: VendorOutcome = outcome.into();
            Ok(zai::vendor::render(&vo, &snap, theme, &opts, now))
        }
        VendorId::Openai => {
            let cache = Cache::for_vendor("openai")?;
            let creds_path = config
                .openai
                .codex_auth_path
                .clone()
                .unwrap_or_else(|| openai::creds::default_path().unwrap_or_default());
            let endpoints = openai::fetch::Endpoints::default();
            let outcome = openai::fetch_snapshot(client, &creds_path, &cache, &endpoints, DEFAULT_TTL).await?;
            let snap = outcome.snapshot.clone();
            let vo: VendorOutcome = outcome.into();
            Ok(openai::vendor::render(&vo, &snap, theme, &opts, now))
        }
    }
}

/// Convenience for the watch-driven binary: how long to wait between
/// automatic refreshes.
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(60);
