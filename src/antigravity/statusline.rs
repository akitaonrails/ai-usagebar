#[cfg(any(target_os = "macos", test))]
use std::collections::HashSet;
#[cfg(any(target_os = "macos", test))]
use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::Command;

#[cfg(any(target_os = "macos", test))]
use chrono::{DateTime, Duration, Utc};
#[cfg(any(target_os = "macos", test))]
use hmac::{Hmac, KeyInit, Mac};
#[cfg(any(target_os = "macos", test))]
use serde::{Deserialize, Serialize};
#[cfg(any(target_os = "macos", test))]
use sha2::{Digest, Sha256};

use crate::error::{AppError, Result};
#[cfg(any(target_os = "macos", test))]
use crate::usage::UsageWindow;

#[cfg(any(target_os = "macos", test))]
const MAX_PROCESS_ANCESTRY: usize = 32;
pub const STALE_AFTER: chrono::Duration = chrono::Duration::minutes(15);

#[cfg(any(target_os = "macos", test))]
type HmacSha256 = Hmac<Sha256>;

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub pid: u32,
    pub account_hash: String,
    pub masked_email: String,
    pub plan: String,
    pub session: Option<SnapshotWindow>,
    pub weekly: Option<SnapshotWindow>,
    pub third_party_session: Option<SnapshotWindow>,
    pub third_party_weekly: Option<SnapshotWindow>,
    /// Quota windows whose bucket id identifies the known Argon model pool.
    /// Older snapshots predate this field and remain readable.
    #[serde(default)]
    pub argon: Vec<SnapshotWindow>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotWindow {
    pub utilization_pct: i32,
    pub resets_at: Option<DateTime<Utc>>,
    pub window_duration: Duration,
}

#[cfg(any(target_os = "macos", test))]
impl From<SnapshotWindow> for UsageWindow {
    fn from(window: SnapshotWindow) -> Self {
        Self {
            utilization_pct: window.utilization_pct,
            resets_at: window.resets_at,
            window_duration: window.window_duration,
        }
    }
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IngestOutcome {
    Ignored,
    Written,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetupOutcome {
    Installed,
    AlreadyInstalled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoveOutcome {
    Removed,
    NotInstalled,
}

fn setup_message(outcome: SetupOutcome) -> &'static str {
    match outcome {
        SetupOutcome::Installed => "Antigravity status line installed.",
        SetupOutcome::AlreadyInstalled => "Antigravity status line was already installed.",
    }
}

fn remove_message(outcome: RemoveOutcome) -> &'static str {
    match outcome {
        RemoveOutcome::Removed => "Antigravity status line removed.",
        RemoveOutcome::NotInstalled => "No AI UsageBar status line to remove.",
    }
}

#[cfg(any(target_os = "macos", test))]
trait ProcessInspector {
    fn parent_pid(&self, pid: u32) -> Option<u32>;
    fn executable_name(&self, pid: u32) -> Option<std::ffi::OsString>;
}

#[cfg(target_os = "macos")]
struct MacProcessInspector;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountSession {
    pub id_suffix: String,
    pub masked_email: String,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub snapshot: crate::usage::AntigravitySnapshot,
}

#[cfg(target_os = "macos")]
impl ProcessInspector for MacProcessInspector {
    fn parent_pid(&self, pid: u32) -> Option<u32> {
        let output = Command::new("/bin/ps")
            .args(["-p", &pid.to_string(), "-o", "ppid="])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8_lossy(&output.stdout).trim().parse().ok()
    }

    fn executable_name(&self, pid: u32) -> Option<std::ffi::OsString> {
        let output = Command::new("/bin/ps")
            .args(["-p", &pid.to_string(), "-o", "comm="])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        Some(std::ffi::OsString::from(
            String::from_utf8_lossy(&output.stdout).trim(),
        ))
    }
}

pub fn ingest<R: Read>(reader: R) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let root = directories::BaseDirs::new()
            .ok_or_else(|| AppError::Other("could not resolve macOS cache directory".into()))?
            .cache_dir()
            .join("ai-usagebar/antigravity/sessions");
        let _ = ingest_with(
            reader,
            &root,
            &MacProcessInspector,
            std::process::id(),
            Utc::now(),
        )?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = reader;
        Err(AppError::Other(
            "Antigravity status line integration is supported only on macOS".into(),
        ))
    }
}

pub fn active_accounts() -> Result<Vec<AccountSession>> {
    #[cfg(target_os = "macos")]
    {
        let root = directories::BaseDirs::new()
            .ok_or_else(|| AppError::Other("could not resolve macOS cache directory".into()))?
            .cache_dir()
            .join("ai-usagebar/antigravity/sessions");
        Ok(read_active_accounts(&root, &MacProcessInspector))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(Vec::new())
    }
}

pub fn run(action: &crate::widget::cli::AntigravityAction) -> i32 {
    use crate::widget::cli::AntigravityAction;

    #[cfg(not(target_os = "macos"))]
    if !matches!(action, AntigravityAction::IngestStatusline) {
        eprintln!("ai-usagebar: Antigravity status line integration is supported only on macOS");
        return 1;
    }

    let result = match action {
        AntigravityAction::IngestStatusline => {
            return match ingest(std::io::stdin().lock()) {
                Ok(()) => 0,
                Err(error) => {
                    eprintln!(
                        "ai-usagebar: {}",
                        crate::display::sanitize_untrusted_field(&error.to_string())
                    );
                    1
                }
            };
        }
        AntigravityAction::SetupStatusline => {
            let path = match antigravity_settings_path() {
                Ok(path) => path,
                Err(error) => {
                    eprintln!(
                        "ai-usagebar: {}",
                        crate::display::sanitize_untrusted_field(&error.to_string())
                    );
                    return 1;
                }
            };
            let executable = match std::env::current_exe() {
                Ok(path) => path,
                Err(error) => {
                    eprintln!(
                        "ai-usagebar: {}",
                        crate::display::sanitize_untrusted_field(&error.to_string())
                    );
                    return 1;
                }
            };
            setup(&path, &executable).map(setup_message)
        }
        AntigravityAction::RemoveStatusline => {
            let path = match antigravity_settings_path() {
                Ok(path) => path,
                Err(error) => {
                    eprintln!(
                        "ai-usagebar: {}",
                        crate::display::sanitize_untrusted_field(&error.to_string())
                    );
                    return 1;
                }
            };
            let executable = match std::env::current_exe() {
                Ok(path) => path,
                Err(error) => {
                    eprintln!(
                        "ai-usagebar: {}",
                        crate::display::sanitize_untrusted_field(&error.to_string())
                    );
                    return 1;
                }
            };
            remove(&path, &executable).map(remove_message)
        }
    };
    match result {
        Ok(message) => {
            println!("{message}");
            0
        }
        Err(error) => {
            eprintln!(
                "ai-usagebar: {}",
                crate::display::sanitize_untrusted_field(&error.to_string())
            );
            1
        }
    }
}

fn antigravity_settings_path() -> Result<PathBuf> {
    let home = directories::BaseDirs::new()
        .ok_or_else(|| AppError::Other("could not resolve home directory".into()))?
        .home_dir()
        .to_path_buf();
    Ok(home.join(".gemini/antigravity-cli/settings.json"))
}

fn setup(settings_path: &Path, executable: &Path) -> Result<SetupOutcome> {
    let mut settings = read_settings(settings_path)?;
    let root = settings
        .as_object_mut()
        .ok_or_else(|| AppError::Schema("Antigravity settings must be a JSON object".into()))?;
    let command = statusline_command(executable);
    if let Some(status_line) = root.get("statusLine") {
        if is_our_status_line(status_line, &command) {
            return Ok(SetupOutcome::AlreadyInstalled);
        }
        return Err(AppError::Other(
            "Antigravity settings already contain another custom status line; refusing to replace it".into(),
        ));
    }
    root.insert(
        "statusLine".into(),
        serde_json::json!({
            "type": "command",
            "command": command,
            "enabled": true,
            "stack_with_default": true
        }),
    );
    write_settings(settings_path, &settings)?;
    Ok(SetupOutcome::Installed)
}

fn remove(settings_path: &Path, executable: &Path) -> Result<RemoveOutcome> {
    let mut settings = match std::fs::read(settings_path) {
        Ok(bytes) => serde_json::from_slice::<serde_json::Value>(&bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RemoveOutcome::NotInstalled);
        }
        Err(error) => return Err(AppError::io_at(settings_path, error)),
    };
    let root = settings
        .as_object_mut()
        .ok_or_else(|| AppError::Schema("Antigravity settings must be a JSON object".into()))?;
    let command = statusline_command(executable);
    if root
        .get("statusLine")
        .is_some_and(|status_line| is_our_status_line(status_line, &command))
    {
        root.remove("statusLine");
        write_settings(settings_path, &settings)?;
        Ok(RemoveOutcome::Removed)
    } else {
        Ok(RemoveOutcome::NotInstalled)
    }
}

fn read_settings(path: &Path) -> Result<serde_json::Value> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::json!({})),
        Err(error) => Err(AppError::io_at(path, error)),
    }
}

fn write_settings(path: &Path, settings: &serde_json::Value) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(settings)?;
    crate::cache::atomic_write(path, &bytes)
}

fn statusline_command(executable: &Path) -> String {
    let executable = executable.to_string_lossy().replace('\'', "'\\''");
    format!("'{executable}' antigravity ingest-statusline")
}

fn is_our_status_line(value: &serde_json::Value, expected_command: &str) -> bool {
    value
        .get("command")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|command| command.trim() == expected_command)
}

#[cfg(any(target_os = "macos", test))]
fn ingest_with<R: Read>(
    mut reader: R,
    root: &Path,
    inspector: &impl ProcessInspector,
    current_pid: u32,
    now: DateTime<Utc>,
) -> Result<IngestOutcome> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Ok(IngestOutcome::Ignored);
    };
    let Some(pid) = find_agy_ancestor(inspector, current_pid) else {
        return Ok(IngestOutcome::Ignored);
    };
    let Some(snapshot) = snapshot_from_payload(&payload, pid, now) else {
        return Ok(IngestOutcome::Ignored);
    };
    write_snapshot_atomic(root, &snapshot)?;
    Ok(IngestOutcome::Written)
}

#[cfg(any(target_os = "macos", test))]
fn snapshot_from_payload(
    value: &serde_json::Value,
    pid: u32,
    now: DateTime<Utc>,
) -> Option<SessionSnapshot> {
    let email = value.get("email")?.as_str()?.trim();
    let masked_email = mask_email(email)?;
    let normalized_email = email.to_lowercase();
    let account_hash = Sha256::digest(normalized_email.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let plan = value
        .get("plan_tier")
        .and_then(serde_json::Value::as_str)
        .map(crate::display::sanitize_untrusted_field)
        .filter(|plan| !plan.is_empty())
        .unwrap_or_else(|| "Antigravity".to_string());
    let quota = value.get("quota")?.as_object()?;
    let mut snapshot = SessionSnapshot {
        pid,
        account_hash,
        masked_email,
        plan,
        session: None,
        weekly: None,
        third_party_session: None,
        third_party_weekly: None,
        argon: Vec::new(),
        updated_at: now,
    };
    // The canonical ids claim their slot first. `quota` iterates in key order,
    // and per-model pools sort ahead of the shared buckets
    // (`gemini-argon-wtu-weekly` < `gemini-weekly`), so taking the first match
    // would show a model's own limit as the Gemini weekly figure. Infer slots
    // only when no canonical id arrived, checking presence before parsing so
    // an invalid canonical value cannot be replaced with a model's quota.
    const CANONICAL_IDS: [&str; 4] = ["gemini-5h", "gemini-weekly", "3p-5h", "3p-weekly"];
    let renamed = !quota.keys().any(|id| CANONICAL_IDS.contains(&id.as_str()));
    let mut exact: [Vec<SnapshotWindow>; 4] = Default::default();
    let mut inferred: [Vec<SnapshotWindow>; 4] = Default::default();
    for (bucket, data) in quota {
        if !renamed && !CANONICAL_IDS.contains(&bucket.as_str()) {
            let weekly = bucket.ends_with("-weekly");
            if bucket.to_ascii_lowercase().contains("argon")
                && (weekly || bucket.ends_with("-session") || bucket.ends_with("-5h"))
                && let Some(window) = quota_window(data, weekly, now)
            {
                snapshot.argon.push(window);
            }
            continue;
        }
        let (third_party, weekly) = match bucket.as_str() {
            name if name.starts_with("gemini-") => (false, name.ends_with("-weekly")),
            name if name.starts_with("claude-")
                || name.starts_with("gpt-")
                || name.starts_with("3p-") =>
            {
                (true, name.ends_with("-weekly"))
            }
            _ => continue,
        };
        if !(bucket.ends_with("-weekly") || bucket.ends_with("-session") || bucket.ends_with("-5h"))
        {
            continue;
        }
        let Some(window) = quota_window(data, weekly, now) else {
            continue;
        };
        let slot = usize::from(third_party) * 2 + usize::from(weekly);
        if CANONICAL_IDS.contains(&bucket.as_str()) {
            exact[slot].push(window);
        } else {
            inferred[slot].push(window);
        }
    }
    let targets = [
        &mut snapshot.session,
        &mut snapshot.weekly,
        &mut snapshot.third_party_session,
        &mut snapshot.third_party_weekly,
    ];
    for ((target, exact), inferred) in targets.into_iter().zip(exact).zip(inferred) {
        let mut candidates = if renamed { inferred } else { exact };
        if candidates.len() == 1 {
            *target = candidates.pop();
        }
    }
    (snapshot.session.is_some()
        || snapshot.weekly.is_some()
        || snapshot.third_party_session.is_some()
        || snapshot.third_party_weekly.is_some()
        || !snapshot.argon.is_empty())
    .then_some(snapshot)
}

#[cfg(any(target_os = "macos", test))]
fn quota_window(
    data: &serde_json::Value,
    weekly: bool,
    now: DateTime<Utc>,
) -> Option<SnapshotWindow> {
    let remaining = data
        .get("remaining_fraction")?
        .as_f64()
        .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))?;
    let resets_at = data
        .get("reset_time")
        .and_then(serde_json::Value::as_str)
        .and_then(|reset| DateTime::parse_from_rfc3339(reset).ok())
        .map(|reset| reset.with_timezone(&Utc))
        .or_else(|| {
            data.get("reset_in_seconds")
                .and_then(serde_json::Value::as_i64)
                .filter(|seconds| *seconds >= 0)
                .and_then(|seconds| now.checked_add_signed(Duration::seconds(seconds)))
        });
    Some(SnapshotWindow {
        utilization_pct: ((1.0 - remaining) * 100.0).round() as i32,
        resets_at,
        window_duration: if weekly {
            Duration::days(7)
        } else {
            Duration::hours(5)
        },
    })
}

#[cfg(any(target_os = "macos", test))]
fn mask_email(email: &str) -> Option<String> {
    let (local, domain) = email.trim().split_once('@')?;
    if local.is_empty()
        || local.starts_with('.')
        || local.ends_with('.')
        || local.contains("..")
        || domain.is_empty()
        || domain.starts_with('.')
        || domain.ends_with('.')
        || domain.contains("..")
        || !domain.contains('.')
    {
        return None;
    }
    let first = local.chars().next()?;
    if email.trim().matches('@').count() != 1
        || email.chars().any(char::is_control)
        || local.chars().any(char::is_whitespace)
        || domain.chars().any(char::is_whitespace)
    {
        return None;
    }
    Some(format!("{first}***@{}", domain.to_ascii_lowercase()))
}

#[cfg(any(target_os = "macos", test))]
fn find_agy_ancestor(inspector: &impl ProcessInspector, current_pid: u32) -> Option<u32> {
    let mut pid = current_pid;
    let mut visited = HashSet::new();
    for _ in 0..MAX_PROCESS_ANCESTRY {
        pid = inspector.parent_pid(pid)?;
        if pid <= 1 || !visited.insert(pid) {
            return None;
        }
        if inspector
            .executable_name(pid)
            .as_deref()
            .is_some_and(is_agy_executable)
        {
            return Some(pid);
        }
    }
    None
}

#[cfg(any(target_os = "macos", test))]
fn is_agy_executable(name: &OsStr) -> bool {
    Path::new(name).file_name() == Some(OsStr::new("agy"))
}

#[cfg(any(target_os = "macos", test))]
fn snapshot_path(root: &Path, pid: u32) -> PathBuf {
    root.join(format!("{pid}.json"))
}

#[cfg(any(target_os = "macos", test))]
fn write_snapshot_atomic(root: &Path, snapshot: &SessionSnapshot) -> Result<()> {
    let path = snapshot_path(root, snapshot.pid);
    let bytes = serde_json::to_vec(snapshot)?;
    crate::cache::atomic_write(&path, &bytes)
}

#[cfg(any(target_os = "macos", test))]
fn read_active_accounts(root: &Path, inspector: &impl ProcessInspector) -> Vec<AccountSession> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut snapshots = Vec::new();
    // Snapshots whose `agy` has exited, with the file each was read from.
    let mut exited = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(pid) = path
            .file_name()
            .and_then(OsStr::to_str)
            .and_then(|name| name.strip_suffix(".json"))
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(snapshot) = serde_json::from_slice::<SessionSnapshot>(&bytes) else {
            continue;
        };
        let process_is_agy = inspector
            .executable_name(pid)
            .as_deref()
            .is_some_and(is_agy_executable);
        let snapshot_is_safe = snapshot.pid == pid
            && snapshot.account_hash.len() == 64
            && snapshot
                .account_hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            && snapshot
                .masked_email
                .split_once('@')
                .is_some_and(|(local, domain)| {
                    local.ends_with("***") && !domain.is_empty() && !domain.contains('@')
                });
        if !snapshot_is_safe {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        if process_is_agy {
            snapshots.push(snapshot);
        } else {
            exited.push((path, snapshot));
        }
    }
    if snapshots.is_empty() {
        for (path, _) in exited {
            let _ = std::fs::remove_file(path);
        }
        return Vec::new();
    }
    let Ok(identity_key) = load_or_create_identity_key(root) else {
        return Vec::new();
    };
    let mut latest = latest_by_account(snapshots, inspector);
    keep_later_exited_readings(&mut latest, exited);
    accounts_with_key(latest, &identity_key)
}

/// The session that did the work is often the first one closed, and its last
/// reading is then the latest the account has: the sessions still open keep
/// sending what they fetched before it (#447). So an exited session's snapshot
/// stands in for its account while its figures are strictly the later reading
/// — never on arrival order, since nothing will arrive from it again — and only
/// while another session of that account is still running. Every other exited
/// snapshot is deleted, as before.
#[cfg(any(target_os = "macos", test))]
fn keep_later_exited_readings(
    latest: &mut std::collections::BTreeMap<String, SessionSnapshot>,
    exited: Vec<(PathBuf, SessionSnapshot)>,
) {
    let mut standing_in = std::collections::BTreeMap::<String, PathBuf>::new();
    for (path, snapshot) in exited {
        let later = latest.get(&snapshot.account_hash).is_some_and(|current| {
            reading_order(&snapshot, current) == Some(std::cmp::Ordering::Greater)
        });
        if !later {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        if let Some(replaced) = standing_in.insert(snapshot.account_hash.clone(), path) {
            let _ = std::fs::remove_file(replaced);
        }
        latest.insert(snapshot.account_hash.clone(), snapshot);
    }
}

#[cfg(any(target_os = "macos", test))]
fn load_or_create_identity_key(root: &Path) -> Result<[u8; 32]> {
    let key_path = root.join(".account-id-key");
    let lock_path = root.join(".account-id-key.lock");
    let _lock = crate::cache::acquire_lock(&lock_path, std::time::Duration::from_secs(5))?;
    match std::fs::read(&key_path) {
        Ok(bytes) => bytes.try_into().map_err(|_| {
            AppError::Schema("Antigravity account identity key must be exactly 32 bytes".into())
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut key = [0; 32];
            // `getrandom`, not `/dev/urandom`: the device does not exist on
            // Windows, and this module's tests compile there (`cfg(test)`),
            // which is how the Windows CI leg went red during the v1.29.0
            // post-audit.
            getrandom::fill(&mut key)
                .map_err(|error| AppError::io_at(&key_path, std::io::Error::other(error)))?;
            crate::cache::atomic_write(&key_path, &key)?;
            Ok(key)
        }
        Err(error) => Err(AppError::io_at(&key_path, error)),
    }
}

#[cfg(any(target_os = "macos", test))]
fn opaque_account_id(identity_key: &[u8; 32], account_hash: &str) -> String {
    let mut mac =
        HmacSha256::new_from_slice(identity_key).expect("HMAC accepts a key of any length");
    mac.update(account_hash.as_bytes());
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// How two snapshots of one account order as readings of its quota, judged by
/// the figures rather than by when they were ingested.
///
/// `updated_at` is when a status line arrived, not when its figures were
/// fetched. Every `agy` process re-sends the quota it last fetched each time it
/// redraws, and it fetches only after a turn of its own, so an idle session
/// keeps sending the figures it read at startup — and with two sessions open,
/// the last to redraw is as likely to be the idle one (#447). The figures carry
/// their own order: a window's reset only ever moves later (an untouched
/// bucket's slides with the clock and anchors on first use, and the next
/// window starts after it), and within one window usage only grows.
///
/// Judged over the shared windows both carry with a reset. `None` when those
/// windows disagree; `Equal` when none of them tells the two apart.
#[cfg(any(target_os = "macos", test))]
fn reading_order(a: &SessionSnapshot, b: &SessionSnapshot) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering;

    fn shared(snapshot: &SessionSnapshot) -> [Option<&SnapshotWindow>; 4] {
        [
            snapshot.session.as_ref(),
            snapshot.weekly.as_ref(),
            snapshot.third_party_session.as_ref(),
            snapshot.third_party_weekly.as_ref(),
        ]
    }

    let mut order = Ordering::Equal;
    for (a, b) in shared(a).into_iter().zip(shared(b)) {
        let (Some(a), Some(b)) = (a, b) else {
            continue;
        };
        // Without a reset, usage alone cannot tell a later reading of one
        // window from an earlier reading of the window before it.
        let (Some(a_reset), Some(b_reset)) = (a.resets_at, b.resets_at) else {
            continue;
        };
        let window = a_reset
            .cmp(&b_reset)
            .then(a.utilization_pct.cmp(&b.utilization_pct));
        if window == Ordering::Equal {
            continue;
        }
        if order != Ordering::Equal && order != window {
            return None;
        }
        order = window;
    }
    Some(order)
}

/// Whether `candidate` should stand for its account in place of `current`: when
/// its figures are the later reading, or — only when the figures cannot tell
/// the two apart — when it arrived more recently.
#[cfg(any(target_os = "macos", test))]
fn supersedes(candidate: &SessionSnapshot, current: &SessionSnapshot) -> bool {
    match reading_order(candidate, current) {
        Some(std::cmp::Ordering::Greater) => true,
        Some(std::cmp::Ordering::Less) => false,
        Some(std::cmp::Ordering::Equal) | None => candidate.updated_at > current.updated_at,
    }
}

/// Per account, the live snapshot that holds the latest reading of its quota.
#[cfg(any(target_os = "macos", test))]
fn latest_by_account(
    snapshots: Vec<SessionSnapshot>,
    inspector: &impl ProcessInspector,
) -> std::collections::BTreeMap<String, SessionSnapshot> {
    let mut newest_by_account = std::collections::BTreeMap::<String, SessionSnapshot>::new();
    for snapshot in snapshots {
        if !inspector
            .executable_name(snapshot.pid)
            .as_deref()
            .is_some_and(is_agy_executable)
        {
            continue;
        }
        match newest_by_account.get(&snapshot.account_hash) {
            Some(existing) if !supersedes(&snapshot, existing) => {}
            _ => {
                newest_by_account.insert(snapshot.account_hash.clone(), snapshot);
            }
        }
    }
    newest_by_account
}

#[cfg(any(target_os = "macos", test))]
fn accounts_with_key(
    mut newest_by_account: std::collections::BTreeMap<String, SessionSnapshot>,
    identity_key: &[u8; 32],
) -> Vec<AccountSession> {
    let hashes = newest_by_account.keys().cloned().collect::<Vec<_>>();
    let identifiers = hashes
        .iter()
        .filter_map(|hash| {
            newest_by_account
                .get(hash)
                .map(|snapshot| opaque_account_id(identity_key, &snapshot.account_hash))
        })
        .collect::<Vec<_>>();
    let mut ids = Vec::with_capacity(identifiers.len());
    for identifier in &identifiers {
        let mut length = 12;
        while identifiers.iter().any(|other| {
            other != identifier && other.starts_with(&identifier[..length.min(identifier.len())])
        }) {
            length += 1;
        }
        ids.push(identifier[..length.min(identifier.len())].to_string());
    }

    let mut accounts = hashes
        .into_iter()
        .zip(ids)
        .filter_map(|(hash, id_suffix)| {
            let session = newest_by_account.remove(&hash)?;
            let snapshot = crate::usage::AntigravitySnapshot {
                plan: session.plan,
                account: hash,
                source: crate::usage::AntigravitySource::Statusline,
                session: session.session.map(Into::into),
                weekly: session.weekly.map(Into::into),
                third_party_session: session.third_party_session.map(Into::into),
                third_party_weekly: session.third_party_weekly.map(Into::into),
                // The flattened status-line payload has no group display
                // names. Argon is the one model pool whose stable bucket id
                // lets us label it without guessing.
                model_pools: session
                    .argon
                    .into_iter()
                    .map(|window| crate::usage::ModelPoolWindow {
                        name: "Gemini 4 Argon".to_string(),
                        window: window.into(),
                    })
                    .collect(),
            };
            Some(AccountSession {
                id_suffix,
                masked_email: session.masked_email,
                updated_at: session.updated_at,
                snapshot,
            })
        })
        .collect::<Vec<_>>();
    accounts.sort_by(|left, right| {
        left.masked_email
            .cmp(&right.masked_email)
            .then_with(|| left.snapshot.account.cmp(&right.snapshot.account))
    });
    accounts
}

#[cfg(test)]
fn aggregate_active(
    snapshots: Vec<SessionSnapshot>,
    inspector: &impl ProcessInspector,
) -> Vec<AccountSession> {
    accounts_with_key(latest_by_account(snapshots, inspector), &[0x42; 32])
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::ffi::{OsStr, OsString};
    use std::fs;

    use chrono::{TimeZone, Utc};
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    fn now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 28, 12, 0, 0).unwrap()
    }

    fn payload(email: &str, quota: serde_json::Value) -> serde_json::Value {
        json!({
            "cwd": "/private/project",
            "session_id": "conversation-secret",
            "transcript_path": "/private/transcript.jsonl",
            "workspace": { "current_dir": "/private/project" },
            "email": email,
            "plan_tier": "Google AI Pro",
            "quota": quota,
        })
    }

    #[test]
    fn setup_and_remove_outcomes_use_english_cli_messages() {
        assert_eq!(
            setup_message(SetupOutcome::Installed),
            "Antigravity status line installed."
        );
        assert_eq!(
            setup_message(SetupOutcome::AlreadyInstalled),
            "Antigravity status line was already installed."
        );
        assert_eq!(
            remove_message(RemoveOutcome::Removed),
            "Antigravity status line removed."
        );
        assert_eq!(
            remove_message(RemoveOutcome::NotInstalled),
            "No AI UsageBar status line to remove."
        );
    }

    #[test]
    fn masks_email_without_retaining_the_local_part() {
        assert_eq!(
            mask_email("jane.doe@gmail.com").as_deref(),
            Some("j***@gmail.com")
        );
        assert_eq!(
            mask_email("a@example.com").as_deref(),
            Some("a***@example.com")
        );
    }

    #[test]
    fn rejects_empty_or_malformed_email() {
        for value in [
            "",
            "   ",
            "missing-at",
            "@example.com",
            "name@",
            "a..b@example.com",
            "a@example..com",
            "a@.example.com",
            "a@example.com.",
        ] {
            assert_eq!(mask_email(value), None, "accepted {value:?}");
        }
        assert!(snapshot_from_payload(&json!({"email": 7, "quota": {}}), 42, now()).is_none());
    }

    #[test]
    fn snapshot_serialization_never_contains_raw_email_or_unrelated_payload_fields() {
        let raw_email = "jane.doe@gmail.com";
        let snapshot = snapshot_from_payload(
            &payload(
                raw_email,
                json!({"gemini-weekly": {
                    "remaining_fraction": 0.9378,
                    "reset_time": "2026-10-05T12:00:00Z",
                    "reset_in_seconds": 604800
                }}),
            ),
            42,
            now(),
        )
        .expect("valid snapshot");
        let serialized = serde_json::to_string(&snapshot).unwrap();
        assert!(!serialized.contains(raw_email));
        for field in ["cwd", "transcript_path", "workspace", "session_id"] {
            assert!(!serialized.contains(field), "persisted {field}");
        }
        assert!(serialized.contains("j***@gmail.com"));
    }

    #[test]
    fn converts_remaining_fraction_to_consumed_percent() {
        let snapshot = snapshot_from_payload(
            &payload(
                "test@example.com",
                json!({"gemini-weekly": {"remaining_fraction": 0.9378}}),
            ),
            42,
            now(),
        )
        .unwrap();
        assert_eq!(snapshot.weekly.unwrap().utilization_pct, 6);
    }

    #[test]
    fn rejects_only_the_out_of_range_metric() {
        let snapshot = snapshot_from_payload(
            &payload(
                "test@example.com",
                json!({
                    "gemini-5h": {"remaining_fraction": 1.1},
                    "gemini-weekly": {"remaining_fraction": 0.5}
                }),
            ),
            42,
            now(),
        )
        .unwrap();
        assert!(snapshot.session.is_none());
        assert_eq!(snapshot.weekly.unwrap().utilization_pct, 50);
    }

    #[test]
    fn invalid_reset_keeps_the_metric_without_reset() {
        let snapshot = snapshot_from_payload(
            &payload(
                "test@example.com",
                json!({"gemini-weekly": {
                    "remaining_fraction": 0.5,
                    "reset_time": "not-a-time",
                    "reset_in_seconds": "not-a-number"
                }}),
            ),
            42,
            now(),
        )
        .unwrap();
        assert_eq!(snapshot.weekly.unwrap().resets_at, None);
    }

    /// Per-model pools sort ahead of the shared buckets in key order; the
    /// canonical id must still own its slot.
    #[test]
    fn a_per_model_pool_never_stands_in_for_the_shared_window() {
        let snapshot = snapshot_from_payload(
            &payload(
                "test@example.com",
                json!({
                    "gemini-argon-wtu-weekly": {"remaining_fraction": 0.1},
                    "gemini-weekly": {"remaining_fraction": 0.8},
                    "gemini-5h": {"remaining_fraction": 0.6}
                }),
            ),
            42,
            now(),
        )
        .unwrap();
        assert_eq!(snapshot.weekly.unwrap().utilization_pct, 20);
        assert_eq!(snapshot.session.unwrap().utilization_pct, 40);
        assert_eq!(snapshot.argon.len(), 1);
        assert_eq!(snapshot.argon[0].utilization_pct, 90);

        // With no canonical id, one lookalike still fills its slot; two cannot
        // be told apart and leave it empty.
        let renamed = snapshot_from_payload(
            &payload(
                "test@example.com",
                json!({
                    "gemini-session": {"remaining_fraction": 0.5},
                    "gemini-pro-weekly": {"remaining_fraction": 0.9},
                    "gemini-argon-weekly": {"remaining_fraction": 0.1}
                }),
            ),
            42,
            now(),
        )
        .unwrap();
        assert_eq!(renamed.session.unwrap().utilization_pct, 50);
        assert!(renamed.weekly.is_none());
    }

    #[test]
    fn canonical_presence_keeps_missing_or_invalid_windows_empty() {
        for shared in [
            None,
            Some(json!(null)),
            Some(json!("unknown")),
            Some(json!(1.1)),
        ] {
            let mut quota = json!({
                "gemini-5h": {"remaining_fraction": 0.5},
                "gemini-argon-wtu-weekly": {"remaining_fraction": 0.1}
            });
            if let Some(remaining) = shared {
                quota["gemini-weekly"] = json!({"remaining_fraction": remaining});
            }
            let snapshot =
                snapshot_from_payload(&payload("test@example.com", quota), 42, now()).unwrap();
            assert_eq!(snapshot.session.unwrap().utilization_pct, 50);
            assert!(snapshot.weekly.is_none());
        }
    }

    #[test]
    fn valid_argon_pool_survives_when_the_shared_canonical_value_is_invalid() {
        let snapshot = snapshot_from_payload(
            &payload(
                "test@example.com",
                json!({
                    "gemini-weekly": {"remaining_fraction": "unknown"},
                    "gemini-argon-wtu-weekly": {"remaining_fraction": 0.1}
                }),
            ),
            42,
            now(),
        )
        .unwrap();
        assert!(snapshot.weekly.is_none());
        assert_eq!(snapshot.argon.len(), 1);
        assert_eq!(snapshot.argon[0].utilization_pct, 90);
    }

    #[test]
    fn unknown_quota_bucket_is_ignored() {
        let snapshot = snapshot_from_payload(
            &payload(
                "test@example.com",
                json!({
                    "gemini-monthly": {"remaining_fraction": 0.1},
                    "gemini-weekly": {"remaining_fraction": 0.8}
                }),
            ),
            42,
            now(),
        )
        .unwrap();
        assert!(snapshot.session.is_none());
        assert_eq!(snapshot.weekly.unwrap().utilization_pct, 20);
    }

    #[derive(Default)]
    struct FakeInspector {
        parents: HashMap<u32, u32>,
        names: HashMap<u32, OsString>,
    }

    impl FakeInspector {
        fn process(mut self, pid: u32, parent: u32, name: &str) -> Self {
            self.parents.insert(pid, parent);
            self.names.insert(pid, OsString::from(name));
            self
        }
    }

    impl ProcessInspector for FakeInspector {
        fn parent_pid(&self, pid: u32) -> Option<u32> {
            self.parents.get(&pid).copied()
        }

        fn executable_name(&self, pid: u32) -> Option<OsString> {
            self.names.get(&pid).cloned()
        }
    }

    #[test]
    fn resolves_direct_agy_parent() {
        let inspector = FakeInspector::default()
            .process(100, 50, "ai-usagebar")
            .process(50, 1, "agy");
        assert_eq!(find_agy_ancestor(&inspector, 100), Some(50));
    }

    #[test]
    fn walks_through_shell_to_agy_ancestor() {
        let inspector = FakeInspector::default()
            .process(100, 75, "ai-usagebar")
            .process(75, 50, "zsh")
            .process(50, 1, "/opt/homebrew/bin/agy");
        assert_eq!(find_agy_ancestor(&inspector, 100), Some(50));
    }

    #[test]
    fn rejects_chain_without_agy() {
        let inspector = FakeInspector::default()
            .process(100, 75, "ai-usagebar")
            .process(75, 1, "zsh");
        assert_eq!(find_agy_ancestor(&inspector, 100), None);
    }

    #[test]
    fn stops_on_parent_cycle() {
        let inspector = FakeInspector::default()
            .process(100, 75, "ai-usagebar")
            .process(75, 100, "zsh");
        assert_eq!(find_agy_ancestor(&inspector, 100), None);
    }

    #[test]
    fn ingestion_writes_under_the_agy_parent_pid() {
        let root = TempDir::new().unwrap();
        let inspector = FakeInspector::default()
            .process(100, 50, "ai-usagebar")
            .process(50, 1, "agy");
        let body = serde_json::to_vec(&payload(
            "test@example.com",
            json!({"gemini-weekly": {"remaining_fraction": 0.5}}),
        ))
        .unwrap();
        assert_eq!(
            ingest_with(body.as_slice(), root.path(), &inspector, 100, now()).unwrap(),
            IngestOutcome::Written
        );
        assert!(snapshot_path(root.path(), 50).is_file());
        assert!(!snapshot_path(root.path(), 100).exists());
    }

    #[test]
    fn invalid_statusline_json_does_not_create_a_snapshot() {
        let root = TempDir::new().unwrap();
        let inspector = FakeInspector::default().process(100, 50, "agy");
        assert_eq!(
            ingest_with(b"{".as_slice(), root.path(), &inspector, 100, now()).unwrap(),
            IngestOutcome::Ignored
        );
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }

    fn sample_snapshot(pid: u32, email: &str) -> SessionSnapshot {
        snapshot_from_payload(
            &payload(
                email,
                json!({"gemini-weekly": {"remaining_fraction": 0.75}}),
            ),
            pid,
            now(),
        )
        .unwrap()
    }

    #[test]
    fn writes_only_pid_named_snapshot() {
        let root = TempDir::new().unwrap();
        write_snapshot_atomic(root.path(), &sample_snapshot(42, "first@example.com")).unwrap();
        let files = fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(files, vec![OsString::from("42.json")]);
    }

    #[test]
    fn second_write_for_same_pid_replaces_the_account() {
        let root = TempDir::new().unwrap();
        write_snapshot_atomic(root.path(), &sample_snapshot(42, "first@example.com")).unwrap();
        write_snapshot_atomic(root.path(), &sample_snapshot(42, "second@example.com")).unwrap();
        let bytes = fs::read(snapshot_path(root.path(), 42)).unwrap();
        let stored: SessionSnapshot = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(stored.masked_email, "s***@example.com");
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn reader_never_observes_partial_json() {
        let root = TempDir::new().unwrap();
        for index in 0..25 {
            let email = format!("account{index}@example.com");
            write_snapshot_atomic(root.path(), &sample_snapshot(42, &email)).unwrap();
            let bytes = fs::read(snapshot_path(root.path(), 42)).unwrap();
            serde_json::from_slice::<SessionSnapshot>(&bytes).expect("complete JSON");
        }
    }

    #[test]
    fn failed_write_preserves_previous_snapshot() {
        let root = TempDir::new().unwrap();
        let previous = sample_snapshot(42, "first@example.com");
        write_snapshot_atomic(root.path(), &previous).unwrap();
        let before = fs::read(snapshot_path(root.path(), 42)).unwrap();
        let impossible_root = root.path().join("not-a-directory");
        fs::write(&impossible_root, b"occupied").unwrap();
        assert!(
            write_snapshot_atomic(&impossible_root, &sample_snapshot(42, "second@example.com"))
                .is_err()
        );
        assert_eq!(fs::read(snapshot_path(root.path(), 42)).unwrap(), before);
    }

    #[test]
    fn executable_basename_must_be_exact_agy() {
        assert!(is_agy_executable(OsStr::new("/usr/local/bin/agy")));
        assert!(!is_agy_executable(OsStr::new("agy-helper")));
    }

    #[test]
    fn accepts_live_agy_process() {
        let inspector = FakeInspector::default().process(42, 1, "agy");
        assert_eq!(
            aggregate_active(vec![sample_snapshot(42, "one@example.com")], &inspector).len(),
            1
        );
    }

    #[test]
    fn aggregates_argon_as_a_named_model_pool() {
        let inspector = FakeInspector::default().process(42, 1, "agy");
        let snapshot = snapshot_from_payload(
            &payload(
                "one@example.com",
                json!({
                    "gemini-weekly": {"remaining_fraction": 0.75},
                    "gemini-argon-wtu-weekly": {"remaining_fraction": 0.1}
                }),
            ),
            42,
            now(),
        )
        .unwrap();
        let accounts = aggregate_active(vec![snapshot], &inspector);
        let [pool] = accounts[0].snapshot.model_pools.as_slice() else {
            panic!("{:?}", accounts[0].snapshot.model_pools);
        };
        assert_eq!(pool.name, "Gemini 4 Argon");
        assert_eq!(pool.window.utilization_pct, 90);
        assert!(pool.is_weekly());
    }

    #[test]
    fn snapshots_written_before_argon_remain_readable() {
        let mut value = serde_json::to_value(sample_snapshot(42, "one@example.com")).unwrap();
        value.as_object_mut().unwrap().remove("argon");
        let restored: SessionSnapshot = serde_json::from_value(value).unwrap();
        assert!(restored.argon.is_empty());
    }

    #[test]
    fn rejects_dead_pid() {
        assert!(
            aggregate_active(
                vec![sample_snapshot(42, "one@example.com")],
                &FakeInspector::default()
            )
            .is_empty()
        );
    }

    #[test]
    fn rejects_pid_reused_by_another_executable() {
        let inspector = FakeInspector::default().process(42, 1, "zsh");
        assert!(
            aggregate_active(vec![sample_snapshot(42, "one@example.com")], &inspector).is_empty()
        );
    }

    #[test]
    fn accepts_agy_resolved_by_absolute_path() {
        let inspector = FakeInspector::default().process(42, 1, "/opt/homebrew/bin/agy");
        assert_eq!(
            aggregate_active(vec![sample_snapshot(42, "one@example.com")], &inspector).len(),
            1
        );
    }

    #[test]
    fn invalid_json_file_does_not_hide_valid_sessions() {
        let root = TempDir::new().unwrap();
        fs::write(root.path().join("bad.json"), b"{").unwrap();
        write_snapshot_atomic(root.path(), &sample_snapshot(42, "one@example.com")).unwrap();
        let inspector = FakeInspector::default().process(42, 1, "agy");
        assert_eq!(read_active_accounts(root.path(), &inspector).len(), 1);
    }

    #[test]
    fn deduplicates_same_account_across_processes_using_newest_snapshot() {
        let older = sample_snapshot(42, "same@example.com");
        let mut newer = sample_snapshot(43, "SAME@example.com");
        newer.updated_at += Duration::minutes(1);
        newer.weekly.as_mut().unwrap().utilization_pct = 25;
        let inspector = FakeInspector::default()
            .process(42, 1, "agy")
            .process(43, 1, "agy");
        let accounts = aggregate_active(vec![older.clone(), newer], &inspector);
        assert_eq!(accounts.len(), 1);
        assert_eq!(
            accounts[0]
                .snapshot
                .weekly
                .as_ref()
                .unwrap()
                .utilization_pct,
            25
        );
        assert_eq!(accounts[0].snapshot.account, older.account_hash);
    }

    fn reading(pid: u32, quota: serde_json::Value, arrived: DateTime<Utc>) -> SessionSnapshot {
        snapshot_from_payload(&payload("same@example.com", quota), pid, arrived).unwrap()
    }

    fn bucket(remaining: f64, reset: &str) -> serde_json::Value {
        json!({"remaining_fraction": remaining, "reset_time": reset})
    }

    /// What one `agy` session re-sends on every redraw: the quota as it last
    /// fetched it. The Claude & GPT OSS buckets are untouched, so their resets
    /// slide from the moment of that fetch; the Gemini ones are anchored.
    fn agy_quota(gemini_5h_remaining: f64, fetched: DateTime<Utc>) -> serde_json::Value {
        let slid = |window: Duration| (fetched + window).to_rfc3339();
        json!({
            "gemini-5h": bucket(gemini_5h_remaining, "2026-09-28T15:25:45Z"),
            "gemini-weekly": bucket(0.65, "2026-10-04T01:11:19Z"),
            "gemini-argon-wtu-weekly": bucket(0.0, "2026-10-04T01:10:47Z"),
            "3p-5h": bucket(1.0, &slid(Duration::hours(5))),
            "3p-weekly": bucket(1.0, &slid(Duration::days(7)))
        })
    }

    /// An idle session keeps re-sending the quota it fetched at startup, so it
    /// is as likely as the busy one to arrive last. Arriving last must not let
    /// its figures replace a later reading.
    #[test]
    fn an_idle_session_redrawing_last_cannot_replace_a_later_reading() {
        let busy_fetch = now() - Duration::minutes(2);
        let busy = reading(
            42,
            agy_quota(0.58, busy_fetch),
            now() - Duration::seconds(4),
        );
        let idle = reading(43, agy_quota(0.64, now() - Duration::minutes(12)), now());
        let inspector = FakeInspector::default()
            .process(42, 1, "agy")
            .process(43, 1, "agy");
        for snapshots in [vec![busy.clone(), idle.clone()], vec![idle, busy]] {
            let accounts = aggregate_active(snapshots, &inspector);
            let [account] = accounts.as_slice() else {
                panic!("{accounts:?}");
            };
            let snapshot = &account.snapshot;
            assert_eq!(snapshot.session.as_ref().unwrap().utilization_pct, 42);
            assert_eq!(
                snapshot.third_party_session.as_ref().unwrap().resets_at,
                Some(busy_fetch + Duration::hours(5))
            );
            // The shared weekly window, with the exhausted Argon pool beside it.
            assert_eq!(snapshot.weekly.as_ref().unwrap().utilization_pct, 35);
            assert_eq!(snapshot.model_pools[0].window.utilization_pct, 100);
        }
    }

    /// A later reset is a later window: a reading taken after the 5-hour window
    /// rolled over wins over one from before it, despite its lower usage and
    /// despite arriving first.
    #[test]
    fn a_reading_of_the_next_window_wins_despite_its_lower_usage() {
        let before = reading(
            43,
            json!({"gemini-5h": bucket(0.1, "2026-09-28T11:50:00Z")}),
            now(),
        );
        let after = reading(
            42,
            json!({"gemini-5h": bucket(0.95, "2026-09-28T16:55:00Z")}),
            now() - Duration::minutes(1),
        );
        let inspector = FakeInspector::default()
            .process(42, 1, "agy")
            .process(43, 1, "agy");
        let accounts = aggregate_active(vec![after, before], &inspector);
        assert_eq!(
            accounts[0]
                .snapshot
                .session
                .as_ref()
                .unwrap()
                .utilization_pct,
            5
        );
    }

    /// The figures overrule the arrival order only when they agree. Identical
    /// readings, readings whose windows disagree, and windows without a reset
    /// all leave the latest arrival standing for the account.
    #[test]
    fn figures_that_cannot_order_two_readings_keep_the_latest_arrival() {
        let quota = |five_hour: f64, weekly: f64| {
            json!({
                "gemini-5h": bucket(five_hour, "2026-09-28T15:00:00Z"),
                "gemini-weekly": bucket(weekly, "2026-10-04T01:00:00Z")
            })
        };
        let unreset = |five_hour: f64| json!({"gemini-5h": {"remaining_fraction": five_hour}});
        let inspector = FakeInspector::default()
            .process(42, 1, "agy")
            .process(43, 1, "agy");
        let arrived = now() + Duration::seconds(5);
        for (earlier, later) in [
            (quota(0.5, 0.8), quota(0.5, 0.8)),
            // The 5-hour window puts the earlier arrival ahead, the weekly
            // window the later one.
            (quota(0.4, 0.8), quota(0.5, 0.7)),
            (unreset(0.4), unreset(0.5)),
        ] {
            let earlier = reading(42, earlier, now());
            let later = reading(43, later, arrived);
            for snapshots in [vec![earlier.clone(), later.clone()], vec![later, earlier]] {
                assert_eq!(
                    aggregate_active(snapshots, &inspector)[0].updated_at,
                    arrived
                );
            }
        }
    }

    /// The busy session is closed and the one left open is idle: the account
    /// keeps the closed session's later reading instead of falling back to the
    /// figures the idle one fetched at startup.
    #[test]
    fn a_closed_session_keeps_its_later_reading_while_its_account_runs() {
        let root = TempDir::new().unwrap();
        let closed = reading(
            42,
            agy_quota(0.55, now() - Duration::minutes(3)),
            now() - Duration::minutes(1),
        );
        let open = reading(43, agy_quota(0.64, now() - Duration::minutes(30)), now());
        write_snapshot_atomic(root.path(), &closed).unwrap();
        write_snapshot_atomic(root.path(), &open).unwrap();
        let inspector = FakeInspector::default().process(43, 1, "agy");
        for _ in 0..2 {
            let accounts = read_active_accounts(root.path(), &inspector);
            let [account] = accounts.as_slice() else {
                panic!("{accounts:?}");
            };
            assert_eq!(
                account.snapshot.session.as_ref().unwrap().utilization_pct,
                45
            );
        }
        assert!(snapshot_path(root.path(), 42).is_file());
    }

    /// A closed session's reading goes as soon as a running session has read
    /// the quota since, and with the last running session of its account.
    #[test]
    fn a_closed_sessions_reading_goes_once_superseded_or_alone() {
        let root = TempDir::new().unwrap();
        let closed = reading(
            42,
            agy_quota(0.55, now() - Duration::minutes(3)),
            now() - Duration::minutes(1),
        );
        let fresher = reading(43, agy_quota(0.5, now() - Duration::minutes(1)), now());
        write_snapshot_atomic(root.path(), &closed).unwrap();
        write_snapshot_atomic(root.path(), &fresher).unwrap();
        let inspector = FakeInspector::default().process(43, 1, "agy");
        let accounts = read_active_accounts(root.path(), &inspector);
        assert_eq!(
            accounts[0]
                .snapshot
                .session
                .as_ref()
                .unwrap()
                .utilization_pct,
            50
        );
        assert!(!snapshot_path(root.path(), 42).exists());

        write_snapshot_atomic(root.path(), &closed).unwrap();
        assert!(read_active_accounts(root.path(), &FakeInspector::default()).is_empty());
        assert!(!snapshot_path(root.path(), 42).exists());
        assert!(!snapshot_path(root.path(), 43).exists());
    }

    #[test]
    fn returns_two_three_and_five_distinct_accounts() {
        for count in [2, 3, 5] {
            let snapshots = (0..count)
                .map(|index| sample_snapshot(100 + index, &format!("account{index}@example.com")))
                .collect::<Vec<_>>();
            let mut inspector = FakeInspector::default();
            for index in 0..count {
                inspector = inspector.process(100 + index, 1, "agy");
            }
            assert_eq!(
                aggregate_active(snapshots, &inspector).len(),
                count as usize
            );
        }
    }

    #[test]
    fn same_pid_account_change_exposes_only_the_replacement() {
        let root = TempDir::new().unwrap();
        write_snapshot_atomic(root.path(), &sample_snapshot(42, "old@example.com")).unwrap();
        write_snapshot_atomic(root.path(), &sample_snapshot(42, "new@example.com")).unwrap();
        let inspector = FakeInspector::default().process(42, 1, "agy");
        let accounts = read_active_accounts(root.path(), &inspector);
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].masked_email, "n***@example.com");
    }

    #[test]
    fn stable_id_uses_short_account_hash_not_email() {
        let inspector = FakeInspector::default().process(42, 1, "agy");
        let accounts =
            aggregate_active(vec![sample_snapshot(42, "jane.doe@gmail.com")], &inspector);
        assert_eq!(accounts[0].id_suffix.len(), 12);
        assert!(!accounts[0].id_suffix.contains('@'));
        assert!(!format!("{:?}", accounts[0]).contains("jane.doe"));
    }

    #[test]
    fn public_account_id_is_not_an_email_hash_prefix_and_stays_stable() {
        let root = TempDir::new().unwrap();
        let inspector = FakeInspector::default()
            .process(100, 50, "ai-usagebar")
            .process(50, 1, "agy");
        let body = serde_json::to_vec(&payload(
            "jane.doe@gmail.com",
            json!({"gemini-weekly": {"remaining_fraction": 0.75}}),
        ))
        .unwrap();
        ingest_with(body.as_slice(), root.path(), &inspector, 100, now()).unwrap();

        let first = read_active_accounts(root.path(), &inspector);
        let second = read_active_accounts(root.path(), &inspector);
        assert_eq!(first.len(), 1);
        assert_eq!(second[0].id_suffix, first[0].id_suffix);

        let plain_hash = Sha256::digest(b"jane.doe@gmail.com");
        let plain_prefix = plain_hash[..6]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_ne!(first[0].id_suffix, plain_prefix);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(root.path().join(".account-id-key"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn sorts_accounts_deterministically() {
        let snapshots = vec![
            sample_snapshot(42, "zulu@example.com"),
            sample_snapshot(43, "alpha@example.com"),
        ];
        let inspector = FakeInspector::default()
            .process(42, 1, "agy")
            .process(43, 1, "agy");
        let accounts = aggregate_active(snapshots, &inspector);
        assert_eq!(accounts[0].masked_email, "a***@example.com");
        assert_eq!(accounts[1].masked_email, "z***@example.com");
    }

    #[test]
    fn setup_installs_owned_statusline_and_preserves_unknown_fields() {
        let root = TempDir::new().unwrap();
        let settings = root.path().join("settings.json");
        fs::write(&settings, r#"{"other":{"kept":true}}"#).unwrap();
        setup(&settings, Path::new("/Applications/AI UsageBar")).unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(settings).unwrap()).unwrap();
        assert_eq!(value["other"]["kept"], true);
        assert_eq!(value["statusLine"]["type"], "command");
        assert_eq!(value["statusLine"]["enabled"], true);
        assert_eq!(value["statusLine"]["stack_with_default"], true);
        assert!(
            value["statusLine"]["command"]
                .as_str()
                .unwrap()
                .contains("ingest-statusline")
        );
    }

    #[test]
    fn setup_is_idempotent() {
        let root = TempDir::new().unwrap();
        let settings = root.path().join("settings.json");
        setup(&settings, Path::new("/bin/ai-usagebar")).unwrap();
        let before = fs::read(&settings).unwrap();
        setup(&settings, Path::new("/bin/ai-usagebar")).unwrap();
        assert_eq!(fs::read(&settings).unwrap(), before);
    }

    #[test]
    fn setup_refuses_foreign_command_without_writing() {
        let root = TempDir::new().unwrap();
        let settings = root.path().join("settings.json");
        fs::write(
            &settings,
            r#"{"statusLine":{"type":"command","command":"other"}}"#,
        )
        .unwrap();
        let before = fs::read(&settings).unwrap();
        assert!(setup(&settings, Path::new("/bin/ai-usagebar")).is_err());
        assert_eq!(fs::read(&settings).unwrap(), before);
    }

    #[test]
    fn setup_refuses_non_object_statusline_without_writing() {
        let root = TempDir::new().unwrap();
        let settings = root.path().join("settings.json");
        fs::write(&settings, r#"{"statusLine":"custom"}"#).unwrap();
        let before = fs::read(&settings).unwrap();
        assert!(setup(&settings, Path::new("/bin/ai-usagebar")).is_err());
        assert_eq!(fs::read(&settings).unwrap(), before);
    }

    #[test]
    fn remove_deletes_only_owned_statusline() {
        let root = TempDir::new().unwrap();
        let settings = root.path().join("settings.json");
        setup(&settings, Path::new("/bin/ai-usagebar")).unwrap();
        remove(&settings, Path::new("/bin/ai-usagebar")).unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(settings).unwrap()).unwrap();
        assert!(value.get("statusLine").is_none());
    }

    #[test]
    fn remove_preserves_foreign_statusline() {
        let root = TempDir::new().unwrap();
        let settings = root.path().join("settings.json");
        fs::write(&settings, r#"{"statusLine":{"command":"other"}}"#).unwrap();
        let before = fs::read(&settings).unwrap();
        remove(&settings, Path::new("/bin/ai-usagebar")).unwrap();
        assert_eq!(fs::read(&settings).unwrap(), before);
    }

    #[test]
    fn malformed_settings_are_not_replaced() {
        let root = TempDir::new().unwrap();
        let settings = root.path().join("settings.json");
        fs::write(&settings, b"{").unwrap();
        let before = fs::read(&settings).unwrap();
        assert!(setup(&settings, Path::new("/bin/ai-usagebar")).is_err());
        assert_eq!(fs::read(&settings).unwrap(), before);
    }
}
