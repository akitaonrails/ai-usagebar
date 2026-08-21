//! Fetch GitHub Copilot quota from `GET /copilot_internal/user`, either through
//! an explicit token or via the authenticated `gh` CLI.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::cache::{Cache, MAX_STALE, acquire_lock_async};
use crate::config::CopilotConfig;
use crate::error::{AppError, Result};
use crate::usage::{CopilotPool, CopilotSnapshot};
use crate::vendor::{MAX_BODY_BYTES, read_body_capped};

use super::types::{UserQuotaResponse, to_snapshot};

pub const BASE_URL: &str = "https://api.github.com";
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const LOCK_TIMEOUT: Duration = Duration::from_secs(15);
const GH_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
pub struct Endpoints {
    pub user: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            user: format!("{BASE_URL}/copilot_internal/user"),
        }
    }
}

impl Endpoints {
    /// Resolve the API base for the configured GitHub host. Mirrors the
    /// hostname → API-host mapping `gh` itself applies:
    /// - unset or `github.com` → the public `api.github.com`
    /// - a GitHub Enterprise Cloud tenant with data residency, `<tenant>.ghe.com`
    ///   → `api.<tenant>.ghe.com`
    /// - anything else is treated as a classic (on-prem) GitHub Enterprise
    ///   Server hostname, whose REST API is mounted under `/api/v3`
    ///
    /// This only affects the explicit-token HTTP path; the `gh` CLI path
    /// instead passes `--hostname` and lets `gh` resolve it (see
    /// `resolve_auth`), so it never needs this mapping duplicated.
    pub fn from_config(config: &CopilotConfig) -> Self {
        Self {
            user: format!(
                "{}/copilot_internal/user",
                api_base(config.hostname.as_deref())
            ),
        }
    }
}

fn api_base(hostname: Option<&str>) -> String {
    match hostname.map(str::trim).filter(|h| !h.is_empty()) {
        None | Some("github.com") => BASE_URL.to_string(),
        Some(h) if h.ends_with(".ghe.com") => format!("https://api.{h}"),
        Some(h) => format!("https://{h}/api/v3"),
    }
}

#[derive(Debug, Clone)]
pub struct FetchOutcome {
    pub snapshot: CopilotSnapshot,
    pub stale: bool,
    pub last_error: Option<(u16, String)>,
    pub cache_age: Option<Duration>,
}

enum AuthSource {
    ExplicitToken {
        token: String,
        target: String,
    },
    GhCli {
        gh_binary: PathBuf,
        hostname: Option<String>,
        target: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct CachedSnapshot {
    account: String,
    login: String,
    plan: String,
    reset_at: String,
    chat: CachedPool,
    completions: CachedPool,
    premium_interactions: CachedPool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CachedPool {
    Metered {
        entitlement: u64,
        remaining: u64,
        percent_used: i32,
    },
    Unlimited,
    NotApplicable,
}

pub async fn fetch_snapshot(
    client: &reqwest::Client,
    config: &CopilotConfig,
    cache: &Cache,
    endpoints: &Endpoints,
    cache_ttl: Duration,
) -> Result<FetchOutcome> {
    fetch_snapshot_with_account(client, config, cache, endpoints, cache_ttl, None).await
}

pub async fn fetch_snapshot_with_account(
    client: &reqwest::Client,
    config: &CopilotConfig,
    cache: &Cache,
    endpoints: &Endpoints,
    cache_ttl: Duration,
    account_label: Option<&str>,
) -> Result<FetchOutcome> {
    fetch_snapshot_at(client, config, cache, endpoints, cache_ttl, Utc::now(), account_label).await
}

async fn fetch_snapshot_at(
    client: &reqwest::Client,
    config: &CopilotConfig,
    cache: &Cache,
    endpoints: &Endpoints,
    cache_ttl: Duration,
    now: DateTime<Utc>,
    account_label: Option<&str>,
) -> Result<FetchOutcome> {
    cache.ensure_dir()?;
    let _lock = acquire_lock_async(&cache.lock_path(), LOCK_TIMEOUT).await?;

    let auth = resolve_auth_for_account(config, account_label).await?;
    let target = auth_target(&auth);

    if let Some(bytes) = cache.fresh_payload(cache_ttl)?
        && let Ok(outcome) = reuse_cache(&bytes, cache, false, target, now)
    {
        return Ok(outcome);
    }

    match fetch_live(client, endpoints, &auth).await {
        Ok(snap) => {
            cache.write_payload(&serde_json::to_vec(&CachedSnapshot::from_snapshot(
                &snap, target,
            ))?)?;
            Ok(FetchOutcome {
                snapshot: snap,
                stale: false,
                last_error: None,
                cache_age: Some(Duration::ZERO),
            })
        }
        Err(e) if e.is_transient() => fallback_silent(cache, target, now, e),
        Err(AppError::Http { status, body }) => {
            cache.mark_stale();
            cache.write_last_error(status, &body);
            fallback_with_error(
                cache,
                Some((status, body.clone())),
                target,
                now,
                AppError::Http { status, body },
            )
        }
        Err(e) => {
            cache.mark_stale();
            cache.write_last_error(0, &e.to_string());
            fallback_with_error(cache, Some((0, e.to_string())), target, now, e)
        }
    }
}

fn auth_target(auth: &AuthSource) -> &str {
    match auth {
        AuthSource::ExplicitToken { target, .. } | AuthSource::GhCli { target, .. } => target,
    }
}

#[allow(dead_code)]
async fn resolve_auth(_config: &CopilotConfig) -> Result<AuthSource> {
    resolve_auth_for_account(_config, None).await
}

async fn resolve_auth_for_account(config: &CopilotConfig, account_label: Option<&str>) -> Result<AuthSource> {
    if let Some(token) = resolve_explicit_token(config) {
        return Ok(AuthSource::ExplicitToken {
            target: target_key(&token),
            token,
        });
    }

    let gh_binary = config
        .gh_binary
        .clone()
        .unwrap_or_else(|| PathBuf::from("gh"));
    
    // Determine hostname: if account_label is specified, find that account
    // Otherwise, if multiple accounts are configured, prefer non-github.com (GHE/GHES)
    let hostname = if let Some(label) = account_label {
        // Find the account with matching label
        config.accounts
            .iter()
            .find(|acc| acc.label == label)
            .and_then(|acc| {
                acc.hostname
                    .as_deref()
                    .map(str::trim)
                    .filter(|h| !h.is_empty() && *h != "github.com")
                    .map(str::to_string)
            })
    } else if !config.accounts.is_empty() {
        // First, try to find a non-github.com account (GitHub Enterprise)
        config.accounts
            .iter()
            .find(|acc| {
                acc.hostname
                    .as_deref()
                    .map(str::trim)
                    .map(|h| !h.is_empty() && h != "github.com")
                    .unwrap_or(false)
            })
            .and_then(|acc| {
                acc.hostname
                    .as_deref()
                    .map(str::trim)
                    .filter(|h| !h.is_empty())
                    .map(str::to_string)
            })
            // Fallback to first account's hostname (may be github.com, which resolves to None)
            .or_else(|| {
                config.accounts[0]
                    .hostname
                    .as_deref()
                    .map(str::trim)
                    .filter(|h| !h.is_empty() && *h != "github.com")
                    .map(str::to_string)
            })
    } else {
        // Single-account mode: use config.hostname
        config
            .hostname
            .as_deref()
            .map(str::trim)
            .filter(|h| !h.is_empty() && *h != "github.com")
            .map(str::to_string)
    };
    
    let login = gh_login(&gh_binary, hostname.as_deref()).await?;
    Ok(AuthSource::GhCli {
        target: format!("gh:{login}"),
        gh_binary,
        hostname,
    })
}

fn resolve_explicit_token(config: &CopilotConfig) -> Option<String> {
    let mut envs = vec![config.token_env.as_str(), "GH_TOKEN", "GITHUB_TOKEN"];
    envs.dedup();
    for name in envs {
        if !valid_env_name(name) {
            continue;
        }
        if let Ok(value) = std::env::var(name)
            && !value.is_empty()
        {
            return Some(value);
        }
    }
    config
        .token
        .as_deref()
        .filter(|value| !value.is_empty())
        .or(config.api_key.as_deref().filter(|value| !value.is_empty()))
        .map(str::to_string)
}

fn valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn target_key(secret: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    secret.hash(&mut hasher);
    format!("key:{:016x}", hasher.finish())
}

fn fallback_silent(
    cache: &Cache,
    target: &str,
    now: DateTime<Utc>,
    original: AppError,
) -> Result<FetchOutcome> {
    let Some(bytes) = cache.fallback_payload(MAX_STALE)? else {
        return Err(original);
    };
    match reuse_cache(&bytes, cache, true, target, now) {
        Ok(outcome) => Ok(outcome),
        Err(_) => Err(original),
    }
}

fn fallback_with_error(
    cache: &Cache,
    last_error: Option<(u16, String)>,
    target: &str,
    now: DateTime<Utc>,
    original: AppError,
) -> Result<FetchOutcome> {
    let Some(bytes) = cache.fallback_payload(MAX_STALE)? else {
        return Err(original);
    };
    match reuse_cache(&bytes, cache, true, target, now) {
        Ok(mut outcome) => {
            outcome.last_error = last_error;
            Ok(outcome)
        }
        Err(_) => Err(original),
    }
}

fn reuse_cache(
    bytes: &[u8],
    cache: &Cache,
    stale: bool,
    target: &str,
    now: DateTime<Utc>,
) -> Result<FetchOutcome> {
    Ok(FetchOutcome {
        snapshot: CachedSnapshot::parse(bytes, target, now)?,
        stale,
        last_error: cache.read_last_error(),
        cache_age: cache.payload_age(),
    })
}

async fn fetch_live(
    client: &reqwest::Client,
    endpoints: &Endpoints,
    auth: &AuthSource,
) -> Result<CopilotSnapshot> {
    match auth {
        AuthSource::ExplicitToken { token, .. } => fetch_live_http(client, endpoints, token).await,
        AuthSource::GhCli {
            gh_binary,
            hostname,
            ..
        } => fetch_live_gh(gh_binary, hostname.as_deref()).await,
    }
}

async fn fetch_live_http(
    client: &reqwest::Client,
    endpoints: &Endpoints,
    token: &str,
) -> Result<CopilotSnapshot> {
    let resp = tokio::time::timeout(
        HTTP_TIMEOUT,
        client
            .get(&endpoints.user)
            .header("Accept", "application/json")
            .bearer_auth(token)
            .send(),
    )
    .await
    .map_err(|_| AppError::Transport(format!("copilot timeout: {}", endpoints.user)))??;

    let status = resp.status();
    let bytes = read_body_capped(resp, MAX_BODY_BYTES).await?;
    if !status.is_success() {
        return Err(AppError::Http {
            status: status.as_u16(),
            body: if matches!(status.as_u16(), 401 | 403) {
                "GitHub Copilot authentication failed".into()
            } else {
                String::from_utf8_lossy(&bytes).trim().to_string()
            },
        });
    }
    let parsed: UserQuotaResponse = serde_json::from_slice(&bytes)
        .map_err(|e| AppError::Schema(format!("copilot response: {e}")))?;
    to_snapshot(parsed)
}

async fn fetch_live_gh(gh_binary: &Path, hostname: Option<&str>) -> Result<CopilotSnapshot> {
    let bytes = gh_api(
        gh_binary,
        hostname,
        &[
            "api",
            "/copilot_internal/user",
            "-H",
            "Accept: application/json",
        ],
    )
    .await?;
    let parsed: UserQuotaResponse = serde_json::from_slice(&bytes)
        .map_err(|e| AppError::Schema(format!("copilot gh response: {e}")))?;
    to_snapshot(parsed)
}

async fn gh_login(gh_binary: &Path, hostname: Option<&str>) -> Result<String> {
    let bytes = gh_api(gh_binary, hostname, &["api", "/user", "--jq", ".login"]).await?;
    let login = String::from_utf8_lossy(&bytes).trim().to_string();
    if login.is_empty() {
        return Err(AppError::Credentials(
            "gh CLI returned no GitHub login; run `gh auth login`, or set [copilot] token".into(),
        ));
    }
    Ok(login)
}

async fn gh_api(gh_binary: &Path, hostname: Option<&str>, args: &[&str]) -> Result<Vec<u8>> {
    let mut command = Command::new(gh_binary);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // `--hostname` must precede the subcommand-specific args to be recognized
    // by `gh api`.
    if let Some(host) = hostname {
        command.args(["--hostname", host]);
    }
    command.args(args);
    for var in crate::vendor::vendor_secret_env_vars_to_remove(&["GH_TOKEN", "GITHUB_TOKEN"]) {
        command.env_remove(var);
    }

    let child = command.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            AppError::Credentials(
                "gh CLI not found — install GitHub CLI and run `gh auth login`, or set [copilot] token"
                    .into(),
            )
        } else {
            AppError::Other("failed to start the configured gh CLI".into())
        }
    })?;
    let output = tokio::time::timeout(GH_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| {
            AppError::Transport("gh CLI timed out while fetching Copilot usage".into())
        })??;

    if output.stdout.len() > MAX_BODY_BYTES {
        return Err(AppError::Schema(
            "gh CLI Copilot response exceeded the size limit".into(),
        ));
    }
    if output.status.success() {
        return Ok(output.stdout);
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.contains("not logged into any hosts") || stderr.contains("gh auth login") {
        let hint = match hostname {
            Some(host) => format!(
                "gh CLI is not authenticated for {host}; run `gh auth login --hostname {host}`, or set [copilot] token"
            ),
            None => {
                "gh CLI is not authenticated; run `gh auth login`, or set [copilot] token".into()
            }
        };
        return Err(AppError::Credentials(hint));
    }
    if let Some(status) = gh_http_status(&stderr) {
        return Err(AppError::Http {
            status,
            body: if matches!(status, 401 | 403) {
                "GitHub Copilot authentication failed".into()
            } else if stderr.is_empty() {
                format!("gh api returned HTTP {status}")
            } else {
                stderr
            },
        });
    }
    Err(AppError::Other(if stderr.is_empty() {
        "gh CLI failed to fetch Copilot usage".into()
    } else {
        stderr
    }))
}

fn gh_http_status(stderr: &str) -> Option<u16> {
    stderr
        .split_whitespace()
        .collect::<Vec<_>>()
        .windows(2)
        .find_map(|window| {
            if window[0] == "HTTP" {
                window[1].trim_end_matches(':').parse::<u16>().ok()
            } else {
                None
            }
        })
}

impl CachedSnapshot {
    fn from_snapshot(snapshot: &CopilotSnapshot, account: &str) -> Self {
        Self {
            account: account.to_string(),
            login: snapshot.login.clone(),
            plan: snapshot.plan.clone(),
            reset_at: snapshot
                .reset_at
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default(),
            chat: CachedPool::from(&snapshot.chat),
            completions: CachedPool::from(&snapshot.completions),
            premium_interactions: CachedPool::from(&snapshot.premium_interactions),
        }
    }

    fn parse(bytes: &[u8], account: &str, now: DateTime<Utc>) -> Result<CopilotSnapshot> {
        let cached: Self = serde_json::from_slice(bytes)
            .map_err(|e| AppError::Schema(format!("copilot cache: {e}")))?;
        if cached.account != account {
            return Err(AppError::Schema(
                "copilot cache belongs to a different account; refetching".into(),
            ));
        }
        let reset_at = DateTime::parse_from_rfc3339(&cached.reset_at)
            .map_err(|e| AppError::Schema(format!("copilot cache: invalid reset timestamp: {e}")))?
            .with_timezone(&Utc);
        if reset_at <= now {
            return Err(AppError::Schema(
                "copilot cache is past its quota reset; refetching".into(),
            ));
        }
        Ok(CopilotSnapshot {
            login: cached.login,
            plan: cached.plan,
            chat: cached.chat.into_pool(),
            completions: cached.completions.into_pool(),
            premium_interactions: cached.premium_interactions.into_pool(),
            reset_at: Some(reset_at),
        })
    }
}

impl From<&CopilotPool> for CachedPool {
    fn from(pool: &CopilotPool) -> Self {
        match pool {
            CopilotPool::Metered {
                entitlement,
                remaining,
                percent_used,
            } => Self::Metered {
                entitlement: *entitlement,
                remaining: *remaining,
                percent_used: *percent_used,
            },
            CopilotPool::Unlimited => Self::Unlimited,
            CopilotPool::NotApplicable => Self::NotApplicable,
        }
    }
}

impl CachedPool {
    fn into_pool(self) -> CopilotPool {
        match self {
            Self::Metered {
                entitlement,
                remaining,
                percent_used,
            } => CopilotPool::Metered {
                entitlement,
                remaining,
                percent_used,
            },
            Self::Unlimited => CopilotPool::Unlimited,
            Self::NotApplicable => CopilotPool::NotApplicable,
        }
    }
}

#[cfg(test)]
mod ghe_tests {
    use super::*;

    #[test]
    fn api_base_defaults_to_public_github() {
        assert_eq!(api_base(None), BASE_URL);
        assert_eq!(api_base(Some("github.com")), BASE_URL);
        assert_eq!(api_base(Some("  ")), BASE_URL);
    }

    #[test]
    fn api_base_resolves_data_residency_tenants_under_api_subdomain() {
        assert_eq!(api_base(Some("acme.ghe.com")), "https://api.acme.ghe.com");
    }

    #[test]
    fn api_base_treats_other_hosts_as_classic_ghes() {
        assert_eq!(
            api_base(Some("github.acme.internal")),
            "https://github.acme.internal/api/v3"
        );
    }

    #[test]
    fn endpoints_from_config_use_the_configured_hostname() {
        let config = CopilotConfig {
            hostname: Some("acme.ghe.com".to_string()),
            ..CopilotConfig::default()
        };
        let endpoints = Endpoints::from_config(&config);
        assert_eq!(
            endpoints.user,
            "https://api.acme.ghe.com/copilot_internal/user"
        );
    }

    #[test]
    fn endpoints_from_config_default_matches_public_default() {
        let config = CopilotConfig::default();
        assert_eq!(
            Endpoints::from_config(&config).user,
            Endpoints::default().user
        );
    }
}
