//! The always-exits-0 orchestrator. Dispatches on `--vendor`, fetches a
//! snapshot, renders the right output mode, and prints. Catches every error
//! into a fallback `⚠` JSON / pretty line so Waybar never hides the module.

use std::io::Write;
use std::time::Duration;

use chrono::Utc;
use reqwest::Client;

use crate::anthropic::{self, fetch::FetchOutcome};
use crate::cache::{Cache, DEFAULT_TTL};
use crate::config::Config;
use crate::error::{AppError, Result};
use crate::openai;
use crate::openrouter;
use crate::zai;
use crate::theme::Theme;
use crate::vendor::{RenderOpts, VendorOutcome};
use crate::waybar::WaybarOutput;
use crate::widget::cli::{Cli, Vendor};
use crate::widget::pretty::print_pretty;
use crate::widget::render::{DEFAULT_FORMAT, RenderInput, render_anthropic};

/// Entry point — runs to completion and ALWAYS returns Ok with exit code 0
/// in the caller. Mirrors claudebar's `die()` invariant.
pub async fn run(cli: Cli) -> i32 {
    if let Some(secs) = cli.watch {
        return run_watch(cli, secs).await;
    }
    run_once(&cli, &mut std::io::stdout()).await;
    0
}

async fn run_watch(cli: Cli, secs: u64) -> i32 {
    let interval = Duration::from_secs(secs.max(1));
    loop {
        // Clear screen + home cursor.
        print!("\x1b[2J\x1b[H");
        let _ = std::io::stdout().flush();
        run_once(&cli, &mut std::io::stdout()).await;
        println!();
        eprintln!("(re-rendering every {secs}s — press Ctrl-C to exit)");
        tokio::select! {
            _ = tokio::time::sleep(interval) => continue,
            _ = tokio::signal::ctrl_c() => return 0,
        }
    }
}

async fn run_once(cli: &Cli, out: &mut impl Write) {
    let output = match build_output(cli).await {
        Ok(o) => o,
        Err(e) => fallback(&e, cli),
    };

    if cli.output_json() {
        let _ = out.write_all(output.to_json_line().as_bytes());
    } else {
        let _ = print_pretty(out, &output);
    }
    let _ = out.flush();
}

async fn build_output(cli: &Cli) -> Result<WaybarOutput> {
    let config = Config::load().unwrap_or_default();
    if !config.is_enabled(cli.vendor.to_id()) {
        return Err(AppError::Other(format!(
            "vendor {:?} is disabled in ~/.config/ai-usagebar/config.toml",
            cli.vendor
        )));
    }
    match cli.vendor {
        Vendor::Anthropic => anthropic_output(cli).await,
        Vendor::Openrouter => openrouter_output(cli, &config).await,
        Vendor::Openai => openai_output(cli, &config).await,
        Vendor::Zai => zai_output(cli, &config).await,
    }
}

async fn openai_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    let client = http_client()?;
    let cache = match cli.cache_dir.as_deref() {
        Some(p) => Cache::at(p.join("openai")),
        None => Cache::for_vendor("openai")?,
    };
    let creds_path = match config.openai.codex_auth_path.as_deref() {
        Some(p) => p.to_path_buf(),
        None => openai::creds::default_path()?,
    };
    let endpoints = openai::fetch::Endpoints::default();
    let outcome = match openai::fetch_snapshot(&client, &creds_path, &cache, &endpoints, DEFAULT_TTL).await {
        Ok(o) => o,
        Err(e) if e.is_transient() => return Ok(WaybarOutput::loading(cli.icon.as_deref())),
        Err(e) => return Err(e),
    };

    let theme = Theme::default()
        .merged_with_omarchy()
        .with_overrides(
            cli.color_low.clone(),
            cli.color_mid.clone(),
            cli.color_high.clone(),
            cli.color_critical.clone(),
        );
    let snap = outcome.snapshot.clone();
    let vendor_outcome: VendorOutcome = outcome.into();
    let opts = RenderOpts::from_cli(cli);
    Ok(openai::vendor::render(
        &vendor_outcome,
        &snap,
        &theme,
        &opts,
        chrono::Utc::now(),
    ))
}

async fn zai_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    let env_var = &config.zai.api_key_env;
    let api_key = std::env::var(env_var).map_err(|_| {
        AppError::Credentials(format!("{env_var} not set."))
    })?;
    let client = http_client()?;
    let cache = match cli.cache_dir.as_deref() {
        Some(p) => Cache::at(p.join("zai")),
        None => Cache::for_vendor("zai")?,
    };
    let endpoints = zai::fetch::Endpoints::default();
    let outcome = match zai::fetch_snapshot(
        &client,
        &api_key,
        &cache,
        &endpoints,
        DEFAULT_TTL,
        config.zai.plan_tier.as_deref(),
    )
    .await
    {
        Ok(o) => o,
        Err(e) if e.is_transient() => return Ok(WaybarOutput::loading(cli.icon.as_deref())),
        Err(e) => return Err(e),
    };

    let theme = Theme::default()
        .merged_with_omarchy()
        .with_overrides(
            cli.color_low.clone(),
            cli.color_mid.clone(),
            cli.color_high.clone(),
            cli.color_critical.clone(),
        );
    let snap = outcome.snapshot.clone();
    let vendor_outcome: VendorOutcome = outcome.into();
    let opts = RenderOpts::from_cli(cli);
    Ok(zai::vendor::render(
        &vendor_outcome,
        &snap,
        &theme,
        &opts,
        chrono::Utc::now(),
    ))
}

async fn openrouter_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    let env_var = &config.openrouter.api_key_env;
    let api_key = std::env::var(env_var).map_err(|_| {
        AppError::Credentials(format!(
            "{env_var} not set. Export it (e.g. in ~/.config/zsh/secrets) before running."
        ))
    })?;
    let client = http_client()?;
    let cache = match cli.cache_dir.as_deref() {
        Some(p) => Cache::at(p.join("openrouter")),
        None => Cache::for_vendor("openrouter")?,
    };
    let endpoints = openrouter::fetch::Endpoints::default();
    let outcome = match openrouter::fetch_snapshot(
        &client,
        &api_key,
        &cache,
        &endpoints,
        DEFAULT_TTL,
    )
    .await
    {
        Ok(o) => o,
        Err(e) if e.is_transient() => return Ok(WaybarOutput::loading(cli.icon.as_deref())),
        Err(e) => return Err(e),
    };

    let theme = Theme::default()
        .merged_with_omarchy()
        .with_overrides(
            cli.color_low.clone(),
            cli.color_mid.clone(),
            cli.color_high.clone(),
            cli.color_critical.clone(),
        );

    let snap = outcome.snapshot.clone();
    let vendor_outcome: VendorOutcome = outcome.into();
    let opts = RenderOpts::from_cli(cli);
    Ok(openrouter::vendor::render(
        &vendor_outcome,
        &snap,
        &theme,
        &opts,
        chrono::Utc::now(),
    ))
}

async fn anthropic_output(cli: &Cli) -> Result<WaybarOutput> {
    let client = http_client()?;
    let cache = match cli.cache_dir.as_deref() {
        Some(p) => Cache::at(p.join("anthropic")),
        None => Cache::for_vendor("anthropic")?,
    };
    let creds_path = match cli.creds_path.as_deref() {
        Some(p) => p.to_path_buf(),
        None => anthropic::creds::default_path()?,
    };
    let endpoints = anthropic::fetch::Endpoints::default();
    let outcome = match anthropic::fetch_snapshot(
        &client,
        &creds_path,
        &cache,
        &endpoints,
        DEFAULT_TTL,
    )
    .await
    {
        Ok(o) => o,
        Err(e) if e.is_transient() => {
            // Mirror claudebar's `loading_network` path.
            return Ok(WaybarOutput::loading(cli.icon.as_deref()));
        }
        Err(e) => return Err(e),
    };

    let theme = Theme::default()
        .merged_with_omarchy()
        .with_overrides(
            cli.color_low.clone(),
            cli.color_mid.clone(),
            cli.color_high.clone(),
            cli.color_critical.clone(),
        );

    Ok(render_with_theme(&outcome, &theme, cli))
}

fn render_with_theme(outcome: &FetchOutcome, theme: &Theme, cli: &Cli) -> WaybarOutput {
    let format_owned = cli.format.clone().unwrap_or_else(|| DEFAULT_FORMAT.to_string());
    let input = RenderInput {
        outcome,
        theme,
        format: &format_owned,
        tooltip_format: cli.tooltip_format.as_deref(),
        icon: cli.icon.as_deref(),
        pace_tolerance: cli.pace_tolerance,
        format_pace_color: cli.format_pace_color,
        tooltip_pace_pts: cli.tooltip_pace_pts,
        now: Utc::now(),
    };
    render_anthropic(&input)
}

fn http_client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| AppError::Other(format!("http client init: {e}")))
}

/// Fallback output when everything goes wrong — always renders a `⚠` widget.
fn fallback(err: &AppError, _cli: &Cli) -> WaybarOutput {
    let tooltip = match err {
        AppError::Credentials(m) => format!("Credentials error.\n{m}"),
        AppError::Http { status, body } => format!("HTTP {status}\n{body}"),
        AppError::Schema(m) => format!("API schema drift.\n{m}"),
        AppError::Io { path, source } => format!("I/O error at {}.\n{source}", path.display()),
        AppError::Other(m) | AppError::Transport(m) => m.clone(),
        AppError::Json(e) => format!("JSON error: {e}"),
        AppError::Toml(e) => format!("TOML error: {e}"),
        AppError::IoBare(e) => format!("I/O error: {e}"),
    };
    WaybarOutput::error(&tooltip)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anthropic::fetch::FetchOutcome;
    use crate::usage::{AnthropicSnapshot, UsageWindow};

    fn cli_default() -> Cli {
        // clap's Default isn't derived for us; build from parse_from with no
        // args to get the canonical defaults.
        use clap::Parser;
        Cli::parse_from(["ai-usagebar"])
    }

    fn dummy_outcome() -> FetchOutcome {
        FetchOutcome {
            snapshot: AnthropicSnapshot {
                plan: "Test".into(),
                session: UsageWindow {
                    utilization_pct: 25,
                    resets_at: None,
                    window_duration: chrono::Duration::hours(5),
                },
                weekly: UsageWindow {
                    utilization_pct: 10,
                    resets_at: None,
                    window_duration: chrono::Duration::days(7),
                },
                sonnet: None,
                extra: None,
            },
            stale: false,
            last_error: None,
            cache_age: None,
        }
    }

    #[test]
    fn render_with_theme_uses_cli_overrides() {
        let cli = {
            let mut c = cli_default();
            c.format = Some("test:{session_pct}".into());
            c.color_low = Some("#123456".into());
            c
        };
        let outcome = dummy_outcome();
        let theme = Theme::default().with_overrides(
            cli.color_low.clone(),
            None,
            None,
            None,
        );
        let out = render_with_theme(&outcome, &theme, &cli);
        // Bar text should contain our format substitution, wrapped in the
        // overridden low-color span.
        assert!(out.text.contains("test:25"));
        assert!(out.text.contains("#123456"));
    }

    #[test]
    fn fallback_wraps_credentials_error_in_warning() {
        let err = AppError::Credentials("missing token".into());
        let out = fallback(&err, &cli_default());
        assert_eq!(out.text, "⚠");
        assert!(out.tooltip.contains("missing token"));
    }
}
