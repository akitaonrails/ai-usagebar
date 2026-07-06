//! The always-exits-0 orchestrator. Dispatches on `--vendor`, fetches a
//! snapshot, renders the right output mode, and prints. Catches every error
//! into a fallback `⚠` JSON / pretty line so Waybar never hides the module.

use std::io::Write;
use std::time::Duration;

use chrono::Utc;
use reqwest::Client;

use crate::anthropic::{self, creds::CredsTarget, fetch::FetchOutcome};
use crate::cache::{Cache, DEFAULT_TTL};
use crate::config::Config;
use crate::deepseek;
use crate::error::{AppError, Result};
use crate::openai;
use crate::openrouter;
use crate::theme::Theme;
use crate::vendor::{HTTP_CLIENT_TIMEOUT, RenderOpts, VendorOutcome};
use crate::waybar::WaybarOutput;
use crate::widget::cli::{Cli, Vendor};
use crate::widget::pretty::print_pretty;
use crate::widget::render::{DEFAULT_FORMAT, RenderInput, render_anthropic};
use crate::zai;

/// Entry point — runs to completion and ALWAYS returns Ok with exit code 0
/// in the caller. Mirrors claudebar's `die()` invariant.
pub async fn run(cli: Cli) -> i32 {
    // Scroll-cycle short-circuit: don't render, just bump state + signal waybar.
    if cli.cycle_next || cli.cycle_prev {
        return run_cycle(&cli).await;
    }
    if let Some(secs) = cli.watch {
        return run_watch(cli, secs).await;
    }
    run_once(&cli, &mut std::io::stdout()).await;
    0
}

/// Cycle to the next/prev enabled vendor and signal waybar to refresh.
/// Always exits 0 — Waybar swallows non-zero exits anyway.
async fn run_cycle(cli: &Cli) -> i32 {
    let config = Config::load().unwrap_or_default();
    let enabled = config.enabled_vendors();
    if enabled.is_empty() {
        return 0;
    }
    // Starting point: current persisted vendor, else config.primary, else
    // anthropic. resolved_vendor() encodes that precedence, minus the
    // `cli.vendor` override (cycle commands ignore --vendor on purpose).
    let start = match config.ui.primary {
        Some(id) if enabled.contains(&id) => id,
        _ => enabled[0],
    };
    let delta = if cli.cycle_next { 1 } else { -1 };
    let _ = crate::active::cycle(&enabled, start, delta);

    // Refresh the bar immediately. The Waybar module's `signal: 13` setting
    // means SIGRTMIN+13 re-runs the exec. SIGRTMIN is libc-dependent; the
    // shell-safe value on Linux glibc is signal 47 (= SIGRTMIN(34)+13).
    crate::waybar::request_refresh();
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
    let vendor = cli.resolved_vendor(&config);
    if !config.is_enabled(vendor.to_id()) {
        return Err(AppError::Other(format!(
            "vendor {:?} is disabled in {}",
            vendor,
            crate::config::config_path_hint()
        )));
    }
    match vendor {
        Vendor::Anthropic => anthropic_output(cli, &config).await,
        Vendor::Openrouter => openrouter_output(cli, &config).await,
        Vendor::Openai => openai_output(cli, &config).await,
        Vendor::Zai => zai_output(cli, &config).await,
        Vendor::Deepseek => deepseek_output(cli, &config).await,
    }
}

async fn openai_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    let client = http_client()?;
    let cache = vendor_cache(cli, "openai")?;
    let creds_path = match config.openai.codex_auth_path.as_deref() {
        Some(p) => p.to_path_buf(),
        None => openai::creds::default_path()?,
    };
    let endpoints = openai::fetch::Endpoints::default();
    let outcome =
        match openai::fetch_snapshot(&client, &creds_path, &cache, &endpoints, DEFAULT_TTL).await {
            Ok(o) => o,
            Err(e) if e.is_transient() => return Ok(WaybarOutput::loading(cli.icon.as_deref())),
            Err(e) => return Err(e),
        };

    let theme = theme_from_cli(cli);
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
    let api_key = crate::config::resolve_api_key(
        "Zai",
        &config.zai.api_key_env,
        config.zai.api_key.as_deref(),
    )?;
    let client = http_client()?;
    let cache = vendor_cache(cli, "zai")?;
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

    let theme = theme_from_cli(cli);
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
    let api_key = crate::config::resolve_api_key(
        "OpenRouter",
        &config.openrouter.api_key_env,
        config.openrouter.api_key.as_deref(),
    )?;
    let client = http_client()?;
    let cache = vendor_cache(cli, "openrouter")?;
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

    let theme = theme_from_cli(cli);

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

async fn deepseek_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    let api_key = crate::config::resolve_api_key(
        "DeepSeek",
        &config.deepseek.api_key_env,
        config.deepseek.api_key.as_deref(),
    )?;
    let client = http_client()?;
    let cache = vendor_cache(cli, "deepseek")?;
    let endpoints = deepseek::fetch::Endpoints::default();
    let outcome =
        match deepseek::fetch_snapshot(&client, &api_key, &cache, &endpoints, DEFAULT_TTL).await {
            Ok(o) => o,
            Err(e) if e.is_transient() => return Ok(WaybarOutput::loading(cli.icon.as_deref())),
            Err(e) => return Err(e),
        };

    let theme = theme_from_cli(cli);
    let snap = outcome.snapshot.clone();
    let vendor_outcome: VendorOutcome = outcome.into();
    let opts = RenderOpts::from_cli(cli);
    Ok(deepseek::vendor::render(
        &vendor_outcome,
        &snap,
        &theme,
        &opts,
        chrono::Utc::now(),
    ))
}

async fn anthropic_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    let client = http_client()?;
    let (creds_target, cache) = anthropic_target(cli, config)?;
    let endpoints = anthropic::fetch::Endpoints::default();
    let outcome =
        match anthropic::fetch_snapshot(&client, &creds_target, &cache, &endpoints, DEFAULT_TTL)
            .await
        {
            Ok(o) => o,
            Err(e) if e.is_transient() => {
                // Mirror claudebar's `loading_network` path.
                return Ok(WaybarOutput::loading(cli.icon.as_deref()));
            }
            Err(e) => return Err(e),
        };

    let theme = theme_from_cli(cli);

    Ok(render_with_theme(&outcome, &theme, cli))
}

fn render_with_theme(outcome: &FetchOutcome, theme: &Theme, cli: &Cli) -> WaybarOutput {
    let format_owned = cli
        .format
        .clone()
        .unwrap_or_else(|| DEFAULT_FORMAT.to_string());
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
        .timeout(HTTP_CLIENT_TIMEOUT)
        .build()
        .map_err(|e| AppError::Other(format!("http client init: {e}")))
}

fn vendor_cache(cli: &Cli, vendor: &str) -> Result<Cache> {
    match cli.cache_dir.as_deref() {
        Some(p) => Ok(Cache::at(p.join(vendor))),
        None => Cache::for_vendor(vendor),
    }
}

/// Resolve the Anthropic credentials target + cache for this run, honoring
/// `--account` (issue #14). `--cache-dir` still overrides the cache location;
/// `--account <label>` selects a configured extra account (its credentials
/// file and an `anthropic/<label>` cache subdir); with neither, the default
/// account behaves byte-identically to before.
fn anthropic_target(cli: &Cli, config: &Config) -> Result<(CredsTarget, Cache)> {
    match cli.account.as_deref() {
        Some(label) => named_account_target(cli, config, label),
        None => Ok((
            anthropic_default_creds(cli, config)?,
            vendor_cache(cli, "anthropic")?,
        )),
    }
}

/// A configured extra account (`--account <label>`): its own credentials file
/// and an isolated `anthropic/<label>` cache. `--account` conflicts with
/// `--creds-path`, so the account's file is the only credentials source here —
/// an Explicit target, so a missing/broken file fails loudly instead of
/// falling back to the (different account's) macOS Keychain item (#15).
fn named_account_target(cli: &Cli, config: &Config, label: &str) -> Result<(CredsTarget, Cache)> {
    let (creds, default_cache) = config.anthropic.account_target(label)?;
    // `--cache-dir` still wins for scripted/multi-monitor setups; otherwise use
    // the account's default `anthropic/<label>` cache from account_target.
    let cache = match cli.cache_dir.as_deref() {
        Some(p) => Cache::at(p.join("anthropic").join(label)),
        None => default_cache,
    };
    Ok((creds, cache))
}

/// Default-account credentials: `--creds-path` wins, then the singular
/// `[anthropic] credentials_path`, then the platform default file. This is the
/// pre-#14 resolution, kept intact so default output never changes. Only the
/// platform-default file is a `Default` target (eligible for the macOS
/// Keychain fallback); the two overrides are explicit user choices, read
/// strictly.
fn anthropic_default_creds(cli: &Cli, config: &Config) -> Result<CredsTarget> {
    if let Some(p) = cli.creds_path.as_deref() {
        return Ok(CredsTarget::Explicit(p.to_path_buf()));
    }
    if let Some(p) = config.anthropic.credentials_path.as_deref() {
        return Ok(CredsTarget::Explicit(p.to_path_buf()));
    }
    Ok(CredsTarget::Default(anthropic::creds::default_path()?))
}

fn theme_from_cli(cli: &Cli) -> Theme {
    Theme::default().merged_with_omarchy().with_overrides(
        cli.color_low.clone(),
        cli.color_mid.clone(),
        cli.color_high.clone(),
        cli.color_critical.clone(),
    )
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
    use std::path::PathBuf;

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
        let theme = Theme::default().with_overrides(cli.color_low.clone(), None, None, None);
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

    // --- issue #14: multi-account Anthropic target resolution ---------------
    // All hermetic: paths are only *resolved*, never opened, and every cache
    // uses an explicit --cache-dir (Cache::at) so no test touches real XDG.

    fn cli_with(account: Option<&str>, creds: Option<&str>, cache: Option<&str>) -> Cli {
        let mut c = cli_default();
        c.account = account.map(str::to_string);
        c.creds_path = creds.map(PathBuf::from);
        c.cache_dir = cache.map(PathBuf::from);
        c
    }

    fn config_with_account(label: &str, creds: &str) -> Config {
        let mut config = Config::default();
        config
            .anthropic
            .accounts
            .push(crate::config::AnthropicAccount {
                label: label.into(),
                credentials_path: creds.into(),
            });
        config
    }

    #[test]
    fn default_account_target_is_unchanged() {
        // No --account: creds come from --creds-path and the cache stays at the
        // vendor root (…/anthropic) — byte-identical to the pre-#14 path. An
        // explicit --creds-path is a strict (non-Keychain) target.
        let cli = cli_with(None, Some("/tmp/creds.json"), Some("/tmp/cache"));
        let (creds, cache) = anthropic_target(&cli, &Config::default()).unwrap();
        assert_eq!(
            creds,
            CredsTarget::Explicit(PathBuf::from("/tmp/creds.json"))
        );
        assert_eq!(cache.dir(), std::path::Path::new("/tmp/cache/anthropic"));
    }

    #[test]
    fn default_account_without_overrides_is_keychain_eligible() {
        // No --account, no --creds-path, no config path → the platform-default
        // location, the only target eligible for the macOS Keychain fallback.
        let cli = cli_with(None, None, Some("/tmp/cache"));
        let (creds, _cache) = anthropic_target(&cli, &Config::default()).unwrap();
        assert!(matches!(creds, CredsTarget::Default(_)));
    }

    #[test]
    fn named_account_uses_its_creds_and_label_subdir() {
        // A named account's file is Explicit: a missing/broken path must fail
        // loudly, never silently read another account's Keychain item (#15).
        let config = config_with_account("work", "/creds/work.json");
        let cli = cli_with(Some("work"), None, Some("/tmp/cache"));
        let (creds, cache) = anthropic_target(&cli, &config).unwrap();
        assert_eq!(
            creds,
            CredsTarget::Explicit(PathBuf::from("/creds/work.json"))
        );
        assert_eq!(
            cache.dir(),
            std::path::Path::new("/tmp/cache/anthropic/work")
        );
    }

    #[test]
    fn unknown_account_errors_listing_known_labels() {
        let config = config_with_account("work", "/creds/work.json");
        let cli = cli_with(Some("nope"), None, Some("/tmp/cache"));
        let err = anthropic_target(&cli, &config).unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("nope"), "names the bad label: {msg}");
        assert!(msg.contains("work"), "lists known labels: {msg}");
    }

    #[test]
    fn account_conflicts_with_creds_path() {
        use clap::Parser;
        let res = Cli::try_parse_from(["ai-usagebar", "--account", "work", "--creds-path", "/x"]);
        assert!(res.is_err(), "--account and --creds-path must conflict");
    }
}
