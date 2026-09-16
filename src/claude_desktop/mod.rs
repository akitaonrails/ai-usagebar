//! Switching which Claude account the **Claude Desktop app** is signed in as,
//! carrying local history along so the account you land on shows the union of
//! everything rather than only its own conversations.
//!
//! The Claude Desktop internals this relies on — the data-directory layout, the
//! `oauth:tokenCache` / `oauth:tokenCacheV2` / `lastKnownAccountUuid` fields in
//! `config.json`, which cookie and LevelDB stores carry the renderer's "who am
//! I", the newest-wins rule for session indexes, and the reason
//! `bridge-state.json` must be deleted rather than restored — were
//! reverse-engineered by **claude-acc** (<https://github.com/ohmaseclaro/claude-acc>,
//! MIT). [`plan_switch`]/[`apply_switch`] are a port of its `cmd_switch`, and
//! they read and write its profile store so the two tools stay interchangeable.
//! claude-acc still owns capturing (`add`), forgetting (`remove`), and chat
//! filtering (`only`/`reset`); this module deliberately covers read + switch.
//!
//! Nothing here is compiled out on Linux. Every path is injected through
//! [`Paths`], the platform commands live behind [`app::AppControl`], and the
//! whole thing simply finds no data directory on a machine with no Claude
//! Desktop app — which keeps the logic under CI's clippy and its tests running
//! everywhere.

pub mod app;
pub mod capture;
pub mod merge;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::config::AnthropicConfig;
use crate::display::sanitize_untrusted_path;
use crate::error::{AppError, Result};

use app::AppControl;
use merge::{ScheduledMerge, SessionMerge};

/// Claude Desktop's own state file: OAuth token caches plus the account pointer.
const CONFIG_JSON: &str = "config.json";
/// Root of the per-account session-index tree, `<root>/<account>/<org>/`.
const SESSIONS_DIR: &str = "claude-code-sessions";
/// Chromium-style cookie jar — part of the renderer's identity, not just
/// `config.json`'s oauth fields.
const COOKIE_FILES: [&str; 2] = ["Cookies", "Cookies-journal"];
/// LevelDB stores that carry the rest of that identity.
const LEVELDB_DIRS: [&str; 3] = ["Local Storage", "Session Storage", "IndexedDB"];
/// Remote-control / cloud-session bridge. Holds a volatile `cse_…` session id
/// that goes stale fast, so it is deleted on every switch and *never* saved
/// into a profile: restoring a dead id makes `/remote-control` fail to
/// disconnect.
const BRIDGE_FILE: &str = "bridge-state.json";
/// Account-keyed map of browser-extension device registrations. Additive only —
/// see [`merge::merge_device_registry`].
const DEVICE_REGISTRY: &str = "ant-device-registry.json";

/// Profile-store filenames, owned by claude-acc's layout.
const TOKEN_CACHE: &str = "config-tokenCache";
const TOKEN_CACHE_V2: &str = "config-tokenCacheV2";
const DESKTOP_STATE: &str = "desktop-state";
const META_JSON: &str = "meta.json";
/// One credential mutation can include a remote OAuth refresh. Account
/// switching waits on the same lock rather than racing or failing after the
/// old two-second window.
pub const ACCOUNT_LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);

/// Where the Claude Desktop app and the saved profiles live.
///
/// Constructed with [`Paths::at`] in tests so nothing reads a real `$HOME`.
#[derive(Debug, Clone)]
pub struct Paths {
    pub data_dir: PathBuf,
    pub profiles_dir: PathBuf,
    pub backups_dir: PathBuf,
}

impl Paths {
    /// Test seam: every root explicit.
    pub fn at(data_dir: PathBuf, profiles_dir: PathBuf, backups_dir: PathBuf) -> Self {
        Self {
            data_dir,
            profiles_dir,
            backups_dir,
        }
    }

    /// Production paths. `desktop_profiles_dir` overrides the claude-acc
    /// default; rollback archives land beside the profile store.
    pub fn resolve(anthropic: &AnthropicConfig) -> Result<Self> {
        let home = crate::cache::home_dir()?;
        let profiles_dir = anthropic
            .desktop_profiles_dir
            .clone()
            .unwrap_or_else(|| home.join(".claude-acc").join("profiles"));
        let backups_dir = profiles_dir
            .parent()
            .map_or_else(|| home.join(".claude-acc"), Path::to_path_buf)
            .join("backups");
        Ok(Self {
            data_dir: home.join("Library/Application Support/Claude"),
            profiles_dir,
            backups_dir,
        })
    }

    /// Production paths against an explicit profile directory, for a relocated
    /// Claude Desktop profile (one launched with `--user-data-dir`).
    ///
    /// `profiles_dir` stays shared: it is the label-keyed store of saved
    /// credentials, and the same accounts back every profile.
    ///
    /// `backups_dir` is deliberately NOT shared, and must never be "simplified"
    /// back to one location. [`Self::synced_path`] derives from it, and that
    /// ledger records what each account held *after the last merge*, which
    /// [`merge::deletion_candidates`] diffs live state against. Two profiles
    /// sharing one ledger would each overwrite it with their own narrower view,
    /// so keys merely absent from one profile would later read as intentional
    /// deletions — phantom candidates that an interactive switch can then
    /// answer, and therefore delete for real.
    pub fn for_data_dir(data_dir: PathBuf, anthropic: &AnthropicConfig) -> Result<Self> {
        let home = crate::cache::home_dir()?;
        let profiles_dir = anthropic
            .desktop_profiles_dir
            .clone()
            .unwrap_or_else(|| home.join(".claude-acc").join("profiles"));
        // A sibling of the profile, never inside it: nothing Claude Desktop
        // reads should gain directories we own.
        let mut sibling = data_dir.clone().into_os_string();
        sibling.push("-ai-usagebar");
        Ok(Self {
            data_dir,
            profiles_dir,
            backups_dir: PathBuf::from(sibling).join("backups"),
        })
    }

    /// Whether there is a Claude Desktop app installation to act on at all.
    /// False on Linux, and on a Mac where the app has never run.
    pub fn available(&self) -> bool {
        self.data_dir.is_dir()
    }

    pub fn config_json(&self) -> PathBuf {
        self.data_dir.join(CONFIG_JSON)
    }

    pub fn sessions_root(&self) -> PathBuf {
        self.data_dir.join(SESSIONS_DIR)
    }

    pub fn profile_dir(&self, label: &str) -> PathBuf {
        self.profiles_dir.join(label)
    }

    /// Shared by Desktop account switching and inactive-profile OAuth refresh.
    /// Both operations can rotate or install the same saved credential.
    pub fn account_switch_lock(&self) -> PathBuf {
        self.backups_dir.join(".account-switch.lock")
    }

    /// Where [`capture`] parks the live login before clearing it, so a
    /// cancelled capture can put the account back. Sits beside the archives.
    pub fn prelogin_dir(&self) -> PathBuf {
        self.backups_dir.with_file_name("prelogin-backup")
    }

    /// What each account's schedule registry held after the last merge, keyed
    /// by account UUID. Kept beside the profile store rather than inside a
    /// profile so both this tool and claude-acc read and write one record, and
    /// so accounts without a captured profile are still tracked.
    pub fn synced_path(&self) -> PathBuf {
        self.backups_dir.with_file_name("synced.json")
    }
}

/// One saved account in the profile store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileMeta {
    /// The profile directory's name — authoritative, so a hand-edited
    /// `meta.json` can never make a profile answer to the wrong label.
    pub label: String,
    pub email: Option<String>,
    pub account_uuid: String,
    /// Absent until the app has created a session folder for the account;
    /// without it the history merges are skipped but the credential swap
    /// still works.
    pub org_uuid: Option<String>,
    pub has_credentials: bool,
    pub has_desktop_state: bool,
}

#[derive(Debug, Deserialize)]
struct RawMeta {
    email: Option<String>,
    #[serde(rename = "accountUuid")]
    account_uuid: Option<String>,
    #[serde(rename = "orgUuid")]
    org_uuid: Option<String>,
}

/// Every saved profile, sorted by label. Best-effort by design: one unreadable
/// or hand-mangled `meta.json` is skipped rather than failing `account status`
/// for every other account.
pub fn load_profiles(profiles_dir: &Path) -> Vec<ProfileMeta> {
    let Ok(entries) = std::fs::read_dir(profiles_dir) else {
        return Vec::new();
    };
    let mut profiles: Vec<ProfileMeta> = entries
        .flatten()
        .filter_map(|entry| {
            let dir = entry.path();
            let label = dir.file_name()?.to_str()?.to_string();
            let raw: RawMeta =
                serde_json::from_slice(&std::fs::read(dir.join(META_JSON)).ok()?).ok()?;
            let account_uuid = raw.account_uuid.filter(|uuid| !uuid.is_empty())?;
            Some(ProfileMeta {
                label,
                email: raw.email.filter(|email| !email.is_empty()),
                account_uuid,
                org_uuid: raw.org_uuid.filter(|uuid| !uuid.is_empty()),
                has_credentials: dir.join(TOKEN_CACHE).is_file()
                    && dir.join(TOKEN_CACHE_V2).is_file(),
                has_desktop_state: dir.join(DESKTOP_STATE).is_dir(),
            })
        })
        .collect();
    profiles.sort_by(|a, b| a.label.cmp(&b.label));
    profiles
}

/// Read the schedule sync record. Absent or malformed reads as empty, which
/// makes every task look new — so a first run, or a corrupted record, reports
/// no deletions rather than inventing them.
pub fn load_synced(path: &Path) -> merge::Synced {
    std::fs::read(path)
        .map(|bytes| merge::parse_synced(&bytes))
        .unwrap_or_default()
}

/// Record what every account holds now, so the next switch can tell a deletion
/// from a task that account never had.
pub fn save_synced(path: &Path, synced: &merge::Synced) -> Result<()> {
    crate::cache::atomic_write(path, &serde_json::to_vec(synced)?)
}

/// Which account the Desktop app currently believes it is. This is the app's
/// own pointer, not a guess from file timestamps.
pub fn active_account_uuid(config_json: &Path) -> Option<String> {
    let bytes = std::fs::read(config_json).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    value
        .get("lastKnownAccountUuid")?
        .as_str()
        .filter(|uuid| !uuid.is_empty())
        .map(str::to_string)
}

pub fn label_for_uuid<'a>(profiles: &'a [ProfileMeta], account_uuid: &str) -> Option<&'a str> {
    profiles
        .iter()
        .find(|profile| profile.account_uuid == account_uuid)
        .map(|profile| profile.label.as_str())
}

/// How many conversations the account's history folder holds.
pub fn session_count(sessions_root: &Path, profile: &ProfileMeta) -> usize {
    let Some(org) = &profile.org_uuid else {
        return 0;
    };
    let Ok(entries) = std::fs::read_dir(sessions_root.join(&profile.account_uuid).join(org)) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "json")
        })
        .count()
}

/// Knobs that change what a switch does rather than which account it targets.
#[derive(Debug, Clone)]
pub struct SwitchOpts {
    /// Keep `bridge-state.json` instead of deleting it. Off by default, so the
    /// shipped behaviour matches claude-acc; on, it turns "does the remote
    /// bridge cause the browser disconnect?" into a one-command experiment.
    pub keep_bridge: bool,
    /// Also archive the whole session tree. Off by default: the session merge
    /// is additive (the older copy always survives in its source folder), so a
    /// full-tree archive costs tens of megabytes per switch to protect against
    /// nothing. On, it matches claude-acc byte for byte.
    pub backup_sessions: bool,
    /// Rollback archives to retain.
    pub keep_backups: usize,
}

impl Default for SwitchOpts {
    fn default() -> Self {
        Self {
            keep_bridge: false,
            backup_sessions: false,
            keep_backups: 10,
        }
    }
}

/// The saved OAuth token caches for the account being switched to. Opaque
/// blobs; never logged or printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedTokens {
    pub token_cache: String,
    pub token_cache_v2: String,
}

/// Everything a switch would do, decided before anything is touched.
#[derive(Debug)]
pub struct SwitchPlan {
    pub target: ProfileMeta,
    /// The account being switched away from, when it is one we manage.
    pub outgoing: Option<String>,
    pub sessions: SessionMerge,
    /// Absent when the target has no known org yet, so there is no history
    /// folder to merge into.
    pub scheduled: Option<ScheduledMerge>,
    /// A switch is only valid with both saved Desktop credential blobs. Mixing
    /// a target account's browser state with the current account's credential
    /// would create an incoherent, destructive half-switch.
    pub tokens: SavedTokens,
    pub archive: PathBuf,
    pub archive_members: Vec<String>,
    pub restores_desktop_state: bool,
    pub opts: SwitchOpts,
    /// Schedules an account deleted that others still hold, so this merge would
    /// resurrect them. The caller decides — a terminal prompt or the menu bar —
    /// and records the verdict in `confirmed_deletions`.
    pub deletions: Vec<merge::DeletionCandidate>,
    /// Type-scoped items the user confirmed should go everywhere. Empty means
    /// keep them all, which is also what a non-interactive switch must do.
    pub confirmed_deletions: BTreeSet<merge::DeletionKey>,
    /// Baseline used to plan this switch. Apply combines it with the actual
    /// post-write state so unresolved definitions remain unresolved safely.
    pub prior_synced: merge::Synced,
}

/// Decide the whole switch without performing any of it.
///
/// This is what makes `--dry-run` free, and a test asserts it leaves the data
/// directory byte-identical.
pub fn plan_switch(paths: &Paths, label: &str, opts: SwitchOpts) -> Result<SwitchPlan> {
    let profiles = load_profiles(&paths.profiles_dir);
    let target = profiles
        .iter()
        .find(|profile| profile.label == label)
        .cloned()
        .ok_or_else(|| {
            let known: Vec<&str> = profiles.iter().map(|p| p.label.as_str()).collect();
            AppError::Credentials(format!(
                "no saved Claude Desktop account {label:?} in {}; known: {known:?}. \
                 Capture one with `claude-acc add {label}` \
                 (https://github.com/ohmaseclaro/claude-acc)",
                paths.profiles_dir.display()
            ))
        })?;

    let sessions_root = paths.sessions_root();
    let synced = load_synced(&paths.synced_path());
    let (sessions, scheduled) = match &target.org_uuid {
        Some(org) => (
            merge::plan_session_merge(&sessions_root, &target.account_uuid, org),
            Some(merge::plan_scheduled_merge(
                &sessions_root,
                &target.account_uuid,
                org,
                &synced,
            )?),
        ),
        None => (SessionMerge::default(), None),
    };
    let deletions = merge::deletion_candidates(&sessions_root, &synced);

    let outgoing = active_account_uuid(&paths.config_json())
        .and_then(|uuid| label_for_uuid(&profiles, &uuid).map(str::to_string));

    let profile_dir = paths.profile_dir(label);
    let token_cache = std::fs::read_to_string(profile_dir.join(TOKEN_CACHE)).map_err(|_| {
        AppError::Credentials(format!(
            "no complete saved Desktop credential for {label:?}; capture or sign into that \
             account before switching"
        ))
    })?;
    let token_cache_v2 =
        std::fs::read_to_string(profile_dir.join(TOKEN_CACHE_V2)).map_err(|_| {
            AppError::Credentials(format!(
                "no complete saved Desktop credential for {label:?}; capture or sign into that \
                 account before switching"
            ))
        })?;
    if token_cache.is_empty() || token_cache_v2.is_empty() {
        return Err(AppError::Credentials(format!(
            "the saved Desktop credential for {label:?} is empty; capture it again before switching"
        )));
    }
    let tokens = SavedTokens {
        token_cache,
        token_cache_v2,
    };
    if !profile_dir.join(DESKTOP_STATE).is_dir() {
        return Err(AppError::Credentials(format!(
            "no saved Desktop browser state for {label:?}; capture that account again before switching"
        )));
    }

    let stamp = crate::claude_desktop::timestamp();
    Ok(SwitchPlan {
        archive: paths
            .backups_dir
            .join(format!("switch-{stamp}-{label}.tar.gz")),
        archive_members: archive_members(paths, &opts),
        restores_desktop_state: true,
        target,
        outgoing,
        sessions,
        scheduled,
        tokens,
        opts,
        deletions,
        confirmed_deletions: BTreeSet::new(),
        prior_synced: synced,
    })
}

/// A history-only merge: bring every account's sessions and schedules into
/// whichever account this profile is already signed into.
///
/// Deliberately narrower than [`SwitchPlan`] — no credential swap, no saved
/// profile needed, no app control — so it is safe to run automatically before
/// a relocated profile's app starts. Quitting "the Claude app" by name is
/// ambiguous once several bundles share a `CFBundleName`, and nothing can
/// answer a prompt during a double-click.
#[derive(Debug)]
pub struct HistoryMerge {
    pub account_uuid: String,
    /// Absent when the account has no history folder yet, so there is nothing
    /// to merge into.
    pub org_uuid: Option<String>,
    pub sessions: SessionMerge,
    pub scheduled: Option<ScheduledMerge>,
    prior_synced: merge::Synced,
}

impl HistoryMerge {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        let scheduled_idle = match &self.scheduled {
            Some(scheduled) => scheduled.added == 0 && scheduled.updated == 0,
            None => true,
        };
        self.sessions.is_empty() && scheduled_idle
    }
}

/// Which organisation folder to merge into. An account normally has exactly
/// one; when several exist the live config's own orgs win, because writing
/// history into a stale org scatters it where the app will not look.
fn resolve_org(sessions_root: &Path, account_uuid: &str, config: &[u8]) -> Option<String> {
    let dir = sessions_root.join(account_uuid);
    let mut orgs: Vec<String> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .collect();
    orgs.sort();
    if orgs.len() > 1 {
        let known = merge::orgs_in_config(config);
        let mut narrowed: Vec<String> =
            orgs.iter().filter(|org| known.contains(org)).cloned().collect();
        if narrowed.len() == 1 {
            return narrowed.pop();
        }
        orgs.sort_by_key(|org| {
            std::fs::metadata(dir.join(org))
                .and_then(|meta| meta.modified())
                .ok()
        });
    }
    orgs.pop()
}

/// Plan a history-only merge for the profile at `paths`.
pub fn plan_history_merge(paths: &Paths) -> Result<HistoryMerge> {
    let config_json = paths.config_json();
    let config = std::fs::read(&config_json).map_err(|e| AppError::io_at(&config_json, e))?;
    let account_uuid = merge::logged_in_account(&config).ok_or_else(|| {
        AppError::Credentials(format!(
            "no account is signed into {}; sign in before merging history",
            paths.data_dir.display()
        ))
    })?;

    let sessions_root = paths.sessions_root();
    let synced = load_synced(&paths.synced_path());
    let org_uuid = resolve_org(&sessions_root, &account_uuid, &config);
    let (sessions, scheduled) = match &org_uuid {
        Some(org) => (
            merge::plan_session_merge(&sessions_root, &account_uuid, org),
            Some(merge::plan_scheduled_merge(
                &sessions_root,
                &account_uuid,
                org,
                &synced,
            )?),
        ),
        None => (SessionMerge::default(), None),
    };

    Ok(HistoryMerge {
        account_uuid,
        org_uuid,
        sessions,
        scheduled,
        prior_synced: synced,
    })
}

/// Apply a planned history merge.
///
/// Strictly additive: indexes are copied and registries rewritten, and no
/// deletion sweep runs. Removing a routine stays reachable only from an
/// answered prompt in `account switch`, which is what makes an automatic
/// pre-launch merge unable to lose history.
pub fn apply_history_merge(paths: &Paths, plan: &HistoryMerge) -> Result<Vec<String>> {
    let mut notes = Vec::new();
    let display_names = plan
        .scheduled
        .as_ref()
        .map(ScheduledMerge::display_names)
        .transpose()?;

    for (source, destination) in plan.sessions.copied.iter().chain(&plan.sessions.updated) {
        copy_file(source, destination)?;
    }
    if let Some(scheduled) = &plan.scheduled {
        crate::cache::atomic_write(&scheduled.target, &scheduled.bytes)?;
    }
    if let Some(display_names) = &display_names {
        match merge::plan_name_convergence(&paths.sessions_root(), display_names) {
            Ok(convergence) => {
                for (path, bytes) in convergence.rewrites {
                    if let Err(error) = crate::cache::atomic_write(&path, &bytes) {
                        notes.push(format!(
                            "could not converge routine names in {}: {error}",
                            sanitize_untrusted_path(&path)
                        ));
                    }
                }
            }
            Err(error) => notes.push(format!("name convergence skipped: {error}")),
        }
    }

    // Record what every account holds now, so a later switch can tell an
    // intentional deletion from a task this profile simply never received.
    let mut synced = merge::current_state(&paths.sessions_root());
    let mut canonical = plan
        .scheduled
        .as_ref()
        .map(|scheduled| scheduled.canonical_routines.clone())
        .unwrap_or_else(|| merge::canonical_routines(&plan.prior_synced));
    let present: BTreeSet<String> = synced
        .values()
        .flat_map(|account| account.routines.iter().cloned())
        .collect();
    canonical.retain(|id, _| present.contains(id));
    merge::set_canonical_routines(&mut synced, &canonical);
    if let Err(error) = save_synced(&paths.synced_path(), &synced) {
        notes.push(format!("could not record the schedule sync: {error}"));
    }
    Ok(notes)
}

/// Perform a planned switch.
///
/// Claude is always relaunched after it has been stopped. If an operation fails
/// after the rollback archive is written, the live Desktop identity is restored
/// from that archive before relaunching.
pub fn apply_switch(paths: &Paths, plan: &SwitchPlan, app: &dyn AppControl) -> Result<Vec<String>> {
    let mut notes = Vec::new();
    let members: Vec<&str> = plan.archive_members.iter().map(String::as_str).collect();

    // Stop before archiving so SQLite/LevelDB and config.json are quiescent.
    if let Err(error) = app.quit() {
        // A fail-closed liveness probe can report an error after the graceful
        // quit request already stopped Claude. Relaunch is harmless if it was
        // still running and preserves the invariant that an attempted switch
        // never leaves the app closed solely because verification failed.
        return match app.relaunch() {
            Ok(()) => Err(error),
            Err(relaunch) => Err(AppError::Other(format!(
                "{error}; Claude Desktop also could not be relaunched: {relaunch}"
            ))),
        };
    }
    let mut archived = false;
    let mut result = (|| {
        if !members.is_empty() {
            app.archive(&plan.archive, &paths.data_dir, &members)?;
            archived = true;
            // Never prune away the archive needed for this switch's rollback,
            // even if the caller supplied --keep-backups 0.
            prune_archives(
                &paths.backups_dir,
                plan.opts.keep_backups.max(1),
                &mut notes,
            );
        }
        apply_switch_while_stopped(paths, plan, &mut notes)
    })();

    if result.is_err()
        && archived
        && let Err(rollback) = app.restore(&plan.archive, &paths.data_dir, &identity_members())
    {
        let original = result.unwrap_err();
        result = Err(AppError::Other(format!(
            "{original}; automatic Desktop rollback was incomplete: {rollback}"
        )));
    }

    let relaunch = app.relaunch();
    match (result, relaunch) {
        (Ok(()), Ok(())) => Ok(notes),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(relaunch)) => Err(AppError::Other(format!(
            "{error}; Claude Desktop also could not be relaunched: {relaunch}"
        ))),
    }
}

fn apply_switch_while_stopped(
    paths: &Paths,
    plan: &SwitchPlan,
    notes: &mut Vec<String>,
) -> Result<()> {
    // Derive convergence input from the immutable plan before touching any
    // registries. Later writes must not be able to change the selected title.
    let display_names = plan
        .scheduled
        .as_ref()
        .map(ScheduledMerge::display_names)
        .transpose()?;

    // History merges abort too — a partial merge is recoverable, but doing it
    // after the credential swap would strand the user on an account whose
    // history never arrived.
    for (source, destination) in plan.sessions.copied.iter().chain(&plan.sessions.updated) {
        copy_file(source, destination)?;
    }
    if let Some(scheduled) = &plan.scheduled {
        crate::cache::atomic_write(&scheduled.target, &scheduled.bytes)?;
    }
    // A confirmed deletion has to leave every registry, or the copy still
    // sitting in another account hands it back on the next switch and the same
    // prompt returns forever. Runs after the merge so it also strips anything
    // the merge just re-added.
    match merge::plan_deletion_sweep(&paths.sessions_root(), &plan.confirmed_deletions) {
        Ok(sweep) => {
            for (path, bytes) in sweep.rewrites {
                if let Err(error) = crate::cache::atomic_write(&path, &bytes) {
                    notes.push(format!(
                        "could not apply deletion to {}: {error}",
                        sanitize_untrusted_path(&path)
                    ));
                }
            }
            // Only the chat's *index* goes. Its transcript lives in the
            // account-agnostic `~/.claude/projects/` and is never touched, so a
            // confirmed chat stops following you between accounts without the
            // conversation itself being destroyed.
            for path in sweep.removals {
                if let Err(error) = std::fs::remove_file(&path) {
                    notes.push(format!(
                        "could not remove {}: {error}",
                        sanitize_untrusted_path(&path)
                    ));
                }
            }
        }
        Err(error) => notes.push(format!("deletion sweep skipped: {error}")),
    }
    // Make every account agree on each routine's name. The merge selects the
    // winning definitions while the switch is planned; carry those names into
    // every registry so writes above cannot change the decision.
    // Runs after the merge and the deletion sweep so it neither renames a
    // just-deleted routine nor is undone by a re-added copy.
    if let Some(display_names) = &display_names {
        match merge::plan_name_convergence(&paths.sessions_root(), display_names) {
            Ok(convergence) => {
                let mut fully_applied = true;
                for (path, bytes) in convergence.rewrites {
                    if let Err(error) = crate::cache::atomic_write(&path, &bytes) {
                        fully_applied = false;
                        notes.push(format!(
                            "could not converge routine names in {}: {error}",
                            sanitize_untrusted_path(&path)
                        ));
                    }
                }
                if fully_applied && convergence.converged > 0 {
                    notes.push(format!(
                        "converged {} routine name(s) across accounts",
                        convergence.converged
                    ));
                }
            }
            Err(error) => notes.push(format!("name convergence skipped: {error}")),
        }
    }
    // Record what every account holds now, so the next switch can tell an
    // intentional deletion from a task that account simply never received.
    let mut synced = merge::current_state(&paths.sessions_root());
    let mut canonical = plan
        .scheduled
        .as_ref()
        .map(|scheduled| scheduled.canonical_routines.clone())
        .unwrap_or_else(|| merge::canonical_routines(&plan.prior_synced));
    for key in &plan.confirmed_deletions {
        if key.kind == merge::ConflictKind::Routine {
            canonical.remove(&key.id);
        }
    }
    let present: BTreeSet<String> = synced
        .values()
        .flat_map(|account| account.routines.iter().cloned())
        .collect();
    canonical.retain(|id, _| present.contains(id));
    merge::set_canonical_routines(&mut synced, &canonical);
    if let Err(error) = save_synced(&paths.synced_path(), &synced) {
        notes.push(format!("could not record the schedule sync: {error}"));
    }

    // Identify the outgoing account from the *live* config, after the quit:
    // the app rewrites this file on shutdown. Snapshotting it must also happen
    // strictly before the swap below, or the outgoing tokens get filed under
    // the incoming label and an account is destroyed.
    let live_config = paths.config_json();
    let outgoing = active_account_uuid(&live_config).and_then(|uuid| {
        label_for_uuid(&load_profiles(&paths.profiles_dir), &uuid).map(str::to_string)
    });
    if let Some(label) = &outgoing {
        snapshot_profile(paths, label, notes)?;
    }

    swap_credentials(&live_config, &plan.tokens, &plan.target.account_uuid)?;

    restore_desktop_state(paths, &plan.target.label)?;

    if !plan.opts.keep_bridge {
        let bridge = paths.data_dir.join(BRIDGE_FILE);
        if bridge.is_file()
            && let Err(error) = std::fs::remove_file(&bridge)
        {
            notes.push(format!("could not clear {BRIDGE_FILE}: {error}"));
        }
    }
    restore_device_registry(paths, &plan.target.label, notes);

    Ok(())
}

/// Copy every other account's history into this one's folder, so it shows the
/// union of everything. Used when capturing a brand-new account, whose first
/// login would otherwise open on an empty sidebar; a switch uses the
/// pre-computed plan instead so `--dry-run` can report it.
///
/// Returns `(session indexes, routines)` brought in.
fn merge_history_into(
    paths: &Paths,
    account_uuid: &str,
    org_uuid: &str,
    notes: &mut Vec<String>,
) -> (usize, usize) {
    let sessions_root = paths.sessions_root();
    let sessions = merge::plan_session_merge(&sessions_root, account_uuid, org_uuid);
    let mut copied = 0;
    for (source, destination) in sessions.copied.iter().chain(&sessions.updated) {
        match copy_file(source, destination) {
            Ok(()) => copied += 1,
            Err(error) => notes.push(format!(
                "could not seed {}: {error}",
                sanitize_untrusted_path(destination)
            )),
        }
    }
    let routines = match merge::plan_scheduled_merge(
        &sessions_root,
        account_uuid,
        org_uuid,
        &load_synced(&paths.synced_path()),
    ) {
        Ok(scheduled) => match crate::cache::atomic_write(&scheduled.target, &scheduled.bytes) {
            Ok(()) => scheduled.added + scheduled.updated,
            Err(error) => {
                notes.push(format!("schedule seed skipped: {error}"));
                0
            }
        },
        Err(error) => {
            notes.push(format!("schedule seed skipped: {error}"));
            0
        }
    };
    (copied, routines)
}

/// Save the outgoing account's live credential and browser state back into its
/// profile, so switching to it later restores what it looked like just now.
/// Only safe with the app fully quit — copying a live SQLite/LevelDB store
/// risks grabbing it mid-write.
///
/// Deliberately never writes `meta.json`: that file belongs to `claude-acc add`,
/// and two tools writing it is how the stores drift apart.
fn snapshot_profile(paths: &Paths, label: &str, notes: &mut Vec<String>) -> Result<()> {
    let profile_dir = paths.profile_dir(label);
    std::fs::create_dir_all(&profile_dir).map_err(|e| AppError::io_at(&profile_dir, e))?;
    restrict(&profile_dir, 0o700, notes);

    let bytes =
        std::fs::read(paths.config_json()).map_err(|e| AppError::io_at(paths.config_json(), e))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    let live_tokens = SavedTokens {
        token_cache: required_token(&value, "oauth:tokenCache")?.to_string(),
        token_cache_v2: required_token(&value, "oauth:tokenCacheV2")?.to_string(),
    };
    write_saved_tokens(&profile_dir, &live_tokens, notes)?;

    // The registry is account-keyed and shared, so it is snapshotted (never
    // moved) purely so a lost key can be folded back in later.
    let registry = paths.data_dir.join(DEVICE_REGISTRY);
    if registry.is_file() {
        let bytes = std::fs::read(&registry).map_err(|e| AppError::io_at(&registry, e))?;
        crate::cache::atomic_write(&profile_dir.join(DEVICE_REGISTRY), &bytes)?;
    }

    snapshot_desktop_state(paths, &profile_dir, notes)?;
    // bridge-state.json is deliberately not snapshotted — see BRIDGE_FILE.
    Ok(())
}

fn required_token<'a>(value: &'a serde_json::Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .filter(|blob| !blob.is_empty())
        .ok_or_else(|| {
            AppError::Credentials(format!(
                "the live Desktop login is missing {key}; refusing to overwrite its saved profile"
            ))
        })
}

fn write_saved_tokens(
    profile_dir: &Path,
    tokens: &SavedTokens,
    notes: &mut Vec<String>,
) -> Result<()> {
    let token_path = profile_dir.join(TOKEN_CACHE);
    let token_v2_path = profile_dir.join(TOKEN_CACHE_V2);
    let original = read_optional_file(&token_path)?;
    let original_v2 = read_optional_file(&token_v2_path)?;
    let write_result = (|| {
        crate::cache::atomic_write(&token_path, tokens.token_cache.as_bytes())?;
        crate::cache::atomic_write(&token_v2_path, tokens.token_cache_v2.as_bytes())?;
        Ok(())
    })();
    if let Err(error) = write_result {
        let mut rollback = Vec::new();
        if let Err(failure) = restore_optional_file(&token_path, original.as_deref()) {
            rollback.push(failure.to_string());
        }
        if let Err(failure) = restore_optional_file(&token_v2_path, original_v2.as_deref()) {
            rollback.push(failure.to_string());
        }
        return if rollback.is_empty() {
            Err(error)
        } else {
            Err(AppError::Other(format!(
                "{error}; saved-token rollback was incomplete: {}",
                rollback.join("; ")
            )))
        };
    }
    restrict(&token_path, 0o600, notes);
    restrict(&token_v2_path, 0o600, notes);
    Ok(())
}

fn snapshot_desktop_state(
    paths: &Paths,
    profile_dir: &Path,
    notes: &mut Vec<String>,
) -> Result<()> {
    let state_dir = profile_dir.join(DESKTOP_STATE);
    let previous = profile_dir.join(".desktop-state.previous");
    if previous.exists() && !state_dir.exists() {
        std::fs::rename(&previous, &state_dir).map_err(|e| AppError::io_at(&previous, e))?;
    } else {
        remove_if_present(&previous)?;
    }

    let staged = tempfile::Builder::new()
        .prefix(".desktop-state.pending-")
        .tempdir_in(profile_dir)
        .map_err(|e| AppError::io_at(profile_dir, e))?;
    restrict(staged.path(), 0o700, notes);
    for name in COOKIE_FILES {
        let source = paths.data_dir.join(name);
        let destination = staged.path().join(name);
        if source.is_file() {
            copy_file(&source, &destination)?;
            restrict(&destination, 0o600, notes);
        }
    }
    for name in LEVELDB_DIRS {
        let source = paths.data_dir.join(name);
        let destination = staged.path().join(name);
        if source.is_dir() {
            copy_dir(&source, &destination)?;
        }
    }

    let staged = staged.keep();
    if state_dir.exists() {
        std::fs::rename(&state_dir, &previous).map_err(|e| AppError::io_at(&state_dir, e))?;
    }
    if let Err(error) = std::fs::rename(&staged, &state_dir) {
        let restore = if previous.exists() {
            std::fs::rename(&previous, &state_dir)
        } else {
            Ok(())
        };
        let _ = remove_if_present(&staged);
        return match restore {
            Ok(()) => Err(AppError::io_at(&staged, error)),
            Err(rollback) => Err(AppError::Other(format!(
                "could not install the Desktop-state snapshot: {error}; could not restore the previous snapshot: {rollback}"
            ))),
        };
    }
    if let Err(error) = remove_if_present(&previous) {
        notes.push(format!(
            "could not remove the previous Desktop-state snapshot: {error}"
        ));
    }
    Ok(())
}

fn swap_credentials(config_json: &Path, tokens: &SavedTokens, account_uuid: &str) -> Result<()> {
    let existing = std::fs::read(config_json).map_err(|e| AppError::io_at(config_json, e))?;
    let bytes = merge::swap_config_tokens(
        &existing,
        &tokens.token_cache,
        &tokens.token_cache_v2,
        account_uuid,
    )?;
    // Atomic, unlike claude-acc's truncating write: a crash mid-write here
    // would take every account's tokens with it.
    crate::cache::atomic_write(config_json, &bytes)
}

fn restore_desktop_state(paths: &Paths, label: &str) -> Result<()> {
    let state_dir = paths.profile_dir(label).join(DESKTOP_STATE);
    for name in COOKIE_FILES {
        let source = state_dir.join(name);
        let destination = paths.data_dir.join(name);
        if source.is_file() {
            copy_file(&source, &destination)?;
        } else {
            remove_if_present(&destination)?;
        }
    }
    for name in LEVELDB_DIRS {
        let source = state_dir.join(name);
        let destination = paths.data_dir.join(name);
        if source.is_dir() {
            replace_dir(&source, &destination)?;
        } else {
            remove_if_present(&destination)?;
        }
    }
    Ok(())
}

/// Fold a snapshotted device registry back into the live one. Purely additive
/// (live wins every conflict), so this can only ever restore a lost entry.
fn restore_device_registry(paths: &Paths, label: &str, notes: &mut Vec<String>) {
    let snapshot = paths.profile_dir(label).join(DEVICE_REGISTRY);
    let live = paths.data_dir.join(DEVICE_REGISTRY);
    let (Ok(saved), Ok(current)) = (std::fs::read(&snapshot), std::fs::read(&live)) else {
        return;
    };
    match merge::merge_device_registry(&current, &saved) {
        Ok(bytes) if bytes != current => {
            if let Err(error) = crate::cache::atomic_write(&live, &bytes) {
                notes.push(format!("could not merge {DEVICE_REGISTRY}: {error}"));
            }
        }
        Ok(_) => {}
        Err(error) => notes.push(format!("could not merge {DEVICE_REGISTRY}: {error}")),
    }
}

/// What goes into the rollback archive, relative to the data directory.
///
/// Only what a switch can actually destroy: the credential pointer, browser
/// identity, bridge, schedule registries, and device registry. Missing entries
/// are dropped, because `tar` fails the whole archive on one of them.
fn archive_members(paths: &Paths, opts: &SwitchOpts) -> Vec<String> {
    let mut members = Vec::new();
    for name in [CONFIG_JSON, DEVICE_REGISTRY, BRIDGE_FILE] {
        if paths.data_dir.join(name).exists() {
            members.push(name.to_string());
        }
    }
    for name in COOKIE_FILES.into_iter().chain(LEVELDB_DIRS) {
        if paths.data_dir.join(name).exists() {
            members.push(name.to_string());
        }
    }
    if opts.backup_sessions {
        if paths.sessions_root().is_dir() {
            members.push(SESSIONS_DIR.to_string());
        }
        return members;
    }
    let sessions_root = paths.sessions_root();
    let Ok(accounts) = std::fs::read_dir(&sessions_root) else {
        return members;
    };
    let mut registries = Vec::new();
    for account in accounts.flatten() {
        let Ok(orgs) = std::fs::read_dir(account.path()) else {
            continue;
        };
        for org in orgs.flatten() {
            let path = org.path().join("scheduled-tasks.json");
            if path.is_file()
                && let Ok(relative) = path.strip_prefix(&paths.data_dir)
            {
                registries.push(relative.display().to_string());
            }
        }
    }
    registries.sort();
    members.extend(registries);
    members
}

fn identity_members() -> Vec<&'static str> {
    [CONFIG_JSON, DEVICE_REGISTRY, BRIDGE_FILE]
        .into_iter()
        .chain(COOKIE_FILES)
        .chain(LEVELDB_DIRS)
        .collect()
}

fn prune_archives(backups_dir: &Path, keep: usize, notes: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(backups_dir) else {
        return;
    };
    // The name embeds a sortable timestamp right after the constant prefix, so
    // lexicographic order is chronological order.
    let mut archives: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("switch-") && name.ends_with(".tar.gz"))
        })
        .collect();
    if archives.len() <= keep {
        return;
    }
    archives.sort();
    let doomed = archives.len() - keep;
    for path in archives.into_iter().take(doomed) {
        if let Err(error) = std::fs::remove_file(&path) {
            notes.push(format!(
                "could not prune {}: {error}",
                sanitize_untrusted_path(&path)
            ));
        }
    }
}

fn copy_file(source: &Path, destination: &Path) -> Result<()> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|e| AppError::io_at(parent, e))?;
    }
    std::fs::copy(source, destination).map_err(|e| AppError::io_at(source, e))?;
    Ok(())
}

fn remove_if_present(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => {
            std::fs::remove_dir_all(path).map_err(|e| AppError::io_at(path, e))
        }
        Ok(_) => std::fs::remove_file(path).map_err(|e| AppError::io_at(path, e)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(AppError::io_at(path, error)),
    }
}

fn read_optional_file(path: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(AppError::io_at(path, error)),
    }
}

fn restore_optional_file(path: &Path, original: Option<&[u8]>) -> Result<()> {
    match original {
        Some(bytes) => crate::cache::atomic_write(path, bytes),
        None => remove_if_present(path),
    }
}

/// Replace `destination` with a fresh copy of `source`. LevelDB stores must not
/// be merged file-by-file — a stale manifest beside fresh log files is a
/// corrupt store — so the old directory goes first.
fn replace_dir(source: &Path, destination: &Path) -> Result<()> {
    if destination.exists() {
        std::fs::remove_dir_all(destination).map_err(|e| AppError::io_at(destination, e))?;
    }
    copy_dir(source, destination)
}

fn copy_dir(source: &Path, destination: &Path) -> Result<()> {
    std::fs::create_dir_all(destination).map_err(|e| AppError::io_at(destination, e))?;
    let entries = std::fs::read_dir(source).map_err(|e| AppError::io_at(source, e))?;
    for entry in entries.flatten() {
        let child = entry.path();
        let target = destination.join(entry.file_name());
        if child.is_dir() {
            copy_dir(&child, &target)?;
        } else {
            std::fs::copy(&child, &target).map_err(|e| AppError::io_at(&child, e))?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn restrict(path: &Path, mode: u32, notes: &mut Vec<String>) {
    use std::os::unix::fs::PermissionsExt;

    if let Err(error) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)) {
        notes.push(format!(
            "could not restrict {}: {error}",
            sanitize_untrusted_path(path)
        ));
    }
}

#[cfg(not(unix))]
fn restrict(_path: &Path, _mode: u32, _notes: &mut Vec<String>) {}

fn timestamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S%.3f").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use app::Recorder;
    use std::cell::RefCell;

    struct Fixture {
        _root: tempfile::TempDir,
        paths: Paths,
    }

    #[derive(Default)]
    struct QuitFailure {
        steps: RefCell<Vec<&'static str>>,
    }

    impl AppControl for QuitFailure {
        fn quit(&self) -> Result<()> {
            self.steps.borrow_mut().push("quit");
            Err(AppError::Other("liveness probe failed".into()))
        }

        fn relaunch(&self) -> Result<()> {
            self.steps.borrow_mut().push("relaunch");
            Ok(())
        }

        fn archive(&self, _archive: &Path, _root: &Path, _members: &[&str]) -> Result<()> {
            panic!("archive must not run after an unconfirmed quit")
        }

        fn restore(&self, _archive: &Path, _root: &Path, _members: &[&str]) -> Result<()> {
            panic!("restore must not run before a switch starts")
        }
    }

    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    /// Two accounts: `here` (active) and `there` (fully captured).
    fn fixture() -> Fixture {
        let root = tempfile::TempDir::new().unwrap();
        let data = root.path().join("data");
        let profiles = root.path().join("profiles");
        let backups = root.path().join("backups");

        write(
            &data.join(CONFIG_JSON),
            r#"{"lastKnownAccountUuid":"uuid-here","oauth:tokenCache":"live-a",
                "oauth:tokenCacheV2":"live-b","dxt:allowlistEnabled:org-1":true}"#,
        );
        write(
            &data.join(DEVICE_REGISTRY),
            r#"{"uuid-here":{"deviceId":"d1"}}"#,
        );
        write(
            &data.join(BRIDGE_FILE),
            r#"{"remoteSessionId":"cse_stale"}"#,
        );
        write(&data.join("Cookies"), "live-cookies");
        write(&data.join("Local Storage/leveldb/CURRENT"), "live-ldb");
        write(
            &data.join(SESSIONS_DIR).join("uuid-here/org-1/local_x.json"),
            r#"{"lastActivityAt":500}"#,
        );
        write(
            &data
                .join(SESSIONS_DIR)
                .join("uuid-here/org-1/scheduled-tasks.json"),
            r#"{"scheduledTasks":[{"id":"t1","createdAt":1}]}"#,
        );

        write(
            &profiles.join("here/meta.json"),
            r#"{"label":"here","email":"here@example.com","accountUuid":"uuid-here","orgUuid":"org-1"}"#,
        );
        write(
            &profiles.join("there/meta.json"),
            r#"{"label":"there","email":"there@example.com","accountUuid":"uuid-there","orgUuid":"org-2"}"#,
        );
        write(&profiles.join("there").join(TOKEN_CACHE), "saved-a");
        write(&profiles.join("there").join(TOKEN_CACHE_V2), "saved-b");
        write(
            &profiles.join("there").join(DESKTOP_STATE).join("Cookies"),
            "there-cookies",
        );
        write(
            &profiles
                .join("there")
                .join(DESKTOP_STATE)
                .join("Local Storage/leveldb/CURRENT"),
            "there-ldb",
        );

        Fixture {
            paths: Paths::at(data, profiles, backups),
            _root: root,
        }
    }

    /// `(relative path, length)` for every file under a root, so a test can
    /// assert nothing changed.
    fn manifest(root: &Path) -> Vec<(String, u64)> {
        fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, u64)>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, root, out);
                } else if let Ok(meta) = entry.metadata() {
                    let relative = path.strip_prefix(root).unwrap().display().to_string();
                    out.push((relative, meta.len()));
                }
            }
        }
        let mut out = Vec::new();
        walk(root, root, &mut out);
        out.sort();
        out
    }

    #[test]
    fn profiles_load_sorted_with_their_capture_state() {
        let fixture = fixture();
        let profiles = load_profiles(&fixture.paths.profiles_dir);

        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].label, "here");
        assert_eq!(profiles[0].email.as_deref(), Some("here@example.com"));
        assert!(!profiles[0].has_credentials);
        assert_eq!(profiles[1].label, "there");
        assert!(profiles[1].has_credentials);
        assert!(profiles[1].has_desktop_state);
    }

    #[test]
    fn a_malformed_profile_is_skipped_not_fatal() {
        let fixture = fixture();
        write(
            &fixture.paths.profiles_dir.join("broken/meta.json"),
            "{ not json",
        );
        write(
            &fixture.paths.profiles_dir.join("no-uuid/meta.json"),
            r#"{"label":"x"}"#,
        );

        let profiles = load_profiles(&fixture.paths.profiles_dir);
        let labels: Vec<&str> = profiles.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(labels, ["here", "there"]);
    }

    #[test]
    fn the_active_account_resolves_to_its_label() {
        let fixture = fixture();
        let profiles = load_profiles(&fixture.paths.profiles_dir);
        let uuid = active_account_uuid(&fixture.paths.config_json()).unwrap();

        assert_eq!(label_for_uuid(&profiles, &uuid), Some("here"));
        assert_eq!(label_for_uuid(&profiles, "uuid-nobody"), None);
    }

    #[test]
    fn session_counts_come_from_the_accounts_own_folder() {
        let fixture = fixture();
        let profiles = load_profiles(&fixture.paths.profiles_dir);
        let sessions_root = fixture.paths.sessions_root();

        assert_eq!(session_count(&sessions_root, &profiles[0]), 2);
        assert_eq!(session_count(&sessions_root, &profiles[1]), 0);
    }

    #[test]
    fn planning_a_switch_changes_nothing_on_disk() {
        let fixture = fixture();
        let before = manifest(&fixture.paths.data_dir);

        let plan = plan_switch(&fixture.paths, "there", SwitchOpts::default()).unwrap();

        assert_eq!(plan.outgoing.as_deref(), Some("here"));
        assert_eq!(plan.tokens.token_cache, "saved-a");
        assert!(plan.restores_desktop_state);
        assert_eq!(plan.sessions.copied.len(), 1, "{:?}", plan.sessions);
        assert_eq!(manifest(&fixture.paths.data_dir), before);
        assert!(!fixture.paths.backups_dir.exists());
    }

    #[test]
    fn planning_an_unknown_label_lists_the_known_ones() {
        let fixture = fixture();
        let error = plan_switch(&fixture.paths, "nope", SwitchOpts::default()).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("here"), "{message}");
        assert!(message.contains("claude-acc"), "{message}");
    }

    #[test]
    fn the_archive_skips_the_session_tree_by_default() {
        let fixture = fixture();

        let plan = plan_switch(&fixture.paths, "there", SwitchOpts::default()).unwrap();
        assert!(plan.archive_members.contains(&CONFIG_JSON.to_string()));
        assert!(plan.archive_members.contains(&DEVICE_REGISTRY.to_string()));
        assert!(plan.archive_members.contains(&BRIDGE_FILE.to_string()));
        assert!(plan.archive_members.contains(&"Cookies".to_string()));
        assert!(plan.archive_members.contains(&"Local Storage".to_string()));
        assert!(!plan.archive_members.contains(&SESSIONS_DIR.to_string()));
        assert!(
            plan.archive_members
                .iter()
                .any(|member| member.ends_with("scheduled-tasks.json"))
        );

        let full = plan_switch(
            &fixture.paths,
            "there",
            SwitchOpts {
                backup_sessions: true,
                ..SwitchOpts::default()
            },
        )
        .unwrap();
        assert!(full.archive_members.contains(&SESSIONS_DIR.to_string()));
    }

    #[test]
    fn applying_a_switch_quits_then_archives_then_relaunches() {
        let fixture = fixture();
        let plan = plan_switch(&fixture.paths, "there", SwitchOpts::default()).unwrap();
        let recorder = Recorder::default();

        apply_switch(&fixture.paths, &plan, &recorder).unwrap();

        let steps = recorder.steps();
        assert_eq!(steps[0], "quit");
        assert!(steps[1].starts_with("archive "), "{steps:?}");
        assert_eq!(steps[2], "relaunch");
    }

    #[test]
    fn a_failed_liveness_probe_relaunches_without_touching_account_data() {
        let fixture = fixture();
        let plan = plan_switch(&fixture.paths, "there", SwitchOpts::default()).unwrap();
        let before = std::fs::read(fixture.paths.config_json()).unwrap();
        let app = QuitFailure::default();

        let error = apply_switch(&fixture.paths, &plan, &app).unwrap_err();

        assert!(error.to_string().contains("liveness probe failed"));
        assert_eq!(*app.steps.borrow(), ["quit", "relaunch"]);
        assert_eq!(std::fs::read(fixture.paths.config_json()).unwrap(), before);
    }

    /// The wiring, not the pieces: a confirmed deletion must actually reach
    /// every registry through `apply_switch`, and the sync record must be
    /// written so the next switch can tell deletions from new tasks. Testing
    /// `plan_deletion_sweep` alone would pass even if nothing called it.
    #[test]
    fn a_confirmed_deletion_reaches_every_account_and_updates_the_record() {
        let fixture = fixture();
        let sessions = fixture.paths.sessions_root();
        // Both accounts hold t1; only `there` holds t2.
        write(
            &sessions.join("uuid-here/org-1/scheduled-tasks.json"),
            r#"{"scheduledTasks":[{"id":"t1","createdAt":1}]}"#,
        );
        write(
            &sessions.join("uuid-there/org-2/scheduled-tasks.json"),
            r#"{"scheduledTasks":[{"id":"t1","createdAt":1},{"id":"t2","createdAt":2}]}"#,
        );

        let mut plan = plan_switch(&fixture.paths, "there", SwitchOpts::default()).unwrap();
        plan.confirmed_deletions = [merge::DeletionKey {
            kind: merge::ConflictKind::Routine,
            id: "t1".to_string(),
            deleted_by: "uuid-here".to_string(),
            still_in: vec!["uuid-there".to_string()],
        }]
        .into_iter()
        .collect();
        apply_switch(&fixture.paths, &plan, &Recorder::default()).unwrap();

        for registry in [
            sessions.join("uuid-here/org-1/scheduled-tasks.json"),
            sessions.join("uuid-there/org-2/scheduled-tasks.json"),
        ] {
            let value: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&registry).unwrap()).unwrap();
            let ids: Vec<&str> = value["scheduledTasks"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|task| task["id"].as_str())
                .collect();
            assert!(
                !ids.contains(&"t1"),
                "{registry:?} still holds the deleted task"
            );
        }
        // The unrelated task survives, so the sweep is targeted rather than a
        // blanket wipe of the registry.
        let there: serde_json::Value = serde_json::from_slice(
            &std::fs::read(sessions.join("uuid-there/org-2/scheduled-tasks.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(there["scheduledTasks"][0]["id"], "t2");

        // And the record now reflects reality, so t1 is not reported as a
        // deletion for ever after.
        let recorded = load_synced(&fixture.paths.synced_path());
        assert!(!recorded.is_empty(), "the sync record was never written");
        assert!(
            recorded
                .values()
                .all(|state| !state.routines.contains("t1")),
            "{recorded:?} still lists the deleted task"
        );
    }

    /// Keeping everything must leave *other* accounts' registries untouched —
    /// the safe answer has to be genuinely inert, not "delete nothing but
    /// rewrite everything anyway". The switch target is excluded because the
    /// ordinary merge rewrites it either way.
    #[test]
    fn keeping_every_conflict_leaves_other_accounts_untouched() {
        let fixture = fixture();
        let sessions = fixture.paths.sessions_root();
        let bystander = sessions.join("uuid-here/org-1/scheduled-tasks.json");
        write(
            &bystander,
            r#"{"scheduledTasks":[{"id":"t1","createdAt":1}]}"#,
        );
        let before = std::fs::read(&bystander).unwrap();

        let plan = plan_switch(&fixture.paths, "there", SwitchOpts::default()).unwrap();
        assert!(plan.confirmed_deletions.is_empty());
        apply_switch(&fixture.paths, &plan, &Recorder::default()).unwrap();

        assert_eq!(std::fs::read(&bystander).unwrap(), before);
    }

    /// The wiring: a switch must actually converge routine names across every
    /// account, not just build the plan. Testing `plan_name_convergence` alone
    /// would pass even if `apply_switch` never called it.
    #[test]
    fn a_switch_converges_routine_names_across_accounts() {
        let fixture = fixture();
        let sessions = fixture.paths.sessions_root();
        let here = sessions.join("uuid-here/org-1/scheduled-tasks.json");
        let there = sessions.join("uuid-there/org-2/scheduled-tasks.json");
        // Same routine id, two different titles — the non-converging case.
        write(
            &here,
            r#"{"scheduledTasks":[{"id":"t1","createdAt":1,"displayName":"Home name"}]}"#,
        );
        write(
            &there,
            r#"{"scheduledTasks":[{"id":"t1","createdAt":1,"displayName":"There name"}]}"#,
        );

        let plan = plan_switch(&fixture.paths, "there", SwitchOpts::default()).unwrap();
        apply_switch(&fixture.paths, &plan, &Recorder::default()).unwrap();

        let name = |path: &std::path::Path| -> String {
            let v: serde_json::Value =
                serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            v["scheduledTasks"][0]["displayName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        };
        // The point is that both accounts agree afterwards, not which name won.
        assert_eq!(name(&here), name(&there), "names did not converge");
        assert!(!name(&here).is_empty());
    }

    #[test]
    fn applying_a_switch_swaps_the_credential_and_carries_history() {
        let fixture = fixture();
        let plan = plan_switch(&fixture.paths, "there", SwitchOpts::default()).unwrap();

        apply_switch(&fixture.paths, &plan, &Recorder::default()).unwrap();

        let config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(fixture.paths.config_json()).unwrap()).unwrap();
        assert_eq!(config["oauth:tokenCache"], "saved-a");
        assert_eq!(config["oauth:tokenCacheV2"], "saved-b");
        assert_eq!(config["lastKnownAccountUuid"], "uuid-there");
        assert_eq!(config["dxt:allowlistEnabled:org-1"], true);

        // History followed the switch, and the browser state came back.
        assert!(
            fixture
                .paths
                .sessions_root()
                .join("uuid-there/org-2/local_x.json")
                .is_file()
        );
        assert_eq!(
            std::fs::read_to_string(fixture.paths.data_dir.join("Cookies")).unwrap(),
            "there-cookies"
        );
        assert_eq!(
            std::fs::read_to_string(fixture.paths.data_dir.join("Local Storage/leveldb/CURRENT"))
                .unwrap(),
            "there-ldb"
        );
        // The volatile remote-control bridge is cleared, never restored.
        assert!(!fixture.paths.data_dir.join(BRIDGE_FILE).exists());
    }

    #[test]
    fn the_outgoing_account_is_snapshotted_before_the_swap() {
        let fixture = fixture();
        let plan = plan_switch(&fixture.paths, "there", SwitchOpts::default()).unwrap();

        apply_switch(&fixture.paths, &plan, &Recorder::default()).unwrap();

        // `here` must have captured the credential that was live *before* the
        // swap. Reading it back as "saved-a" would mean we filed the incoming
        // account's tokens under the outgoing label.
        let here = fixture.paths.profile_dir("here");
        assert_eq!(
            std::fs::read_to_string(here.join(TOKEN_CACHE)).unwrap(),
            "live-a"
        );
        assert_eq!(
            std::fs::read_to_string(here.join(TOKEN_CACHE_V2)).unwrap(),
            "live-b"
        );
        assert_eq!(
            std::fs::read_to_string(here.join(DESKTOP_STATE).join("Cookies")).unwrap(),
            "live-cookies"
        );
        // meta.json belongs to claude-acc; we must never rewrite it.
        let meta = std::fs::read_to_string(here.join(META_JSON)).unwrap();
        assert!(meta.contains("here@example.com"), "{meta}");
    }

    #[test]
    fn planning_refuses_an_incomplete_saved_identity_without_touching_the_app() {
        let fixture = fixture();
        std::fs::remove_file(fixture.paths.profile_dir("there").join(TOKEN_CACHE)).unwrap();
        std::fs::remove_dir_all(fixture.paths.profile_dir("there").join(DESKTOP_STATE)).unwrap();
        let error = plan_switch(&fixture.paths, "there", SwitchOpts::default()).unwrap_err();
        assert!(error.to_string().contains("credential"), "{error}");
    }

    #[test]
    fn planning_refuses_credentials_without_saved_browser_state() {
        let fixture = fixture();
        std::fs::remove_dir_all(fixture.paths.profile_dir("there").join(DESKTOP_STATE)).unwrap();

        let error = plan_switch(&fixture.paths, "there", SwitchOpts::default()).unwrap_err();

        assert!(error.to_string().contains("browser state"), "{error}");
    }

    #[test]
    fn a_failed_switch_requests_rollback_and_still_relaunches() {
        let fixture = fixture();
        let plan = plan_switch(&fixture.paths, "there", SwitchOpts::default()).unwrap();
        write(&fixture.paths.config_json(), "{ not json");
        let recorder = Recorder::default();

        let error = apply_switch(&fixture.paths, &plan, &recorder).unwrap_err();

        assert!(error.to_string().contains("json"), "{error}");
        let steps = recorder.steps();
        assert_eq!(steps.first().map(String::as_str), Some("quit"));
        assert!(
            steps.iter().any(|step| step.starts_with("restore ")),
            "{steps:?}"
        );
        assert_eq!(steps.last().map(String::as_str), Some("relaunch"));
    }

    #[test]
    fn restoring_browser_state_removes_files_the_target_does_not_have() {
        let fixture = fixture();
        write(&fixture.paths.data_dir.join("Cookies-journal"), "outgoing");
        write(
            &fixture.paths.data_dir.join("Session Storage/CURRENT"),
            "outgoing",
        );
        let plan = plan_switch(&fixture.paths, "there", SwitchOpts::default()).unwrap();

        apply_switch(&fixture.paths, &plan, &Recorder::default()).unwrap();

        assert!(!fixture.paths.data_dir.join("Cookies-journal").exists());
        assert!(!fixture.paths.data_dir.join("Session Storage").exists());
    }

    #[test]
    fn keeping_the_bridge_leaves_it_in_place() {
        let fixture = fixture();
        let plan = plan_switch(
            &fixture.paths,
            "there",
            SwitchOpts {
                keep_bridge: true,
                ..SwitchOpts::default()
            },
        )
        .unwrap();

        apply_switch(&fixture.paths, &plan, &Recorder::default()).unwrap();
        assert!(fixture.paths.data_dir.join(BRIDGE_FILE).is_file());
    }

    #[test]
    fn archives_are_pruned_oldest_first() {
        let dir = tempfile::TempDir::new().unwrap();
        for stamp in ["20260101-000000", "20260102-000000", "20260103-000000"] {
            std::fs::write(dir.path().join(format!("switch-{stamp}-x.tar.gz")), "z").unwrap();
        }
        std::fs::write(dir.path().join("unrelated.txt"), "keep me").unwrap();
        let mut notes = Vec::new();

        prune_archives(dir.path(), 2, &mut notes);

        assert!(notes.is_empty(), "{notes:?}");
        assert!(!dir.path().join("switch-20260101-000000-x.tar.gz").exists());
        assert!(dir.path().join("switch-20260102-000000-x.tar.gz").exists());
        assert!(dir.path().join("switch-20260103-000000-x.tar.gz").exists());
        assert!(dir.path().join("unrelated.txt").exists());
    }

    /// Notes are printed verbatim by `account`, so a path entering one must go
    /// through `sanitize_untrusted_path` — `Display for Path` escapes nothing.
    ///
    /// Scoped to `notes.push` rather than to the file, because elsewhere in
    /// this module a bare `.display()` is the *correct* call:
    /// `collect_registry_members` builds tar arguments, and sanitizing one
    /// would corrupt the filename actually handed to `tar`. The distinction is
    /// whether the string is read by a person or by a program.
    #[test]
    fn no_note_interpolates_an_unsanitized_path() {
        let mut sites = Vec::new();
        for file in crate::guard::rs_files_in("src") {
            let source = std::fs::read_to_string(&file).expect("readable module");
            let body = crate::guard::production_code(&source);
            let mut rest = body.as_str();
            while let Some(at) = rest.find("notes.push(") {
                let call = &rest[at..];
                let mut depth = 0usize;
                let mut end = call.len();
                for (i, ch) in call.char_indices() {
                    match ch {
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                end = i;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                if call[..end].contains(".display()") {
                    sites.push(format!("{}: {}", file.display(), &call[..end]));
                }
                rest = &call[end.max(1)..];
            }
        }
        assert!(
            sites.is_empty(),
            "a note reaches the terminal verbatim; render its path with \
             `sanitize_untrusted_path`. Found: {sites:#?}"
        );
    }

    /// `account` prints every note straight to the terminal, so a path that
    /// reaches one must not carry an escape sequence there. Making the doomed
    /// archive a directory is the shortest route to the failure branch that
    /// builds a note: `remove_file` refuses it.
    ///
    /// Unix-only because a `\x1b` is not a legal filename byte on Windows.
    #[cfg(unix)]
    #[test]
    fn a_note_does_not_carry_a_terminal_escape_out_of_a_path() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("switch-20260101-000000-\x1b[2Kx.tar.gz")).unwrap();
        std::fs::write(dir.path().join("switch-20260102-000000-y.tar.gz"), "z").unwrap();
        let mut notes = Vec::new();

        prune_archives(dir.path(), 1, &mut notes);

        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(!notes[0].contains('\u{1b}'), "{:?}", notes[0]);
        assert!(!notes[0].contains('\n'), "{:?}", notes[0]);
        assert!(notes[0].contains("could not prune"), "{}", notes[0]);
    }

    #[test]
    fn pruning_never_discards_the_only_rollback_archive() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("switch-20260101-000000-x.tar.gz"), "z").unwrap();
        let mut notes = Vec::new();

        prune_archives(dir.path(), 1, &mut notes);

        assert!(dir.path().join("switch-20260101-000000-x.tar.gz").exists());
    }
}
