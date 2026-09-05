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
    "XAI_API_KEY",
    "GROK_API_KEY",
    "OPENCODE_GO_API_KEY",
    "COMMANDCODE_API_KEY",
    "GITHUB_COPILOT_TOKEN",
    "GH_TOKEN",
    "GITHUB_TOKEN",
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
    Copilot,
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
    Minimax,
    Kiro,
    #[serde(rename = "nous")]
    NousResearch,
    #[serde(rename = "opencode-go")]
    OpenCodeGo,
    #[serde(rename = "commandcode")]
    CommandCode,
}

impl VendorId {
    pub fn slug(self) -> &'static str {
        match self {
            VendorId::Anthropic => "anthropic",
            VendorId::AnthropicApi => "anthropic_api",
            VendorId::Openai => "openai",
            VendorId::Copilot => "copilot",
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
            VendorId::Minimax => "minimax",
            VendorId::Kiro => "kiro",
            VendorId::NousResearch => "nous",
            VendorId::OpenCodeGo => "opencode-go",
            VendorId::CommandCode => "commandcode",
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
            VendorId::Copilot => "GitHub Copilot",
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
            VendorId::Minimax => "MiniMax",
            VendorId::Kiro => "Kiro",
            VendorId::NousResearch => "Nous Research",
            VendorId::OpenCodeGo => "OpenCode Go",
            VendorId::CommandCode => "Command Code",
        }
    }

    /// Glyph for a compact bar chip. Same role as [`Self::short_name`]: the
    /// Omarchy top bar (and any other frontend) takes it from `usage --json`
    /// rather than keeping its own provider-icon table.
    pub const fn bar_icon(self) -> &'static str {
        match self {
            VendorId::Anthropic => "󰚩",
            VendorId::AnthropicApi => "󰢗",
            VendorId::Openai => "󱢆",
            VendorId::Copilot => "󰊤",
            VendorId::Zai => "zai",
            VendorId::Openrouter => "󱙺",
            VendorId::Deepseek => "󰧑",
            VendorId::Kimi => "kmi",
            VendorId::Kilo => "󰭟",
            VendorId::Novita => "󰄔",
            VendorId::Moonshot => "msh",
            VendorId::Grok | VendorId::Supergrok => "󰇷",
            VendorId::Antigravity => "agy",
            VendorId::Cursor => "❯",
            VendorId::Minimax => "mmx",
            VendorId::Kiro => "◆",
            VendorId::NousResearch => "nrs",
            VendorId::OpenCodeGo => "ocg",
            VendorId::CommandCode => "cmc",
        }
    }

    /// Compact three-letter code for the bar. This is the single source for
    /// `{vendor_short}` in every renderer, the `usage --json` `short_name`
    /// field, and any frontend that wants a Waybar-style provider tag; a
    /// second copy in a placeholder map or a QML file is how the table forks.
    pub const fn short_name(self) -> &'static str {
        match self {
            VendorId::Anthropic => "cld",
            VendorId::AnthropicApi => "aac",
            VendorId::Openai => "gpt",
            VendorId::Copilot => "ghc",
            VendorId::Zai => "zai",
            VendorId::Openrouter => "opr",
            VendorId::Deepseek => "dsk",
            VendorId::Kimi => "kmi",
            VendorId::Kilo => "klo",
            VendorId::Novita => "nvt",
            VendorId::Moonshot => "msh",
            VendorId::Grok => "grk",
            VendorId::Supergrok => "sgk",
            VendorId::Antigravity => "agy",
            VendorId::Cursor => "cur",
            VendorId::Minimax => "mmx",
            VendorId::Kiro => "kir",
            VendorId::NousResearch => "nrs",
            VendorId::OpenCodeGo => "ocg",
            VendorId::CommandCode => "cmc",
        }
    }

    pub fn all() -> &'static [VendorId] {
        &[
            VendorId::Anthropic,
            VendorId::AnthropicApi,
            VendorId::Openai,
            VendorId::Copilot,
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
            VendorId::Minimax,
            VendorId::Kiro,
            VendorId::NousResearch,
            VendorId::OpenCodeGo,
            VendorId::CommandCode,
        ]
    }
}

/// What a vendor returns from a successful fetch — the same
/// [`Outcome`](crate::outcome::Outcome) every vendor produces, once its own
/// snapshot type has been widened to [`VendorSnapshot`]. Each vendor gets
/// there with a single `outcome.map(VendorSnapshot::Whichever)`.
pub type VendorOutcome = crate::outcome::Outcome<VendorSnapshot>;

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

    /// `{vendor_short}` is a documented format placeholder and now also rides
    /// the `usage --json` report, so a duplicate or a re-typed code would make
    /// two providers indistinguishable in a bar that shows nothing else.
    #[test]
    fn every_vendor_short_name_is_a_unique_three_letter_code() {
        let mut seen = std::collections::BTreeSet::new();
        for vendor in VendorId::all() {
            let short = vendor.short_name();
            assert_eq!(short.len(), 3, "{} is not three letters", vendor.slug());
            assert!(
                short.chars().all(|c| c.is_ascii_lowercase()),
                "{} is not lowercase ascii",
                vendor.slug()
            );
            assert!(seen.insert(short), "{short} is used by two vendors");
        }
        assert_eq!(VendorId::Anthropic.short_name(), "cld");
        assert_eq!(VendorId::Openai.short_name(), "gpt");
        assert_eq!(VendorId::Zai.short_name(), "zai");
        assert_eq!(VendorId::Antigravity.short_name(), "agy");
    }

    /// The bar can show every provider at once, so a glyph two providers share
    /// tells the user nothing about which row is which. Grok and SuperGrok are
    /// the one sanctioned pair — same brand, two products. Providers without a
    /// distinct Nerd Font mark use their `short_name`, which is unique by
    /// construction and cannot render as tofu.
    #[test]
    fn every_vendor_has_a_bar_icon_and_no_two_share_one() {
        use std::collections::BTreeMap;

        let mut by_icon: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for vendor in VendorId::all() {
            assert!(!vendor.bar_icon().is_empty(), "{}", vendor.slug());
            by_icon
                .entry(vendor.bar_icon())
                .or_default()
                .push(vendor.slug());
        }

        let shared: Vec<_> = by_icon
            .iter()
            .filter(|(_, vendors)| vendors.len() > 1)
            .filter(|(_, vendors)| vendors.as_slice() != ["grok", "supergrok"])
            .collect();
        assert!(
            shared.is_empty(),
            "these providers are indistinguishable in a bar that shows them \
             side by side: {shared:#?}"
        );
        assert_eq!(VendorId::Anthropic.bar_icon(), "󰚩");
        assert_eq!(VendorId::Openai.bar_icon(), "󱢆");
        assert_eq!(VendorId::Supergrok.bar_icon(), VendorId::Grok.bar_icon());
        assert_eq!(VendorId::CommandCode.bar_icon(), "cmc");
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
            "GITHUB_COPILOT_TOKEN",
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

    #[test]
    fn copilot_token_is_removed_before_unrelated_subprocesses_launch() {
        let removed = vendor_secret_env_vars_to_remove(&[]);
        assert!(removed.contains(&"GITHUB_COPILOT_TOKEN"));
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
