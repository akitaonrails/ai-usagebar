//! Vendor abstraction. Each vendor (`anthropic`, `openai`, `zai`,
//! `openrouter`) implements the `Vendor` trait so the widget and TUI can
//! dispatch over them without knowing the wire details.
//!
//! Snapshots remain a discriminated `VendorSnapshot` enum because the four
//! vendors have genuinely different shapes — see `usage.rs`. The trait is a
//! narrow surface around fetching + rendering; the actual data passes through
//! as a snapshot variant.

use std::collections::HashMap;

use async_trait::async_trait;
use clap::ValueEnum;

use crate::error::Result;
use crate::theme::Theme;
use crate::usage::VendorSnapshot;
use crate::widget::cli::Cli;

/// Stable enum used by `--vendor` and in config files.
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VendorId {
    Anthropic,
    Openai,
    Zai,
    Openrouter,
}

impl VendorId {
    pub fn slug(self) -> &'static str {
        match self {
            VendorId::Anthropic => "anthropic",
            VendorId::Openai => "openai",
            VendorId::Zai => "zai",
            VendorId::Openrouter => "openrouter",
        }
    }

    pub fn all() -> &'static [VendorId] {
        &[
            VendorId::Anthropic,
            VendorId::Openai,
            VendorId::Zai,
            VendorId::Openrouter,
        ]
    }
}

/// What a vendor returns from a successful fetch — snapshot + meta. Mirrors
/// `anthropic::fetch::FetchOutcome` but vendor-agnostic.
#[derive(Debug, Clone)]
pub struct VendorOutcome {
    pub snapshot: VendorSnapshot,
    pub stale: bool,
    pub last_error: Option<(u16, String)>,
    pub cache_age: Option<std::time::Duration>,
}

/// Options forwarded to renderers from the CLI.
#[derive(Debug, Clone)]
pub struct RenderOpts {
    pub format: Option<String>,
    pub tooltip_format: Option<String>,
    pub icon: Option<String>,
    pub pace_tolerance: u32,
    pub format_pace_color: bool,
    pub tooltip_pace_pts: bool,
}

impl RenderOpts {
    pub fn from_cli(cli: &Cli) -> Self {
        Self {
            format: cli.format.clone(),
            tooltip_format: cli.tooltip_format.clone(),
            icon: cli.icon.clone(),
            pace_tolerance: cli.pace_tolerance,
            format_pace_color: cli.format_pace_color,
            tooltip_pace_pts: cli.tooltip_pace_pts,
        }
    }
}

/// Vendor surface — fetch + render. Implementations live in
/// `crate::{anthropic,openai,zai,openrouter}`.
#[async_trait]
pub trait Vendor: Send + Sync {
    fn id(&self) -> VendorId;
    fn display_name(&self) -> String;
    fn default_format(&self) -> &'static str;
    async fn fetch(&self, cli: &Cli) -> Result<VendorOutcome>;
    fn placeholders(
        &self,
        snap: &VendorSnapshot,
        theme: &Theme,
        opts: &RenderOpts,
        now: chrono::DateTime<chrono::Utc>,
    ) -> HashMap<&'static str, String>;
    fn render_tooltip(
        &self,
        outcome: &VendorOutcome,
        theme: &Theme,
        opts: &RenderOpts,
        now: chrono::DateTime<chrono::Utc>,
    ) -> String;
    fn severity(&self, snap: &VendorSnapshot) -> crate::pacing::PaceSeverity;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendor_id_slug_round_trip() {
        for id in VendorId::all() {
            assert_eq!(
                id.slug(),
                serde_json::to_value(id).unwrap().as_str().unwrap()
            );
        }
    }
}
