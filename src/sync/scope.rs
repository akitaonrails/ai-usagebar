//! What gets collected, and — more importantly — what never does.
//!
//! One bounded walker, one exclusion predicate, five categories funnelling
//! through both. Owned by plan 2-01; plan 2-02 fills the three collector arms
//! it left empty and plan 2-04 fills [`super::transcripts`].
//!
//! The exclusion predicate lives *in the walker*, not in each collector. D2's
//! entries are not a size optimisation — each one is wrong to carry to another
//! machine — so a category added later inherits the rule instead of having to
//! remember it.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::config::{SyncCategory, SyncConfig};
use crate::sync::{FixedName, SyncRoots};

/// Ceiling on entries visited per walk. Mirrors [`crate::context`]: a runaway
/// tree stops and says so rather than scanning the machine. Generous enough for
/// this user's measured worst case (4110 transcripts, ~1300 session indexes).
const MAX_WALK_ENTRIES: usize = 200_000;

/// Never collected, whatever the category. See D2.
///
/// `bridge-state.json` holds a volatile remote-control session id — restoring a
/// stale one has already broken `/remote-control` here with a `session_url`
/// crash, and it is deleted on every account switch anyway.
/// `ant-device-registry.json` is a browser-extension pairing authorised
/// server-side per account; it cannot be made valid elsewhere.
///
/// [`FixedName`], not `&str`, throughout this module's vocabulary: on the
/// restore side these names are compared against strings a hostile remote
/// chose, and a byte-exact comparison admits `Bridge-State.json` on the very
/// filesystems this project's users run. The type makes that comparison a build
/// error rather than a silent hole (see [`FixedName`] and F-2).
const EXCLUDED_NAMES: [FixedName; 5] = [
    FixedName::new("bridge-state.json"),
    FixedName::new("ant-device-registry.json"),
    FixedName::new(".stale"),
    FixedName::new(".last_error"),
    FixedName::new(".fetch.lock"),
];

/// Directory names that are never descended into, and never collected.
///
/// `backups`/`prelogin-backup`/`hidden` are local rollback state whose meaning
/// is machine-specific. `local-agent-mode-sessions` is Cowork: its paths embed
/// the owning account UUID plus an unreconstructable suffix, so a copy renders
/// as an empty chat — already documented as unmigratable.
const EXCLUDED_DIRS: [FixedName; 4] = [
    FixedName::new("backups"),
    FixedName::new("prelogin-backup"),
    FixedName::new("hidden"),
    FixedName::new("local-agent-mode-sessions"),
];

/// Regenerable or in-flight. `.tmp.` is the prefix [`crate::cache::atomic_write`]
/// gives its tempfiles, so a concurrent write is never half-collected.
const EXCLUDED_SUFFIXES: [FixedName; 3] = [
    FixedName::new(".lock"),
    FixedName::new(".tmp"),
    FixedName::new("-journal"),
];
const EXCLUDED_PREFIX: FixedName = FixedName::new(".tmp.");

/// The file name that makes an entry credential-bearing whatever category it
/// was collected under — `~/.claude/.credentials.json` and every
/// `accounts/<name>/.credentials.json` beside the config.
///
/// One spelling for both directions: the [`SyncCategory::Config`] collector
/// below filters on it, and [`crate::sync::restore::merge`] decides D2's second
/// consent with it. Two copies of this literal is how one of them ends up
/// case-sensitive while the other is not.
pub(crate) const CREDENTIAL_FILE: FixedName = FixedName::new(".credentials.json");

// Names owned by claude-acc's profile store and Claude Desktop's data dir.
// They mirror the private consts in [`crate::claude_desktop`] (`META_JSON`,
// `DESKTOP_STATE`, `SESSIONS_DIR`) and `claude_desktop::merge::SCHEDULED_TASKS`.
// Restated here rather than reached for as a literal at each use site: if that
// layout moves, both blocks move.
//
// The two `config-tokenCache{,V2}` names used to be here too. They are not any
// more, and their absence is load-bearing: those files travel through
// [`crate::sync::keystore::TokenSlot`], which owns their spelling now, because
// their *bytes* are useless on another machine.
const META_JSON: &str = "meta.json";
const DESKTOP_STATE: &str = "desktop-state";
const SESSIONS_DIR: &str = "claude-code-sessions";
/// The per-account registry, one per `<account>/<org>/`.
const SCHEDULED_TASKS: &str = "scheduled-tasks.json";
/// `~/.claude/scheduled-tasks/` — Claude Code's own routine definitions, a
/// different tree from the per-account registry above.
const SCHEDULED_TASKS_DIR: &str = "scheduled-tasks";
/// A chat session index. The sibling `scheduled-tasks.json` lives in the same
/// folder but belongs to the routines category, so the two never double-count.
const SESSION_PREFIX: &str = "local_";

// The three shapes under Cursor's user-data directory that carry conversations.
// Exact names, matched by [`Path::join`] rather than by a suffix test, because
// the 33 GB file this must never admit is called `state.vscdb.bloated.bak`.
const CURSOR_GLOBAL_STORAGE: &str = "globalStorage";
const CURSOR_WORKSPACE_STORAGE: &str = "workspaceStorage";
const CURSOR_STATE_DB: &str = "state.vscdb";
const CURSOR_CONVERSATIONS_DB: &str = "conversation-search.db";

/// One collected file and the three quarters of D5's change-detection key that
/// come from its metadata; the fourth is the path itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub path: PathBuf,
    pub size: u64,
    pub mtime_ns: i128,
    pub inode: u64,
}

/// The result of scanning one category.
#[derive(Debug, Clone)]
pub struct CategoryScan {
    pub category: SyncCategory,
    pub files: Vec<FileEntry>,
    pub bytes: u64,
    /// Files dropped by a *bound*, not by an exclusion. Stays zero for every
    /// category except transcripts, whose D3 age/byte bounds leave a remainder
    /// the user needs told about. Declared here rather than by plan 2-04 so
    /// that plan owns exactly one file.
    pub excluded_files: usize,
    pub excluded_bytes: u64,
    /// The walk hit [`MAX_WALK_ENTRIES`] and stopped. Reported, never silent.
    pub walk_capped: bool,
    /// Entries whose directory or metadata could not be read.
    pub skipped: usize,
}

impl CategoryScan {
    /// A category that is switched off, unimplemented, or has no tree on disk.
    /// Also the starting point every collector accumulates into.
    pub fn empty(category: SyncCategory) -> Self {
        Self {
            category,
            files: Vec::new(),
            bytes: 0,
            excluded_files: 0,
            excluded_bytes: 0,
            walk_capped: false,
            skipped: 0,
        }
    }

    fn push(&mut self, entry: FileEntry) {
        self.bytes += entry.size;
        self.files.push(entry);
    }
}

/// D2 in full. True means "never carry this to another machine".
pub fn is_excluded(path: &Path) -> bool {
    // Any excluded directory anywhere above the entry disqualifies it. The
    // walker already refuses to descend into one, so this is belt-and-braces
    // for the paths that are added directly rather than walked to.
    if path.components().any(|c| {
        let c = c.as_os_str().to_string_lossy();
        EXCLUDED_DIRS.iter().any(|d| d.matches(&c))
    }) {
        return true;
    }
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        // A non-UTF-8 name cannot be matched against the rules above, and this
        // is a security predicate: refuse what cannot be checked.
        return true;
    };
    EXCLUDED_NAMES.iter().any(|n| n.matches(name))
        || EXCLUDED_PREFIX.is_prefix_of(name)
        || EXCLUDED_SUFFIXES.iter().any(|s| s.is_suffix_of(name))
}

/// `(size, mtime_ns, inode)` — the single place the platform split lives, so
/// D5's change-detection key has one producer.
fn stats(md: &fs::Metadata) -> (u64, i128, u64) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let mtime_ns = i128::from(md.mtime()) * 1_000_000_000 + i128::from(md.mtime_nsec());
        (md.size(), mtime_ns, md.ino())
    }
    #[cfg(not(unix))]
    {
        // No inode concept: 0 is a sentinel the comparison treats as "no
        // opinion", leaving (path, size, mtime_ns) to carry the key.
        let mtime_ns = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0i128, |d| i128::from(d.as_nanos() as u64));
        (md.len(), mtime_ns, 0)
    }
}

/// Collect one explicitly named file — the config.toml case, where there is no
/// directory to walk. Honours the same exclusion and symlink rules as the
/// walker; a missing file is simply absent.
pub(crate) fn push_path(path: &Path, out: &mut CategoryScan) {
    if is_excluded(path) {
        return;
    }
    let Ok(md) = fs::symlink_metadata(path) else {
        return;
    };
    if !md.is_file() {
        return;
    }
    let (size, mtime_ns, inode) = stats(&md);
    out.push(FileEntry {
        path: path.to_path_buf(),
        size,
        mtime_ns,
        inode,
    });
}

/// The single walker. Every category funnels through it, so [`is_excluded`]
/// cannot be bypassed by a collector added later.
///
/// A missing root is not an error — an account that has never run the Desktop
/// app simply has no tree — and symlinks are never followed, for files or
/// directories alike, so a link planted in a scanned tree cannot pull an
/// arbitrary host file into the bundle set.
pub(crate) fn walk(root: &Path, out: &mut CategoryScan) {
    walk_bounded(root, out, MAX_WALK_ENTRIES);
}

fn walk_bounded(root: &Path, out: &mut CategoryScan, max_entries: usize) {
    if !root.is_dir() {
        return;
    }
    let mut stack = vec![root.to_path_buf()];
    let mut visited = 0usize;

    'walk: while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => {
                out.skipped += 1;
                continue;
            }
        };
        for entry in entries {
            if visited >= max_entries {
                out.walk_capped = true;
                break 'walk;
            }
            visited += 1;

            let Ok(entry) = entry else {
                out.skipped += 1;
                continue;
            };
            let Ok(file_type) = entry.file_type() else {
                out.skipped += 1;
                continue;
            };
            // Never followed. `file_type` here comes from `read_dir`, which
            // does not traverse the link, so this is the symlink itself.
            if file_type.is_symlink() {
                continue;
            }
            let path = entry.path();
            if is_excluded(&path) {
                continue;
            }
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let Ok(md) = entry.metadata() else {
                out.skipped += 1;
                continue;
            };
            let (size, mtime_ns, inode) = stats(&md);
            out.push(FileEntry {
                path,
                size,
                mtime_ns,
                inode,
            });
        }
    }
}

/// Immediate subdirectories, by directory listing alone — a profile is a
/// directory, exactly as [`crate::claude_desktop::load_profiles`] already
/// defines one, and an account is a directory whether or not its profile was
/// ever captured.
///
/// A symlinked directory is not returned, on the same rule as [`walk`]:
/// `file_type` comes from `read_dir`, which does not traverse the link, so a
/// link planted in the store cannot pull an arbitrary host tree into the scan.
/// An unreadable directory reads as empty rather than as an error.
fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .map(|entry| entry.path())
        .collect()
}

/// Every `<sessions_root>/<account>/<org>/`.
fn account_org_dirs(sessions_root: &Path) -> Vec<PathBuf> {
    subdirs(sessions_root)
        .iter()
        .flat_map(|account| subdirs(account))
        .collect()
}

/// Scan one category against the injected roots.
///
/// `now` is threaded in because the transcripts arm's D3 bounds are
/// time-dependent and no test in this crate reads the wall clock. The other
/// four arms ignore it; declaring it here means plan 2-04 never edits this file.
pub fn collect(
    cat: SyncCategory,
    roots: &SyncRoots,
    cfg: &SyncConfig,
    now: DateTime<Utc>,
) -> CategoryScan {
    if !cfg.includes(cat) {
        return CategoryScan::empty(cat);
    }
    let mut scan = CategoryScan::empty(cat);
    match cat {
        SyncCategory::Config => {
            // D1: config.toml itself, plus `accounts/*/.credentials.json` —
            // the credential, not the rest of a CLAUDE_CONFIG_DIR account tree.
            walk(&roots.config_dir.join("accounts"), &mut scan);
            scan.files.retain(|f| {
                f.path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| CREDENTIAL_FILE.matches(n))
            });
            scan.bytes = scan.files.iter().map(|f| f.size).sum();
            push_path(&roots.config_file, &mut scan);
        }
        SyncCategory::Credentials => {
            // Claude Code's own OAuth credential, where it is a file — which is
            // every platform except macOS, whose recent builds keep it in the
            // login Keychain instead and whose copy therefore travels as a
            // [`crate::sync::keystore`] entry rather than as bytes. Absent, this
            // is a no-op, so one arm covers both.
            //
            // It is in *this* category and not `Config` so that the D-04
            // private-repo gate — which asks `includes(Credentials)` — refuses a
            // public repository over it, and so that switching `credentials` off
            // leaves it behind like every other secret here.
            push_path(
                &roots.claude_home.join(CREDENTIAL_FILE.to_string()),
                &mut scan,
            );
            // D1: per profile, `meta.json` and `desktop-state/` — and nothing
            // else in the store. A profile without a readable meta.json is
            // skipped rather than failing the other accounts, exactly as
            // `load_profiles` already treats one.
            //
            // **The two `config-tokenCache{,V2}` files are deliberately absent
            // here.** They are Chromium safeStorage ciphertext under a key in
            // *this* Mac's login Keychain, so their bytes are inert on any
            // other machine; they travel through
            // [`crate::sync::keystore::Store::DesktopTokenCache`] instead,
            // decrypted on push and re-sealed under the target's own key on
            // restore. Collecting them here as well would put two carriers on
            // one credential — and two carriers for one thing is two carriers
            // that disagree, which is this milestone's most expensive shape.
            for profile in subdirs(&roots.desktop_profiles_dir) {
                let meta = profile.join(META_JSON);
                if !meta.is_file() {
                    continue;
                }
                push_path(&meta, &mut scan);
                walk(&profile.join(DESKTOP_STATE), &mut scan);
            }
        }
        SyncCategory::Routines => {
            // D1: Claude Code's own routine definitions, plus each account's
            // registry. The account and org levels are enumerated by listing,
            // not from a profile's meta.json, so an account that was never
            // captured still has its routines carried.
            walk(&roots.claude_home.join(SCHEDULED_TASKS_DIR), &mut scan);
            for org in account_org_dirs(&roots.desktop_data_dir.join(SESSIONS_DIR)) {
                push_path(&org.join(SCHEDULED_TASKS), &mut scan);
            }
        }
        SyncCategory::ChatIndex => {
            // D1: `claude-code-sessions/<account>/<org>/local_*.json`. Walking
            // the root and filtering by name keeps `local-agent-mode-sessions/`
            // on the shared D2 predicate — a Cowork transcript's path embeds an
            // unreconstructable account suffix, so a copy renders as an empty
            // chat. No file body is read; a stat per entry is the whole cost.
            walk(&roots.desktop_data_dir.join(SESSIONS_DIR), &mut scan);
            scan.files.retain(|f| {
                f.path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(SESSION_PREFIX) && n.ends_with(".json"))
            });
            scan.bytes = scan.files.iter().map(|f| f.size).sum();
        }
        // Owned by plan 2-04, plus Cursor's conversation stores, which are the
        // same kind of thing under the same opt-in switch.
        SyncCategory::Transcripts => {
            let mut scan = super::transcripts::collect_bounded(roots, cfg, now);
            collect_cursor(roots, &mut scan);
            return scan;
        }
    }
    scan
}

/// Cursor's conversations, as an **allow-list of three shapes**.
///
/// # Why an allow-list, and why no walk
///
/// Measured on this user's Mac, `~/Library/Application Support/Cursor` is
/// **37 GB**. The part worth carrying is these three shapes; the rest is
/// derived, rebuildable or stale — a 33 GB `state.vscdb.bloated.bak` Cursor
/// left behind, 1.6 GB of agent-worker data, 688 MB of file-edit `History`,
/// and about 6 GB of caches. A deny-list that missed `.bloated.bak` would have
/// multiplied the bundle fifteen-fold for nothing, and would silently admit
/// whatever Cursor adds next. So this names what travels, and everything else
/// is excluded by not being named.
///
/// That also means **no directory walk**: three explicit files plus one
/// listing of `workspaceStorage`. Nothing here can hit
/// [`MAX_WALK_ENTRIES`] and quietly truncate a user's chat history at some
/// count — the 197 workspaces measured on that Mac are 197 [`push_path`] calls,
/// not 197 subtrees.
///
/// # Ceiling
///
/// ponytail: the `-wal` and `-shm` sidecars are not carried. A database
/// copied while Cursor is running can therefore be missing whatever is still
/// only in its write-ahead log — the same exposure every other SQLite file in
/// this bundle already has. Quitting Cursor before a push checkpoints them.
fn collect_cursor(roots: &SyncRoots, scan: &mut CategoryScan) {
    let global = roots.cursor_user_dir.join(CURSOR_GLOBAL_STORAGE);
    // Global editor state — where `composerData:*` chat bubbles live, and the
    // same file the `cursorAuth/*` rows come out of.
    push_path(&global.join(CURSOR_STATE_DB), scan);
    // The conversation index.
    push_path(&global.join(CURSOR_CONVERSATIONS_DB), scan);
    // Per-workspace chat: `workspaceStorage/<hash>/state.vscdb`, one level deep
    // and by exact name, so a sibling `.bloated.bak` is not a candidate.
    for workspace in subdirs(&roots.cursor_user_dir.join(CURSOR_WORKSPACE_STORAGE)) {
        push_path(&workspace.join(CURSOR_STATE_DB), scan);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn seed(dir: &Path, rel: &str, body: &str) -> PathBuf {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, body).unwrap();
        path
    }

    /// Roots that all point into one TempDir. Nothing here resolves `$HOME`.
    fn roots_at(dir: &TempDir) -> SyncRoots {
        SyncRoots::at(
            dir.path().join("config.toml"),
            dir.path().to_path_buf(),
            dir.path().join("desktop"),
            dir.path().join("profiles"),
            dir.path().join("claude-home"),
        )
    }

    /// Every spelling of `name` a case-insensitive volume treats as the same
    /// file. The last two are the non-ASCII folds: U+212A KELVIN SIGN lowercases
    /// to `k`, and U+017F LATIN SMALL LETTER LONG S folds to `s` — both of which
    /// appear in the lists above.
    fn every_spelling(name: &str) -> Vec<String> {
        let capitalised = {
            let mut c = name.chars();
            c.next()
                .map(|f| f.to_uppercase().to_string() + c.as_str())
                .unwrap_or_default()
        };
        let alternating: String = name
            .chars()
            .enumerate()
            .map(|(i, c)| {
                if i % 2 == 0 {
                    c.to_ascii_uppercase()
                } else {
                    c
                }
            })
            .collect();
        vec![
            name.to_string(),
            name.to_uppercase(),
            capitalised,
            alternating,
            name.replace('k', "\u{212a}"),
            name.replace('s', "\u{17f}"),
        ]
    }

    /// F-2, and the net that keeps it fixed: D2's lists are matched the way the
    /// filesystem matches them, in **every** spelling, for **every** entry.
    ///
    /// It iterates the constants rather than a hand-written fixture list, so a
    /// name added to any of them is covered without anyone remembering — which
    /// is the failure mode that produced this finding. A byte-exact comparison
    /// reintroduced anywhere in [`is_excluded`] fails here; a byte-exact
    /// comparison against a [`FixedName`] does not compile at all.
    #[test]
    fn every_machine_bound_name_is_excluded_in_every_spelling_the_filesystem_folds() {
        for name in EXCLUDED_NAMES {
            for spelling in every_spelling(&name.to_string()) {
                let path = PathBuf::from("account").join(&spelling);
                assert!(
                    is_excluded(&path),
                    "{spelling:?} is {name} on a case-insensitive volume, and was collected"
                );
            }
        }
        for dir in EXCLUDED_DIRS {
            for spelling in every_spelling(&dir.to_string()) {
                let path = PathBuf::from("root").join(&spelling).join("deep/x.json");
                assert!(
                    is_excluded(&path),
                    "{spelling:?} is the {dir} directory, and something under it was collected"
                );
            }
        }
        for suffix in EXCLUDED_SUFFIXES {
            for spelling in every_spelling(&suffix.to_string()) {
                let path = PathBuf::from(format!("root/index{spelling}"));
                assert!(is_excluded(&path), "index{spelling} was collected");
            }
        }
        for spelling in every_spelling(&EXCLUDED_PREFIX.to_string()) {
            let path = PathBuf::from(format!("root/{spelling}credentials"));
            assert!(is_excluded(&path), "{spelling}credentials was collected");
        }
    }

    /// A fixed name not written in its own folded spelling is a dead entry in a
    /// security list. Checked over the constants themselves.
    #[test]
    fn every_fixed_name_is_written_in_its_own_folded_spelling() {
        let all = EXCLUDED_NAMES
            .iter()
            .chain(EXCLUDED_DIRS.iter())
            .chain(EXCLUDED_SUFFIXES.iter())
            .chain([EXCLUDED_PREFIX, CREDENTIAL_FILE].iter());
        for name in all {
            assert!(
                name.is_folded(),
                "{name} can never match: it is not written in its own folded spelling"
            );
        }
    }

    /// The widening is deliberate, and it stops where D2 stops: an ordinary
    /// file whose name merely resembles an excluded one is still collected.
    #[test]
    fn a_name_that_is_not_one_of_the_excluded_ones_is_still_collected() {
        for kept in [
            "root/bridge-state.json.bak",
            "root/my-bridge-state.json",
            "root/backups-of-mine/x.json",
            "root/.stale-notes",
        ] {
            assert!(!is_excluded(Path::new(kept)), "{kept} was dropped");
        }
    }

    fn names(scan: &CategoryScan) -> Vec<String> {
        let mut v: Vec<String> = scan
            .files
            .iter()
            .map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn a_file_under_the_root_is_collected_with_size_mtime_and_inode() {
        let dir = TempDir::new().unwrap();
        seed(dir.path(), "a/one.json", "hello");
        let mut scan = CategoryScan::empty(SyncCategory::Config);
        walk(dir.path(), &mut scan);

        assert_eq!(scan.files.len(), 1);
        assert_eq!(scan.files[0].size, 5);
        assert_eq!(scan.bytes, 5);
        assert!(scan.files[0].mtime_ns > 0);
        #[cfg(unix)]
        assert!(scan.files[0].inode > 0);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_to_a_file_outside_the_root_contributes_nothing() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let secret = seed(outside.path(), "id_rsa", "PRIVATE KEY");
        fs::create_dir_all(dir.path().join("a")).unwrap();
        symlink(&secret, dir.path().join("a/linked.json")).unwrap();

        let mut scan = CategoryScan::empty(SyncCategory::Config);
        walk(dir.path(), &mut scan);
        assert!(scan.files.is_empty(), "{:?}", scan.files);
        assert_eq!(scan.bytes, 0);
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_symlink_is_not_descended_into() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        seed(outside.path(), "deep/secret.json", "{}");
        symlink(outside.path(), dir.path().join("elsewhere")).unwrap();

        let mut scan = CategoryScan::empty(SyncCategory::Config);
        walk(dir.path(), &mut scan);
        assert!(scan.files.is_empty(), "{:?}", scan.files);
    }

    #[cfg(unix)]
    #[test]
    fn an_explicitly_named_path_that_is_a_symlink_is_refused_too() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let secret = seed(outside.path(), "id_rsa", "PRIVATE KEY");
        let link = dir.path().join("config.toml");
        symlink(&secret, &link).unwrap();

        let mut scan = CategoryScan::empty(SyncCategory::Config);
        push_path(&link, &mut scan);
        assert!(scan.files.is_empty());
    }

    #[test]
    fn every_d2_hard_excluded_name_is_rejected() {
        let dir = TempDir::new().unwrap();
        for name in EXCLUDED_NAMES {
            seed(dir.path(), &name.to_string(), "x");
        }
        seed(dir.path(), "keep.json", "x");

        let mut scan = CategoryScan::empty(SyncCategory::Config);
        walk(dir.path(), &mut scan);
        assert_eq!(names(&scan), vec!["keep.json"]);
    }

    #[test]
    fn every_d2_excluded_directory_component_is_rejected() {
        let dir = TempDir::new().unwrap();
        for d in EXCLUDED_DIRS {
            seed(dir.path(), &format!("{d}/inner.json"), "x");
            seed(dir.path(), &format!("nested/{d}/inner.json"), "x");
            assert!(is_excluded(
                &dir.path().join(d.to_string()).join("inner.json")
            ));
        }
        seed(dir.path(), "keep.json", "x");

        let mut scan = CategoryScan::empty(SyncCategory::Config);
        walk(dir.path(), &mut scan);
        assert_eq!(names(&scan), vec!["keep.json"]);
    }

    #[test]
    fn every_d2_suffix_and_the_atomic_write_tempfile_prefix_are_rejected() {
        let dir = TempDir::new().unwrap();
        for name in ["a.lock", "b.tmp", "state-journal", ".tmp.abc123"] {
            seed(dir.path(), name, "x");
            assert!(is_excluded(&dir.path().join(name)), "{name}");
        }
        seed(dir.path(), "keep.json", "x");

        let mut scan = CategoryScan::empty(SyncCategory::Config);
        walk(dir.path(), &mut scan);
        assert_eq!(names(&scan), vec!["keep.json"]);
    }

    #[test]
    fn a_tree_wider_than_the_cap_stops_and_says_it_was_capped() {
        let dir = TempDir::new().unwrap();
        for i in 0..10 {
            seed(dir.path(), &format!("f{i}.json"), "x");
        }
        let mut scan = CategoryScan::empty(SyncCategory::Config);
        walk_bounded(dir.path(), &mut scan, 3);

        assert!(scan.walk_capped);
        assert_eq!(scan.files.len(), 3);
    }

    #[test]
    fn a_missing_root_is_an_empty_scan_not_an_error() {
        let dir = TempDir::new().unwrap();
        let mut scan = CategoryScan::empty(SyncCategory::Config);
        walk(&dir.path().join("never-ran"), &mut scan);
        assert!(scan.files.is_empty());
        assert!(!scan.walk_capped);
    }

    #[test]
    fn the_config_category_takes_config_toml_and_account_credentials_only() {
        let dir = TempDir::new().unwrap();
        seed(dir.path(), "config.toml", "[sync]\n");
        seed(dir.path(), "accounts/work/.credentials.json", "{}");
        seed(dir.path(), "accounts/personal/.credentials.json", "{}");
        // The rest of a CLAUDE_CONFIG_DIR account tree is not ours to carry.
        seed(dir.path(), "accounts/work/history.jsonl", "{}");
        seed(dir.path(), "accounts/work/.credentials.json.lock", "");

        let scan = collect(
            SyncCategory::Config,
            &roots_at(&dir),
            &SyncConfig::default(),
            Utc::now(),
        );
        assert_eq!(
            names(&scan),
            vec![".credentials.json", ".credentials.json", "config.toml"]
        );
        assert_eq!(scan.bytes, scan.files.iter().map(|f| f.size).sum::<u64>());
    }

    fn scan_of(cat: SyncCategory, dir: &TempDir) -> CategoryScan {
        collect(cat, &roots_at(dir), &SyncConfig::default(), Utc::now())
    }

    // ---- credentials: the four D1 profile members, and nothing else --------

    /// **The two token caches are deliberately not here.** They are Chromium
    /// safeStorage ciphertext under this Mac's own key, and they travel through
    /// [`crate::sync::keystore`] — decrypted on push, re-sealed under the
    /// target's key on restore. Collecting them as files too would put two
    /// carriers on one credential.
    #[test]
    fn a_profile_yields_its_meta_and_desktop_state_but_never_the_sealed_token_caches() {
        let dir = TempDir::new().unwrap();
        seed(dir.path(), "profiles/gmail/meta.json", "{}");
        seed(dir.path(), "profiles/gmail/config-tokenCache", "x");
        seed(dir.path(), "profiles/gmail/config-tokenCacheV2", "x");
        seed(dir.path(), "profiles/gmail/desktop-state/Cookies", "x");
        seed(
            dir.path(),
            "profiles/gmail/desktop-state/Local Storage/leveldb/000003.log",
            "x",
        );
        // The rest of a profile directory is not a D1 member.
        seed(dir.path(), "profiles/gmail/config.json", "{}");

        let scan = scan_of(SyncCategory::Credentials, &dir);
        assert_eq!(names(&scan), vec!["000003.log", "Cookies", "meta.json"]);
        assert_eq!(scan.bytes, scan.files.iter().map(|f| f.size).sum::<u64>());
    }

    #[test]
    fn every_profile_is_collected_and_one_without_meta_json_does_not_fail_the_others() {
        let dir = TempDir::new().unwrap();
        // Four accounts is this user's normal case, not an edge case.
        for label in ["gmail", "hotmail", "struct", "toptal"] {
            seed(dir.path(), &format!("profiles/{label}/meta.json"), "{}");
            seed(
                dir.path(),
                &format!("profiles/{label}/config-tokenCache"),
                "x",
            );
        }
        // Hand-mangled: no readable meta.json, skipped as `load_profiles` does.
        seed(dir.path(), "profiles/broken/config-tokenCache", "x");

        let scan = scan_of(SyncCategory::Credentials, &dir);
        // Four `meta.json`, and no token cache: those are the keystore's now.
        assert_eq!(scan.files.len(), 4, "{:?}", names(&scan));
        assert!(
            !scan
                .files
                .iter()
                .any(|f| f.path.to_string_lossy().contains("broken"))
        );
    }

    #[test]
    fn bridge_state_and_the_device_registry_never_leave_a_profile() {
        let dir = TempDir::new().unwrap();
        seed(dir.path(), "profiles/gmail/meta.json", "{}");
        for name in ["bridge-state.json", "ant-device-registry.json"] {
            seed(dir.path(), &format!("profiles/gmail/{name}"), "{}");
            seed(
                dir.path(),
                &format!("profiles/gmail/desktop-state/{name}"),
                "{}",
            );
        }
        seed(dir.path(), "profiles/gmail/desktop-state/Cookies", "x");

        let scan = scan_of(SyncCategory::Credentials, &dir);
        assert_eq!(names(&scan), vec!["Cookies", "meta.json"]);
    }

    #[test]
    fn rollback_state_beside_and_inside_the_profile_store_contributes_nothing() {
        let dir = TempDir::new().unwrap();
        seed(dir.path(), "profiles/gmail/meta.json", "{}");
        // Siblings of the store: outside the only root this collector is handed.
        seed(dir.path(), "backups/gmail/config-tokenCache", "x");
        seed(dir.path(), "prelogin-backup/config-tokenCache", "x");
        // The same names inside it, where the shared D2 predicate catches them.
        seed(dir.path(), "profiles/backups/meta.json", "{}");
        seed(dir.path(), "profiles/hidden/meta.json", "{}");

        let scan = scan_of(SyncCategory::Credentials, &dir);
        assert_eq!(names(&scan), vec!["meta.json"]);
    }

    #[test]
    fn a_missing_profile_store_is_an_empty_scan_not_an_error() {
        let dir = TempDir::new().unwrap();
        let scan = scan_of(SyncCategory::Credentials, &dir);
        assert!(scan.files.is_empty());
        assert_eq!(scan.bytes, 0);
        assert_eq!(scan.category, SyncCategory::Credentials);
    }

    #[test]
    fn unchecking_credentials_scans_nothing_even_with_a_full_profile_store() {
        let dir = TempDir::new().unwrap();
        seed(dir.path(), "profiles/gmail/meta.json", "{}");
        seed(dir.path(), "profiles/gmail/config-tokenCache", "x");
        let cfg = SyncConfig {
            categories: vec![SyncCategory::Config],
            ..SyncConfig::default()
        };
        let scan = collect(SyncCategory::Credentials, &roots_at(&dir), &cfg, Utc::now());
        assert!(scan.files.is_empty());
        assert_eq!(scan.bytes, 0);
    }

    /// Where Claude Code writes its OAuth credential to a *file* — Linux, and
    /// any macOS build predating the Keychain move — that file is the credential
    /// this whole category exists for, and it used to be collected by nothing at
    /// all. The macOS Keychain half travels as a `keystore/…` entry instead; see
    /// [`crate::sync::keystore`].
    #[test]
    fn claude_codes_own_credential_file_is_collected_with_the_credentials() {
        let dir = TempDir::new().unwrap();
        seed(
            dir.path(),
            "claude-home/.credentials.json",
            "{\"claudeAiOauth\":{}}",
        );
        seed(dir.path(), "profiles/gmail/meta.json", "{}");

        let scan = scan_of(SyncCategory::Credentials, &dir);
        assert_eq!(names(&scan), vec![".credentials.json", "meta.json"]);
    }

    /// And it is off when the category is off, like every other secret here —
    /// which is also what keeps the D-04 private-repo gate honest, since that
    /// gate asks `includes(Credentials)`.
    #[test]
    fn claude_codes_credential_file_is_left_behind_when_credentials_are_off() {
        let dir = TempDir::new().unwrap();
        seed(dir.path(), "claude-home/.credentials.json", "{}");
        let cfg = SyncConfig {
            categories: vec![SyncCategory::Config, SyncCategory::Routines],
            ..SyncConfig::default()
        };
        for cat in [SyncCategory::Config, SyncCategory::Routines] {
            let scan = collect(cat, &roots_at(&dir), &cfg, Utc::now());
            assert!(
                !scan
                    .files
                    .iter()
                    .any(|f| f.path.ends_with(".credentials.json")),
                "{cat:?} carried the Claude Code credential with credentials switched off"
            );
        }
    }

    // ---- routines: ~/.claude/scheduled-tasks/** plus each account registry --

    #[test]
    fn routines_take_the_claude_home_tree_and_every_account_registry() {
        let dir = TempDir::new().unwrap();
        seed(
            dir.path(),
            "claude-home/scheduled-tasks/daily-skill-update/task.json",
            "{}",
        );
        seed(
            dir.path(),
            "claude-home/scheduled-tasks/standup/report.md",
            "x",
        );
        seed(
            dir.path(),
            "desktop/claude-code-sessions/acct-1/org-1/scheduled-tasks.json",
            "{}",
        );
        seed(
            dir.path(),
            "desktop/claude-code-sessions/acct-2/org-2/scheduled-tasks.json",
            "{}",
        );
        // Chat indexes are the other category's.
        seed(
            dir.path(),
            "desktop/claude-code-sessions/acct-1/org-1/local_abc.json",
            "{}",
        );

        let scan = scan_of(SyncCategory::Routines, &dir);
        assert_eq!(
            names(&scan),
            vec![
                "report.md",
                "scheduled-tasks.json",
                "scheduled-tasks.json",
                "task.json",
            ]
        );
    }

    #[test]
    fn a_missing_routines_tree_is_an_empty_scan_not_an_error() {
        let dir = TempDir::new().unwrap();
        let scan = scan_of(SyncCategory::Routines, &dir);
        assert!(scan.files.is_empty());
        assert_eq!(scan.bytes, 0);
    }

    // ---- chat_index: local_*.json only -------------------------------------

    #[test]
    fn the_chat_index_takes_local_session_files_from_every_account_and_nothing_else() {
        let dir = TempDir::new().unwrap();
        seed(
            dir.path(),
            "desktop/claude-code-sessions/acct-1/org-1/local_abc.json",
            "{}",
        );
        seed(
            dir.path(),
            "desktop/claude-code-sessions/acct-2/org-2/local_def.json",
            "{}",
        );
        seed(
            dir.path(),
            "desktop/claude-code-sessions/acct-1/org-1/scheduled-tasks.json",
            "{}",
        );
        seed(
            dir.path(),
            "desktop/claude-code-sessions/acct-1/org-1/notes.json",
            "{}",
        );

        let scan = scan_of(SyncCategory::ChatIndex, &dir);
        assert_eq!(names(&scan), vec!["local_abc.json", "local_def.json"]);
        assert_eq!(scan.bytes, scan.files.iter().map(|f| f.size).sum::<u64>());

        let paths: Vec<String> = scan
            .files
            .iter()
            .map(|f| f.path.to_string_lossy().into_owned())
            .collect();
        for keyed in ["acct-1", "org-1", "acct-2", "org-2"] {
            assert!(
                paths.iter().any(|p| p.contains(keyed)),
                "{keyed}: {paths:?}"
            );
        }
    }

    #[test]
    fn a_cowork_local_agent_mode_sessions_tree_contributes_nothing() {
        let dir = TempDir::new().unwrap();
        seed(
            dir.path(),
            "desktop/claude-code-sessions/acct-1/org-1/local_keep.json",
            "{}",
        );
        // At the account level and below it — the shared predicate is
        // component-level, so both are refused without a second rule here.
        seed(
            dir.path(),
            "desktop/claude-code-sessions/local-agent-mode-sessions/acct-1/local_cowork.json",
            "{}",
        );
        seed(
            dir.path(),
            "desktop/claude-code-sessions/acct-1/local-agent-mode-sessions/local_cowork.json",
            "{}",
        );

        let scan = scan_of(SyncCategory::ChatIndex, &dir);
        assert_eq!(names(&scan), vec!["local_keep.json"]);
    }

    #[test]
    fn a_missing_sessions_root_is_an_empty_chat_index_scan() {
        let dir = TempDir::new().unwrap();
        let scan = scan_of(SyncCategory::ChatIndex, &dir);
        assert!(scan.files.is_empty());
        assert_eq!(scan.bytes, 0);
        assert_eq!(scan.category, SyncCategory::ChatIndex);
    }

    #[test]
    fn an_account_registry_lands_in_routines_and_never_in_the_chat_index() {
        let dir = TempDir::new().unwrap();
        seed(
            dir.path(),
            "desktop/claude-code-sessions/acct-1/org-1/scheduled-tasks.json",
            "{}",
        );
        seed(
            dir.path(),
            "desktop/claude-code-sessions/acct-1/org-1/local_abc.json",
            "{}",
        );

        assert_eq!(
            names(&scan_of(SyncCategory::Routines, &dir)),
            vec!["scheduled-tasks.json"]
        );
        assert_eq!(
            names(&scan_of(SyncCategory::ChatIndex, &dir)),
            vec!["local_abc.json"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_profile_or_account_directory_is_not_entered() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        seed(outside.path(), "meta.json", "{}");
        seed(outside.path(), "config-tokenCache", "x");
        seed(outside.path(), "org-1/scheduled-tasks.json", "{}");

        fs::create_dir_all(dir.path().join("profiles")).unwrap();
        symlink(outside.path(), dir.path().join("profiles/linked")).unwrap();
        assert!(scan_of(SyncCategory::Credentials, &dir).files.is_empty());

        let sessions = dir.path().join("desktop/claude-code-sessions");
        fs::create_dir_all(&sessions).unwrap();
        symlink(outside.path(), sessions.join("acct-1")).unwrap();
        assert!(scan_of(SyncCategory::Routines, &dir).files.is_empty());
    }

    #[test]
    fn a_category_absent_from_the_configured_set_scans_nothing() {
        let dir = TempDir::new().unwrap();
        seed(dir.path(), "config.toml", "[sync]\n");
        let cfg = SyncConfig {
            categories: vec![SyncCategory::Routines],
            ..SyncConfig::default()
        };
        let scan = collect(SyncCategory::Config, &roots_at(&dir), &cfg, Utc::now());
        assert!(scan.files.is_empty());
        assert_eq!(scan.bytes, 0);
        assert_eq!(scan.category, SyncCategory::Config);
    }

    // ---- cursor: three named shapes out of a 37 GB directory ---------------

    /// **The allow-list, measured.** On this user's Mac the Cursor directory is
    /// 37 GB and the part worth carrying is about 1.2 GB. Everything named here
    /// that is *not* collected was measured on that machine, and a deny-list
    /// that had missed any one of them would have multiplied the bundle.
    #[test]
    fn cursor_carries_the_three_conversation_shapes_and_nothing_else() {
        let dir = TempDir::new().unwrap();
        let user = "cursor-user";
        seed(
            dir.path(),
            &format!("{user}/globalStorage/state.vscdb"),
            "x",
        );
        seed(
            dir.path(),
            &format!("{user}/globalStorage/conversation-search.db"),
            "x",
        );
        for ws in ["9f2c", "aa11", "bb22"] {
            seed(
                dir.path(),
                &format!("{user}/workspaceStorage/{ws}/state.vscdb"),
                "x",
            );
        }

        // 33 GB of stale backup Cursor left behind — the single entry that
        // makes this an allow-list rather than a deny-list.
        seed(
            dir.path(),
            &format!("{user}/globalStorage/state.vscdb.bloated.bak"),
            "x",
        );
        // 1.6 GB of derived worker data, and a 688 MB rebuildable edit history.
        seed(
            dir.path(),
            &format!("{user}/globalStorage/anysphere.cursor-agent-worker/index.bin"),
            "x",
        );
        seed(
            dir.path(),
            &format!("{user}/History/1a2b/entries.json"),
            "x",
        );
        // ~6 GB of caches.
        for junk in [
            "CachedData/x.code",
            "GPUCache/data_0",
            "logs/main.log",
            "snapshots/s1.bin",
            "WebStorage/1/x.db",
        ] {
            seed(dir.path(), &format!("{user}/{junk}"), "x");
        }
        // Per-workspace clutter beside the one file that is wanted.
        seed(
            dir.path(),
            &format!("{user}/workspaceStorage/9f2c/anysphere.cursor-retrieval/index"),
            "x",
        );

        let cfg = SyncConfig {
            categories: vec![SyncCategory::Transcripts],
            ..SyncConfig::default()
        };
        let scan = collect(SyncCategory::Transcripts, &roots_at(&dir), &cfg, Utc::now());
        let mut got: Vec<String> = scan
            .files
            .iter()
            .map(|f| {
                f.path
                    .strip_prefix(dir.path().join(user))
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        got.sort();
        assert_eq!(
            got,
            vec![
                "globalStorage/conversation-search.db",
                "globalStorage/state.vscdb",
                "workspaceStorage/9f2c/state.vscdb",
                "workspaceStorage/aa11/state.vscdb",
                "workspaceStorage/bb22/state.vscdb",
            ]
        );
        // No walk was involved, so no count of workspaces can silently truncate.
        assert!(!scan.walk_capped);
    }

    /// Conversations are transcripts, and transcripts are opt-in. A user who
    /// has not asked for them does not get 1.2 GB of Cursor state.
    #[test]
    fn cursor_conversations_do_not_travel_unless_transcripts_are_switched_on() {
        let dir = TempDir::new().unwrap();
        seed(dir.path(), "cursor-user/globalStorage/state.vscdb", "x");
        assert!(
            !SyncConfig::default().includes(SyncCategory::Transcripts),
            "the default set is what makes this opt-in"
        );
        let scan = collect(
            SyncCategory::Transcripts,
            &roots_at(&dir),
            &SyncConfig::default(),
            Utc::now(),
        );
        assert!(scan.files.is_empty(), "{:?}", names(&scan));
    }
}
