//! Shared vendor IDs and renderer/fetcher structs used by the widget and TUI.
//!
//! Snapshots remain a discriminated `VendorSnapshot` enum because the vendors
//! have genuinely different shapes — see `usage.rs`.

use std::time::Duration;

use clap::ValueEnum;

use crate::usage::VendorSnapshot;
use crate::widget::cli::Cli;

/// Outer reqwest client timeout shared by widget and TUI entry points.
/// Vendor fetchers still apply their own tighter per-request timeouts.
pub const HTTP_CLIENT_TIMEOUT: Duration = Duration::from_secs(30);

/// Upper bound on a vendor response body. Every one of these endpoints returns
/// a small JSON document — the largest observed is a few kilobytes — so this is
/// generous by three orders of magnitude while still bounding the damage from a
/// misbehaving proxy or a hijacked endpoint.
pub const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// Read a response body with an upper bound.
///
/// Every vendor buffered the whole body with `resp.bytes()` *before* anything
/// validated it. The widget is re-executed by Waybar every 60s, so an endpoint
/// answering with an unbounded stream had a free hand at the machine's memory.
/// `Content-Length` is checked first when present, then the body is read in
/// chunks so a lying or absent length cannot get past the cap either.
pub async fn read_body_capped(
    mut resp: reqwest::Response,
    max: usize,
) -> crate::error::Result<Vec<u8>> {
    let too_big = |n: u64| {
        crate::error::AppError::Schema(format!(
            "response body exceeds the {max}-byte limit ({n} bytes); refusing to buffer it"
        ))
    };
    if let Some(len) = resp.content_length()
        && len > max as u64
    {
        return Err(too_big(len));
    }
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        if chunk.len() > max.saturating_sub(buf.len()) {
            return Err(too_big(buf.len().saturating_add(chunk.len()) as u64));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

/// Stable enum used by `--vendor` and in config files.
#[derive(
    Debug, Clone, Copy, ValueEnum, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize,
)]
#[serde(rename_all = "lowercase")]
pub enum VendorId {
    Anthropic,
    #[serde(rename = "anthropic_api")]
    AnthropicApi,
    Openai,
    Zai,
    Openrouter,
    Deepseek,
    Kimi,
    Kilo,
    Novita,
    Moonshot,
    Grok,
    Antigravity,
}

impl VendorId {
    pub fn slug(self) -> &'static str {
        match self {
            VendorId::Anthropic => "anthropic",
            VendorId::AnthropicApi => "anthropic_api",
            VendorId::Openai => "openai",
            VendorId::Zai => "zai",
            VendorId::Openrouter => "openrouter",
            VendorId::Deepseek => "deepseek",
            VendorId::Kimi => "kimi",
            VendorId::Kilo => "kilo",
            VendorId::Novita => "novita",
            VendorId::Moonshot => "moonshot",
            VendorId::Grok => "grok",
            VendorId::Antigravity => "antigravity",
        }
    }

    pub fn all() -> &'static [VendorId] {
        &[
            VendorId::Anthropic,
            VendorId::AnthropicApi,
            VendorId::Openai,
            VendorId::Zai,
            VendorId::Openrouter,
            VendorId::Deepseek,
            VendorId::Kimi,
            VendorId::Kilo,
            VendorId::Novita,
            VendorId::Moonshot,
            VendorId::Grok,
            VendorId::Antigravity,
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

    #[tokio::test]
    async fn body_over_the_cap_is_refused_and_under_it_round_trips() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/big")
            .with_status(200)
            .with_body("x".repeat(4096))
            .create_async()
            .await;
        server
            .mock("GET", "/small")
            .with_status(200)
            .with_body("hello")
            .create_async()
            .await;

        let client = reqwest::Client::new();

        // Over the cap: refused rather than buffered.
        let resp = client
            .get(format!("{}/big", server.url()))
            .send()
            .await
            .unwrap();
        let err = read_body_capped(resp, 1024).await.unwrap_err();
        assert!(
            err.to_string().contains("exceeds"),
            "unexpected error: {err}"
        );

        // Under the cap: identical to the previous `resp.bytes()` behaviour.
        let resp = client
            .get(format!("{}/small", server.url()))
            .send()
            .await
            .unwrap();
        assert_eq!(read_body_capped(resp, 1024).await.unwrap(), b"hello");
    }

    #[tokio::test]
    async fn chunked_body_without_content_length_still_hits_the_cap() {
        let mut server = mockito::Server::new_async().await;
        server
            .mock("GET", "/chunked")
            .with_status(200)
            .with_chunked_body(|writer| writer.write_all(&[b'x'; 4096]))
            .create_async()
            .await;

        let response = reqwest::Client::new()
            .get(format!("{}/chunked", server.url()))
            .send()
            .await
            .unwrap();
        assert!(response.content_length().is_none());
        let error = read_body_capped(response, 1024).await.unwrap_err();
        assert!(error.to_string().contains("exceeds"), "{error}");
    }

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
