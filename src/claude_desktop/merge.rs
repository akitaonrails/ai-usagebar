//! The pure half of a Claude Desktop account switch: deciding which session
//! indexes to copy, what the merged schedule registry looks like, and what the
//! rewritten `config.json` / `ant-device-registry.json` contain.
//!
//! Nothing here touches `$HOME`, spawns a process, or writes a file — every
//! function takes an explicit root or the file's bytes, which is what lets
//! `--dry-run` be free (compute the plan, print it, stop) and lets the whole
//! algorithm be unit-tested against a temporary directory on any platform.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::error::{AppError, Result};

/// Per-account schedule registry, stored beside that account's session indexes.
pub(super) const SCHEDULED_TASKS: &str = "scheduled-tasks.json";

/// Session-index files that a switch would bring into the target account's
/// history folder, so it shows the union of everything rather than only the
/// conversations that account happened to start.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SessionMerge {
    /// `(source, destination)` for indexes the target has never seen.
    pub copied: Vec<(PathBuf, PathBuf)>,
    /// `(source, destination)` where the source is strictly newer. A resumed
    /// chat's index advances, so a stale copy in the target folder *must* be
    /// overwritten — otherwise the app reopens it at an old resume point and
    /// the recent messages look like they vanished.
    pub updated: Vec<(PathBuf, PathBuf)>,
}

impl SessionMerge {
    pub fn is_empty(&self) -> bool {
        self.copied.is_empty() && self.updated.is_empty()
    }
}

/// The merged `scheduled-tasks.json` for the target account, rendered but not
/// yet written.
#[derive(Debug, PartialEq, Eq)]
pub struct ScheduledMerge {
    pub target: PathBuf,
    pub bytes: Vec<u8>,
    /// Tasks the target did not already have.
    pub added: usize,
    /// Tasks the target already had, whose definition a more recently edited
    /// registry replaced — an edit made while signed into another account.
    pub updated: usize,
    /// Same-id definitions changed independently, so the target's local copy
    /// was preserved instead of guessing and discarding one side.
    pub conflicts: usize,
    /// Last successfully reconciled definition per task. Missing entries are
    /// deliberate unresolved conflicts and must not be propagated later.
    pub canonical_routines: BTreeMap<String, Value>,
}

impl ScheduledMerge {
    /// User-visible names from the definitions selected for this switch.
    /// These bytes were rendered while planning, so later registry writes
    /// cannot change which title the convergence pass propagates.
    pub(super) fn display_names(&self) -> Result<BTreeMap<String, String>> {
        let document: Value = serde_json::from_slice(&self.bytes)?;
        let tasks = document
            .get("scheduledTasks")
            .and_then(Value::as_array)
            .ok_or_else(|| AppError::Other("planned scheduledTasks is not a JSON array".into()))?;
        Ok(tasks
            .iter()
            .filter_map(|task| Some((task_id(task)?, display_name(task)?)))
            .collect())
    }
}

/// Plan the session-index merge into `<sessions_root>/<account_uuid>/<org_uuid>`.
///
/// Infallible: an unreadable or absent tree simply means there is nothing to
/// merge. Only the *writes* can fail, and those abort the switch before it
/// touches the app or any credential.
pub fn plan_session_merge(
    sessions_root: &Path,
    account_uuid: &str,
    org_uuid: &str,
) -> SessionMerge {
    let target_dir = sessions_root.join(account_uuid).join(org_uuid);

    // Keyed by destination so two source accounts holding the same index can't
    // both queue a write — the newest one wins, exactly as it would if the
    // copies were applied one at a time.
    let mut best: BTreeMap<PathBuf, (PathBuf, i64)> = BTreeMap::new();
    for source_dir in account_org_dirs(sessions_root) {
        if source_dir == target_dir {
            continue;
        }
        for source in local_session_files(&source_dir) {
            let Some(name) = source.file_name() else {
                continue;
            };
            let destination = target_dir.join(name);
            let activity = last_activity(&source);
            match best.get(&destination) {
                Some((_, seen)) if *seen >= activity => {}
                _ => {
                    best.insert(destination, (source, activity));
                }
            }
        }
    }

    let mut merge = SessionMerge::default();
    for (destination, (source, activity)) in best {
        if !destination.exists() {
            merge.copied.push((source, destination));
        } else if activity > last_activity(&destination) {
            merge.updated.push((source, destination));
        }
    }
    merge
}

/// What one account held the last time a merge touched it.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SyncedAccount {
    #[serde(default)]
    pub routines: BTreeSet<String>,
    /// Session-index filenames (`local_<id>.json`), not transcripts.
    #[serde(default)]
    pub sessions: BTreeSet<String>,
    /// Exact definitions observed the last time a switch completed. This is a
    /// three-way merge baseline, not an authoritative copy.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub routine_definitions: BTreeMap<String, Value>,
    /// Successfully reconciled definitions, duplicated in every account row
    /// so the established flat claude-acc-compatible file shape stays intact.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub canonical_routines: BTreeMap<String, Value>,
}

/// What every account held after the last merge, keyed by account UUID. The
/// flat shape is shared with claude-acc; new fields on each row are additive.
pub type Synced = BTreeMap<String, SyncedAccount>;

/// Accept a canonical definition only when every recorded account agrees. An
/// older writer dropping the additive field therefore fails safe instead of
/// making a stale definition authoritative.
pub fn canonical_routines(synced: &Synced) -> BTreeMap<String, Value> {
    let mut accounts = synced.values();
    let Some(first) = accounts.next() else {
        return BTreeMap::new();
    };
    let mut canonical = first.canonical_routines.clone();
    for account in accounts {
        canonical.retain(|id, task| account.canonical_routines.get(id) == Some(task));
    }
    canonical
}

pub fn set_canonical_routines(synced: &mut Synced, canonical: &BTreeMap<String, Value>) {
    for account in synced.values_mut() {
        account.canonical_routines.clone_from(canonical);
    }
}

/// Kept for the record written before conversations were covered: a bare list
/// was routines only. Reading it as such means an existing file keeps working
/// instead of being silently discarded and re-learned.
pub fn parse_synced(bytes: &[u8]) -> Synced {
    if let Ok(current) = serde_json::from_slice::<Synced>(bytes) {
        return current;
    }
    serde_json::from_slice::<BTreeMap<String, BTreeSet<String>>>(bytes)
        .map(|old| {
            old.into_iter()
                .map(|(account, routines)| {
                    (
                        account,
                        SyncedAccount {
                            routines,
                            sessions: BTreeSet::new(),
                            routine_definitions: BTreeMap::new(),
                            canonical_routines: BTreeMap::new(),
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Which of the two things a conflict is about. They live in different files
/// and are swept differently, but the user answers one question about both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ConflictKind {
    Routine,
    Chat,
}

/// Stable, type-scoped identity for a destructive conflict decision.
///
/// Routine ids and chat-index filenames are both arbitrary strings and can
/// collide. Keeping the kind beside the id all the way to the sweep prevents a
/// verdict about one namespace from authorizing deletion in the other.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DeletionKey {
    pub kind: ConflictKind,
    pub id: String,
    pub deleted_by: String,
    pub still_in: Vec<String>,
}

impl DeletionKey {
    pub fn external(&self) -> String {
        // JSON is only an opaque transport token here. Length-aware encoding
        // avoids delimiter collisions in arbitrary ids, and including the
        // observed account topology makes stale dialogs fail closed.
        serde_json::to_string(&(
            1,
            self.kind.label(),
            &self.id,
            &self.deleted_by,
            &self.still_in,
        ))
        .expect("a tuple of strings is always JSON-serializable")
    }
}

impl ConflictKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Routine => "routine",
            Self::Chat => "chat",
        }
    }
}

/// Something an account deleted that other accounts still hold, so the next
/// merge would hand it straight back. Only the user can say which they meant,
/// so the merge surfaces these rather than guessing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletionCandidate {
    pub kind: ConflictKind,
    /// Task id for a routine; session-index filename for a chat.
    pub id: String,
    /// Account UUID that had it and no longer does.
    pub deleted_by: String,
    /// Account UUIDs that still hold a copy.
    pub still_in: Vec<String>,
    /// Short human description for the prompt — the id alone is opaque.
    pub summary: String,
}

impl DeletionCandidate {
    pub fn key(&self) -> DeletionKey {
        DeletionKey {
            kind: self.kind,
            id: self.id.clone(),
            deleted_by: self.deleted_by.clone(),
            still_in: self.still_in.clone(),
        }
    }

    pub fn external_key(&self) -> String {
        self.key().external()
    }
}

/// Every account's current contents, for refreshing [`Synced`] after a merge.
pub fn current_state(sessions_root: &Path) -> Synced {
    let mut out = Synced::new();
    for (account, path) in scheduled_registries(sessions_root) {
        let (tasks, _) = load_scheduled(&path);
        let account = out.entry(account).or_default();
        for task in tasks {
            let Some(id) = task_id(&task) else { continue };
            account.routines.insert(id.clone());
            account.routine_definitions.insert(id, task);
        }
    }
    for (account, dir) in account_dirs(sessions_root) {
        out.entry(account).or_default().sessions.extend(
            local_session_files(&dir)
                .iter()
                .filter_map(|path| file_name(path)),
        );
    }
    out
}

/// Routines and chats an account dropped since its last recorded sync that
/// survive in another account. Sorted for deterministic prompts and tests.
///
/// Anything absent from `synced` was never recorded as reaching that account,
/// so it is simply new and never reported — which also means the very first
/// run, before any record exists, reports nothing and behaves exactly as
/// before.
pub fn deletion_candidates(sessions_root: &Path, synced: &Synced) -> Vec<DeletionCandidate> {
    let mut out = Vec::new();
    let mut routines: Index = Index::default();
    let mut chats: Index = Index::default();

    for (account, path) in scheduled_registries(sessions_root) {
        let (tasks, _) = load_scheduled(&path);
        routines.seen(&account);
        for task in &tasks {
            let Some(id) = task_id(task) else { continue };
            routines.record(&account, id, || describe_task(task));
        }
    }
    for (account, dir) in account_dirs(sessions_root) {
        chats.seen(&account);
        for path in local_session_files(&dir) {
            let Some(name) = file_name(&path) else {
                continue;
            };
            chats.record(&account, name, || describe_session(&path));
        }
    }

    for (account, had) in synced.iter() {
        routines.collect(ConflictKind::Routine, account, &had.routines, &mut out);
        chats.collect(ConflictKind::Chat, account, &had.sessions, &mut out);
    }
    out.sort_by(|a, b| {
        a.kind
            .label()
            .cmp(b.kind.label())
            .then(a.id.cmp(&b.id))
            .then(a.deleted_by.cmp(&b.deleted_by))
    });
    out.dedup_by(|a, b| a.kind == b.kind && a.id == b.id);
    out
}

/// Who holds what, built once per kind so the deletion scan is a lookup rather
/// than a rescan per account.
#[derive(Default)]
struct Index {
    holders: BTreeMap<String, Vec<String>>,
    summaries: BTreeMap<String, String>,
    present: BTreeMap<String, BTreeSet<String>>,
}

impl Index {
    fn seen(&mut self, account: &str) {
        self.present.entry(account.to_string()).or_default();
    }

    fn record(&mut self, account: &str, id: String, summary: impl FnOnce() -> String) {
        self.holders
            .entry(id.clone())
            .or_default()
            .push(account.to_string());
        self.summaries.entry(id.clone()).or_insert_with(summary);
        self.present
            .entry(account.to_string())
            .or_default()
            .insert(id);
    }

    fn collect(
        &self,
        kind: ConflictKind,
        account: &str,
        had: &BTreeSet<String>,
        out: &mut Vec<DeletionCandidate>,
    ) {
        let holds = self.present.get(account);
        for id in had {
            if holds.is_some_and(|ids| ids.contains(id)) {
                continue; // still there — not deleted
            }
            let Some(still_in) = self.holders.get(id) else {
                continue; // gone everywhere; nothing would resurrect it
            };
            out.push(DeletionCandidate {
                kind,
                id: id.clone(),
                deleted_by: account.to_string(),
                still_in: still_in.clone(),
                summary: self.summaries.get(id).cloned().unwrap_or_default(),
            });
        }
    }
}

/// Registry rewrites and session-index removals a confirmed deletion implies.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DeletionSweep {
    pub rewrites: Vec<(PathBuf, Vec<u8>)>,
    pub removals: Vec<PathBuf>,
}

impl DeletionSweep {
    pub fn is_empty(&self) -> bool {
        self.rewrites.is_empty() && self.removals.is_empty()
    }
}

/// Remove `doomed` from every account, so a deletion the user confirmed sticks
/// instead of returning on the next switch. Returns the work; nothing is
/// touched here.
///
/// A chat is removed by dropping its *index* file. The transcript itself lives
/// in the account-agnostic `~/.claude/projects/`, which this never touches — so
/// the conversation stops following you between accounts without the text
/// being destroyed.
pub fn plan_deletion_sweep(
    sessions_root: &Path,
    doomed: &BTreeSet<DeletionKey>,
) -> Result<DeletionSweep> {
    let mut sweep = DeletionSweep::default();
    if doomed.is_empty() {
        return Ok(sweep);
    }
    let routine_ids: BTreeSet<&str> = doomed
        .iter()
        .filter(|key| key.kind == ConflictKind::Routine)
        .map(|key| key.id.as_str())
        .collect();
    let chat_names: BTreeSet<&str> = doomed
        .iter()
        .filter(|key| key.kind == ConflictKind::Chat)
        .map(|key| key.id.as_str())
        .collect();
    for (_, path) in scheduled_registries(sessions_root) {
        let (tasks, skips) = load_scheduled(&path);
        let before = tasks.len();
        let kept: Vec<Value> = tasks
            .into_iter()
            .filter(|task| task_id(task).is_none_or(|id| !routine_ids.contains(id.as_str())))
            .collect();
        if kept.len() == before {
            continue; // this registry held none of them
        }
        let merged = serde_json::json!({ "scheduledTasks": kept, "recordedSkips": skips });
        sweep.rewrites.push((path, serde_json::to_vec(&merged)?));
    }
    for (_, dir) in account_dirs(sessions_root) {
        for path in local_session_files(&dir) {
            if file_name(&path).is_some_and(|name| chat_names.contains(name.as_str())) {
                sweep.removals.push(path);
            }
        }
    }
    Ok(sweep)
}

/// Registry rewrites that make every account agree on each routine's
/// `displayName`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct NameConvergence {
    pub rewrites: Vec<(PathBuf, Vec<u8>)>,
    /// How many distinct routines had a name reconciled.
    pub converged: usize,
}

/// Rewrite every registry so a routine present in more than one account shows
/// the same `displayName` everywhere. `selected` comes from the routine merge
/// planned before any files are written, so an unrelated write cannot change
/// the winner by refreshing a registry's mtime.
///
/// No prompt: renames converge on the next switch the way the merge already
/// converges every other field. The complete registry document is retained and
/// only task titles are mutated, including top-level fields newer Desktop
/// versions may add.
pub fn plan_name_convergence(
    sessions_root: &Path,
    selected: &BTreeMap<String, String>,
) -> Result<NameConvergence> {
    let mut out = NameConvergence::default();
    let mut converged_ids: BTreeSet<String> = BTreeSet::new();
    for (_, path) in scheduled_registries(sessions_root) {
        let bytes = std::fs::read(&path).map_err(|error| AppError::io_at(&path, error))?;
        let mut document: Value = serde_json::from_slice(&bytes)?;
        let object = document.as_object_mut().ok_or_else(|| {
            AppError::Other(format!(
                "scheduled task registry {} is not a JSON object",
                path.display()
            ))
        })?;
        let Some(tasks) = object.get_mut("scheduledTasks") else {
            continue;
        };
        let tasks = tasks.as_array_mut().ok_or_else(|| {
            AppError::Other(format!(
                "scheduledTasks in {} is not a JSON array",
                path.display()
            ))
        })?;
        let mut changed = false;
        for task in tasks {
            if let Some(id) = task_id(task)
                && let Some(name) = selected.get(&id)
                && display_name(task).as_deref() != Some(name.as_str())
                && let Some(obj) = task.as_object_mut()
            {
                obj.insert("displayName".into(), Value::String(name.clone()));
                changed = true;
                converged_ids.insert(id);
            }
        }
        if changed {
            out.rewrites.push((path, serde_json::to_vec(&document)?));
        }
    }
    out.converged = converged_ids.len();
    Ok(out)
}

/// `(account uuid, org dir)` for every account/org folder.
fn account_dirs(sessions_root: &Path) -> Vec<(String, PathBuf)> {
    account_org_dirs(sessions_root)
        .into_iter()
        .filter_map(|dir| {
            let account = dir.parent()?.file_name()?.to_str()?.to_string();
            Some((account, dir))
        })
        .collect()
}

fn file_name(path: &Path) -> Option<String> {
    path.file_name()?.to_str().map(str::to_string)
}

/// One line naming a chat: its title and the project it ran in.
fn describe_session(path: &Path) -> String {
    let Ok(bytes) = std::fs::read(path) else {
        return file_name(path).unwrap_or_default();
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return file_name(path).unwrap_or_default();
    };
    let title = value
        .get("title")
        .and_then(Value::as_str)
        .filter(|title| !title.is_empty())
        .unwrap_or("untitled");
    match value.get("cwd").and_then(Value::as_str) {
        Some(cwd) => {
            let project = Path::new(cwd)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(cwd);
            format!("{title}  ({project})")
        }
        None => title.to_string(),
    }
}

/// One line naming a task in the confirmation prompt. The id alone is opaque.
fn describe_task(task: &Value) -> String {
    let cron = task
        .get("cronExpression")
        .and_then(Value::as_str)
        .unwrap_or("no schedule");
    match task.get("filePath").and_then(Value::as_str) {
        Some(path) => {
            let name = Path::new(path)
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                .unwrap_or(path);
            format!("{cron}  ({name})")
        }
        None => cron.to_string(),
    }
}

/// `(account uuid, registry path)` for every account on this machine.
fn scheduled_registries(sessions_root: &Path) -> Vec<(String, PathBuf)> {
    account_org_dirs(sessions_root)
        .into_iter()
        .filter_map(|dir| {
            let path = dir.join(SCHEDULED_TASKS);
            if !path.is_file() {
                return None;
            }
            let account = dir.parent()?.file_name()?.to_str()?.to_string();
            Some((account, path))
        })
        .collect()
}

#[derive(Debug)]
struct RoutineCandidate {
    account: String,
    task: Value,
    target: bool,
}

/// Whether `task` changed since that account was last observed. `None` means
/// there is no trustworthy baseline (first run or a pre-definition record).
fn task_changed(synced: &Synced, account: &str, id: &str, task: &Value) -> Option<bool> {
    let state = synced.get(account)?;
    match state.routine_definitions.get(id) {
        Some(previous) => Some(previous != task),
        None if state.routines.contains(id) => None,
        None => Some(true),
    }
}

fn unique_values<'a>(values: impl Iterator<Item = &'a Value>) -> Vec<Value> {
    let mut unique = Vec::new();
    for value in values {
        if !unique.contains(value) {
            unique.push(value.clone());
        }
    }
    unique
}

/// Plan the routine/schedule merge as a three-way reconciliation by task `id`.
/// The task *scripts* these point at live in
/// `~/.claude/scheduled-tasks/<name>/` and are global already, so only the
/// account-scoped registry needs merging.
///
/// The sync record holds both the last observation per account and the last
/// successfully reconciled definition. An edit in one account therefore
/// propagates to stale copies without treating an unrelated registry write as
/// an edit to every task. Concurrent edits to the same task are kept local and
/// reported as a conflict; the user can resolve one by editing the desired
/// copy, which becomes the sole change against the next baseline.
pub fn plan_scheduled_merge(
    sessions_root: &Path,
    account_uuid: &str,
    org_uuid: &str,
    synced: &Synced,
) -> Result<ScheduledMerge> {
    let target = sessions_root
        .join(account_uuid)
        .join(org_uuid)
        .join(SCHEDULED_TASKS);
    let (target_tasks, mut skips) = load_scheduled(&target);
    let prior_canonical = canonical_routines(synced);

    let mut candidates: BTreeMap<String, Vec<RoutineCandidate>> = BTreeMap::new();
    let mut order = Vec::new();
    let mut seen = BTreeSet::new();
    for task in target_tasks {
        let Some(id) = task_id(&task) else { continue };
        if seen.insert(id.clone()) {
            order.push(id.clone());
        }
        candidates.entry(id).or_default().push(RoutineCandidate {
            account: account_uuid.to_string(),
            task,
            target: true,
        });
    }

    for (account, source) in scheduled_registries(sessions_root) {
        if source == target {
            continue;
        }
        let (source_tasks, source_skips) = load_scheduled(&source);
        for task in source_tasks {
            let Some(id) = task_id(&task) else {
                continue;
            };
            if seen.insert(id.clone()) {
                order.push(id.clone());
            }
            candidates.entry(id).or_default().push(RoutineCandidate {
                account: account.clone(),
                task,
                target: false,
            });
        }
        for (key, value) in source_skips {
            // First recorded skip wins; a later account's copy never overrides.
            skips.entry(key).or_insert(value);
        }
    }

    let mut tasks = Vec::new();
    let mut canonical_routines = BTreeMap::new();
    let mut added = 0usize;
    let mut updated = 0usize;
    let mut conflicts = 0usize;

    for id in order {
        let choices = &candidates[&id];
        let target_task = choices
            .iter()
            .find(|candidate| candidate.target)
            .map(|candidate| &candidate.task);
        let changed = unique_values(choices.iter().filter_map(|candidate| {
            task_changed(synced, &candidate.account, &id, &candidate.task)
                .is_some_and(|changed| changed)
                .then_some(&candidate.task)
        }));

        let (selected, resolved) = if let Some(canonical) = prior_canonical.get(&id) {
            let unknown_divergence = choices.iter().any(|candidate| {
                task_changed(synced, &candidate.account, &id, &candidate.task).is_none()
                    && candidate.task != *canonical
            });
            if changed.len() == 1 && !unknown_divergence {
                (changed[0].clone(), true)
            } else if changed.is_empty() && !unknown_divergence {
                (canonical.clone(), true)
            } else {
                (target_task.unwrap_or(&choices[0].task).clone(), false)
            }
        } else if changed.len() == 1 {
            (changed[0].clone(), true)
        } else if changed.len() > 1 {
            (target_task.unwrap_or(&choices[0].task).clone(), false)
        } else {
            let max_created = choices
                .iter()
                .map(|candidate| created_at(&candidate.task))
                .max();
            let newest = unique_values(choices.iter().filter_map(|candidate| {
                (Some(created_at(&candidate.task)) == max_created).then_some(&candidate.task)
            }));
            if newest.len() == 1 {
                (newest[0].clone(), true)
            } else {
                (target_task.unwrap_or(&choices[0].task).clone(), false)
            }
        };

        match target_task {
            None => added += 1,
            Some(existing) if *existing != selected => updated += 1,
            Some(_) => {}
        }
        if resolved {
            canonical_routines.insert(id, selected.clone());
        } else {
            conflicts += 1;
        }
        tasks.push(selected);
    }

    let merged = serde_json::json!({
        "scheduledTasks": tasks,
        "recordedSkips": skips,
    });
    Ok(ScheduledMerge {
        target,
        bytes: serde_json::to_vec(&merged)?,
        added,
        updated,
        conflicts,
        canonical_routines,
    })
}

/// Rewrite Claude Desktop's `config.json` so the app comes back as a different
/// account: both OAuth token-cache blobs plus the `lastKnownAccountUuid`
/// pointer that tells the app who it is. Every other key — the dxt allowlists,
/// window state, feature flags — is carried through untouched.
pub fn swap_config_tokens(
    existing: &[u8],
    token_cache: &str,
    token_cache_v2: &str,
    account_uuid: &str,
) -> Result<Vec<u8>> {
    let mut document: Value = serde_json::from_slice(existing)?;
    let object = document
        .as_object_mut()
        .ok_or_else(|| AppError::Other("Claude Desktop config.json is not a JSON object".into()))?;
    object.insert("oauth:tokenCache".into(), token_cache.into());
    object.insert("oauth:tokenCacheV2".into(), token_cache_v2.into());
    object.insert("lastKnownAccountUuid".into(), account_uuid.into());
    Ok(serde_json::to_vec(&document)?)
}

/// Strip everything the Desktop app uses to remember an account, so it reopens
/// at the login screen. Only the identity keys go: the dxt allowlists, window
/// state and feature flags are the user's settings, not their session.
pub fn clear_config_tokens(existing: &[u8]) -> Result<Vec<u8>> {
    let mut document: Value = serde_json::from_slice(existing)?;
    let object = document
        .as_object_mut()
        .ok_or_else(|| AppError::Other("Claude Desktop config.json is not a JSON object".into()))?;
    for key in [
        "oauth:tokenCache",
        "oauth:tokenCacheV2",
        "lastKnownAccountUuid",
    ] {
        object.remove(key);
    }
    Ok(serde_json::to_vec(&document)?)
}

/// The account the app has finished signing in as, or `None` while the login
/// is still in progress. Both fields matter: `lastKnownAccountUuid` appears
/// before the token cache is written, so keying on it alone captures a
/// half-finished login.
pub fn logged_in_account(config: &[u8]) -> Option<String> {
    let document: Value = serde_json::from_slice(config).ok()?;
    let has_both_tokens = ["oauth:tokenCache", "oauth:tokenCacheV2"]
        .into_iter()
        .all(|key| {
            document
                .get(key)
                .and_then(Value::as_str)
                .is_some_and(|token| !token.is_empty())
        });
    if !has_both_tokens {
        return None;
    }
    document
        .get("lastKnownAccountUuid")?
        .as_str()
        .filter(|uuid| !uuid.is_empty())
        .map(str::to_string)
}

/// An organisation this machine has not seen before, recovered from the dxt
/// allowlist keys the app writes per org (`dxt:allowlistEnabled:<org>`). The
/// fallback for when the account's session folder has not appeared yet.
pub fn new_org_in_config(config: &[u8], known: &[String]) -> Option<String> {
    let mut unseen = orgs_in_config(config)
        .into_iter()
        .filter(|org| !known.contains(org));
    let candidate = unseen.next()?;
    unseen.next().is_none().then_some(candidate)
}

/// Every organisation already represented by a dxt allowlist key. Capture
/// records this baseline before clearing the login, so an old unmanaged
/// account's key cannot be mistaken for the new account's organisation.
pub fn orgs_in_config(config: &[u8]) -> Vec<String> {
    const PREFIX: &str = "dxt:allowlistEnabled:";
    let Ok(document) = serde_json::from_slice::<Value>(config) else {
        return Vec::new();
    };
    let Some(object) = document.as_object() else {
        return Vec::new();
    };
    let mut orgs: Vec<String> = object
        .keys()
        .filter_map(|key| key.strip_prefix(PREFIX))
        .filter(|org| !org.is_empty())
        .map(str::to_string)
        .collect();
    orgs.sort();
    orgs.dedup();
    orgs
}

/// Fold a per-profile snapshot of `ant-device-registry.json` back into the live
/// one. The file is a map of account UUID → device registration and already
/// holds every account, so this is purely additive: **the live value wins every
/// conflict**, and the only thing a snapshot can contribute is a key the live
/// file somehow lost. That makes it impossible for this step to be the cause of
/// a broken browser pairing, which is the whole point of doing it at all.
///
/// A malformed snapshot returns the live file unchanged.
pub fn merge_device_registry(live: &[u8], snapshot: &[u8]) -> Result<Vec<u8>> {
    let mut merged: Map<String, Value> = serde_json::from_slice(live)?;
    if let Ok(saved) = serde_json::from_slice::<Map<String, Value>>(snapshot) {
        for (account_uuid, registration) in saved {
            merged.entry(account_uuid).or_insert(registration);
        }
    }
    Ok(serde_json::to_vec(&merged)?)
}

/// Every `<account>/<org>` directory under the session root.
fn account_org_dirs(sessions_root: &Path) -> Vec<PathBuf> {
    let Ok(accounts) = std::fs::read_dir(sessions_root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for account in accounts.flatten() {
        let Ok(orgs) = std::fs::read_dir(account.path()) else {
            continue;
        };
        out.extend(orgs.flatten().map(|org| org.path()).filter(|p| p.is_dir()));
    }
    out.sort();
    out
}

/// `local_*.json` only — never an account-level file such as
/// `scheduled-tasks.json`, which lives in the same folder but is merged by id.
fn local_session_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("local_") && name.ends_with(".json"))
        })
        .collect();
    out.sort();
    out
}

fn load_scheduled(path: &Path) -> (Vec<Value>, Map<String, Value>) {
    let Ok(bytes) = std::fs::read(path) else {
        return (Vec::new(), Map::new());
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return (Vec::new(), Map::new());
    };
    let tasks = value
        .get("scheduledTasks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let skips = value
        .get("recordedSkips")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    (tasks, skips)
}

fn task_id(task: &Value) -> Option<String> {
    task.get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// A routine's user-visible title. Empty is treated as absent so a blank name
/// never becomes the value every account converges to.
fn display_name(task: &Value) -> Option<String> {
    task.get("displayName")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

fn created_at(task: &Value) -> i64 {
    task.get("createdAt").map_or(0, json_i64)
}

fn last_activity(path: &Path) -> i64 {
    let Ok(bytes) = std::fs::read(path) else {
        return 0;
    };
    serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|value| value.get("lastActivityAt").map(json_i64))
        .unwrap_or(0)
}

/// Timestamps arrive as JSON numbers, but a schema-tolerant read costs one line
/// and a wrong `0` would silently make every merge decision "keep the target".
fn json_i64(value: &Value) -> i64 {
    value
        .as_i64()
        .or_else(|| value.as_f64().map(|seconds| seconds as i64))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    fn session(activity: i64) -> String {
        format!("{{\"lastActivityAt\":{activity},\"cwd\":\"/tmp/demo\"}}")
    }

    #[test]
    fn session_merge_takes_the_newest_copy() {
        let root = tempfile::TempDir::new().unwrap();
        let sessions = root.path();
        write(&sessions.join("A/O1/local_x.json"), &session(100));
        write(&sessions.join("B/O2/local_x.json"), &session(200));

        let merge = plan_session_merge(sessions, "A", "O1");
        assert!(merge.copied.is_empty());
        assert_eq!(merge.updated.len(), 1);
        assert_eq!(merge.updated[0].0, sessions.join("B/O2/local_x.json"));
        assert_eq!(merge.updated[0].1, sessions.join("A/O1/local_x.json"));
    }

    #[test]
    fn session_merge_keeps_a_newer_target() {
        let root = tempfile::TempDir::new().unwrap();
        let sessions = root.path();
        write(&sessions.join("A/O1/local_x.json"), &session(300));
        write(&sessions.join("B/O2/local_x.json"), &session(200));

        assert_eq!(
            plan_session_merge(sessions, "A", "O1"),
            SessionMerge::default()
        );
    }

    #[test]
    fn session_merge_copies_indexes_the_target_lacks() {
        let root = tempfile::TempDir::new().unwrap();
        let sessions = root.path();
        write(&sessions.join("A/O1/local_x.json"), &session(100));
        write(&sessions.join("B/O2/local_y.json"), &session(50));

        let merge = plan_session_merge(sessions, "A", "O1");
        assert_eq!(merge.updated.len(), 0);
        assert_eq!(merge.copied.len(), 1);
        assert_eq!(merge.copied[0].1, sessions.join("A/O1/local_y.json"));
    }

    #[test]
    fn session_merge_prefers_the_newest_of_several_sources() {
        let root = tempfile::TempDir::new().unwrap();
        let sessions = root.path();
        write(&sessions.join("A/O1/local_x.json"), &session(10));
        write(&sessions.join("B/O2/local_x.json"), &session(200));
        write(&sessions.join("C/O3/local_x.json"), &session(150));

        let merge = plan_session_merge(sessions, "A", "O1");
        assert_eq!(merge.updated.len(), 1);
        assert_eq!(merge.updated[0].0, sessions.join("B/O2/local_x.json"));
    }

    #[test]
    fn session_merge_never_touches_the_schedule_registry() {
        let root = tempfile::TempDir::new().unwrap();
        let sessions = root.path();
        write(&sessions.join("A/O1/local_x.json"), &session(100));
        write(&sessions.join("B/O2/scheduled-tasks.json"), "{}");

        let merge = plan_session_merge(sessions, "A", "O1");
        assert!(merge.is_empty(), "{merge:?}");
    }

    #[test]
    fn scheduled_merge_unions_by_id_with_the_newer_winning() {
        let root = tempfile::TempDir::new().unwrap();
        let sessions = root.path();
        write(
            &sessions.join("A/O1/scheduled-tasks.json"),
            r#"{"scheduledTasks":[{"id":"t1","createdAt":100,"name":"old"}],
                "recordedSkips":{"t1":"target"}}"#,
        );
        write(
            &sessions.join("B/O2/scheduled-tasks.json"),
            r#"{"scheduledTasks":[{"id":"t1","createdAt":200,"name":"new"},
                                  {"id":"t2","createdAt":5,"name":"extra"}],
                "recordedSkips":{"t1":"other","t2":"other"}}"#,
        );

        let merge = plan_scheduled_merge(sessions, "A", "O1", &Synced::default()).unwrap();
        assert_eq!(merge.added, 1);
        let value: Value = serde_json::from_slice(&merge.bytes).unwrap();
        let tasks = value["scheduledTasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0]["name"], "new", "newer createdAt must win");
        assert_eq!(tasks[1]["id"], "t2");
        // setdefault semantics: the target's own skip is never overridden.
        assert_eq!(value["recordedSkips"]["t1"], "target");
        assert_eq!(value["recordedSkips"]["t2"], "other");
    }

    fn sync_baseline(sessions: &Path) -> Synced {
        let mut synced = current_state(sessions);
        let mut canonical = BTreeMap::new();
        let mut conflicts = BTreeSet::new();
        for account in synced.values() {
            for (id, task) in &account.routine_definitions {
                match canonical.get(id) {
                    Some(existing) if existing != task => {
                        conflicts.insert(id.clone());
                    }
                    Some(_) => {}
                    None => {
                        canonical.insert(id.clone(), task.clone());
                    }
                }
            }
        }
        for id in conflicts {
            canonical.remove(&id);
        }
        set_canonical_routines(&mut synced, &canonical);
        synced
    }

    /// A schedule edited while signed into another account: same `id`, same
    /// `createdAt` (the format has no `updatedAt`), different cron. Comparing
    /// `createdAt` alone ties, and a tie used to keep the incumbent — silently
    /// discarding the edit on every switch back.
    #[test]
    fn scheduled_merge_takes_an_edit_that_did_not_change_created_at() {
        let root = tempfile::TempDir::new().unwrap();
        let sessions = root.path();
        let target = sessions.join("A/O1/scheduled-tasks.json");
        let source = sessions.join("B/O2/scheduled-tasks.json");
        let old = r#"{"scheduledTasks":[{"id":"t1","createdAt":100,"displayName":"Morning","cronExpression":"0 5 * * *","enabled":true}]}"#;
        write(&target, old);
        write(&source, old);
        let synced = sync_baseline(sessions);
        write(
            &source,
            r#"{"scheduledTasks":[{"id":"t1","createdAt":100,"displayName":"Renamed","cronExpression":"30 9 * * *","enabled":false}]}"#,
        );

        let merge = plan_scheduled_merge(sessions, "A", "O1", &synced).unwrap();

        assert_eq!(merge.added, 0);
        assert_eq!(merge.updated, 1, "the edit must be carried across");
        let value: Value = serde_json::from_slice(&merge.bytes).unwrap();
        assert_eq!(value["scheduledTasks"][0]["cronExpression"], "30 9 * * *");
        assert_eq!(value["scheduledTasks"][0]["enabled"], false);
        assert_eq!(merge.display_names().unwrap()["t1"], "Renamed");
    }

    // Reads a registry's first task's displayName back off disk.
    fn first_display_name(path: &Path) -> String {
        let value: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        value["scheduledTasks"][0]["displayName"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    #[test]
    fn name_convergence_pushes_the_selected_title_to_every_account() {
        // Same routine id, two different titles — the classic "renamed in one
        // account, never settles" case. The title selected by the earlier
        // routine merge must win everywhere, not just in the target.
        let root = tempfile::TempDir::new().unwrap();
        let sessions = root.path();
        let a = sessions.join("A/O1/scheduled-tasks.json");
        let b = sessions.join("B/O2/scheduled-tasks.json");
        write(
            &a,
            r#"{"scheduledTasks":[{"id":"t1","createdAt":100,"displayName":"Old name","cronExpression":"0 5 * * *"}]}"#,
        );
        write(
            &b,
            r#"{"scheduledTasks":[{"id":"t1","createdAt":100,"displayName":"New name","cronExpression":"0 5 * * *"}]}"#,
        );

        let selected = BTreeMap::from([("t1".to_string(), "New name".to_string())]);
        let plan = plan_name_convergence(sessions, &selected).unwrap();
        assert_eq!(plan.converged, 1);
        // Only the stale registry (A) is rewritten; B already holds the winner.
        assert_eq!(plan.rewrites.len(), 1);
        for (path, bytes) in &plan.rewrites {
            std::fs::write(path, bytes).unwrap();
        }
        assert_eq!(first_display_name(&a), "New name");
        assert_eq!(first_display_name(&b), "New name");
    }

    #[test]
    fn name_convergence_touches_only_the_title() {
        // Converging the name must not disturb any other field.
        let root = tempfile::TempDir::new().unwrap();
        let sessions = root.path();
        let a = sessions.join("A/O1/scheduled-tasks.json");
        let b = sessions.join("B/O2/scheduled-tasks.json");
        write(
            &a,
            r#"{"scheduledTasks":[{"id":"t1","createdAt":100,"displayName":"Stale","cronExpression":"0 5 * * *","enabled":true,"model":"opus"}]}"#,
        );
        write(
            &b,
            r#"{"scheduledTasks":[{"id":"t1","createdAt":100,"displayName":"Fresh","cronExpression":"30 9 * * *","enabled":false}]}"#,
        );
        let selected = BTreeMap::from([("t1".to_string(), "Stale".to_string())]);
        let plan = plan_name_convergence(sessions, &selected).unwrap();
        for (path, bytes) in &plan.rewrites {
            std::fs::write(path, bytes).unwrap();
        }
        // B took A's name…
        assert_eq!(first_display_name(&b), "Stale");
        // …but kept its own cron/enabled — convergence is name-only.
        let vb: Value = serde_json::from_slice(&std::fs::read(&b).unwrap()).unwrap();
        assert_eq!(vb["scheduledTasks"][0]["cronExpression"], "30 9 * * *");
        assert_eq!(vb["scheduledTasks"][0]["enabled"], false);
    }

    #[test]
    fn name_convergence_preserves_unknown_registry_fields() {
        let root = tempfile::TempDir::new().unwrap();
        let sessions = root.path();
        let registry = sessions.join("A/O1/scheduled-tasks.json");
        write(
            &registry,
            r#"{"scheduledTasks":[{"id":"t1","displayName":"Old"}],
                "recordedSkips":{"t1":"kept"},
                "futureMetadata":{"must":"survive"}}"#,
        );
        let selected = BTreeMap::from([("t1".to_string(), "New".to_string())]);

        let plan = plan_name_convergence(sessions, &selected).unwrap();
        assert_eq!(plan.rewrites.len(), 1);
        let value: Value = serde_json::from_slice(&plan.rewrites[0].1).unwrap();

        assert_eq!(value["scheduledTasks"][0]["displayName"], "New");
        assert_eq!(value["recordedSkips"]["t1"], "kept");
        assert_eq!(value["futureMetadata"]["must"], "survive");
    }

    #[test]
    fn name_convergence_fails_closed_on_an_invalid_registry_shape() {
        let root = tempfile::TempDir::new().unwrap();
        let sessions = root.path();
        write(
            &sessions.join("A/O1/scheduled-tasks.json"),
            r#"{"scheduledTasks":"not-an-array","futureMetadata":true}"#,
        );
        let selected = BTreeMap::from([("t1".to_string(), "New".to_string())]);

        let error = plan_name_convergence(sessions, &selected).unwrap_err();

        assert!(error.to_string().contains("scheduledTasks"));
        assert!(error.to_string().contains("not a JSON array"));
    }

    #[test]
    fn name_convergence_keeps_the_prewrite_merge_decision() {
        let root = tempfile::TempDir::new().unwrap();
        let sessions = root.path();
        let a = sessions.join("A/O1/scheduled-tasks.json");
        let b = sessions.join("B/O2/scheduled-tasks.json");
        write(
            &a,
            r#"{"scheduledTasks":[
                {"id":"t1","createdAt":100,"displayName":"Old"},
                {"id":"doomed","createdAt":200,"displayName":"Delete me"}]}"#,
        );
        write(
            &b,
            r#"{"scheduledTasks":[{"id":"t1","createdAt":100,"displayName":"Old"}]}"#,
        );
        let synced = sync_baseline(sessions);
        write(
            &b,
            r#"{"scheduledTasks":[{"id":"t1","createdAt":100,"displayName":"New"}]}"#,
        );

        // Capture the three-way merge's decision before anything is written.
        let scheduled = plan_scheduled_merge(sessions, "B", "O2", &synced).unwrap();
        let selected = scheduled.display_names().unwrap();
        assert_eq!(selected["t1"], "New");

        // An unrelated deletion then rewrites A. That must not make A's stale
        // title the winner merely because its registry was written later.
        let doomed = BTreeSet::from([DeletionKey {
            kind: ConflictKind::Routine,
            id: "doomed".to_string(),
            deleted_by: "B".to_string(),
            still_in: vec!["A".to_string()],
        }]);
        let sweep = plan_deletion_sweep(sessions, &doomed).unwrap();
        for (path, bytes) in sweep.rewrites {
            std::fs::write(path, bytes).unwrap();
        }

        let convergence = plan_name_convergence(sessions, &selected).unwrap();
        for (path, bytes) in convergence.rewrites {
            std::fs::write(path, bytes).unwrap();
        }
        assert_eq!(first_display_name(&a), "New");
        assert_eq!(first_display_name(&b), "New");
    }

    #[test]
    fn name_convergence_is_a_noop_when_titles_already_agree() {
        let root = tempfile::TempDir::new().unwrap();
        let sessions = root.path();
        write(
            &sessions.join("A/O1/scheduled-tasks.json"),
            r#"{"scheduledTasks":[{"id":"t1","createdAt":100,"displayName":"Same"}]}"#,
        );
        write(
            &sessions.join("B/O2/scheduled-tasks.json"),
            r#"{"scheduledTasks":[{"id":"t1","createdAt":100,"displayName":"Same"}]}"#,
        );
        let selected = BTreeMap::from([("t1".to_string(), "Same".to_string())]);
        let plan = plan_name_convergence(sessions, &selected).unwrap();
        assert_eq!(plan.converged, 0);
        assert!(plan.rewrites.is_empty());
    }

    /// The mirror image: only the target changed against the shared baseline,
    /// so the source's stale copy must not overwrite it.
    #[test]
    fn scheduled_merge_keeps_the_target_when_its_registry_is_fresher() {
        let root = tempfile::TempDir::new().unwrap();
        let sessions = root.path();
        let target = sessions.join("A/O1/scheduled-tasks.json");
        let source = sessions.join("B/O2/scheduled-tasks.json");
        let old =
            r#"{"scheduledTasks":[{"id":"t1","createdAt":100,"cronExpression":"0 5 * * *"}]}"#;
        write(&target, old);
        write(&source, old);
        let synced = sync_baseline(sessions);
        write(
            &target,
            r#"{"scheduledTasks":[{"id":"t1","createdAt":100,"cronExpression":"30 9 * * *"}]}"#,
        );

        let merge = plan_scheduled_merge(sessions, "A", "O1", &synced).unwrap();

        assert_eq!(merge.updated, 0);
        let value: Value = serde_json::from_slice(&merge.bytes).unwrap();
        assert_eq!(value["scheduledTasks"][0]["cronExpression"], "30 9 * * *");
    }

    /// Editing different tasks in different accounts is the case a file-level
    /// mtime cannot represent: whichever file wins overwrites one valid edit.
    #[test]
    fn independent_task_edits_are_both_reconciled() {
        let root = tempfile::TempDir::new().unwrap();
        let sessions = root.path();
        let a = sessions.join("A/O1/scheduled-tasks.json");
        let b = sessions.join("B/O2/scheduled-tasks.json");
        let old = r#"{"scheduledTasks":[
            {"id":"t1","createdAt":100,"name":"old-t1"},
            {"id":"t2","createdAt":100,"name":"old-t2"}
        ]}"#;
        write(&a, old);
        write(&b, old);
        let synced = sync_baseline(sessions);
        write(
            &a,
            r#"{"scheduledTasks":[
                {"id":"t1","createdAt":100,"name":"A-edited"},
                {"id":"t2","createdAt":100,"name":"old-t2"}
            ]}"#,
        );
        write(
            &b,
            r#"{"scheduledTasks":[
                {"id":"t1","createdAt":100,"name":"old-t1"},
                {"id":"t2","createdAt":100,"name":"B-edited"}
            ]}"#,
        );

        let merge = plan_scheduled_merge(sessions, "B", "O2", &synced).unwrap();
        let value: Value = serde_json::from_slice(&merge.bytes).unwrap();
        let tasks = value["scheduledTasks"].as_array().unwrap();

        assert_eq!(tasks[0]["name"], "A-edited");
        assert_eq!(tasks[1]["name"], "B-edited");
        assert_eq!(merge.updated, 1);
        assert_eq!(merge.conflicts, 0);

        // After B receives A's edit, the canonical t2 edit must still flow
        // back to A on the next switch; recording actual observations must not
        // accidentally declare A's stale copy authoritative.
        write(&b, std::str::from_utf8(&merge.bytes).unwrap());
        let mut next = current_state(sessions);
        set_canonical_routines(&mut next, &merge.canonical_routines);
        let into_a = plan_scheduled_merge(sessions, "A", "O1", &next).unwrap();
        let value: Value = serde_json::from_slice(&into_a.bytes).unwrap();
        let tasks = value["scheduledTasks"].as_array().unwrap();
        assert_eq!(tasks[0]["name"], "A-edited");
        assert_eq!(tasks[1]["name"], "B-edited");
        assert_eq!(into_a.updated, 1);
    }

    /// Without a baseline, equal-createdAt divergent definitions are
    /// ambiguous. Preserve the target and leave the canonical entry absent so
    /// a later switch cannot silently overwrite either side.
    #[test]
    fn an_unresolved_same_task_edit_is_kept_local() {
        let root = tempfile::TempDir::new().unwrap();
        let sessions = root.path();
        write(
            &sessions.join("A/O1/scheduled-tasks.json"),
            r#"{"scheduledTasks":[{"id":"t1","createdAt":100,"name":"A"}]}"#,
        );
        write(
            &sessions.join("B/O2/scheduled-tasks.json"),
            r#"{"scheduledTasks":[{"id":"t1","createdAt":100,"name":"B"}]}"#,
        );

        let merge = plan_scheduled_merge(sessions, "A", "O1", &Synced::default()).unwrap();
        let value: Value = serde_json::from_slice(&merge.bytes).unwrap();

        assert_eq!(value["scheduledTasks"][0]["name"], "A");
        assert_eq!(merge.updated, 0);
        assert_eq!(merge.conflicts, 1);
        assert!(!merge.canonical_routines.contains_key("t1"));
    }

    /// A genuinely newer `createdAt` means the task was deleted and recreated,
    /// so it still wins before a three-way baseline exists.
    #[test]
    fn a_newer_created_at_wins_regardless_of_file_mtime() {
        let root = tempfile::TempDir::new().unwrap();
        let sessions = root.path();
        let target = sessions.join("A/O1/scheduled-tasks.json");
        let source = sessions.join("B/O2/scheduled-tasks.json");
        write(
            &target,
            r#"{"scheduledTasks":[{"id":"t1","createdAt":100,"name":"old"}]}"#,
        );
        write(
            &source,
            r#"{"scheduledTasks":[{"id":"t1","createdAt":500,"name":"recreated"}]}"#,
        );
        let merge = plan_scheduled_merge(sessions, "A", "O1", &Synced::default()).unwrap();
        let value: Value = serde_json::from_slice(&merge.bytes).unwrap();
        assert_eq!(value["scheduledTasks"][0]["name"], "recreated");
    }

    /// An identical copy is not an edit; reporting it as one would make every
    /// switch claim to have changed something.
    #[test]
    fn an_identical_copy_is_not_counted_as_an_edit() {
        let root = tempfile::TempDir::new().unwrap();
        let sessions = root.path();
        let target = sessions.join("A/O1/scheduled-tasks.json");
        let source = sessions.join("B/O2/scheduled-tasks.json");
        let same =
            r#"{"scheduledTasks":[{"id":"t1","createdAt":100,"cronExpression":"0 5 * * *"}]}"#;
        write(&target, same);
        write(&source, same);
        let merge = plan_scheduled_merge(sessions, "A", "O1", &Synced::default()).unwrap();
        assert_eq!(merge.added, 0);
        assert_eq!(merge.updated, 0);
    }

    fn synced(pairs: &[(&str, &[&str])]) -> Synced {
        pairs
            .iter()
            .map(|(account, ids)| {
                (
                    (*account).to_string(),
                    SyncedAccount {
                        routines: ids.iter().map(|id| (*id).to_string()).collect(),
                        sessions: BTreeSet::new(),
                        routine_definitions: BTreeMap::new(),
                        canonical_routines: BTreeMap::new(),
                    },
                )
            })
            .collect()
    }

    fn synced_chats(pairs: &[(&str, &[&str])]) -> Synced {
        pairs
            .iter()
            .map(|(account, names)| {
                (
                    (*account).to_string(),
                    SyncedAccount {
                        routines: BTreeSet::new(),
                        sessions: names.iter().map(|name| (*name).to_string()).collect(),
                        routine_definitions: BTreeMap::new(),
                        canonical_routines: BTreeMap::new(),
                    },
                )
            })
            .collect()
    }

    /// A task B once had and no longer does, while A still holds it: the next
    /// merge into B would hand it straight back, so it needs confirming.
    #[test]
    fn a_task_dropped_by_one_account_is_reported_as_a_deletion() {
        let root = tempfile::TempDir::new().unwrap();
        let sessions = root.path();
        write(
            &sessions.join("A/O1/scheduled-tasks.json"),
            r#"{"scheduledTasks":[{"id":"t1","createdAt":1,"cronExpression":"0 5 * * *",
                "filePath":"/x/daily-report/SKILL.md"},{"id":"t2","createdAt":2}]}"#,
        );
        write(
            &sessions.join("B/O2/scheduled-tasks.json"),
            r#"{"scheduledTasks":[{"id":"t2","createdAt":2}]}"#,
        );

        let found = deletion_candidates(sessions, &synced(&[("B", &["t1", "t2"])]));

        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].id, "t1");
        assert_eq!(found[0].deleted_by, "B");
        assert_eq!(found[0].still_in, ["A"]);
        assert_eq!(found[0].summary, "0 5 * * *  (daily-report)");
    }

    /// The safe-rollout property: with no manifest yet, nothing is a deletion,
    /// so an upgrade never opens a prompt about tasks it has no history for.
    #[test]
    fn nothing_is_a_deletion_without_a_recorded_sync() {
        let root = tempfile::TempDir::new().unwrap();
        let sessions = root.path();
        write(
            &sessions.join("A/O1/scheduled-tasks.json"),
            r#"{"scheduledTasks":[{"id":"t1","createdAt":1}]}"#,
        );
        write(
            &sessions.join("B/O2/scheduled-tasks.json"),
            r#"{"scheduledTasks":[]}"#,
        );

        assert!(deletion_candidates(sessions, &Synced::new()).is_empty());
        // …and a brand-new account that simply never received it is not a
        // deletion either.
        assert!(deletion_candidates(sessions, &synced(&[("B", &[])])).is_empty());
    }

    /// Deleted in every account is just deleted — nothing would resurrect it,
    /// so there is nothing to ask about.
    #[test]
    fn a_task_gone_everywhere_is_not_a_conflict() {
        let root = tempfile::TempDir::new().unwrap();
        let sessions = root.path();
        write(
            &sessions.join("A/O1/scheduled-tasks.json"),
            r#"{"scheduledTasks":[]}"#,
        );
        write(
            &sessions.join("B/O2/scheduled-tasks.json"),
            r#"{"scheduledTasks":[]}"#,
        );

        let found = deletion_candidates(sessions, &synced(&[("A", &["t1"]), ("B", &["t1"])]));
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn a_confirmed_deletion_is_swept_from_every_registry() {
        let root = tempfile::TempDir::new().unwrap();
        let sessions = root.path();
        write(
            &sessions.join("A/O1/scheduled-tasks.json"),
            r#"{"scheduledTasks":[{"id":"t1","createdAt":1},{"id":"t2","createdAt":2}],
                "recordedSkips":{"t1":"keep me"}}"#,
        );
        write(
            &sessions.join("B/O2/scheduled-tasks.json"),
            r#"{"scheduledTasks":[{"id":"t1","createdAt":1}]}"#,
        );
        write(
            &sessions.join("C/O3/scheduled-tasks.json"),
            r#"{"scheduledTasks":[{"id":"t2","createdAt":2}]}"#,
        );

        let doomed = BTreeSet::from([DeletionKey {
            kind: ConflictKind::Routine,
            id: "t1".to_string(),
            deleted_by: "A".to_string(),
            still_in: vec!["B".to_string()],
        }]);
        let writes = plan_deletion_sweep(sessions, &doomed).unwrap().rewrites;

        // C never held t1, so it is not rewritten at all.
        assert_eq!(writes.len(), 2, "{writes:?}");
        for (path, bytes) in &writes {
            let value: Value = serde_json::from_slice(bytes).unwrap();
            let ids: Vec<&str> = value["scheduledTasks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|task| task["id"].as_str().unwrap())
                .collect();
            assert!(!ids.contains(&"t1"), "{path:?} still has t1");
        }
        // Unrelated state in a rewritten registry survives the sweep.
        let (_, a_bytes) = writes
            .iter()
            .find(|(p, _)| p.ends_with("A/O1/scheduled-tasks.json"))
            .unwrap();
        let a: Value = serde_json::from_slice(a_bytes).unwrap();
        assert_eq!(a["recordedSkips"]["t1"], "keep me");
        assert_eq!(a["scheduledTasks"][0]["id"], "t2");
    }

    /// A chat B once had and no longer does, while A still holds it — the same
    /// resurrection a routine suffers, in the other file.
    #[test]
    fn a_chat_dropped_by_one_account_is_reported_as_a_deletion() {
        let root = tempfile::TempDir::new().unwrap();
        let sessions = root.path();
        write(
            &sessions.join("A/O1/local_x.json"),
            r#"{"lastActivityAt":1,"title":"Refactor the parser","cwd":"/w/ai-usagebar"}"#,
        );
        write(
            &sessions.join("B/O2/local_y.json"),
            r#"{"lastActivityAt":2}"#,
        );

        let found = deletion_candidates(sessions, &synced_chats(&[("B", &["local_x.json"])]));

        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].kind, ConflictKind::Chat);
        assert_eq!(found[0].id, "local_x.json");
        assert_eq!(found[0].deleted_by, "B");
        assert_eq!(found[0].summary, "Refactor the parser  (ai-usagebar)");
    }

    /// Both kinds surface from one scan, so the user answers a single question.
    #[test]
    fn routines_and_chats_are_reported_together() {
        let root = tempfile::TempDir::new().unwrap();
        let sessions = root.path();
        write(
            &sessions.join("A/O1/scheduled-tasks.json"),
            r#"{"scheduledTasks":[{"id":"t1","cronExpression":"0 5 * * *"}]}"#,
        );
        write(&sessions.join("A/O1/local_x.json"), r#"{"title":"Chat"}"#);
        write(
            &sessions.join("B/O2/scheduled-tasks.json"),
            r#"{"scheduledTasks":[]}"#,
        );

        let mut had = synced(&[("B", &["t1"])]);
        had.get_mut("B")
            .unwrap()
            .sessions
            .insert("local_x.json".into());
        let found = deletion_candidates(sessions, &had);

        let kinds: Vec<&str> = found.iter().map(|c| c.kind.label()).collect();
        assert_eq!(kinds, ["chat", "routine"], "{found:?}");
    }

    /// Confirming a chat removes its index from every account — and only the
    /// index, since the transcript lives outside this tree entirely.
    #[test]
    fn a_confirmed_chat_deletion_removes_every_index_copy() {
        let root = tempfile::TempDir::new().unwrap();
        let sessions = root.path();
        write(&sessions.join("A/O1/local_x.json"), r#"{"title":"Doomed"}"#);
        write(&sessions.join("B/O2/local_x.json"), r#"{"title":"Doomed"}"#);
        write(
            &sessions.join("B/O2/local_keep.json"),
            r#"{"title":"Innocent"}"#,
        );

        let doomed = BTreeSet::from([DeletionKey {
            kind: ConflictKind::Chat,
            id: "local_x.json".to_string(),
            deleted_by: "A".to_string(),
            still_in: vec!["B".to_string()],
        }]);
        let sweep = plan_deletion_sweep(sessions, &doomed).unwrap();

        assert!(sweep.rewrites.is_empty(), "no registry is involved");
        assert_eq!(sweep.removals.len(), 2, "{:?}", sweep.removals);
        assert!(sweep.removals.iter().all(|p| p.ends_with("local_x.json")));
    }

    /// Routine ids and chat filenames share no namespace. A verdict for one
    /// must never authorize deleting the other even when the strings collide.
    #[test]
    fn a_deletion_verdict_is_scoped_to_its_conflict_kind() {
        let root = tempfile::TempDir::new().unwrap();
        let sessions = root.path();
        write(
            &sessions.join("A/O1/scheduled-tasks.json"),
            r#"{"scheduledTasks":[{"id":"local_x.json","createdAt":1}]}"#,
        );
        write(
            &sessions.join("A/O1/local_x.json"),
            r#"{"title":"Keep chat"}"#,
        );

        let doomed = BTreeSet::from([DeletionKey {
            kind: ConflictKind::Routine,
            id: "local_x.json".to_string(),
            deleted_by: "A".to_string(),
            still_in: vec!["B".to_string()],
        }]);
        let sweep = plan_deletion_sweep(sessions, &doomed).unwrap();

        assert_eq!(sweep.rewrites.len(), 1, "the routine is removed");
        assert!(sweep.removals.is_empty(), "the colliding chat must survive");
    }

    #[test]
    fn a_conflict_key_is_bound_to_the_observed_deletion() {
        let original = DeletionCandidate {
            kind: ConflictKind::Routine,
            id: "t1".to_string(),
            deleted_by: "A".to_string(),
            still_in: vec!["B".to_string()],
            summary: "daily".to_string(),
        };
        let mut later = original.clone();
        later.deleted_by = "B".to_string();
        later.still_in = vec!["A".to_string()];

        assert_ne!(original.external_key(), later.external_key());
    }

    /// The record written before chats were covered was a bare id list. It has
    /// to keep working, or upgrading would silently forget every routine and
    /// re-learn them as new.
    #[test]
    fn the_previous_record_format_is_still_understood() {
        let old = br#"{"acct-a":["t1","t2"]}"#;
        let parsed = parse_synced(old);
        assert_eq!(
            parsed["acct-a"].routines,
            BTreeSet::from(["t1".to_string(), "t2".to_string()])
        );
        assert!(parsed["acct-a"].sessions.is_empty());

        let current = br#"{"acct-a":{"routines":["t1"],"sessions":["local_x.json"]}}"#;
        let parsed = parse_synced(current);
        assert_eq!(
            parsed["acct-a"].routines,
            BTreeSet::from(["t1".to_string()])
        );
        assert_eq!(
            parsed["acct-a"].sessions,
            BTreeSet::from(["local_x.json".to_string()])
        );

        let v2 = br#"{
            "acct-a":{"routines":["t1"],"sessions":[],
                "routine_definitions":{"t1":{"id":"t1","enabled":true}},
                "canonical_routines":{"t1":{"id":"t1","enabled":true}}}
        }"#;
        let parsed = parse_synced(v2);
        assert_eq!(parsed["acct-a"].routine_definitions["t1"]["enabled"], true);
        assert_eq!(canonical_routines(&parsed)["t1"]["enabled"], true);

        assert!(parse_synced(b"not json").is_empty());
    }

    #[test]
    fn keeping_everything_rewrites_nothing() {
        let root = tempfile::TempDir::new().unwrap();
        let sessions = root.path();
        write(
            &sessions.join("A/O1/scheduled-tasks.json"),
            r#"{"scheduledTasks":[{"id":"t1","createdAt":1}]}"#,
        );
        assert!(
            plan_deletion_sweep(sessions, &BTreeSet::new())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn current_task_ids_records_every_account() {
        let root = tempfile::TempDir::new().unwrap();
        let sessions = root.path();
        write(
            &sessions.join("A/O1/scheduled-tasks.json"),
            r#"{"scheduledTasks":[{"id":"t1"},{"id":"t2"}]}"#,
        );
        write(
            &sessions.join("B/O2/scheduled-tasks.json"),
            r#"{"scheduledTasks":[{"id":"t2"}]}"#,
        );

        let ids = current_state(sessions);
        assert_eq!(
            ids["A"].routines,
            BTreeSet::from(["t1".to_string(), "t2".to_string()])
        );
        assert_eq!(ids["B"].routines, BTreeSet::from(["t2".to_string()]));
    }

    #[test]
    fn scheduled_merge_keeps_the_target_when_it_is_newer() {
        let root = tempfile::TempDir::new().unwrap();
        let sessions = root.path();
        write(
            &sessions.join("A/O1/scheduled-tasks.json"),
            r#"{"scheduledTasks":[{"id":"t1","createdAt":300,"name":"target"}]}"#,
        );
        write(
            &sessions.join("B/O2/scheduled-tasks.json"),
            r#"{"scheduledTasks":[{"id":"t1","createdAt":100,"name":"other"}]}"#,
        );

        let merge = plan_scheduled_merge(sessions, "A", "O1", &Synced::default()).unwrap();
        assert_eq!(merge.added, 0);
        let value: Value = serde_json::from_slice(&merge.bytes).unwrap();
        assert_eq!(value["scheduledTasks"][0]["name"], "target");
    }

    #[test]
    fn config_swap_preserves_every_unrelated_key() {
        let existing = br#"{"lastKnownAccountUuid":"old","oauth:tokenCache":"a",
            "oauth:tokenCacheV2":"b","dxt:allowlistEnabled:org-1":true,
            "windowBounds":{"width":1200}}"#;

        let bytes = swap_config_tokens(existing, "new-a", "new-b", "new-uuid").unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["oauth:tokenCache"], "new-a");
        assert_eq!(value["oauth:tokenCacheV2"], "new-b");
        assert_eq!(value["lastKnownAccountUuid"], "new-uuid");
        assert_eq!(value["dxt:allowlistEnabled:org-1"], true);
        assert_eq!(value["windowBounds"]["width"], 1200);
    }

    #[test]
    fn config_swap_rejects_a_non_object_document() {
        assert!(swap_config_tokens(b"[]", "a", "b", "u").is_err());
    }

    #[test]
    fn clearing_tokens_keeps_the_users_settings() {
        let existing = br#"{"lastKnownAccountUuid":"u","oauth:tokenCache":"a",
            "oauth:tokenCacheV2":"b","dxt:allowlistEnabled:org-1":true,"autoUpdates":true}"#;

        let bytes = clear_config_tokens(existing).unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(value.get("oauth:tokenCache").is_none());
        assert!(value.get("oauth:tokenCacheV2").is_none());
        assert!(value.get("lastKnownAccountUuid").is_none());
        assert_eq!(value["dxt:allowlistEnabled:org-1"], true);
        assert_eq!(value["autoUpdates"], true);
    }

    #[test]
    fn a_login_counts_only_once_both_fields_are_written() {
        assert_eq!(
            logged_in_account(&clear_config_tokens(b"{}").unwrap()),
            None
        );
        // The uuid lands before the token cache does; capturing here would
        // save a profile with no credential in it.
        assert_eq!(logged_in_account(br#"{"lastKnownAccountUuid":"u"}"#), None);
        assert_eq!(
            logged_in_account(br#"{"lastKnownAccountUuid":"u","oauth:tokenCacheV2":""}"#),
            None
        );
        assert_eq!(
            logged_in_account(br#"{"lastKnownAccountUuid":"u","oauth:tokenCacheV2":"t"}"#),
            None
        );
        assert_eq!(
            logged_in_account(
                br#"{"lastKnownAccountUuid":"u","oauth:tokenCache":"a","oauth:tokenCacheV2":"b"}"#
            ),
            Some("u".into())
        );
        assert_eq!(logged_in_account(b"not json"), None);
    }

    #[test]
    fn a_new_org_is_recovered_from_the_allowlist_keys() {
        let config = br#"{"dxt:allowlistEnabled:org-old":true,
            "dxt:allowlistEnabled:org-new":true,"unrelated":1}"#;

        assert_eq!(
            new_org_in_config(config, &["org-old".into()]),
            Some("org-new".into())
        );
        assert_eq!(
            new_org_in_config(config, &["org-old".into(), "org-new".into()]),
            None
        );
        assert_eq!(new_org_in_config(b"{}", &[]), None);
        assert_eq!(
            new_org_in_config(
                br#"{"dxt:allowlistEnabled:org-a":true,"dxt:allowlistEnabled:org-b":true}"#,
                &[]
            ),
            None,
            "multiple new orgs are ambiguous"
        );
        assert_eq!(
            orgs_in_config(config),
            ["org-new".to_string(), "org-old".to_string()]
        );
    }

    #[test]
    fn device_registry_merge_lets_the_live_value_win() {
        let live = br#"{"acct-a":{"deviceId":"live-a"},"acct-b":{"deviceId":"live-b"}}"#;
        let snapshot = br#"{"acct-b":{"deviceId":"stale-b"},"acct-c":{"deviceId":"saved-c"}}"#;

        let bytes = merge_device_registry(live, snapshot).unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["acct-a"]["deviceId"], "live-a");
        assert_eq!(value["acct-b"]["deviceId"], "live-b", "live must win");
        assert_eq!(value["acct-c"]["deviceId"], "saved-c");
    }

    #[test]
    fn device_registry_merge_ignores_a_malformed_snapshot() {
        let live = br#"{"acct-a":{"deviceId":"live-a"}}"#;
        let bytes = merge_device_registry(live, b"not json at all").unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["acct-a"]["deviceId"], "live-a");
        assert_eq!(value.as_object().unwrap().len(), 1);
    }
}
