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

/// Credential-bearing environment variables owned by ai-usagebar vendors.
/// Subprocesses receive only the entries that belong to their own provider.
pub(crate) const VENDOR_SECRET_ENV_VARS: &[&str] = &[
    "ZAI_API_KEY",
    "OPENROUTER_API_KEY",
    "DEEPSEEK_API_KEY",
    "KIMI_API_KEY",
    "KILO_API_KEY",
    "NOVITA_API_KEY",
    "MINIMAX_API_KEY",
    "MOONSHOT_API_KEY",
    "XAI_MANAGEMENT_KEY",
    "ANTHROPIC_ADMIN_KEY",
    "GITHUB_TOKEN",
    "GH_TOKEN",
    "XAI_API_KEY",
    "GROK_API_KEY",
    "OPENCODE_GO_API_KEY",
];

pub(crate) fn vendor_secret_env_vars_to_remove(keep: &[&str]) -> Vec<&'static str> {
    VENDOR_SECRET_ENV_VARS
        .iter()
        .copied()
        .filter(|var| !keep.contains(var))
        .collect()
}

/// Follow ordinary vendor redirects without forwarding non-standard API-key
/// headers to a different origin. Reqwest strips `Authorization` on sensitive
/// redirects, but vendors also use headers such as `x-api-key`, which are not
/// covered by that built-in list.
pub fn same_origin_redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 10 {
            return attempt.error("too many redirects");
        }
        let Some(origin) = attempt.previous().first() else {
            return attempt.stop();
        };
        let target = attempt.url();
        if target.scheme() == origin.scheme()
            && target.host_str() == origin.host_str()
            && target.port_or_known_default() == origin.port_or_known_default()
        {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

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
    Supergrok,
    Antigravity,
    Cursor,
    Copilot,
    Minimax,
    Kiro,
    #[serde(rename = "nous")]
    NousResearch,
    #[serde(rename = "opencode-go")]
    OpenCodeGo,
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
            VendorId::Supergrok => "supergrok",
            VendorId::Antigravity => "antigravity",
            VendorId::Cursor => "cursor",
            VendorId::Copilot => "copilot",
            VendorId::Minimax => "minimax",
            VendorId::Kiro => "kiro",
            VendorId::NousResearch => "nous",
            VendorId::OpenCodeGo => "opencode-go",
        }
    }

    /// Canonical human-readable name for shared reports and compact UI labels.
    /// Platform frontends may add context (for example, "GLM (Z.AI)" in a
    /// wide TUI tab), but should not carry their own full vendor-name table.
    pub fn display_name(self) -> &'static str {
        match self {
            VendorId::Anthropic => "Claude",
            VendorId::AnthropicApi => "Anthropic API",
            VendorId::Openai => "Codex",
            VendorId::Zai => "Z.AI",
            VendorId::Openrouter => "OpenRouter",
            VendorId::Deepseek => "DeepSeek",
            VendorId::Kimi => "Kimi",
            VendorId::Kilo => "Kilo",
            VendorId::Novita => "Novita",
            VendorId::Moonshot => "Moonshot",
            VendorId::Grok => "Grok",
            VendorId::Supergrok => "SuperGrok",
            VendorId::Antigravity => "Antigravity",
            VendorId::Cursor => "Cursor",
            VendorId::Copilot => "GitHub Copilot",
            VendorId::Minimax => "MiniMax",
            VendorId::Kiro => "Kiro",
            VendorId::NousResearch => "Nous Research",
            VendorId::OpenCodeGo => "OpenCode Go",
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
            VendorId::Supergrok,
            VendorId::Antigravity,
            VendorId::Cursor,
            VendorId::Copilot,
            VendorId::Minimax,
            VendorId::Kiro,
            VendorId::NousResearch,
            VendorId::OpenCodeGo,
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
    fn every_vendor_has_stable_machine_and_display_names() {
        for vendor in VendorId::all() {
            assert!(!vendor.slug().is_empty());
            assert!(!vendor.display_name().is_empty());
        }
        assert_eq!(VendorId::Anthropic.slug(), "anthropic");
        assert_eq!(VendorId::Anthropic.display_name(), "Claude");
        assert_eq!(VendorId::Openai.display_name(), "Codex");
        assert_eq!(VendorId::Zai.display_name(), "Z.AI");
    }

    #[test]
    fn new_vendor_contracts_keep_public_names_and_slugs() {
        assert_eq!(VendorId::NousResearch.slug(), "nous");
        assert_eq!(VendorId::NousResearch.display_name(), "Nous Research");
        assert_eq!(VendorId::OpenCodeGo.slug(), "opencode-go");
        assert_eq!(VendorId::OpenCodeGo.display_name(), "OpenCode Go");
        assert_eq!(
            serde_json::to_value(VendorId::OpenCodeGo).unwrap(),
            serde_json::json!("opencode-go")
        );
    }

    #[test]
    fn vendor_secret_env_vars_cover_config_defaults() {
        let configured_defaults = [
            "ZAI_API_KEY",
            "OPENROUTER_API_KEY",
            "DEEPSEEK_API_KEY",
            "KIMI_API_KEY",
            "KILO_API_KEY",
            "NOVITA_API_KEY",
            "MINIMAX_API_KEY",
            "MOONSHOT_API_KEY",
            "XAI_MANAGEMENT_KEY",
            "ANTHROPIC_ADMIN_KEY",
        ];
        for name in configured_defaults {
            assert!(VENDOR_SECRET_ENV_VARS.contains(&name), "missing {name}");
        }
    }

    #[test]
    fn vars_to_remove_preserves_only_requested_grok_credentials() {
        let removed = vendor_secret_env_vars_to_remove(&["XAI_API_KEY", "GROK_API_KEY"]);
        assert!(!removed.contains(&"XAI_API_KEY"));
        assert!(!removed.contains(&"GROK_API_KEY"));
        assert!(removed.contains(&"ANTHROPIC_ADMIN_KEY"));
        assert!(removed.contains(&"OPENROUTER_API_KEY"));
        assert_eq!(removed.len(), VENDOR_SECRET_ENV_VARS.len() - 2);
    }

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

    #[tokio::test]
    async fn same_origin_redirects_still_work_with_vendor_headers() {
        let mut server = mockito::Server::new_async().await;
        let redirect = server
            .mock("GET", "/start")
            .match_header("x-api-key", "secret")
            .with_status(302)
            .with_header("location", "/finish")
            .create_async()
            .await;
        let finish = server
            .mock("GET", "/finish")
            .match_header("x-api-key", "secret")
            .with_status(200)
            .create_async()
            .await;
        let client = reqwest::Client::builder()
            .redirect(same_origin_redirect_policy())
            .build()
            .unwrap();

        let response = client
            .get(format!("{}/start", server.url()))
            .header("x-api-key", "secret")
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        redirect.assert_async().await;
        finish.assert_async().await;
    }

    #[tokio::test]
    async fn cross_origin_redirects_are_not_followed_with_vendor_headers() {
        let mut origin = mockito::Server::new_async().await;
        let mut target = mockito::Server::new_async().await;
        let target_url = format!("{}/capture", target.url());
        let redirect = origin
            .mock("GET", "/start")
            .match_header("x-api-key", "secret")
            .with_status(302)
            .with_header("location", &target_url)
            .create_async()
            .await;
        let capture = target
            .mock("GET", "/capture")
            .expect(0)
            .create_async()
            .await;
        let client = reqwest::Client::builder()
            .redirect(same_origin_redirect_policy())
            .build()
            .unwrap();

        let response = client
            .get(format!("{}/start", origin.url()))
            .header("x-api-key", "secret")
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), reqwest::StatusCode::FOUND);
        redirect.assert_async().await;
        capture.assert_async().await;
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
