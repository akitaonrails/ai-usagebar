//! Ask the `droid` CLI to renew its own login.
//!
//! The access token in `auth.v2.keyring` lives about a day and the CLI renews
//! it only when it runs, so a machine where `droid` sits unused leaves the bar
//! on a stale figure. `droid doctor --checks auth` verifies the stored login
//! through Factory's WorkOS refresh and saves the renewed session itself —
//! no model call, no cost. Running it keeps the whole refresh inside the
//! official CLI: this provider still never reads the `refresh_token`, talks
//! to WorkOS, or writes the credential store.
//!
//! The command runs with stdin closed, its output discarded (it can name the
//! account), the provider secrets of this process removed from its
//! environment, and a hard timeout. One attempt per [`RETRY_AFTER`] per
//! machine, recorded in the cache directory, so a login the CLI cannot renew
//! (signed out, offline) does not spawn the CLI on every bar refresh.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, SystemTime};

use crate::cache::Cache;

/// Renew this close to `exp`, so the bar never shows a gap at expiry.
pub const RENEW_MARGIN: chrono::Duration = chrono::Duration::minutes(10);
/// Minimum spacing between two attempts, successful or not.
pub const RETRY_AFTER: Duration = Duration::from_secs(15 * 60);
/// `droid doctor` probes with its own 10s per-check timeout; this bounds the
/// whole process, startup included.
const RUN_TIMEOUT: Duration = Duration::from_secs(30);
const STAMP_FILE: &str = ".cli_renewal";
pub const DEFAULT_PROGRAM: &str = "droid";
pub const ARGS: [&str; 5] = ["doctor", "--checks", "auth", "--timeout", "10000"];

/// The renewal command, with every safety setting applied. Split out so the
/// tests can inspect it without spawning anything.
pub fn command(program: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(program);
    command
        .args(ARGS)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for var in crate::vendor::vendor_secret_env_vars_to_remove(&[]) {
        command.env_remove(var);
    }
    command
}

pub fn program(cfg: &crate::config::DroidConfig) -> PathBuf {
    cfg.cli_path
        .clone()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from(DEFAULT_PROGRAM))
}

fn stamp_path(cache: &Cache) -> PathBuf {
    cache.dir().join(STAMP_FILE)
}

/// Whether the last attempt is old enough to try again. A stamp from the
/// future (clock moved back) counts as old.
pub fn due_at(cache: &Cache, now: SystemTime) -> bool {
    let Ok(modified) = std::fs::metadata(stamp_path(cache)).and_then(|meta| meta.modified()) else {
        return true;
    };
    now.duration_since(modified)
        .map_or(true, |age| age >= RETRY_AFTER)
}

/// Run the CLI once if an attempt is due. Returns whether it ran and exited
/// cleanly; the caller re-reads the login either way it changed. The stamp is
/// written before the spawn, so a hung or crashing CLI still counts.
pub async fn renew(cfg: &crate::config::DroidConfig, cache: &Cache) -> bool {
    if !due_at(cache, SystemTime::now()) {
        return false;
    }
    if std::fs::write(stamp_path(cache), b"").is_err() {
        return false;
    }
    run(command(&program(cfg)), RUN_TIMEOUT).await
}

async fn run(mut command: tokio::process::Command, timeout: Duration) -> bool {
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => status.success(),
        _ => {
            let _ = child.kill().await;
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn cache_fixture() -> (TempDir, Cache) {
        let dir = TempDir::new().unwrap();
        let cache = Cache::at(dir.path().join("droid"));
        cache.ensure_dir().unwrap();
        (dir, cache)
    }

    #[test]
    fn the_command_only_runs_the_auth_check_and_drops_provider_secrets() {
        let command = command(Path::new("/opt/droid"));
        let std = command.as_std();
        assert_eq!(std.get_program(), "/opt/droid");
        let args: Vec<_> = std.get_args().collect();
        assert_eq!(args, ARGS);
        let removed: Vec<_> = std
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(key, _)| key.to_string_lossy().into_owned())
            .collect();
        for var in crate::vendor::vendor_secret_env_vars_to_remove(&[]) {
            assert!(removed.iter().any(|key| key == var), "{var} not removed");
        }
    }

    #[test]
    fn the_program_defaults_to_droid_on_path() {
        let mut cfg = crate::config::DroidConfig::default();
        assert_eq!(program(&cfg), PathBuf::from("droid"));
        cfg.cli_path = Some(PathBuf::new());
        assert_eq!(program(&cfg), PathBuf::from("droid"));
        cfg.cli_path = Some(PathBuf::from("/opt/factory/droid"));
        assert_eq!(program(&cfg), PathBuf::from("/opt/factory/droid"));
    }

    #[test]
    fn attempts_are_spaced_by_the_stamp() {
        let (_dir, cache) = cache_fixture();
        assert!(due_at(&cache, SystemTime::now()));
        std::fs::write(stamp_path(&cache), b"").unwrap();
        let now = SystemTime::now();
        assert!(!due_at(&cache, now));
        assert!(due_at(&cache, now + RETRY_AFTER + Duration::from_secs(1)));
        // A stamp ahead of the clock does not block forever.
        assert!(due_at(&cache, now - Duration::from_secs(3600)));
    }

    #[tokio::test]
    async fn a_second_call_inside_the_window_does_not_spawn() {
        let (_dir, cache) = cache_fixture();
        let cfg = crate::config::DroidConfig {
            cli_path: Some(PathBuf::from("/nonexistent/droid-cli")),
            ..Default::default()
        };
        // A missing CLI is a failed attempt, not an error, and still stamps.
        assert!(!renew(&cfg, &cache).await);
        assert!(stamp_path(&cache).exists());
        assert!(!due_at(&cache, SystemTime::now()));
        assert!(!renew(&cfg, &cache).await);
    }

    #[tokio::test]
    async fn exit_status_and_timeout_decide_the_result() {
        let mut ok = tokio::process::Command::new("true");
        ok.kill_on_drop(true);
        assert!(run(ok, Duration::from_secs(5)).await);
        let mut failed = tokio::process::Command::new("false");
        failed.kill_on_drop(true);
        assert!(!run(failed, Duration::from_secs(5)).await);
        let mut hung = tokio::process::Command::new("sleep");
        hung.arg("30").kill_on_drop(true);
        let started = std::time::Instant::now();
        assert!(!run(hung, Duration::from_millis(200)).await);
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
