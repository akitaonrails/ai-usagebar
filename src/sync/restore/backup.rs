//! The archive taken before the first restore write.
//!
//! It is the last line of defence (D3, SAFE-04), so it is taken even for a
//! partial restore and even under `--force` — `--force` is exactly when it is
//! needed. A backup that cannot be taken aborts the restore rather than
//! warning.
//!
//! `Ok(None)` means there was genuinely nothing on disk to preserve: a restore
//! onto an empty machine, which is the milestone's headline case, has nothing
//! to undo. It never means "an archive was skipped" — **nothing archived
//! implies nothing overwritten** is the property [`take`] holds, and the only
//! two exits from this function are `Ok(None)` with no member on disk and
//! `Ok(Some(..))` with a closed, complete, mode-0600 tarball. There is no third
//! state where a write may proceed.
//!
//! # Why the archive is complete before `take` returns
//!
//! [`take`] runs `tar` to completion, checks its exit status, chmods the result
//! and stats it — all before it hands a [`BackupRecord`] back. `run` calls it
//! at step 5 and [`write::apply`](super::write::apply) at step 6, so a restore
//! that dies half-way — or is killed outright — leaves a finished archive of
//! everything it could have touched, not a truncated one. That ordering is what
//! the round-trip test in this module pins down: it archives a seeded tree,
//! clobbers it exactly as a restore would, runs the rendered rollback command,
//! and compares contents *and* modes.
//!
//! Plan 5-01 filled the signature and the nothing-to-preserve arm. Plan 5-05
//! owns the archive: `~/.claude-acc/backups/sync-restore-<stamp>.tar.gz` at
//! mode 0600 inside a mode 0700 directory, rooted at the user's home so one
//! `-C` covers all four sync roots, through an injected `tar` program path.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{AppError, Result};
use crate::sync::SyncRoots;

use super::{BackupRecord, RestoreCtx};

/// A fixed path, never resolved off `PATH` (T-5-42) — the same reasoning
/// [`claude_desktop::app`](crate::claude_desktop::app) applies to `tar` and
/// [`anthropic::keychain`](crate::anthropic::keychain) to `security(1)`.
const TAR: &str = "/usr/bin/tar";

/// Preserve exactly the paths the restore is about to write.
///
/// `targets` comes from the plan, so this structurally cannot archive something
/// the restore will not touch (T-5-46).
pub fn take(ctx: &RestoreCtx<'_>, targets: &[PathBuf]) -> Result<Option<BackupRecord>> {
    take_with(ctx, targets, Path::new(TAR))
}

/// Test seam. Production always passes [`TAR`]; the CLI has no way to reach
/// this, which is what keeps T-5-42 closed.
pub(crate) fn take_with(
    ctx: &RestoreCtx<'_>,
    targets: &[PathBuf],
    tar: &Path,
) -> Result<Option<BackupRecord>> {
    // Only what is already on disk: a `Create` has nothing to preserve, and an
    // empty tarball would be a misleading artifact suggesting otherwise.
    let mut present: Vec<&PathBuf> = targets.iter().filter(|p| p.exists()).collect();
    present.sort();
    present.dedup();
    if present.is_empty() {
        return Ok(None);
    }

    let root = archive_root(ctx.roots, &present);
    let mut members = Vec::with_capacity(present.len());
    for target in &present {
        let rel = target.strip_prefix(&root).map_err(|_| {
            AppError::Other(format!(
                "cannot back up {}: it is not beneath the archive root {}",
                target.display(),
                root.display()
            ))
        })?;
        members.push(rel.to_path_buf());
    }
    // Sorted and deduplicated so two runs over the same tree produce the same
    // member list, which is what lets a test compare them.
    members.sort();
    members.dedup();

    let dir = ctx.backups_dir;
    std::fs::create_dir_all(dir).map_err(|error| AppError::io_at(dir, error))?;
    // Before `tar` creates the file, so even the pre-chmod window is contained
    // (T-5-40). This archive holds credentials in the clear by design.
    set_private_mode(dir, 0o700)?;

    // The account switcher's naming, in the account switcher's directory, so a
    // user has one place to look for undo (D3). The stamp is `ctx.now`, never
    // the wall clock — see the module note in the summary on why
    // `claude_desktop::timestamp` is not reused.
    let archive = dir.join(format!(
        "sync-restore-{}.tar.gz",
        ctx.now.format("%Y%m%d-%H%M%S")
    ));

    let output = Command::new(tar)
        .arg("-czf")
        .arg(&archive)
        .arg("-C")
        .arg(&root)
        // A member beginning with a dash is never read as a flag (T-5-41).
        .arg("--")
        .args(&members)
        .output()
        .map_err(|error| AppError::Other(format!("could not run `tar`: {error}")))?;

    if !output.status.success() {
        // The caller must not proceed to write (T-5-45).
        return Err(AppError::Other(format!(
            "could not write the pre-restore backup {} (tar exited {}): {}",
            archive.display(),
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    set_private_mode(&archive, 0o600)?;
    let bytes = std::fs::metadata(&archive)
        .map_err(|error| AppError::io_at(&archive, error))?
        .len();

    Ok(Some(BackupRecord {
        archive,
        root,
        members: members.len(),
        bytes,
    }))
}

/// `tar -xzf <archive> -C <root>`, with both paths rendered as one safe shell
/// argument each.
///
/// [`BackupRecord::rollback_command`] delegates here: the string is only useful
/// if it survives a home directory with a space in it, which is ordinary on
/// macOS, and a rollback command that breaks on one is a rollback command that
/// does not exist when it is needed (T-5-43).
pub(crate) fn rollback_command(record: &BackupRecord) -> String {
    format!(
        "tar -xzf {} -C {}",
        shell_quote(&record.archive),
        shell_quote(&record.root)
    )
}

/// One shell word. Ordinary paths stay bare; anything else is single-quoted,
/// with embedded single quotes closed-escaped-reopened the standard way.
fn shell_quote(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let safe = |c: char| c.is_ascii_alphanumeric() || "-_./=:+,@%".contains(c);
    if !raw.is_empty() && raw.chars().all(safe) {
        return raw.into_owned();
    }
    format!("'{}'", raw.replace('\'', r"'\''"))
}

/// What the archive's members are relative to.
///
/// The user's home on a real install — the four sync roots all sit beneath it,
/// and one `-C` root means one rollback command rather than four. A customised
/// `CLAUDE_CONFIG_DIR` elsewhere is supported, so when a target escapes that
/// ancestor the root falls back to the targets' own longest common ancestor.
/// The chosen root travels in the record because the printed command must name
/// the same root the archive was created with or it silently restores into the
/// wrong place.
fn archive_root(roots: &SyncRoots, targets: &[&PathBuf]) -> PathBuf {
    // `config_file` is a child of `config_dir`, and `index_file` lives in the
    // cache dir — outside the home — so neither belongs in this set.
    let home = common_ancestor(
        [
            roots.config_dir.as_path(),
            roots.desktop_data_dir.as_path(),
            roots.desktop_profiles_dir.as_path(),
            roots.claude_home.as_path(),
        ]
        .into_iter(),
    );
    if let Some(home) = home
        && targets.iter().all(|target| target.starts_with(&home))
    {
        return home;
    }
    common_ancestor(targets.iter().filter_map(|target| target.parent()))
        .unwrap_or_else(|| PathBuf::from(std::path::MAIN_SEPARATOR_STR))
}

/// The longest path prefix every input shares, compared component-wise so
/// `/home/bo` is never treated as an ancestor of `/home/bob`.
fn common_ancestor<'a>(paths: impl Iterator<Item = &'a Path>) -> Option<PathBuf> {
    let mut paths = paths;
    let mut shared: Vec<_> = paths.next()?.components().collect();
    for path in paths {
        let kept = shared
            .iter()
            .zip(path.components())
            .take_while(|(a, b)| **a == *b)
            .count();
        shared.truncate(kept);
    }
    if shared.is_empty() {
        None
    } else {
        Some(shared.iter().collect())
    }
}

#[cfg(unix)]
fn set_private_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|error| AppError::io_at(path, error))
}

#[cfg(not(unix))]
fn set_private_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::github::token::TokenSource;
    use crate::sync::github::{Client, Endpoints, RepoRef};
    use chrono::{DateTime, Utc};
    use std::fs;
    use tempfile::TempDir;
    use zeroize::Zeroizing;

    const NOW: DateTime<Utc> = match DateTime::from_timestamp(1_700_000_000, 0) {
        Some(t) => t,
        None => panic!("a fixed timestamp"),
    };
    /// `NOW` rendered the way the archive name renders it.
    const STAMP: &str = "20231114-221320";

    /// Everything a [`RestoreCtx`] needs, all of it inside one `TempDir`.
    struct Fixture {
        dir: TempDir,
        roots: SyncRoots,
        backups_dir: PathBuf,
        anchor_path: PathBuf,
        repo: RepoRef,
        client: Client,
        passphrase: Zeroizing<String>,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = TempDir::new().unwrap();
            let home = dir.path().join("bob");
            let roots = SyncRoots::at(
                home.join(".config/ai-usagebar/config.toml"),
                home.join(".config/ai-usagebar"),
                home.join("Library/Application Support/Claude"),
                home.join(".claude-acc/profiles"),
                home.join(".claude"),
            );
            Self {
                backups_dir: home.join(".claude-acc/backups"),
                anchor_path: dir.path().join("anchor.json"),
                roots,
                repo: RepoRef::parse("o/n").unwrap(),
                // Never used: this module makes no request. It exists because
                // `RestoreCtx` borrows one.
                client: Client::new(
                    &Endpoints {
                        api_base: "http://127.0.0.1:1/unused".into(),
                        uploads_base: "http://127.0.0.1:1/unused".into(),
                    },
                    Zeroizing::new("github_pat_fixture_not_a_real_token".into()),
                    TokenSource::Env,
                )
                .unwrap(),
                passphrase: Zeroizing::new("correct horse battery staple".into()),
                dir,
            }
        }

        fn home(&self) -> PathBuf {
            self.dir.path().join("bob")
        }

        fn ctx(&self) -> RestoreCtx<'_> {
            RestoreCtx {
                client: &self.client,
                repo: &self.repo,
                roots: &self.roots,
                repo_id: "github:1",
                passphrase: &self.passphrase,
                anchor_path: &self.anchor_path,
                backups_dir: &self.backups_dir,
                opts: Default::default(),
                now: NOW,
            }
        }

        /// Seed a file beneath the home at `mode`, creating its parents.
        fn seed(&self, rel: &str, body: &[u8], mode: u32) -> PathBuf {
            let path = self.home().join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, body).unwrap();
            set_private_mode(&path, mode).unwrap();
            path
        }
    }

    /// A `tar` stand-in: records its argv one line per argument, creates the
    /// archive named by `$2` so the caller's stat succeeds, and exits `code`.
    /// Nothing here reaches the real binary.
    fn recorder(dir: &Path, log: &Path, code: i32, stderr: &str) -> PathBuf {
        let program = dir.join(format!("tar-recorder-{code}.sh"));
        fs::write(
            &program,
            format!(
                "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > '{log}'\n\
                 : > \"$2\"\nprintf '%s' '{stderr}' >&2\nexit {code}\n",
                log = log.display(),
            ),
        )
        .unwrap();
        set_private_mode(&program, 0o755).unwrap();
        program
    }

    fn recorded(log: &Path) -> Vec<String> {
        fs::read_to_string(log)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// The one thing this module must never do is return `None` for a tree it
    /// simply did not archive — so `None` is reachable only when no target is
    /// on disk, and then no `tar` runs at all.
    #[test]
    fn nothing_on_disk_means_no_archive_and_no_tar() {
        let fixture = Fixture::new();
        let log = fixture.dir.path().join("argv");
        let tar = recorder(fixture.dir.path(), &log, 0, "");

        let targets = vec![fixture.home().join(".claude/projects/a.jsonl")];
        let record = take_with(&fixture.ctx(), &targets, &tar).unwrap();

        assert!(record.is_none(), "a create-only plan produced an archive");
        assert!(!log.exists(), "tar ran for a plan with nothing to preserve");
        assert!(!fixture.backups_dir.exists());
    }

    #[test]
    fn an_empty_target_list_takes_no_archive() {
        let fixture = Fixture::new();
        assert!(
            take_with(&fixture.ctx(), &[], Path::new("/nonexistent"))
                .unwrap()
                .is_none()
        );
    }

    /// Name, directory, root and argv in one assertion, because they are one
    /// contract: the printed rollback command has to name the same root the
    /// archive was created with.
    #[test]
    fn the_archive_follows_the_switchers_naming_and_the_argv_is_flag_safe() {
        let fixture = Fixture::new();
        let log = fixture.dir.path().join("argv");
        let tar = recorder(fixture.dir.path(), &log, 0, "");

        let targets = vec![
            fixture.seed(".claude/projects/a.jsonl", b"a", 0o600),
            fixture.seed(".config/ai-usagebar/config.toml", b"c", 0o600),
        ];
        let record = take_with(&fixture.ctx(), &targets, &tar)
            .unwrap()
            .expect("an existing tree must be archived");

        let expected = fixture
            .backups_dir
            .join(format!("sync-restore-{STAMP}.tar.gz"));
        assert_eq!(record.archive, expected);
        assert_eq!(record.root, fixture.home(), "the -C root is not the home");
        assert_eq!(record.members, 2);

        assert_eq!(
            recorded(&log),
            vec![
                "-czf".to_string(),
                expected.display().to_string(),
                "-C".to_string(),
                fixture.home().display().to_string(),
                "--".to_string(),
                ".claude/projects/a.jsonl".to_string(),
                ".config/ai-usagebar/config.toml".to_string(),
            ],
            "`--` must precede the members and the members must be relative"
        );
    }

    #[test]
    fn members_are_existing_only_deduplicated_and_sorted() {
        let fixture = Fixture::new();
        let log = fixture.dir.path().join("argv");
        let tar = recorder(fixture.dir.path(), &log, 0, "");

        let zed = fixture.seed(".claude/z.jsonl", b"z", 0o600);
        let amy = fixture.seed(".claude/a.jsonl", b"a", 0o600);
        let targets = vec![
            zed.clone(),
            amy.clone(),
            zed,
            amy,
            // Never written yet: a `Create` contributes no member.
            fixture.home().join(".claude/absent.jsonl"),
        ];

        let record = take_with(&fixture.ctx(), &targets, &tar).unwrap().unwrap();
        assert_eq!(record.members, 2);
        assert_eq!(
            &recorded(&log)[5..],
            &[".claude/a.jsonl".to_string(), ".claude/z.jsonl".to_string()]
        );
    }

    /// T-5-45: a backup that cannot be taken is not advisory. `take` returns
    /// `Err`, so `run` never reaches `write::apply`.
    #[test]
    fn a_tar_failure_is_an_error_carrying_its_stderr() {
        let fixture = Fixture::new();
        let log = fixture.dir.path().join("argv");
        let tar = recorder(fixture.dir.path(), &log, 2, "tar: disk full");

        let targets = vec![fixture.seed(".claude/a.jsonl", b"a", 0o600)];
        let error = take_with(&fixture.ctx(), &targets, &tar)
            .expect_err("a failed backup must abort the restore");
        let text = error.to_string();
        assert!(
            text.contains("tar: disk full"),
            "stderr was dropped: {text}"
        );
        assert!(
            text.contains("tar exited 2"),
            "the exit code was dropped: {text}"
        );
    }

    #[test]
    fn a_missing_tar_is_an_error_not_a_skipped_backup() {
        let fixture = Fixture::new();
        let targets = vec![fixture.seed(".claude/a.jsonl", b"a", 0o600)];
        let error = take_with(
            &fixture.ctx(),
            &targets,
            &fixture.dir.path().join("no-such-tar"),
        )
        .expect_err("an unrunnable tar must abort the restore");
        assert!(error.to_string().contains("could not run `tar`"));
    }

    /// T-5-40. The directory is restricted *before* `tar` creates the file, so
    /// there is no window in which a credential archive is world-readable.
    #[test]
    fn the_archive_is_0600_inside_a_0700_directory() {
        let fixture = Fixture::new();
        let log = fixture.dir.path().join("argv");
        let tar = recorder(fixture.dir.path(), &log, 0, "");

        let targets = vec![fixture.seed(".claude/a.jsonl", b"a", 0o600)];
        let record = take_with(&fixture.ctx(), &targets, &tar).unwrap().unwrap();

        assert_eq!(mode_of(&fixture.backups_dir), 0o700);
        assert_eq!(mode_of(&record.archive), 0o600);
    }

    /// A customised `CLAUDE_CONFIG_DIR` outside the home is supported, and the
    /// archive root has to follow the targets rather than silently dropping
    /// one — or extracting the rollback would put files in the wrong place.
    #[test]
    fn a_target_outside_the_home_widens_the_root_to_the_common_ancestor() {
        let fixture = Fixture::new();
        let log = fixture.dir.path().join("argv");
        let tar = recorder(fixture.dir.path(), &log, 0, "");

        let inside = fixture.seed(".claude/a.jsonl", b"a", 0o600);
        let outside = fixture.dir.path().join("elsewhere/config.toml");
        fs::create_dir_all(outside.parent().unwrap()).unwrap();
        fs::write(&outside, b"c").unwrap();

        let record = take_with(&fixture.ctx(), &[inside, outside], &tar)
            .unwrap()
            .unwrap();

        assert_eq!(record.root, fixture.dir.path());
        assert_eq!(
            &recorded(&log)[5..],
            &[
                "bob/.claude/a.jsonl".to_string(),
                "elsewhere/config.toml".to_string()
            ]
        );
    }

    /// T-5-43. A home with a space in it is ordinary on macOS; a quote and a
    /// dollar sign are the other two ways a naive command changes meaning.
    #[test]
    fn the_rollback_command_quotes_awkward_paths_into_one_argument_each() {
        let plain = BackupRecord {
            archive: PathBuf::from("/home/bob/.claude-acc/backups/sync-restore-20231114.tar.gz"),
            root: PathBuf::from("/home/bob"),
            members: 3,
            bytes: 1024,
        };
        assert_eq!(
            plain.rollback_command(),
            "tar -xzf /home/bob/.claude-acc/backups/sync-restore-20231114.tar.gz -C /home/bob",
            "an ordinary path must not grow quotes it does not need"
        );

        let awkward = BackupRecord {
            archive: PathBuf::from("/Users/o'brien $HOME/My Backups/sync-restore.tar.gz"),
            root: PathBuf::from("/Users/o'brien $HOME"),
            members: 1,
            bytes: 1,
        };
        assert_eq!(
            awkward.rollback_command(),
            "tar -xzf '/Users/o'\\''brien $HOME/My Backups/sync-restore.tar.gz' \
             -C '/Users/o'\\''brien $HOME'"
        );
    }

    /// Where an unusual host is missing either binary, skip rather than fail:
    /// the AUR `check()` runs this suite on an installer's machine.
    fn round_trip_tools() -> Option<(PathBuf, PathBuf)> {
        let tar = PathBuf::from(TAR);
        let sh = PathBuf::from("/bin/sh");
        (tar.exists() && sh.exists()).then_some((tar, sh))
    }

    /// The only assertion that proves SAFE-04. A rollback command that is
    /// well-formed and wrong looks identical to one that works (T-5-44), so
    /// this seeds a tree, archives it, clobbers it exactly as a restore would,
    /// runs the *rendered string* through a shell, and compares contents and
    /// modes. Modes are 0600/0700 deliberately: no umask can clear a
    /// user-only bit, so the assertion does not depend on the host's.
    #[test]
    fn the_rendered_command_restores_contents_and_modes_exactly() {
        let Some((tar, sh)) = round_trip_tools() else {
            return;
        };
        let fixture = Fixture::new();
        let seeded: Vec<(PathBuf, &[u8], u32)> = vec![
            (
                fixture.seed(
                    ".config/ai-usagebar/accounts/work/.credentials.json",
                    b"the original credential",
                    0o600,
                ),
                b"the original credential",
                0o600,
            ),
            (
                fixture.seed(".claude/projects/a.jsonl", b"original history", 0o700),
                b"original history",
                0o700,
            ),
        ];
        let targets: Vec<PathBuf> = seeded.iter().map(|(p, _, _)| p.clone()).collect();

        let record = take_with(&fixture.ctx(), &targets, &tar).unwrap().unwrap();
        assert_eq!(record.members, 2);
        assert!(record.bytes > 0, "the archive is empty");

        // Exactly what a restore does next.
        for (path, _, _) in &seeded {
            fs::write(path, b"clobbered by a restore that went wrong").unwrap();
            set_private_mode(path, 0o644).unwrap();
        }

        let status = Command::new(&sh)
            .arg("-c")
            .arg(record.rollback_command())
            .status()
            .unwrap();
        assert!(status.success(), "the rollback command itself failed");

        for (path, body, mode) in &seeded {
            assert_eq!(
                &fs::read(path).unwrap(),
                body,
                "{} not restored",
                path.display()
            );
            assert_eq!(
                mode_of(path),
                *mode,
                "{} came back at the wrong mode",
                path.display()
            );
        }
    }

    /// The archive has to survive the failure it exists for: a restore killed
    /// part-way must leave a complete, findable archive of everything it had
    /// already touched. `take` returning is that guarantee, so extract the
    /// archive the instant it returns, into a directory nothing else wrote.
    #[test]
    fn the_archive_is_complete_and_extractable_the_moment_take_returns() {
        let Some((tar, _)) = round_trip_tools() else {
            return;
        };
        let fixture = Fixture::new();
        let targets = vec![
            fixture.seed(".claude/projects/a.jsonl", b"history", 0o600),
            fixture.seed(".claude-acc/profiles/work/meta.json", b"{}", 0o600),
        ];

        let record = take_with(&fixture.ctx(), &targets, &tar).unwrap().unwrap();

        let into = fixture.dir.path().join("extracted");
        fs::create_dir_all(&into).unwrap();
        let status = Command::new(&tar)
            .arg("-xzf")
            .arg(&record.archive)
            .arg("-C")
            .arg(&into)
            .status()
            .unwrap();
        assert!(status.success(), "the archive was not a complete tarball");

        assert_eq!(
            fs::read(into.join(".claude/projects/a.jsonl")).unwrap(),
            b"history"
        );
        assert_eq!(
            fs::read(into.join(".claude-acc/profiles/work/meta.json")).unwrap(),
            b"{}"
        );
    }

    #[test]
    fn a_root_is_never_a_string_prefix_of_a_sibling() {
        let ancestor =
            common_ancestor([Path::new("/home/bob/a"), Path::new("/home/bobby/b")].into_iter());
        assert_eq!(ancestor, Some(PathBuf::from("/home")));
    }
}
