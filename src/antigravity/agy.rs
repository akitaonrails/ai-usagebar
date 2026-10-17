//! Run the `agy` CLI itself: to read the quota it reports, and to renew
//! Antigravity's saved Google session.
//!
//! **Quota.** `agy -p "/quota" --output-format json` prints the summary that
//! `agy`'s own Models & Quota page shows, as structured JSON, without starting
//! an agent turn or leaving a conversation behind. For an `agy` account that
//! reading is the reference, and the Cloud Code request the remote fallback
//! makes cannot reproduce it: prod reports another Gemini tracker and daily
//! leaves the Gemini group out (#450). [`quota_snapshot`] runs it, bounded in
//! time, in output and — after a failure — in frequency.
//!
//! **Renewal.** The saved session's access token lasts about an hour, and the
//! only thing that renews it is a running Antigravity. With the desktop app
//! open that happens in the background; with only the `agy` CLI installed
//! nothing does between the user's own runs, so the session sits expired and
//! this program — which does not ship Antigravity's OAuth client — cannot mint
//! a new token. `agy models` is the lightest command that authenticates: it
//! lists the models, needs no TTY and no prompt, and rewrites the saved
//! credential (the OS keyring entry) with a fresh token as a side effect.
//! [`renew_session`] runs it once, bounded in time and in frequency, so the
//! caller can read the credential again; it neither parses `agy`'s output nor
//! touches a token.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::cache::Cache;
use crate::usage::AntigravitySnapshot;

/// `agy models` took about nine seconds on a cold start; well past that it is
/// asking for a login rather than renewing.
const RENEW_TIMEOUT: Duration = Duration::from_secs(25);
/// A refresh token that is itself dead makes every attempt fail, so failures
/// are remembered too: the widget polls every few seconds and must not spawn
/// `agy` on each poll.
const COOLDOWN_SECS: i64 = 600;
const ATTEMPT_FILE: &str = "agy_renew_attempt";

/// `agy -p "/quota"` answers in about five seconds; well past that it is stuck
/// or asking for a login.
const QUOTA_TIMEOUT: Duration = Duration::from_secs(20);
/// Each reading starts `agy` and costs about three seconds of CPU, so it is
/// taken at most this often, however short the cache TTL. `agy` refreshes its
/// own figures on about the same cadence while it works.
pub const QUOTA_INTERVAL: Duration = Duration::from_secs(120);
/// A failed reading holds off the next for [`COOLDOWN_SECS`], like a failed
/// renewal: `agy` that cannot answer now will not answer on the next poll.
const QUOTA_FAILURE_FILE: &str = "agy_quota_failure";
/// Where `agy` writes its log for a quota reading: nowhere. Without
/// `--log-file` every run adds a file to `~/.gemini/antigravity-cli/log`, and
/// a file of ours would keep the signed-in address `agy` logs.
const NULL_DEVICE: &str = if cfg!(windows) { "NUL" } else { "/dev/null" };

/// Where the renewal command comes from. Production discovers `agy`; a test
/// names a script of its own or turns the whole mechanism off so it can never
/// run the real CLI.
#[derive(Debug, Clone, Default)]
pub enum AgyCommand {
    /// Never run anything. What [`RemoteOverride::default`] means, so a test
    /// that does not mention `agy` is hermetic by construction.
    ///
    /// [`RemoteOverride::default`]: super::fetch::RemoteOverride
    #[default]
    Off,
    /// Look `agy` up on `PATH`, then in `~/.local/bin`.
    Discover,
    /// Run exactly this program.
    At(PathBuf),
}

impl AgyCommand {
    fn program(&self) -> Option<PathBuf> {
        match self {
            Self::Off => None,
            Self::Discover => discover(),
            Self::At(path) => Some(path.clone()),
        }
    }

    /// Whether there is an `agy` to run at all.
    pub fn is_available(&self) -> bool {
        self.program().is_some()
    }
}

/// `agy` on `PATH`, else `~/.local/bin/agy`: the installer's own target, which
/// a tray started by launchd or a desktop session does not have on `PATH`.
fn discover() -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let home = crate::cache::home_dir().ok();
    find_agy(&path, home.as_deref())
}

fn find_agy(path_var: &std::ffi::OsStr, home: Option<&Path>) -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) {
        &["agy.exe", "agy.cmd", "agy.bat"]
    } else {
        &["agy"]
    };
    let mut dirs: Vec<PathBuf> = std::env::split_paths(path_var).collect();
    if let Some(home) = home {
        dirs.push(home.join(".local").join("bin"));
    }
    dirs.iter()
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|candidate| candidate.is_file())
}

/// Whether the last attempt recorded in `file` is recent enough that another
/// would be noise.
fn cooling_down(cache: &Cache, file: &str, now: DateTime<Utc>) -> bool {
    std::fs::read_to_string(cache.dir().join(file))
        .ok()
        .and_then(|text| text.trim().parse::<i64>().ok())
        .is_some_and(|at| (0..COOLDOWN_SECS).contains(&(now.timestamp() - at)))
}

fn note_attempt(cache: &Cache, file: &str, now: DateTime<Utc>) {
    // Best effort: failing to record only costs one more attempt next poll.
    let _ = crate::cache::atomic_write(
        &cache.dir().join(file),
        now.timestamp().to_string().as_bytes(),
    );
}

/// `agy`, spawned the way every run here is: no input, no stderr, and killed
/// with its handle when a timeout drops it.
fn agy_command(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    // `agy` otherwise launches its own `--bg-updater` at most every 15
    // minutes, and on Windows that child opens a console window of its own
    // that CREATE_NO_WINDOW on this spawn cannot reach. Only the literal
    // `true` is honored (`1` is ignored); this touches only this run, never
    // the user's own `agy` sessions.
    cmd.env("AGY_CLI_DISABLE_AUTO_UPDATE", "true")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    // A vendor-CLI spawn must not inherit this process's provider keys —
    // the same discipline as running `claude` for account capture.
    for var in crate::vendor::vendor_secret_env_vars_to_remove(&[]) {
        cmd.env_remove(var);
    }
    #[cfg(windows)]
    cmd.creation_flags(crate::process::CREATE_NO_WINDOW);
    cmd
}

/// Run `agy models` so it renews the saved session. `true` means the command
/// ran and exited successfully — the caller then reads the credential again,
/// and is the only judge of whether that produced a usable token. `false`
/// covers everything else: no `agy`, cooling down, a spawn error, a nonzero
/// exit, the timeout.
pub async fn renew_session(command: &AgyCommand, cache: &Cache, now: DateTime<Utc>) -> bool {
    let Some(program) = command.program() else {
        return false;
    };
    if cooling_down(cache, ATTEMPT_FILE, now) {
        return false;
    }
    note_attempt(cache, ATTEMPT_FILE, now);

    let mut cmd = agy_command(&program);
    cmd.arg("models").stdout(Stdio::null());
    let Ok(mut child) = cmd.spawn() else {
        return false;
    };
    matches!(
        tokio::time::timeout(RENEW_TIMEOUT, child.wait()).await,
        Ok(Ok(status)) if status.success()
    )
}

/// The quota as `agy` itself reports it, parsed like the RPC's summary. The
/// caller supplies what the reading lacks: the plan, the account, the source.
///
/// `None` is everything short of a clean reading — no `agy`, a recent failure,
/// a spawn error, a nonzero exit, the timeout, output past the cap or without
/// a summary we can use — and every attempt that ends that way holds off the
/// next one for the cooldown.
pub async fn quota_snapshot(
    command: &AgyCommand,
    cache: &Cache,
    now: DateTime<Utc>,
) -> Option<AntigravitySnapshot> {
    let program = command.program()?;
    if cooling_down(cache, QUOTA_FAILURE_FILE, now) {
        return None;
    }
    let snapshot = read_quota(&program)
        .await
        .as_ref()
        .and_then(reshape_quota)
        .and_then(|summary| super::fetch::parse_quota_summary(&summary, String::new()).ok());
    if snapshot.is_none() {
        note_attempt(cache, QUOTA_FAILURE_FILE, now);
    }
    snapshot
}

/// `agy -p "/quota" --output-format json`, read like any untrusted response:
/// capped at [`MAX_BODY_BYTES`](crate::vendor::MAX_BODY_BYTES), and only from
/// a run that exits cleanly in time.
async fn read_quota(program: &Path) -> Option<serde_json::Value> {
    let mut cmd = agy_command(program);
    cmd.args([
        "-p",
        "/quota",
        "--output-format",
        "json",
        "--log-file",
        NULL_DEVICE,
    ])
    .stdout(Stdio::piped());
    let mut child = cmd.spawn().ok()?;
    let stdout = child.stdout.take()?;
    let run = async move {
        let mut bytes = Vec::new();
        stdout
            .take(crate::vendor::MAX_BODY_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .await
            .ok()?;
        if bytes.len() > crate::vendor::MAX_BODY_BYTES {
            return None;
        }
        let status = child.wait().await.ok()?;
        status.success().then_some(bytes)
    };
    let bytes = tokio::time::timeout(QUOTA_TIMEOUT, run).await.ok()??;
    serde_json::from_slice(&bytes).ok()
}

/// `agy` prints the summary under `command.data.groups`, in snake_case; the
/// RPC and the Cloud Code API carry the same fields in camelCase, which is
/// what [`parse_quota_summary`](super::fetch::parse_quota_summary) reads. Only
/// a run that reports success counts. A disabled bucket — shown dimmed, with
/// no figure — is left out rather than failing the whole reading.
fn reshape_quota(output: &serde_json::Value) -> Option<serde_json::Value> {
    if output["status"].as_str() != Some("SUCCESS") {
        return None;
    }
    let groups = output["command"]["data"]["groups"]
        .as_array()?
        .iter()
        .map(|group| {
            let buckets = group["buckets"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|bucket| bucket["disabled"].as_bool() != Some(true))
                .map(|bucket| {
                    serde_json::json!({
                        "bucketId": bucket["id"],
                        "displayName": bucket["name"],
                        "window": bucket["window"],
                        "remainingFraction": bucket["remaining_fraction"],
                        "resetTime": bucket["reset_time"],
                    })
                })
                .collect::<Vec<_>>();
            serde_json::json!({ "displayName": group["name"], "buckets": buckets })
        })
        .collect::<Vec<_>>();
    Some(serde_json::json!({ "groups": groups }))
}

/// Stand-in `agy` scripts for the tests of this module and of `fetch.rs`.
#[cfg(all(test, unix))]
pub(crate) mod test_support {
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    /// The argument a stand-in answers before running its body.
    const PROBE: &str = "--test-probe";

    /// Write `body` as an executable `#!/bin/sh` script at `path`, and return
    /// once it can be executed.
    ///
    /// Linux refuses to execute a file that any process holds open for writing
    /// (`ETXTBSY`), and a sibling test that spawns while this one writes
    /// inherits the descriptor until its own child execs. So the script first
    /// answers a probe with `exit 0`, and the probe is retried until it runs:
    /// once one exec succeeds, nothing can hold the file open for writing any
    /// more, and the code under test never races that window.
    pub(crate) fn write_script(path: &Path, body: &str) {
        std::fs::write(
            path,
            format!("#!/bin/sh\n[ \"$1\" = {PROBE} ] && exit 0\n{body}\n"),
        )
        .unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        for _ in 0..500 {
            match std::process::Command::new(path).arg(PROBE).status() {
                Ok(_) => return,
                Err(error) if error.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(error) => panic!("{}: {error}", path.display()),
            }
        }
        panic!("{} stayed busy", path.display());
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        test_support::write_script(&path, body);
        path
    }

    fn cache(dir: &TempDir) -> Cache {
        let cache = Cache::at(dir.path().join("cache"));
        cache.ensure_dir().unwrap();
        cache
    }

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000, 0).unwrap()
    }

    #[tokio::test]
    async fn off_never_runs_anything() {
        let dir = TempDir::new().unwrap();
        assert!(!renew_session(&AgyCommand::Off, &cache(&dir), now()).await);
    }

    #[tokio::test]
    async fn runs_models_with_the_updater_off_and_reports_success() {
        let dir = TempDir::new().unwrap();
        let marker = dir.path().join("args");
        let agy = script(
            dir.path(),
            "agy",
            &format!(
                "echo \"$@ $AGY_CLI_DISABLE_AUTO_UPDATE\" > '{}'",
                marker.display()
            ),
        );
        assert!(renew_session(&AgyCommand::At(agy), &cache(&dir), now()).await);
        // `models`, with agy's own background updater switched off.
        assert_eq!(
            std::fs::read_to_string(marker).unwrap().trim(),
            "models true"
        );
    }

    #[tokio::test]
    async fn a_failing_command_reports_failure() {
        let dir = TempDir::new().unwrap();
        let agy = script(dir.path(), "agy", "exit 3");
        assert!(!renew_session(&AgyCommand::At(agy), &cache(&dir), now()).await);
    }

    #[tokio::test]
    async fn a_missing_program_reports_failure() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("no-such-agy");
        assert!(!renew_session(&AgyCommand::At(missing), &cache(&dir), now()).await);
    }

    #[tokio::test]
    async fn an_attempt_is_not_repeated_inside_the_cooldown_even_after_failure() {
        let dir = TempDir::new().unwrap();
        let cache = cache(&dir);
        let count = dir.path().join("count");
        let agy = script(
            dir.path(),
            "agy",
            &format!("echo x >> '{}'; exit 1", count.display()),
        );
        let command = AgyCommand::At(agy);
        assert!(!renew_session(&command, &cache, now()).await);
        assert!(!renew_session(&command, &cache, now() + chrono::Duration::seconds(30)).await);
        assert_eq!(std::fs::read_to_string(&count).unwrap().lines().count(), 1);
        // Past the cooldown it tries again.
        let later = now() + chrono::Duration::seconds(COOLDOWN_SECS + 1);
        assert!(!renew_session(&command, &cache, later).await);
        assert_eq!(std::fs::read_to_string(&count).unwrap().lines().count(), 2);
    }

    /// `agy -p "/quota" --output-format json` from agy 1.4.0 on a Pro account,
    /// with the descriptions shortened.
    const QUOTA_OUTPUT: &str = r#"{"conversation_id":"","status":"SUCCESS","response":"Gemini Models\tWeekly Limit Remaining\t63%\t2026-10-24T01:11:19Z","duration_seconds":0,"num_turns":0,
"usage":{"input_tokens":0,"output_tokens":0,"thinking_tokens":0,"cache_read_tokens":0,"total_tokens":0},
"command":{"name":"usage","data":{"description":"Within each group, models share a weekly limit and a 5-hour limit.","groups":[
{"name":"Gemini Models","description":"Models within this group: Gemini Flash, Gemini Pro","buckets":[
 {"id":"gemini-weekly","name":"Weekly Limit Remaining","description":"Weekly","window":"weekly","remaining_fraction":0.6316714286804199,"reset_time":"2026-10-24T01:11:19Z"},
 {"id":"gemini-5h","name":"Five Hour Limit Remaining","description":"Five hour","window":"5h","remaining_fraction":0.4934307336807251,"reset_time":"2026-10-17T20:25:45Z"}]},
{"name":"Claude and GPT models","description":"Models within this group: Claude Opus, Claude Sonnet, GPT-OSS","buckets":[
 {"id":"3p-weekly","name":"Weekly Limit Remaining","description":"Weekly","window":"weekly","remaining_fraction":1,"reset_time":"2026-10-24T17:29:37Z"},
 {"id":"3p-5h","name":"Five Hour Limit Remaining","description":"Five hour","window":"5h","remaining_fraction":1,"reset_time":"2026-10-17T22:29:37Z"}]},
{"name":"Gemini 4 Argon","description":"For paid Pro plans, Argon quota is separate.","buckets":[
 {"id":"gemini-argon-wtu-weekly","name":"Gemini 4 Argon Weekly Limit","description":"Weekly","window":"weekly","remaining_fraction":0,"reset_time":"2026-10-24T01:10:47Z"}]}]}}}"#;

    fn printing(dir: &Path, output: &str) -> PathBuf {
        script(dir, "agy", &format!("cat <<'EOF'\n{output}\nEOF"))
    }

    #[tokio::test]
    async fn off_never_reads_the_quota() {
        let dir = TempDir::new().unwrap();
        assert!(
            quota_snapshot(&AgyCommand::Off, &cache(&dir), now())
                .await
                .is_none()
        );
    }

    /// The figures `agy` shows, Argon included, read with its log and its
    /// background updater both switched off.
    #[tokio::test]
    async fn reads_the_quota_agy_reports_with_its_log_and_updater_off() {
        let dir = TempDir::new().unwrap();
        let marker = dir.path().join("args");
        let agy = script(
            dir.path(),
            "agy",
            &format!(
                "echo \"$@ $AGY_CLI_DISABLE_AUTO_UPDATE\" > '{}'\ncat <<'EOF'\n{QUOTA_OUTPUT}\nEOF",
                marker.display()
            ),
        );
        let snap = quota_snapshot(&AgyCommand::At(agy), &cache(&dir), now())
            .await
            .expect("a clean reading");
        assert_eq!(
            std::fs::read_to_string(marker).unwrap().trim(),
            "-p /quota --output-format json --log-file /dev/null true"
        );
        assert_eq!(snap.session.as_ref().unwrap().utilization_pct, 51);
        assert_eq!(snap.weekly.as_ref().unwrap().utilization_pct, 37);
        assert_eq!(
            snap.third_party_session.as_ref().unwrap().utilization_pct,
            0
        );
        assert_eq!(snap.third_party_weekly.as_ref().unwrap().utilization_pct, 0);
        let [argon] = snap.model_pools.as_slice() else {
            panic!("{:?}", snap.model_pools);
        };
        assert_eq!(argon.name, "Gemini 4 Argon");
        assert_eq!(argon.window.utilization_pct, 100);
    }

    /// Anything short of a summary we can use is no reading at all.
    #[tokio::test]
    async fn output_without_a_usable_summary_is_no_reading() {
        for output in [
            r#"{"status":"ERROR","command":{"data":{"groups":[]}}}"#,
            r#"{"status":"SUCCESS","command":{"name":"usage","data":{}}}"#,
            r#"{"status":"SUCCESS","command":{"data":{"groups":[{"name":"X","buckets":[{"id":"x-monthly","window":"monthly","remaining_fraction":0.5}]}]}}}"#,
            "not json",
        ] {
            let dir = TempDir::new().unwrap();
            let agy = printing(dir.path(), output);
            assert!(
                quota_snapshot(&AgyCommand::At(agy), &cache(&dir), now())
                    .await
                    .is_none(),
                "{output}"
            );
        }
    }

    #[tokio::test]
    async fn a_failing_or_oversized_run_is_no_reading() {
        for body in [
            format!("cat <<'EOF'\n{QUOTA_OUTPUT}\nEOF\nexit 1"),
            format!(
                "head -c {} /dev/zero | tr '\\0' ' '\ncat <<'EOF'\n{QUOTA_OUTPUT}\nEOF",
                crate::vendor::MAX_BODY_BYTES
            ),
        ] {
            let dir = TempDir::new().unwrap();
            let agy = script(dir.path(), "agy", &body);
            assert!(
                quota_snapshot(&AgyCommand::At(agy), &cache(&dir), now())
                    .await
                    .is_none()
            );
        }
    }

    /// A failed reading is not retried on every poll; a clean one is not held
    /// off at all.
    #[tokio::test]
    async fn a_failed_reading_is_not_repeated_inside_the_cooldown() {
        let dir = TempDir::new().unwrap();
        let store = cache(&dir);
        let count = dir.path().join("count");
        let agy = script(
            dir.path(),
            "agy",
            &format!("echo x >> '{}'; exit 1", count.display()),
        );
        let command = AgyCommand::At(agy);
        assert!(quota_snapshot(&command, &store, now()).await.is_none());
        let soon = now() + chrono::Duration::seconds(30);
        assert!(quota_snapshot(&command, &store, soon).await.is_none());
        assert_eq!(std::fs::read_to_string(&count).unwrap().lines().count(), 1);
        let later = now() + chrono::Duration::seconds(COOLDOWN_SECS + 1);
        assert!(quota_snapshot(&command, &store, later).await.is_none());
        assert_eq!(std::fs::read_to_string(&count).unwrap().lines().count(), 2);
    }

    /// Only a failure holds the next reading off: how often a clean one is
    /// taken is the cache's business.
    #[tokio::test]
    async fn a_clean_reading_is_not_held_off() {
        let dir = TempDir::new().unwrap();
        let store = cache(&dir);
        let command = AgyCommand::At(printing(dir.path(), QUOTA_OUTPUT));
        assert!(quota_snapshot(&command, &store, now()).await.is_some());
        let soon = now() + chrono::Duration::seconds(30);
        assert!(quota_snapshot(&command, &store, soon).await.is_some());
    }

    /// A disabled bucket has no figure to show; it is left out instead of
    /// taking the rest of the reading down with it.
    #[test]
    fn a_disabled_bucket_is_left_out_of_the_reading() {
        let output = serde_json::json!({
            "status": "SUCCESS",
            "command": {"data": {"groups": [{"name": "Gemini Models", "buckets": [
                {"id": "gemini-5h", "window": "5h", "remaining_fraction": 0.5,
                 "reset_time": "2026-10-17T20:25:45Z"},
                {"id": "gemini-weekly", "window": "weekly", "disabled": true}
            ]}]}}
        });
        let summary = reshape_quota(&output).unwrap();
        assert_eq!(summary["groups"][0]["buckets"].as_array().unwrap().len(), 1);
        let snap = super::super::fetch::parse_quota_summary(&summary, String::new()).unwrap();
        assert_eq!(snap.session.unwrap().utilization_pct, 50);
        assert!(snap.weekly.is_none());
    }

    #[test]
    fn discovery_prefers_path_then_falls_back_to_local_bin() {
        let dir = TempDir::new().unwrap();
        let on_path = dir.path().join("bin");
        let home = dir.path().join("home");
        std::fs::create_dir_all(&on_path).unwrap();
        std::fs::create_dir_all(home.join(".local/bin")).unwrap();
        let fallback = script(&home.join(".local/bin"), "agy", "true");

        let path_var = std::env::join_paths([&on_path]).unwrap();
        assert_eq!(find_agy(&path_var, Some(&home)), Some(fallback.clone()));

        let first = script(&on_path, "agy", "true");
        assert_eq!(find_agy(&path_var, Some(&home)), Some(first));
        assert_eq!(find_agy("".as_ref(), None), None);
    }
}
