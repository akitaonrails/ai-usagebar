//! The always-exits-0 orchestrator. Dispatches on `--vendor`, fetches a
//! snapshot, renders the right output mode, and prints. Catches every error
//! into a fallback `⚠` JSON / pretty line so Waybar never hides the module.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use chrono::Utc;
use reqwest::Client;

use crate::anthropic::{self, creds::CredsTarget, fetch::FetchOutcome};
use crate::anthropic_api;
use crate::antigravity;
use crate::cache::{Cache, DEFAULT_TTL};
use crate::config::Config;
use crate::cursor;
use crate::deepseek;
use crate::error::{AppError, Result};
use crate::grok;
use crate::kilo;
use crate::kimi;
use crate::kiro;
use crate::minimax;
use crate::moonshot;
use crate::novita;
use crate::openai;
use crate::openrouter;
use crate::pango::escape;
use crate::supergrok;
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
    run_once(&cli, &mut std::io::stdout(), None).await
}

/// Cycle to the next/prev enabled vendor and signal waybar to refresh.
/// Always exits 0 — Waybar swallows non-zero exits anyway.
async fn run_cycle(cli: &Cli) -> i32 {
    // Cycling against the *default* vendor set because the config failed to
    // parse would persist a selection the user never made. Do nothing instead;
    // the next render surfaces the config error through the `⚠` fallback.
    let Ok(config) = Config::load() else {
        return 0;
    };
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
    // Signalling after a *failed* persist told Waybar to re-render a selection
    // that was never written, so the bar redrew the same vendor and the scroll
    // looked like it had been swallowed. Only announce a change that happened.
    if crate::active::cycle(&enabled, start, delta).is_err() {
        return 0;
    }

    // Refresh the bar immediately. The Waybar module's `signal: 13` setting
    // means SIGRTMIN+13 re-runs the exec. SIGRTMIN is libc-dependent; the
    // shell-safe value on Linux glibc is signal 47 (= SIGRTMIN(34)+13).
    crate::waybar::request_refresh();
    0
}

/// `--watch` repaints in place, which only makes sense on a terminal. Piped or
/// redirected, the escape sequence is just garbage in the captured output.
/// Split out from the loop so the decision is testable without a real TTY.
fn should_clear_screen(stdout_is_tty: bool) -> bool {
    stdout_is_tty
}

async fn run_watch(cli: Cli, secs: u64) -> i32 {
    let interval = Duration::from_secs(secs.max(1));
    let clear = {
        use std::io::IsTerminal;
        should_clear_screen(std::io::stdout().is_terminal())
    };
    loop {
        if clear {
            // Clear screen + home cursor.
            print!("\x1b[2J\x1b[H");
        }
        let _ = std::io::stdout().flush();
        run_once(&cli, &mut std::io::stdout(), None).await;
        println!();
        eprintln!("(re-rendering every {secs}s — press Ctrl-C to exit)");
        tokio::select! {
            _ = tokio::time::sleep(interval) => continue,
            _ = tokio::signal::ctrl_c() => return 0,
        }
    }
}

/// Renders and returns the process exit code. The code is the point: Waybar
/// hides a module whose exec exits non-zero, so returning the constant 0 from
/// the function under test is what lets a test fail if anyone ever makes it
/// conditional.
async fn run_once(cli: &Cli, out: &mut impl Write, config_path: Option<&Path>) -> i32 {
    let output = match build_output(cli, config_path).await {
        Ok(o) => o,
        Err(e) => fallback(&e, cli),
    };

    if cli.output_json() {
        let _ = out.write_all(output.to_json_line().as_bytes());
    } else {
        let _ = print_pretty(out, &output);
    }
    let _ = out.flush();
    0
}

async fn build_output(cli: &Cli, config_path: Option<&Path>) -> Result<WaybarOutput> {
    // A broken config is reported through the `⚠` fallback (still exit 0)
    // rather than silently reverting to the default vendor set — otherwise a
    // typo'd section shows another account's usage with no diagnostic.
    let config = match config_path {
        Some(path) => Config::load_from(path)?,
        None => Config::load()?,
    };
    let vendor = cli.resolved_vendor(&config);
    validate_vendor_options(cli, vendor)?;
    if !dispatch_is_eligible(cli, &config, vendor) {
        return Err(AppError::Other(format!(
            "vendor {:?} is disabled in {}",
            vendor,
            crate::config::config_path_hint()
        )));
    }
    match vendor {
        Vendor::Anthropic => anthropic_output(cli, &config).await,
        Vendor::AnthropicApi => anthropic_api_output(cli, &config).await,
        Vendor::Openrouter => openrouter_output(cli, &config).await,
        Vendor::Openai => openai_output(cli, &config).await,
        Vendor::Zai => zai_output(cli, &config).await,
        Vendor::Deepseek => deepseek_output(cli, &config).await,
        Vendor::Kimi => kimi_output(cli, &config).await,
        Vendor::Kilo => kilo_output(cli, &config).await,
        Vendor::Novita => novita_output(cli, &config).await,
        Vendor::Moonshot => moonshot_output(cli, &config).await,
        Vendor::Grok => grok_output(cli, &config).await,
        Vendor::Supergrok => supergrok_output(cli, &config).await,
        Vendor::Antigravity => antigravity_output(cli, &config).await,
        Vendor::Cursor => cursor_output(cli, &config).await,
        Vendor::Minimax => minimax_output(cli, &config).await,
        Vendor::Kiro => kiro_output(cli, &config).await,
        Vendor::NousResearch => nous_output(cli).await,
        Vendor::OpenCodeGo => opencode_go_output(cli, &config).await,
    }
}

fn validate_vendor_options(cli: &Cli, vendor: Vendor) -> Result<()> {
    if cli.account.is_some() && !matches!(vendor, Vendor::Anthropic | Vendor::Openrouter) {
        return Err(AppError::Other(
            "--account is supported only for Claude and OpenRouter".into(),
        ));
    }
    if cli.desktop && vendor != Vendor::Anthropic {
        return Err(AppError::Other(
            "--desktop is supported only for Claude accounts".into(),
        ));
    }
    Ok(())
}

/// Config and persisted selections may only dispatch enabled vendors. An
/// explicit `--vendor` is an intentional CLI opt-in, including for vendors
/// that default to disabled (such as Kimi).
fn dispatch_is_eligible(cli: &Cli, config: &Config, vendor: Vendor) -> bool {
    cli.has_explicit_vendor() || config.is_enabled(vendor.to_id())
}

/// Nous Research authenticates with the independent OAuth credential store.
async fn nous_output(cli: &Cli) -> Result<WaybarOutput> {
    let client = http_client()?;
    let store = crate::nous::credentials::CredentialStore::default();
    let endpoints = crate::nous::fetch::Endpoints::default();
    let account =
        crate::nous::fetch::fetch_account_with_refresh(&client, &store, &endpoints, Utc::now())
            .await?;
    let snapshot = account.clone();
    let outcome = VendorOutcome {
        snapshot: crate::usage::VendorSnapshot::NousResearch(account),
        stale: false,
        last_error: None,
        cache_age: Some(Duration::ZERO),
    };
    let theme = theme_from_cli(cli);
    Ok(crate::nous::vendor::render(
        &outcome,
        &snapshot,
        &theme,
        &RenderOpts::from_cli(cli),
        Utc::now(),
    ))
}

async fn opencode_go_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    let api_key = crate::config::resolve_api_key(
        "OpenCode Go",
        &config.opencode_go.api_key_env,
        config.opencode_go.api_key.as_deref(),
    )?;
    let client = http_client()?;
    let cache = vendor_cache(cli, "opencode-go")?;
    let endpoints = crate::opencode_go::fetch::Endpoints::default();
    let outcome = match crate::opencode_go::fetch::fetch_snapshot(
        &client,
        &api_key,
        &cache,
        &endpoints,
        DEFAULT_TTL,
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(error) if error.is_transient() => {
            return Ok(WaybarOutput::loading(cli.icon.as_deref()));
        }
        Err(error) => return Err(error),
    };
    let snapshot = outcome.snapshot.clone();
    let vendor_outcome: VendorOutcome = outcome.into();
    Ok(crate::opencode_go::vendor::render(
        &vendor_outcome,
        &snapshot,
        &theme_from_cli(cli),
        &RenderOpts::from_cli(cli),
        Utc::now(),
    ))
}

/// Antigravity authenticates through whichever local product is running (the
/// 2.0 app, the `agy` CLI, or the IDE) — there is no API key to resolve.
async fn antigravity_output(cli: &Cli, _config: &Config) -> Result<WaybarOutput> {
    let client = http_client()?;
    let cache = vendor_cache(cli, "antigravity")?;
    let outcome = match antigravity::fetch_snapshot(&client, &cache, DEFAULT_TTL).await {
        Ok(o) => o,
        Err(e) if e.is_transient() => return Ok(WaybarOutput::loading(cli.icon.as_deref())),
        Err(e) => return Err(e),
    };

    let theme = theme_from_cli(cli);
    let snap = outcome.snapshot.clone();
    let vendor_outcome: VendorOutcome = outcome.into();
    let opts = RenderOpts::from_cli(cli);
    Ok(antigravity::vendor::render(
        &vendor_outcome,
        &snap,
        &theme,
        &opts,
        chrono::Utc::now(),
    ))
}

/// Cursor authenticates via a session token the Cursor IDE already wrote to
/// its local `state.vscdb` — no API key to resolve, but (unlike Antigravity)
/// a real on-disk path that can be overridden, mirroring OpenAI's
/// `codex_auth_path`.
async fn cursor_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    let client = http_client()?;
    let cache = vendor_cache(cli, "cursor")?;
    let db_path = match config.cursor.db_path.as_deref() {
        Some(p) => p.to_path_buf(),
        None => cursor::db::default_db_path()?,
    };
    let agent_auth_path = match config.cursor.agent_auth_path.as_deref() {
        Some(p) => p.to_path_buf(),
        None => cursor::db::default_agent_auth_path()?,
    };
    let endpoints = cursor::fetch::Endpoints::default();
    let outcome = match cursor::fetch_snapshot(
        &client,
        &db_path,
        &agent_auth_path,
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
    Ok(cursor::vendor::render(
        &vendor_outcome,
        &snap,
        &theme,
        &opts,
        chrono::Utc::now(),
    ))
}

/// Kiro CLI authenticates via the AWS SSO OIDC session kiro-cli already wrote
/// to its own local `data.sqlite3` — no API key to resolve, but (like Cursor)
/// a real on-disk path that can be overridden.
async fn kiro_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    let client = http_client()?;
    let cache = vendor_cache(cli, "kiro")?;
    let db_path = match config.kiro.db_path.as_deref() {
        Some(p) => p.to_path_buf(),
        None => kiro::db::default_db_path()?,
    };
    let outcome = match kiro::fetch_snapshot(&client, &db_path, &cache, DEFAULT_TTL).await {
        Ok(o) => o,
        Err(e) if e.is_transient() => return Ok(WaybarOutput::loading(cli.icon.as_deref())),
        Err(e) => return Err(e),
    };

    let theme = theme_from_cli(cli);
    let snap = outcome.snapshot.clone();
    let vendor_outcome: VendorOutcome = outcome.into();
    let opts = RenderOpts::from_cli(cli);
    Ok(kiro::vendor::render(
        &vendor_outcome,
        &snap,
        &theme,
        &opts,
        chrono::Utc::now(),
    ))
}

async fn grok_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    let key = crate::config::resolve_api_key(
        "Grok",
        &config.grok.api_key_env,
        config.grok.api_key.as_deref(),
    )?;
    let client = http_client()?;
    let cache = vendor_cache(cli, "grok")?;
    let endpoints = grok::fetch::Endpoints::default();
    let outcome = match grok::fetch_snapshot(
        &client,
        &key,
        &cache,
        &endpoints,
        DEFAULT_TTL,
        config.grok.team_id.as_deref(),
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
    Ok(grok::vendor::render(
        &vendor_outcome,
        &snap,
        &theme,
        &opts,
        chrono::Utc::now(),
    ))
}

/// SuperGrok delegates auth and billing transport to the official Grok Build
/// ACP process — no token or API key is parsed, cached, refreshed, or placed
/// in an ACP message by ai-usagebar.
async fn supergrok_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    let cache = vendor_cache(cli, "supergrok")?;
    let scope_paths = supergrok::scope::ScopePaths::with_overrides(
        config.supergrok.auth_path.as_deref(),
        config.supergrok.config_path.as_deref(),
    )?;
    let outcome = match supergrok::fetch_snapshot(
        &config.supergrok.grok_binary,
        &scope_paths,
        &cache,
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
    Ok(supergrok::vendor::render(
        &vendor_outcome,
        &snap,
        &theme,
        &opts,
        chrono::Utc::now(),
    ))
}

async fn moonshot_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    let api_key = crate::config::resolve_api_key(
        "Moonshot",
        &config.moonshot.api_key_env,
        config.moonshot.api_key.as_deref(),
    )?;
    let client = http_client()?;
    let cache = vendor_cache(cli, "moonshot")?;
    let (endpoints, currency) = moonshot::fetch::Endpoints::for_region(&config.moonshot.region);
    let outcome = match moonshot::fetch_snapshot(
        &client,
        &api_key,
        &cache,
        &endpoints,
        DEFAULT_TTL,
        currency,
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
    Ok(moonshot::vendor::render(
        &vendor_outcome,
        &snap,
        &theme,
        &opts,
        chrono::Utc::now(),
    ))
}

async fn minimax_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    let api_key = crate::config::resolve_api_key(
        "MiniMax",
        &config.minimax.api_key_env,
        config.minimax.api_key.as_deref(),
    )?;
    let client = http_client()?;
    let cache = vendor_cache(cli, "minimax")?;
    let endpoints = minimax::fetch::Endpoints::for_region(&config.minimax.region);
    let outcome =
        match minimax::fetch_snapshot(&client, &api_key, &cache, &endpoints, DEFAULT_TTL).await {
            Ok(o) => o,
            Err(e) if e.is_transient() => return Ok(WaybarOutput::loading(cli.icon.as_deref())),
            Err(e) => return Err(e),
        };

    let theme = theme_from_cli(cli);
    let snap = outcome.snapshot.clone();
    let vendor_outcome: VendorOutcome = outcome.into();
    let opts = RenderOpts::from_cli(cli);
    Ok(minimax::vendor::render(
        &vendor_outcome,
        &snap,
        &theme,
        &opts,
        chrono::Utc::now(),
    ))
}

async fn novita_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    let api_key = crate::config::resolve_api_key(
        "Novita",
        &config.novita.api_key_env,
        config.novita.api_key.as_deref(),
    )?;
    let client = http_client()?;
    let cache = vendor_cache(cli, "novita")?;
    let endpoints = novita::fetch::Endpoints::default();
    let outcome =
        match novita::fetch_snapshot(&client, &api_key, &cache, &endpoints, DEFAULT_TTL).await {
            Ok(o) => o,
            Err(e) if e.is_transient() => return Ok(WaybarOutput::loading(cli.icon.as_deref())),
            Err(e) => return Err(e),
        };

    let theme = theme_from_cli(cli);
    let snap = outcome.snapshot.clone();
    let vendor_outcome: VendorOutcome = outcome.into();
    let opts = RenderOpts::from_cli(cli);
    Ok(novita::vendor::render(
        &vendor_outcome,
        &snap,
        &theme,
        &opts,
        chrono::Utc::now(),
    ))
}

async fn kilo_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    let api_key = crate::config::resolve_api_key(
        "Kilo",
        &config.kilo.api_key_env,
        config.kilo.api_key.as_deref(),
    )?;
    let client = http_client()?;
    let cache = vendor_cache(cli, "kilo")?;
    let endpoints = kilo::fetch::Endpoints::default();
    let outcome = match kilo::fetch_snapshot(
        &client,
        &api_key,
        &cache,
        &endpoints,
        DEFAULT_TTL,
        config.kilo.organization_id.as_deref(),
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
    Ok(kilo::vendor::render(
        &vendor_outcome,
        &snap,
        &theme,
        &opts,
        chrono::Utc::now(),
    ))
}

async fn anthropic_api_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    let key = crate::config::resolve_api_key(
        "Anthropic_API",
        &config.anthropic_api.api_key_env,
        config.anthropic_api.api_key.as_deref(),
    )?;
    let client = http_client()?;
    let cache = vendor_cache(cli, "anthropic_api")?;
    let endpoints = anthropic_api::fetch::Endpoints::default();
    let outcome = match anthropic_api::fetch_snapshot(
        &client,
        &key,
        &cache,
        &endpoints,
        DEFAULT_TTL,
        config.anthropic_api.monthly_limit,
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
    Ok(anthropic_api::vendor::render(
        &vendor_outcome,
        &snap,
        &theme,
        &opts,
        chrono::Utc::now(),
    ))
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
    let (api_key, cache) = openrouter_target(cli, config)?;
    let client = http_client()?;
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

/// Resolve an OpenRouter key and its cache as one identity. The unnamed key
/// keeps the historical vendor-root cache; each named key is isolated below
/// `openrouter/<label>` so fresh data can never cross accounts.
fn openrouter_target(cli: &Cli, config: &Config) -> Result<(String, Cache)> {
    let label = cli.account.as_deref();
    let api_key = config.openrouter.resolve_api_key(label)?;
    let cache = match (cli.cache_dir.as_deref(), label) {
        (Some(root), Some(label)) => Cache::at(root.join("openrouter").join(label)),
        (Some(root), None) => Cache::at(root.join("openrouter")),
        (None, Some(label)) => Cache::for_vendor_account("openrouter", label)?,
        (None, None) => Cache::for_vendor("openrouter")?,
    };
    Ok((api_key, cache))
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

async fn kimi_output(cli: &Cli, config: &Config) -> Result<WaybarOutput> {
    let api_key = crate::config::resolve_api_key(
        "Kimi",
        &config.kimi.api_key_env,
        config.kimi.api_key.as_deref(),
    )?;
    let client = http_client()?;
    let cache = vendor_cache(cli, "kimi")?;
    let endpoints = kimi::fetch::Endpoints::default();
    let outcome =
        match kimi::fetch_snapshot(&client, &api_key, &cache, &endpoints, DEFAULT_TTL).await {
            Ok(o) => o,
            Err(e) if e.is_transient() => return Ok(WaybarOutput::loading(cli.icon.as_deref())),
            Err(e) => return Err(e),
        };

    let theme = theme_from_cli(cli);
    let snap = outcome.snapshot.clone();
    let vendor_outcome: VendorOutcome = outcome.into();
    let opts = RenderOpts::from_cli(cli);
    Ok(kimi::vendor::render(
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

pub(crate) fn http_client() -> Result<Client> {
    Client::builder()
        .timeout(HTTP_CLIENT_TIMEOUT)
        .redirect(crate::vendor::same_origin_redirect_policy())
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
        // `--account <label> --desktop`: read the Claude Desktop app's own token
        // for that saved profile (the menu bar's overview path). `--cache-dir`
        // still relocates the cache for scripted/multi-monitor setups.
        Some(label) if cli.desktop => {
            let (target, default_cache) =
                crate::anthropic::desktop_creds::account_target(config, label)?;
            let cache = match cli.cache_dir.as_deref() {
                Some(p) => Cache::at(p.join("anthropic").join(label)),
                None => default_cache,
            };
            Ok((target, cache))
        }
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
        AppError::Http { status, .. } if matches!(status, 401 | 403) => {
            format!("HTTP {status}\n{}", crate::error::AUTH_FAILURE_MESSAGE)
        }
        AppError::Http { status, body } => format!("HTTP {status}\n{body}"),
        AppError::Schema(m) => format!("API schema drift.\n{m}"),
        AppError::Io { path, source } => format!("I/O error at {}.\n{source}", path.display()),
        AppError::Other(m) | AppError::Transport(m) => m.clone(),
        AppError::Json(e) => format!("JSON error: {e}"),
        AppError::Toml(e) => format!("TOML error: {e}"),
        AppError::IoBare(e) => format!("I/O error: {e}"),
    };
    // Tooltips are Pango markup. Escape error text before serializing it so an
    // error cannot inject markup; serde still produces valid one-line JSON.
    WaybarOutput::error(&escape(&tooltip))
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
                scoped: vec![],
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

    #[test]
    fn watch_only_clears_the_screen_on_a_terminal() {
        // Redirected to a file or piped, the clear-screen escape is garbage in
        // the captured output.
        assert!(should_clear_screen(true));
        assert!(!should_clear_screen(false));
    }

    #[test]
    fn a_failed_cycle_persist_is_not_announced_to_waybar() {
        // Signalling after a failed persist made Waybar re-render the *same*
        // vendor, so the scroll looked swallowed. An empty vendor set is the
        // reachable failure: `cycle_at` errors and nothing is written.
        let td = tempfile::TempDir::new().unwrap();
        let path = td.path().join("active_vendor");
        let r = crate::active::cycle_at(&path, &[], crate::vendor::VendorId::Anthropic, 1);
        assert!(r.is_err());
        assert!(
            crate::active::read_from(&path).is_none(),
            "a failed cycle must not have persisted anything"
        );
    }

    #[test]
    fn fallback_reports_a_broken_config_without_breaking_exit_0() {
        // Propagating the config error must still land in the `⚠` fallback —
        // Waybar hides modules that exit non-zero, so a broken config has to
        // be *visible*, not fatal.
        let toml_err = toml::from_str::<Config>("[zai\nenabled = true\n").unwrap_err();
        let out = fallback(&AppError::Toml(toml_err), &cli_default());
        assert_eq!(out.text, "⚠");
        assert!(out.tooltip.contains("TOML error"));
    }

    #[test]
    fn fallback_escapes_pango_and_keeps_valid_json() {
        let out = fallback(
            &AppError::Other("bad <markup> & value".into()),
            &cli_default(),
        );
        assert_eq!(out.tooltip, "bad &lt;markup&gt; &amp; value");
        let json: serde_json::Value = serde_json::from_str(out.to_json_line().trim()).unwrap();
        assert_eq!(json["text"], "⚠");
        assert_eq!(json["tooltip"], "bad &lt;markup&gt; &amp; value");
    }

    #[test]
    fn fallback_does_not_expose_authentication_response_bodies() {
        let out = fallback(
            &AppError::Http {
                status: 401,
                body: "PANCEA user@example.test <credential>&token".into(),
            },
            &cli_default(),
        );
        assert!(out.tooltip.contains(crate::error::AUTH_FAILURE_MESSAGE));
        assert!(!out.tooltip.contains("PANCEA"));
        assert!(!out.tooltip.contains("user@example.test"));
        assert!(!out.tooltip.contains("&amp;token"));
    }

    #[test]
    fn explicit_kimi_is_eligible_when_disabled_in_config() {
        use clap::Parser;
        let cli = Cli::parse_from(["ai-usagebar", "--vendor", "kimi"]);
        let config = Config::default();
        let vendor = cli.resolve_vendor_with(&config, None);
        assert_eq!(vendor, Vendor::Kimi);
        assert!(dispatch_is_eligible(&cli, &config, vendor));
    }

    #[test]
    fn implicit_disabled_vendor_is_not_dispatch_eligible() {
        let cli = cli_default();
        let config = Config::default();
        assert!(!dispatch_is_eligible(&cli, &config, Vendor::Kimi));
        // Normal implicit resolution avoids that disabled vendor entirely.
        assert_eq!(cli.resolve_vendor_with(&config, None), Vendor::Anthropic);
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
        // A named account's file is read strictly first; the Keychain fallback
        // (if the file is missing/unusable) is scoped to its own directory, so
        // it can never silently read a *different* account's item (#15).
        let config = config_with_account("work", "/creds/work.json");
        let cli = cli_with(Some("work"), None, Some("/tmp/cache"));
        let (creds, cache) = anthropic_target(&cli, &config).unwrap();
        assert_eq!(
            creds,
            CredsTarget::Named {
                path: PathBuf::from("/creds/work.json"),
                config_dir: PathBuf::from("/creds"),
            }
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

    // --- UX-06: a broken [sync] section must never take the status bar down --
    //
    // Every assertion here drives the real `run_once` writer seam with an
    // injected config path, not the private `fallback` helper: a panic or an
    // early return *before* the fallback would still hide the module, and only
    // the seam catches that. Hermetic — config, cache and credentials all live
    // under a TempDir, no case opens a socket or reaches the macOS Keychain.

    async fn render_config_at(cfg: &std::path::Path, cache: &std::path::Path) -> (i32, String) {
        use clap::Parser;
        let cli = Cli::parse_from([
            "ai-usagebar",
            "--json",
            "--cache-dir",
            cache.to_str().unwrap(),
        ]);
        let mut out = Vec::new();
        let code = run_once(&cli, &mut out, Some(cfg)).await;
        (code, String::from_utf8(out).unwrap())
    }

    /// Render a `--json` widget with `body` as its entire config file.
    async fn render_config(body: &str) -> (i32, String) {
        let td = tempfile::TempDir::new().unwrap();
        let cfg = td.path().join("config.toml");
        std::fs::write(&cfg, body).unwrap();
        render_config_at(&cfg, &td.path().join("cache")).await
    }

    /// The Waybar contract on a failing render: exit 0, exactly one line, valid
    /// JSON, `⚠`. Returns the tooltip for the caller's own assertions.
    fn assert_fallback_line(code: i32, stdout: &str) -> String {
        assert_eq!(code, 0, "Waybar hides modules that exit non-zero");
        assert_eq!(stdout.lines().count(), 1, "one line for Waybar: {stdout:?}");
        assert!(stdout.ends_with('\n'), "Waybar splits on the newline");
        let json: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap();
        assert_eq!(json["text"], "⚠");
        assert!(json["class"].is_string(), "Waybar needs a class: {json}");
        json["tooltip"].as_str().unwrap().to_string()
    }

    /// Shapes that must never reach a rendered tooltip. Assembled from
    /// fragments so this file carries none of them as a literal — otherwise a
    /// later `grep` for a token prefix hits the test that guards against one,
    /// and the assertion could match its own source.
    fn secret_shaped_fragments() -> [String; 3] {
        [
            format!("{}{}", "http", "s://"),
            format!("{}{}", "ghp", "_"),
            format!("{}{}", "github", "_pat_"),
        ]
    }

    fn assert_carries_no_secret_shaped_text(tooltip: &str) {
        for fragment in secret_shaped_fragments() {
            assert!(
                !tooltip.contains(&fragment),
                "tooltip leaked {fragment:?}: {tooltip}"
            );
        }
    }

    /// What a `sync pull` from a machine on a newer build leaves behind: a
    /// `[sync]` key this build has never heard of, whose value is deliberately
    /// shaped like the two things that must never be rendered. The synthetic
    /// token is all zeroes — no real credential belongs in a fixture.
    /// `credentials_path` points at a file that does not exist, so the run
    /// fails *after* the config parsed, offline and away from the Keychain.
    fn a_newer_builds_config(dir: &std::path::Path) -> String {
        let url = format!("{}{}", "http", "s://sync.example.test/owner/name.git");
        let token = format!("{}{}", "ghp", "_000000000000000000000000000000000000");
        format!(
            "[sync]\nrepo = \"owner/name\"\nremote_url = \"{url}\"\nupload_token = \"{token}\"\n\n\
             [anthropic]\ncredentials_path = \"{}\"\n",
            missing_creds(dir)
        )
    }

    /// A path that does not exist, spelled so TOML reads it verbatim on every
    /// platform (a Windows `\` would be a string escape).
    fn missing_creds(dir: &std::path::Path) -> String {
        dir.join("missing-credentials.json")
            .display()
            .to_string()
            .replace('\\', "/")
    }

    #[tokio::test]
    async fn a_wrongly_typed_sync_value_still_exits_0_with_the_warning_payload() {
        let (code, stdout) = render_config("[sync]\nkeep_snapshots = \"ten\"\n").await;
        let tooltip = assert_fallback_line(code, &stdout);
        assert!(tooltip.contains("TOML error"), "{tooltip}");
        assert_carries_no_secret_shaped_text(&tooltip);
    }

    #[tokio::test]
    async fn a_half_written_config_still_exits_0_with_the_warning_payload() {
        // What an interrupted restore leaves on disk.
        let (code, stdout) = render_config("[sync\nrepo = \"owner/name\"\n").await;
        let tooltip = assert_fallback_line(code, &stdout);
        assert!(tooltip.contains("TOML error"), "{tooltip}");
        assert_carries_no_secret_shaped_text(&tooltip);
    }

    #[tokio::test]
    async fn a_refused_keep_snapshots_still_exits_0_with_the_warning_payload() {
        // 0 is refused at load — the flip that publishes a snapshot would also
        // drop it. "Refused" has to mean `⚠`, never a module that vanishes.
        let (code, stdout) = render_config("[sync]\nkeep_snapshots = 0\n").await;
        let tooltip = assert_fallback_line(code, &stdout);
        assert!(tooltip.contains("keep_snapshots"), "{tooltip}");
        assert_carries_no_secret_shaped_text(&tooltip);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_unreadable_config_still_exits_0_with_the_warning_payload() {
        use std::os::unix::fs::PermissionsExt;
        let td = tempfile::TempDir::new().unwrap();
        let cfg = td.path().join("config.toml");
        std::fs::write(&cfg, "[sync]\nkeep_snapshots = 3\n").unwrap();
        std::fs::set_permissions(&cfg, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Running as root, mode 000 is not a read barrier and there is no
        // failure to assert on. Skip rather than assert something untrue.
        if std::fs::read_to_string(&cfg).is_ok() {
            return;
        }
        let (code, stdout) = render_config_at(&cfg, &td.path().join("cache")).await;
        let tooltip = assert_fallback_line(code, &stdout);
        assert!(tooltip.contains("I/O error"), "{tooltip}");
        assert_carries_no_secret_shaped_text(&tooltip);
    }

    #[test]
    fn an_unknown_sync_key_round_trips_without_disturbing_the_known_ones() {
        // A newer build's key must not fail the load: refusing it would turn a
        // *successful* sync into a broken status bar, the exact failure UX-06
        // forbids. This is the round trip that decides whether the sync config
        // types need extra tolerance.
        let td = tempfile::TempDir::new().unwrap();
        let cfg = td.path().join("config.toml");
        std::fs::write(&cfg, a_newer_builds_config(td.path())).unwrap();
        let config = Config::load_from(&cfg).expect("an unknown [sync] key must still load");
        assert_eq!(config.sync.repo.as_deref(), Some("owner/name"));
        assert_eq!(
            config.sync.keep_snapshots,
            Config::default().sync.keep_snapshots
        );
    }

    #[tokio::test]
    async fn an_unknown_sync_key_from_a_newer_build_does_not_break_the_widget() {
        let td = tempfile::TempDir::new().unwrap();
        let cfg = td.path().join("config.toml");
        std::fs::write(&cfg, a_newer_builds_config(td.path())).unwrap();
        let (code, stdout) = render_config_at(&cfg, &td.path().join("cache")).await;
        let tooltip = assert_fallback_line(code, &stdout);
        // The config parsed: the run got all the way to the credentials it was
        // told to read. A TOML error here would mean an older machine bricked
        // by a restore from a newer one.
        assert!(!tooltip.contains("TOML error"), "{tooltip}");
        assert!(tooltip.contains("missing-credentials.json"), "{tooltip}");
        // And neither the unknown key's URL nor its token-shaped value came
        // along for the ride.
        assert_carries_no_secret_shaped_text(&tooltip);
    }

    #[tokio::test]
    async fn a_fetch_that_fails_for_a_non_config_reason_lands_in_the_same_fallback() {
        // The ROADMAP asks for an injected failing transport. An absent
        // credential fails the fetch before a socket is opened, reaching the
        // same shipped path end to end with no network — see the SUMMARY for
        // why no client seam was added to reach an outcome this produces.
        let td = tempfile::TempDir::new().unwrap();
        let cfg = td.path().join("config.toml");
        std::fs::write(
            &cfg,
            format!(
                "[sync]\nrepo = \"owner/name\"\n\n[anthropic]\ncredentials_path = \"{}\"\n",
                missing_creds(td.path())
            ),
        )
        .unwrap();
        let (code, stdout) = render_config_at(&cfg, &td.path().join("cache")).await;
        let tooltip = assert_fallback_line(code, &stdout);
        assert!(tooltip.contains("missing-credentials.json"), "{tooltip}");
        // The repository the config names is not the widget's business to show.
        assert!(!tooltip.contains("owner/name"), "{tooltip}");
        assert_carries_no_secret_shaped_text(&tooltip);
    }

    /// UX-06's real mitigation, and the strongest form of it: the widget's
    /// render path does not recover well from a backup failure — it cannot
    /// reach one at all. The claim is scoped to the render path, not to the
    /// binary: `ai-usagebar` is a single binary carrying both the backup
    /// subcommand and this path, so "the binary does not link it" would be
    /// false. What the two genuinely share is the config file on disk, and
    /// that is what the fixtures above cover.
    ///
    /// **These three files are the whole render path.** `run_once` calls
    /// `print_pretty` on every non-JSON render, so `pretty.rs` counts as much
    /// as `run.rs` and `render.rs` do. A refactor that adds a fourth file has
    /// to add it here too.
    #[test]
    fn the_render_path_holds_no_reference_to_the_encrypted_backup_module() {
        // Assembled at runtime rather than written as literals, so this file
        // cannot contain the very text it forbids — a literal here would trip
        // the gate on the test guarding it. Do not "simplify" these back.
        // Ceiling: this catches `crate::sync…` and `sync::…`, the two spellings
        // reachable from a direct or a grouped `use`. A rename of the module
        // has to be reflected here.
        let forbidden = [format!("crate{}{}", "::", "sync"), format!("{}::", "sync")];
        for (file, source) in [
            ("run.rs", include_str!("run.rs")),
            ("render.rs", include_str!("render.rs")),
            ("pretty.rs", include_str!("pretty.rs")),
        ] {
            let code = source
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            // Without this the gate passes just as happily against an empty
            // string, which is how a structural check rots into decoration.
            assert!(
                code.contains("WaybarOutput"),
                "{file} did not read back as the file this gate is meant to check"
            );
            for path in &forbidden {
                assert!(
                    !code.contains(path.as_str()),
                    "{file} references {path:?}: the widget render path must not be \
                     able to reach the encrypted-backup module, because a failure \
                     there would then be able to take the status bar down"
                );
            }
        }
    }

    #[test]
    fn openrouter_named_account_uses_its_key_and_cache_subdir() {
        let mut config = Config::default();
        config
            .openrouter
            .accounts
            .push(crate::config::OpenRouterAccount {
                label: "work".into(),
                api_key_env: None,
                api_key: Some("work-key".into()),
            });
        config
            .openrouter
            .accounts
            .push(crate::config::OpenRouterAccount {
                label: "personal".into(),
                api_key_env: None,
                api_key: Some("personal-key".into()),
            });
        let root = tempfile::tempdir().unwrap();
        let root_str = root.path().to_str().unwrap();
        let cli = cli_with(Some("work"), None, Some(root_str));
        let (key, cache) = openrouter_target(&cli, &config).unwrap();
        assert_eq!(key, "work-key");
        assert_eq!(cache.dir(), root.path().join("openrouter/work"));
        cache.write_payload(b"work-only").unwrap();

        let personal_cli = cli_with(Some("personal"), None, Some(root_str));
        let (personal_key, personal_cache) = openrouter_target(&personal_cli, &config).unwrap();
        assert_eq!(personal_key, "personal-key");
        assert!(
            personal_cache.maybe_payload().unwrap().is_none(),
            "a named account must not reuse another account's fresh cache"
        );
    }

    #[test]
    fn openrouter_default_target_keeps_the_original_cache_path() {
        let mut config = Config::default();
        config.openrouter.api_key_env.clear();
        config.openrouter.api_key = Some("default-key".into());
        let cli = cli_with(None, None, Some("/tmp/cache"));
        let (key, cache) = openrouter_target(&cli, &config).unwrap();
        assert_eq!(key, "default-key");
        assert_eq!(cache.dir(), std::path::Path::new("/tmp/cache/openrouter"));
    }

    #[test]
    fn account_flag_rejects_unrelated_vendors() {
        let cli = cli_with(Some("work"), None, Some("/tmp/cache"));
        assert!(validate_vendor_options(&cli, Vendor::Zai).is_err());
        assert!(validate_vendor_options(&cli, Vendor::Anthropic).is_ok());
        assert!(validate_vendor_options(&cli, Vendor::Openrouter).is_ok());
    }

    #[test]
    fn desktop_flag_remains_claude_only() {
        let mut cli = cli_with(Some("work"), None, Some("/tmp/cache"));
        cli.desktop = true;
        assert!(validate_vendor_options(&cli, Vendor::Openrouter).is_err());
        assert!(validate_vendor_options(&cli, Vendor::Anthropic).is_ok());
    }
}
