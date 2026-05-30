//! Shared vendor IDs and renderer/fetcher structs used by the widget and TUI.
//!
//! Snapshots remain a discriminated `VendorSnapshot` enum because the four
//! vendors have genuinely different shapes — see `usage.rs`.

use std::time::Duration;

use clap::ValueEnum;

use crate::usage::VendorSnapshot;
use crate::widget::cli::Cli;

/// Outer reqwest client timeout shared by widget and TUI entry points.
/// Vendor fetchers still apply their own tighter per-request timeouts.
pub const HTTP_CLIENT_TIMEOUT: Duration = Duration::from_secs(30);

/// Stable enum used by `--vendor` and in config files.
#[derive(
    Debug, Clone, Copy, ValueEnum, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize,
)]
#[serde(rename_all = "lowercase")]
pub enum VendorId {
    Anthropic,
    Openai,
    Zai,
    Openrouter,
    Deepseek,
}

impl VendorId {
    pub fn slug(self) -> &'static str {
        match self {
            VendorId::Anthropic => "anthropic",
            VendorId::Openai => "openai",
            VendorId::Zai => "zai",
            VendorId::Openrouter => "openrouter",
            VendorId::Deepseek => "deepseek",
        }
    }

    pub fn all() -> &'static [VendorId] {
        &[
            VendorId::Anthropic,
            VendorId::Openai,
            VendorId::Zai,
            VendorId::Openrouter,
            VendorId::Deepseek,
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
