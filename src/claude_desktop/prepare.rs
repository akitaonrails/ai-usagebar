//! A manual, isolated Claude Code turn for an inactive Desktop profile.
//! Never switches/restarts Desktop, never saves a conversation, and never
//! guesses a new reset time from a successful process exit.

use chrono::{DateTime, Utc};
use std::path::Path;
#[cfg(unix)]
use std::time::Duration;

use crate::error::{AppError, Result};
use crate::usage::AnthropicSnapshot;

#[derive(Debug, PartialEq, Eq)]
pub struct Preparation {
    pub reset_at: DateTime<Utc>,
    pub sent: bool,
}

/// A future reset is already a running window, even when rounded usage is 0%.
/// An expired timestamp on a live response is not a new running window.
fn existing_window(
    snapshot: &AnthropicSnapshot,
    now: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>> {
    if snapshot.weekly.utilization_pct >= 100 {
        return Err(AppError::Other(
            "Weekly usage limit reached; no message sent.".into(),
        ));
    }
    if let Some(reset) = snapshot.session.resets_at.filter(|reset| *reset > now) {
        if snapshot.session.utilization_pct >= 100 {
            return Err(AppError::Other(
                "The five-hour limit has not reset yet; no message sent.".into(),
            ));
        }
        return Ok(Some(reset));
    }
    if snapshot.session.resets_at.is_none() && snapshot.session.utilization_pct > 0 {
        return Err(AppError::Other(
            "Session reset time is unknown; no message sent.".into(),
        ));
    }
    Ok(None)
}

fn trusted_binary(path: Option<&Path>) -> Result<&Path> {
    path.filter(|path| path.is_absolute() && path.is_file()).ok_or_else(|| {
        AppError::Other("Configure an absolute desktop_prepare_binary path to the official Claude Code CLI or its trusted wrapper.".into())
    })
}

fn inactive_identity(active: Option<String>, target: &str) -> Result<String> {
    active
        .filter(|id| !id.is_empty() && id != target)
        .ok_or_else(|| AppError::Other("Prepare requires a known inactive Desktop account.".into()))
}

fn isolated_command(
    binary: &Path,
    directory: &Path,
    token: &str,
    transport: &[(std::ffi::OsString, std::ffi::OsString)],
) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(binary);
    // An environment token is the sole auth source. In particular, do not
    // inherit API keys, alternate API endpoints, project config, or sessions.
    command.env_clear().env("PATH", "/usr/bin:/bin");
    command.envs(transport.iter().cloned());
    command
        .env("HOME", directory)
        .env("CLAUDE_CONFIG_DIR", directory.join("config"));
    command
        .env("CLAUDE_CODE_OAUTH_TOKEN", token)
        .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1");
    command.args([
        "--safe-mode",
        "-p",
        "Reply with OK only.",
        "--model",
        "haiku",
        "--max-turns",
        "1",
        "--no-session-persistence",
        "--tools",
        "",
        "--strict-mcp-config",
        "--mcp-config",
        "{\"mcpServers\":{}}",
        "--setting-sources",
        "",
        "--system-prompt",
        "Reply briefly.",
        "--output-format",
        "stream-json",
        "--verbose",
    ]);
    command
        .current_dir(directory)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    command
}

/// Output can contain auth diagnostics; do not relay it to a card/log.
fn request_succeeded(success: bool, stdout: &[u8]) -> bool {
    success
        && stdout.split(|byte| *byte == b'\n').any(|line| {
            serde_json::from_slice::<serde_json::Value>(line).is_ok_and(|value| {
                value["type"] == "result"
                    && value["is_error"] == false
                    && value["subtype"] == "success"
            })
        })
}

/// Prefer the official CLI's server rate-limit event over a second usage poll.
/// Weekly/overage events and inferred timestamps are never session evidence.
fn response_reset(stdout: &[u8], now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    stdout
        .split(|byte| *byte == b'\n')
        .filter_map(|line| {
            let value: serde_json::Value = serde_json::from_slice(line).ok()?;
            if value["type"] != "rate_limit_event" {
                return None;
            }
            let info = &value["rate_limit_info"];
            if info["rateLimitType"] != "five_hour"
                || !matches!(info["status"].as_str(), Some("allowed" | "allowed_warning"))
                || info["isUsingOverage"] == true
                || info["overageInUse"] == true
            {
                return None;
            }
            let reset = DateTime::from_timestamp(info["resetsAt"].as_i64()?, 0)?;
            (reset > now && reset <= now + chrono::Duration::minutes(305)).then_some(reset)
        })
        .next_back()
}

#[cfg(unix)]
async fn isolated_turn(
    binary: &Path,
    token: &str,
    transport: &[(std::ffi::OsString, std::ffi::OsString)],
    timeout: Duration,
) -> Result<Option<DateTime<Utc>>> {
    let temp = tempfile::Builder::new()
        .prefix("aiusage-prepare-")
        .tempdir()?;
    let mut command = isolated_command(binary, temp.path(), token, transport);
    let output = tokio::time::timeout(timeout, command.output()).await
        .map_err(|_| AppError::Other("Preparation timed out. Refresh usage before retrying; the request may have completed.".into()))?
        .map_err(|_| AppError::Other("Could not start the configured Claude Code CLI.".into()))?;
    // Drop deletes all isolated state on success, failure, and timeout.
    drop(temp);
    if !request_succeeded(output.status.success(), &output.stdout) {
        return Err(AppError::Other(
            "The isolated Claude Code request failed. Refresh usage before retrying.".into(),
        ));
    }
    Ok(response_reset(&output.stdout, Utc::now()))
}

#[cfg(target_os = "macos")]
pub async fn run(config: &crate::config::Config, label: &str) -> Result<Preparation> {
    use super::{Paths, active_account_uuid, load_profiles};
    use crate::anthropic::{creds::CredsTarget, desktop_creds, fetch};
    use crate::cache::acquire_lock_async;

    let binary = trusted_binary(config.anthropic.desktop_prepare_binary.as_deref())?;
    crate::config::validate_account_label(label)?;
    let paths = Paths::resolve(&config.anthropic)?;
    // Serialize manual prepares across processes as well as popover clicks.
    // The next caller rechecks live quota after the previous turn completed.
    let _preparation_lock = acquire_lock_async(
        &paths.profiles_dir.join(".prepare.lock"),
        Duration::from_secs(2),
    )
    .await?;
    let profile = load_profiles(&paths.profiles_dir)
        .into_iter()
        .find(|p| p.label == label)
        .ok_or_else(|| AppError::Other("Unknown Desktop account; no message sent.".into()))?;
    let active = inactive_identity(
        active_account_uuid(&paths.config_json()),
        &profile.account_uuid,
    )?;
    let (target, cache) = desktop_creds::account_target(config, label)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let before = fetch::fetch_snapshot(
        &client,
        &target,
        &cache,
        &Default::default(),
        Duration::ZERO,
    )
    .await?;
    if !before.off_the_wire() {
        return Err(AppError::Other(
            "Live usage could not be checked; no message sent.".into(),
        ));
    }
    if let Some(reset_at) = existing_window(&before.snapshot, Utc::now())? {
        return Ok(Preparation {
            reset_at,
            sent: false,
        });
    }

    // Coordinate with every in-app/CLI Desktop switch and token refresh.
    // Do not hold the cache lock while spawning a client, avoiding lock-order
    // inversion with periodic quota polling.
    let guard =
        acquire_lock_async(&paths.account_switch_lock(), super::ACCOUNT_LOCK_TIMEOUT).await?;
    let still_same_profile = load_profiles(&paths.profiles_dir)
        .iter()
        .any(|p| p.label == label && p.account_uuid == profile.account_uuid);
    if !still_same_profile || active_account_uuid(&paths.config_json()).as_deref() != Some(&active)
    {
        return Err(AppError::Other(
            "Desktop identity changed; no message sent.".into(),
        ));
    }
    let CredsTarget::Desktop(source) = target else {
        return Err(AppError::Other(
            "Prepare only supports Desktop profiles.".into(),
        ));
    };
    let (creds, _) = source.read()?;
    let transport: Vec<_> = [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "NO_PROXY",
        "no_proxy",
        "NODE_EXTRA_CA_CERTS",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "CLAUDE_CODE_PROXY_RESOLVES_HOSTS",
    ]
    .into_iter()
    .filter_map(|key| std::env::var_os(key).map(|value| (key.into(), value)))
    .collect();
    let confirmed = isolated_turn(
        binary,
        &creds.claude_ai_oauth.access_token,
        &transport,
        Duration::from_secs(60),
    )
    .await?;
    drop(guard);
    if active_account_uuid(&paths.config_json()).as_deref() != Some(&active) {
        return Err(AppError::Other(
            "Request completed, but Desktop identity changed. Refresh usage before retrying."
                .into(),
        ));
    }
    if let Some(reset_at) = confirmed {
        return Ok(Preparation {
            reset_at,
            sent: true,
        });
    }
    let (target, _) = desktop_creds::account_target(config, label)?;
    let after = fetch::fetch_snapshot(
        &client,
        &target,
        &cache,
        &Default::default(),
        Duration::ZERO,
    )
    .await?;
    if !after.off_the_wire()
        || active_account_uuid(&paths.config_json()).as_deref() != Some(&active)
    {
        return Err(AppError::Other("Request completed, but the new reset time could not be confirmed. Refresh usage before retrying.".into()));
    }
    let reset_at = after.snapshot.session.resets_at.filter(|reset| *reset > Utc::now())
        .ok_or_else(|| AppError::Other("Request completed, but the server did not report a new window. Refresh usage before retrying.".into()))?;
    Ok(Preparation {
        reset_at,
        sent: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anthropic::types::UsageResponse;
    use serde_json::json;

    fn snapshot(used: i32, reset: Option<&str>, weekly: i32) -> AnthropicSnapshot {
        let response: UsageResponse = serde_json::from_value(json!({
            "five_hour":{"utilization":used,"resets_at":reset},
            "seven_day":{"utilization":weekly}
        }))
        .unwrap();
        response.into_snapshot("Pro".into())
    }
    fn now() -> DateTime<Utc> {
        "2026-10-10T14:00:01Z".parse().unwrap()
    }

    #[test]
    fn does_not_spend_another_turn_on_a_running_window_even_at_zero_percent() {
        let reset = "2026-10-10T19:00:00Z";
        assert_eq!(
            existing_window(&snapshot(0, Some(reset), 20), now()).unwrap(),
            Some(reset.parse().unwrap())
        );
    }
    #[test]
    fn expired_or_unused_windows_are_eligible_but_unknown_nonzero_usage_is_not() {
        assert_eq!(
            existing_window(&snapshot(100, Some("2026-10-10T14:00:00Z"), 20), now()).unwrap(),
            None
        );
        assert_eq!(
            existing_window(&snapshot(0, None, 20), now()).unwrap(),
            None
        );
        assert!(existing_window(&snapshot(31, None, 20), now()).is_err());
    }
    #[test]
    fn neither_live_exhaustion_nor_weekly_exhaustion_sends_a_turn() {
        assert!(existing_window(&snapshot(100, Some("2026-10-10T19:00:00Z"), 20), now()).is_err());
        assert!(existing_window(&snapshot(0, None, 100), now()).is_err());
    }
    #[test]
    fn trusted_cli_must_be_explicit_absolute_and_a_file() {
        assert!(trusted_binary(None).is_err());
        assert!(trusted_binary(Some(Path::new("claude"))).is_err());
        let directory = tempfile::tempdir().unwrap();
        assert!(trusted_binary(Some(directory.path())).is_err());
        let binary = directory.path().join("claude");
        std::fs::write(&binary, "fake").unwrap();
        assert_eq!(trusted_binary(Some(&binary)).unwrap(), binary);
    }
    #[test]
    fn unknown_and_current_identities_cannot_be_prepared() {
        assert!(inactive_identity(None, "two").is_err());
        assert!(inactive_identity(Some(String::new()), "two").is_err());
        assert!(inactive_identity(Some("two".into()), "two").is_err());
        assert_eq!(inactive_identity(Some("one".into()), "two").unwrap(), "one");
    }
    #[test]
    fn cli_uses_an_ephemeral_home_no_tools_no_history_and_no_token_argument() {
        let transport = [("HTTPS_PROXY".into(), "http://127.0.0.1:17714".into())];
        let mut command = isolated_command(
            Path::new("/trusted/claude"),
            Path::new("/isolated"),
            "fake-private-token",
            &transport,
        );
        let cmd = command.as_std_mut();
        let args: Vec<_> = cmd.get_args().map(|arg| arg.to_str().unwrap()).collect();
        assert!(args.contains(&"--safe-mode"));
        assert!(args.contains(&"--no-session-persistence"));
        assert!(args.windows(2).any(|args| args == ["--tools", ""]));
        assert!(args.windows(2).any(|args| args == ["--max-turns", "1"]));
        assert!(!args.iter().any(|arg| arg.contains("fake-private-token")));
        assert!(!args.contains(&"--resume"));
        assert!(!args.contains(&"--continue"));
        assert_eq!(cmd.get_current_dir(), Some(Path::new("/isolated")));
        let env: Vec<_> = cmd.get_envs().collect();
        assert!(env.iter().any(|(key, value)| *key == "CLAUDE_CONFIG_DIR"
            && *value == Some(std::ffi::OsStr::new("/isolated/config"))));
        assert!(
            env.iter()
                .any(|(key, value)| *key == "CLAUDE_CODE_OAUTH_TOKEN"
                    && *value == Some(std::ffi::OsStr::new("fake-private-token")))
        );
        assert!(env.iter().any(|(key, value)| *key == "HTTPS_PROXY"
            && *value == Some(std::ffi::OsStr::new("http://127.0.0.1:17714"))));
        assert!(
            !env.iter()
                .any(|(key, _)| *key == "ANTHROPIC_API_KEY" || *key == "ANTHROPIC_BASE_URL")
        );
    }
    #[test]
    fn a_process_exit_alone_is_not_a_successful_preparation_request() {
        assert!(request_succeeded(
            true,
            br#"{"type":"result","is_error":false,"subtype":"success"}"#
        ));
        assert!(!request_succeeded(
            false,
            br#"{"type":"result","is_error":false,"subtype":"success"}"#
        ));
        assert!(!request_succeeded(
            true,
            br#"{"type":"result","is_error":true,"subtype":"success"}"#
        ));
        assert!(!request_succeeded(
            true,
            br#"{"type":"result","is_error":false,"subtype":"error_max_turns"}"#
        ));
        assert!(!request_succeeded(true, b"token-or-diagnostics-not-json"));
    }

    #[test]
    fn only_an_allowed_future_five_hour_server_event_confirms_a_window() {
        let reset = now() + chrono::Duration::hours(5);
        let event = json!({"type":"rate_limit_event", "rate_limit_info":{
            "status":"allowed", "rateLimitType":"five_hour", "resetsAt":reset.timestamp()
        }});
        assert_eq!(
            response_reset(event.to_string().as_bytes(), now()),
            Some(reset)
        );
        let stream = format!(
            "diagnostics\n{}\n{{\"type\":\"result\",\"is_error\":false,\"subtype\":\"success\"}}\n",
            event
        );
        assert!(request_succeeded(true, stream.as_bytes()));
        assert_eq!(response_reset(stream.as_bytes(), now()), Some(reset));
        for (key, value) in [
            ("status", json!("rejected")),
            ("rateLimitType", json!("seven_day")),
            ("isUsingOverage", json!(true)),
            ("overageInUse", json!(true)),
            ("resetsAt", json!(now().timestamp())),
            ("resetsAt", json!("unknown")),
            (
                "resetsAt",
                json!((now() + chrono::Duration::hours(6)).timestamp()),
            ),
        ] {
            let mut invalid = event.clone();
            invalid["rate_limit_info"][key] = value;
            assert_eq!(response_reset(invalid.to_string().as_bytes(), now()), None);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn subprocess_state_is_deleted_after_both_a_reply_and_a_failure() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = tempfile::tempdir().unwrap();
        let binary = fixture.path().join("fake-claude");
        let record = fixture.path().join("record");
        let transport = [("TEST_RECORD_PATH".into(), record.as_os_str().to_owned())];
        for (exit, expected) in [(0, true), (1, false)] {
            std::fs::write(&binary, format!("#!/bin/sh\nprintf '%s' \"$HOME\" > \"$TEST_RECORD_PATH\"\n/bin/mkdir -p \"$CLAUDE_CONFIG_DIR\"\nprintf test > \"$CLAUDE_CONFIG_DIR/transcript\"\nprintf '%s' '{{\"type\":\"result\",\"is_error\":false,\"subtype\":\"success\"}}'\nexit {exit}\n")).unwrap();
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
            let result =
                isolated_turn(&binary, "fake-token", &transport, Duration::from_secs(5)).await;
            assert_eq!(result.is_ok(), expected);
            let temporary_home = std::fs::read_to_string(&record).unwrap();
            assert!(
                !Path::new(&temporary_home).exists(),
                "isolated state remained after child exit"
            );
            assert!(fixture.path().is_dir(), "test parent was touched");
        }
    }
}
