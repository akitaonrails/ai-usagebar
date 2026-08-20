//! The wire spelling of a path inside a bundle — and the **only** place in the
//! crate where a string from a remote becomes a [`PathBuf`].
//!
//! Every string in a manifest arrives from a remote the threat model treats as
//! hostile (D5): anyone with write access to the paired repository can put an
//! arbitrary manifest there, and every entry in it turns into a local write on
//! whatever machine runs `sync pull`. So the whole of
//! [`from_manifest_path`] refuses **before** it touches the filesystem, and the
//! result is built one component at a time onto the root rather than by joining
//! an untrusted remainder — a single absolute component handed to
//! [`Path::join`] replaces the root wholesale, which is the classic shape of
//! this bug.
//!
//! # The encoding is root-prefixed and relative, and that is the whole point
//!
//! A bundle pushed from one machine is restored on a *second* machine, with a
//! different username and therefore a different `$HOME`. An absolute path in
//! the manifest is unresolvable there — and is also precisely what
//! [`from_manifest_path`] refuses, so a bundle carrying one could only be
//! restored by disabling its own traversal defence. The push side already
//! renders the relocatable form ([`crate::sync::push::packer::manifest_path`],
//! which errors rather than falling back to an absolute path); this module is
//! the other direction, plus the D4 policy check the write side applies.
//!
//! The separator is `/`, always, including on Windows. The bundle is portable
//! or it is nothing.
//!
//! # What this module deliberately does not do
//!
//! It never calls `canonicalize`. The destination usually does not exist yet on
//! a fresh machine, and resolving symlinks the bundle can influence is exactly
//! how an escape sneaks back in after the textual checks have passed. Symlinks
//! are handled at the write boundary instead — plan 5-04 owns that.
//!
//! Owned by plan 5-01. Called by `merge` (5-03) and `write` (5-04); neither
//! restates a path rule.

use std::path::{Component, Path, PathBuf};

use crate::error::{AppError, Result};
use crate::sync::{SyncRoots, scope};

// There is deliberately no encoder in this module. `push::packer::manifest_path`
// has emitted the wire spelling since Phase 4, and the round-trip tests below
// call it directly — a local mirror would make the drift test compare a copy
// against itself, which is exactly the drift it exists to catch.

// The four prefixes. `config_file` needs no fifth: `SyncRoots::resolve` derives
// `config_dir` as its parent, so the file itself already lives under `config/`.
fn config_dir(roots: &SyncRoots) -> &Path {
    roots.config_dir.as_path()
}
fn desktop_data_dir(roots: &SyncRoots) -> &Path {
    roots.desktop_data_dir.as_path()
}
fn desktop_profiles_dir(roots: &SyncRoots) -> &Path {
    roots.desktop_profiles_dir.as_path()
}
fn claude_home(roots: &SyncRoots) -> &Path {
    roots.claude_home.as_path()
}

type RootOf = fn(&SyncRoots) -> &Path;

/// The prefix table, once: name on the wire, and the root it resolves against
/// on *this* machine. A second table is a second thing to get wrong.
///
/// The push direction reads the same four literals from
/// [`crate::sync::push::packer::manifest_path`], which predates this module;
/// `the_two_directions_agree_on_every_prefix` below is the mechanical guard
/// that keeps them one vocabulary rather than two.
const ROOT_PREFIXES: [(&str, RootOf); 4] = [
    ("config", config_dir),
    ("desktop-data", desktop_data_dir),
    ("desktop-profiles", desktop_profiles_dir),
    ("claude-home", claude_home),
];

/// Ceilings on the *size* of a manifest entry, alongside the eight checks on
/// its shape.
///
/// Until Phase 5's audit this module bounded eight shapes and zero sizes. A
/// manifest is bounded only by `MAX_MANIFEST_CHUNKS × CHUNK_SIZE` = 32 MiB,
/// which is room for a great many thousand-component paths, and
/// `write::ensure_dir` is `recursive(true)`. Two things follow, and the second
/// is why this is a security bound rather than tidiness:
///
/// - deep directory chains get created inside the roots — litter the user
///   consented to, but litter — before the kernel refuses; and
/// - `ENAMETOOLONG` becomes a trigger an attacker pulls **on demand**, which is
///   how a hostile manifest reaches a failure path and whatever it prints. That
///   is the delivery mechanism for F-3, and bounding the input here is the half
///   of that fix that does not depend on every print site being careful.
///
/// The numbers sit far above anything the push side emits — the longest real
/// entry is a transcript project directory, one dash-encoded component of a
/// couple of hundred characters at a depth of four — and far below `PATH_MAX`
/// (1024 on macOS) once a root is prepended. `MAX_COMPONENT_BYTES` is the
/// `NAME_MAX` every mainstream filesystem enforces anyway.
const MAX_MANIFEST_PATH_BYTES: usize = 1024;
const MAX_MANIFEST_COMPONENTS: usize = 32;
const MAX_COMPONENT_BYTES: usize = 255;

/// Resolve one manifest entry against *this* machine's roots.
///
/// The hostile-input boundary. Every refusal has its own message, because
/// "invalid path" tells a user nothing about a bundle that may have been
/// tampered with.
pub fn from_manifest_path(roots: &SyncRoots, s: &str) -> Result<PathBuf> {
    let refuse = |why: &str| AppError::Other(format!("refusing the manifest entry {s:?}: {why}"));

    // First, and deliberately: it is the one refusal that does not echo `s`,
    // which is what makes every message below it bounded. A 32 MiB entry must
    // not become a 32 MiB error string on its way to a terminal.
    if s.len() > MAX_MANIFEST_PATH_BYTES {
        return Err(AppError::Other(format!(
            "refusing a manifest entry of {} bytes: no path in a bundle is longer than \
             {MAX_MANIFEST_PATH_BYTES}",
            s.len()
        )));
    }
    if s.is_empty() {
        return Err(refuse("it is empty"));
    }
    if s.contains('\0') {
        return Err(refuse("it contains a NUL byte"));
    }
    if s.starts_with('/') {
        return Err(refuse(
            "it is an absolute path, and every path in a bundle is relative to one of its roots",
        ));
    }
    if s.contains('\\') {
        return Err(refuse(
            "it contains a backslash — a bundle's only separator is `/`, on every platform",
        ));
    }
    if is_drive_prefixed(s) {
        return Err(refuse(
            "it begins with a Windows drive letter, which is an absolute path in disguise",
        ));
    }

    let Some((prefix, rest)) = s.split_once('/') else {
        return Err(refuse(
            "it names no root: a bundle path is `<root>/<path beneath it>`",
        ));
    };
    let root = ROOT_PREFIXES
        .iter()
        .find(|(name, _)| *name == prefix)
        .map(|(_, resolve)| resolve(roots))
        .ok_or_else(|| refuse("it names a root this build does not know"))?;
    if rest.is_empty() {
        return Err(refuse("it names a root with nothing beneath it"));
    }
    if rest.split('/').count() > MAX_MANIFEST_COMPONENTS {
        return Err(refuse(
            "it is nested deeper than any path a bundle can legitimately name",
        ));
    }

    // One component at a time onto the root. Never `root.join(rest)`: a single
    // absolute or drive-rooted component in `rest` would replace the root.
    let mut out = root.to_path_buf();
    for part in rest.split('/') {
        match part {
            "" => return Err(refuse("it has an empty path component")),
            "." => return Err(refuse("it contains a `.` component")),
            ".." => return Err(refuse("it contains a `..` component")),
            _ => {}
        }
        if part.len() > MAX_COMPONENT_BYTES {
            return Err(refuse(
                "it has a single name longer than a filesystem will accept",
            ));
        }
        let mut components = Path::new(part).components();
        if !matches!(
            (components.next(), components.next()),
            (Some(Component::Normal(_)), None)
        ) {
            return Err(refuse("it has a component that is not a plain file name"));
        }
        out.push(part);
    }

    // Belt and braces on the loop above: if it ever stops holding, this is what
    // notices before anything is written.
    if !out.starts_with(root) {
        return Err(refuse("it resolves outside the root it names"));
    }
    Ok(out)
}

/// `C:` / `c:` and friends, before the string is split on `/`.
fn is_drive_prefixed(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

/// D4 on the **write** side: may this bundle path be written at all?
///
/// Reuses [`scope::is_excluded`] rather than restating its lists. The whole
/// point of D4 is that the collector and the restorer agree about what is
/// machine-bound, and two copies of a list diverge on the day one of them is
/// edited. `is_excluded` already refuses any path with an excluded *directory*
/// anywhere above it, so `claude-home/local-agent-mode-sessions/x` is refused
/// whatever prefix it arrives under.
///
/// `rel` is the **manifest path** — prefix included — as a `Path`, never the
/// resolved destination. The resolved path carries this machine's own directory
/// names above the root, and one of them happening to be called `backups` is
/// not the bundle's fault.
pub fn accept_for_write(rel: &Path) -> bool {
    !scope::is_excluded(rel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::push;
    use tempfile::TempDir;

    /// Two machines. Same bundle, different usernames — which is the entire
    /// reason this module exists.
    fn machine(dir: &Path, user: &str) -> SyncRoots {
        let home = dir.join("Users").join(user);
        SyncRoots::at(
            home.join(".config/ai-usagebar/config.toml"),
            home.join(".config/ai-usagebar"),
            home.join("Library/Application Support/Claude"),
            home.join(".claude-acc/profiles"),
            home.join(".claude"),
        )
    }

    /// One realistic path per category, as the collectors would produce them.
    fn realistic(roots: &SyncRoots) -> Vec<PathBuf> {
        vec![
            roots.config_file.clone(),
            roots.config_dir.join("accounts/work/.credentials.json"),
            roots.claude_home.join("scheduled-tasks/daily.json"),
            roots
                .desktop_data_dir
                .join("claude-code-sessions/acct/org/local_1.json"),
            roots.claude_home.join("projects/repo/session.jsonl"),
            roots.desktop_profiles_dir.join("work/meta.json"),
        ]
    }

    #[test]
    fn the_encoding_is_a_bijection_over_every_category() {
        let dir = TempDir::new().unwrap();
        let roots = machine(dir.path(), "alice");
        for path in realistic(&roots) {
            let wire = push::packer::manifest_path(&roots, &path).expect("a path under a root");
            let back = from_manifest_path(&roots, &wire).expect("its own spelling resolves");
            assert_eq!(back, path, "round trip through {wire:?}");
        }
    }

    #[test]
    fn a_bundle_pushed_by_one_user_resolves_under_a_second_users_roots() {
        let dir = TempDir::new().unwrap();
        let alice = machine(dir.path(), "alice");
        let bob = machine(dir.path(), "bob");

        for path in realistic(&alice) {
            let wire = push::packer::manifest_path(&alice, &path).unwrap();
            let on_bob = from_manifest_path(&bob, &wire).unwrap();
            assert!(
                on_bob.starts_with(dir.path().join("Users/bob")),
                "{wire:?} resolved to {on_bob:?}, which is not under bob's home"
            );
            assert!(
                !on_bob.to_string_lossy().contains("alice"),
                "{on_bob:?} still carries the pushing machine's username"
            );
        }
    }

    #[test]
    fn no_manifest_path_carries_an_absolute_path_or_a_username() {
        let dir = TempDir::new().unwrap();
        let roots = machine(dir.path(), "alice");
        for path in realistic(&roots) {
            let wire = push::packer::manifest_path(&roots, &path).unwrap();
            assert!(!wire.starts_with('/'), "{wire:?} is absolute");
            assert!(!wire.contains("alice"), "{wire:?} names the pushing user");
        }
    }

    #[test]
    fn a_path_under_no_root_is_an_error_naming_the_path() {
        let dir = TempDir::new().unwrap();
        let roots = machine(dir.path(), "alice");
        let stray = dir.path().join("etc/shadow");
        let err = push::packer::manifest_path(&roots, &stray).expect_err("under none of the roots");
        assert!(
            err.to_string().contains("shadow"),
            "the error must name the path: {err}"
        );
    }

    /// A customised install can nest one root inside another. The shortest
    /// match would file the path under the wrong tree and restore it to the
    /// wrong place on the second machine.
    #[test]
    fn nested_roots_resolve_to_the_longer_one() {
        let dir = TempDir::new().unwrap();
        let home = dir.path().join("home");
        let roots = SyncRoots::at(
            home.join("config.toml"),
            home.clone(),
            home.join("desktop"),
            home.join("profiles"),
            // Deliberately nested inside `config_dir`.
            home.join("nested/claude"),
        );
        let inside = roots.claude_home.join("projects/x.jsonl");
        let wire = push::packer::manifest_path(&roots, &inside).unwrap();
        assert_eq!(wire, "claude-home/projects/x.jsonl");
        assert_eq!(from_manifest_path(&roots, &wire).unwrap(), inside);
    }

    #[test]
    fn every_hostile_spelling_is_refused_with_its_own_message() {
        let dir = TempDir::new().unwrap();
        let roots = machine(dir.path(), "alice");

        // The size bounds need built strings, and a `&format!(…)` inside the
        // array literal below would be dropped at the end of the `let`.
        let too_long = format!("config/{}", "x".repeat(MAX_MANIFEST_PATH_BYTES));
        let too_deep = format!("config/{}", "d/".repeat(MAX_MANIFEST_COMPONENTS));
        let component_too_long = format!("config/{}", "n".repeat(MAX_COMPONENT_BYTES + 1));

        let cases = [
            ("", "empty"),
            ("config/a\0b", "NUL"),
            ("/etc/shadow", "absolute"),
            ("config\\..\\etc", "backslash"),
            ("C:/Users/alice/x", "Windows drive"),
            ("config", "names no root"),
            ("config/", "nothing beneath it"),
            ("elsewhere/x", "does not know"),
            ("config/../../etc/shadow", "`..` component"),
            ("config/./x", "`.` component"),
            ("config/a//b", "empty path component"),
            // The size bounds, which the shape checks never asked about.
            (too_long.as_str(), "bytes"),
            (too_deep.as_str(), "nested deeper"),
            (component_too_long.as_str(), "single name longer"),
        ];

        // The reason, with the echoed input stripped off — otherwise every
        // message is trivially unique because it quotes its own input.
        let mut seen: Vec<String> = Vec::new();
        for (input, needle) in cases {
            let err = from_manifest_path(&roots, input)
                .expect_err(&format!("{input:?} must be refused"))
                .to_string();
            assert!(
                err.contains(needle),
                "{input:?} was refused, but not for the stated reason: {err}"
            );
            let reason = err
                .split_once("}: ")
                .or_else(|| err.split_once("\": "))
                .map_or(err.clone(), |(_, why)| why.to_string());
            assert!(
                !seen.contains(&reason),
                "{input:?} shares a refusal with an earlier case: {reason}"
            );
            seen.push(reason);
        }
    }

    /// The traversal that actually matters: the escape must not reach the
    /// filesystem as a resolved parent outside the root.
    #[test]
    fn a_traversal_never_yields_a_path_outside_its_root() {
        let dir = TempDir::new().unwrap();
        let roots = machine(dir.path(), "alice");
        for hostile in [
            "config/../../../../../../etc/shadow",
            "claude-home/projects/../../../../.ssh/id_ed25519",
            "desktop-data/..",
        ] {
            assert!(
                from_manifest_path(&roots, hostile).is_err(),
                "{hostile:?} resolved instead of being refused"
            );
        }
    }

    #[test]
    fn the_two_directions_agree_on_every_prefix() {
        let dir = TempDir::new().unwrap();
        let roots = machine(dir.path(), "alice");
        for (name, resolve) in ROOT_PREFIXES {
            let under = resolve(&roots).join("probe.json");
            let wire = push::packer::manifest_path(&roots, &under)
                .unwrap_or_else(|e| panic!("the push side refuses the {name} root: {e}"));
            assert_eq!(
                wire,
                format!("{name}/probe.json"),
                "the push side spells the {name} root differently"
            );
            assert_eq!(from_manifest_path(&roots, &wire).unwrap(), under);
        }
    }

    /// D4: a bundle naming machine-bound state is dropped on the write side,
    /// whatever it claims, and whatever prefix it arrives under.
    #[test]
    fn machine_bound_state_is_refused_under_every_prefix() {
        for refused in [
            "claude-home/local-agent-mode-sessions/x.json",
            "desktop-data/local-agent-mode-sessions/deep/x.json",
            "config/bridge-state.json",
            "desktop-profiles/work/ant-device-registry.json",
            "config/sync/index.sqlite3-journal",
            "config/accounts/work/.tmp.credentials",
            "desktop-data/backups/old.tar.gz",
            "config/.fetch.lock",
        ] {
            assert!(
                !accept_for_write(Path::new(refused)),
                "{refused} would have been written"
            );
        }
    }

    /// **F-2.** D4's whole stated purpose is that "a bundle produced by a
    /// future or modified client must not be able to talk this side into
    /// writing them". A modified client only had to hold down Shift: these four
    /// spellings were `accept_for_write == true`, resolved through this gate
    /// cleanly, and — on the case-insensitive volume this project's users run —
    /// landed on the very device-identity files D4 exists to keep off the
    /// machine. It needed no `--force` and no consent of any kind.
    ///
    /// These are the audit's PoC A2 verbatim. The exhaustive version, which
    /// iterates D2's lists themselves so a name added later is covered, lives
    /// beside those lists in `scope`.
    #[test]
    fn machine_bound_state_is_refused_however_the_bundle_capitalises_it() {
        for refused in [
            "config/Bridge-State.json",
            "desktop-profiles/work/Ant-Device-Registry.json",
            "desktop-data/Backups/old.tar.gz",
            "claude-home/Local-Agent-Mode-Sessions/x.json",
            "config/BRIDGE-STATE.JSON",
            "config/accounts/work/.TMP.credentials",
            "config/sync/index.sqlite3-JOURNAL",
        ] {
            assert!(
                !accept_for_write(Path::new(refused)),
                "{refused} would have been written over this machine's own identity state"
            );
        }
    }

    /// **A store is not a path, and this is the refusal that keeps it one.**
    ///
    /// `keystore/…` is deliberately absent from [`ROOT_PREFIXES`], so every
    /// spelling of it — the one this build writes, one from a later build, and
    /// a hostile one dressed up as a traversal — dies here rather than becoming
    /// a destination. A synthetic entry that resolved to a file would be a live
    /// OAuth token written in plaintext under the user's home directory;
    /// `restore::merge` intercepts these long before this point, and this is
    /// what makes that interception a belt as well as braces.
    #[test]
    fn a_machine_bound_stores_wire_name_never_resolves_to_a_place_on_disk() {
        let dir = TempDir::new().unwrap();
        let roots = machine(dir.path(), "bob");
        for spelling in [
            crate::sync::keystore::Store::ClaudeCodeOauth.manifest_path(),
            "keystore/from-a-later-version",
            "keystore/../config/config.toml",
            "keystore",
        ] {
            let err = from_manifest_path(&roots, spelling)
                .expect_err("a store resolved to a filesystem destination");
            assert!(err.to_string().contains("refusing"), "{spelling}: {err}");
        }
    }

    #[test]
    fn ordinary_synced_state_is_accepted() {
        for accepted in [
            "config/config.toml",
            "config/accounts/work/.credentials.json",
            "claude-home/scheduled-tasks/daily.json",
            "desktop-data/claude-code-sessions/acct/org/local_1.json",
            "claude-home/projects/repo/session.jsonl",
        ] {
            assert!(
                accept_for_write(Path::new(accepted)),
                "{accepted} would have been dropped"
            );
        }
    }
}
