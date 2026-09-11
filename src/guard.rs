//! Source-scanning helpers for structural guard tests.
//!
//! A guard that enumerates what to check fails open on everything added after
//! it. A guard that *walks* fails closed: a new file is scanned by default and
//! an exemption has to be written down. `claude_desktop`'s note-sanitization
//! guard is built on this.
//!
//! Written by Augusto Claro for the encrypted-sync bundle format (#123) and
//! kept when that module was reverted, because the walking-guard idea outlived
//! the feature it was written for. The `production_code` comment below records
//! a real defect it was hardened against; the file names in it refer to that
//! now-removed module and are left as the history of the fix.

use std::path::{Path, PathBuf};

/// Every `.rs` file under `dir`, recursively.
pub(crate) fn rs_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(dir, &mut out);
    out
}

/// Every `.rs` file under `CARGO_MANIFEST_DIR`-relative `rel`.
///
/// Resolved from the manifest directory rather than from a relative path, so
/// a guard is independent of the working directory and survives the AUR
/// `srcdir` layout.
pub(crate) fn rs_files_in(rel: &str) -> Vec<PathBuf> {
    rs_files(&Path::new(env!("CARGO_MANIFEST_DIR")).join(rel))
}

/// A file's production code: every line that is neither a comment nor part
/// of the file's own `#[cfg(test)]` module. A test that names a needle is a
/// test, not a violation, and prose that discusses one is neither.
///
/// **Comments are removed before the marker is looked for, and that is the
/// fix.** The previous shape split the raw source on the first *textual*
/// `#[cfg(test)]`. In `github/pairing.rs` the first occurrence is inside a
/// doc comment at line 76, so the scanned region ended at line 75 and the
/// 397 lines below it — five production functions — were invisible to every
/// guard built on this helper. Phase 5's audit put
/// `std::env::var("SYNC_PASSWORD")` in that region and watched the T-5-66
/// guard pass.
///
/// `github/mod.rs`'s own guard recorded this exact defect and worked around
/// it for itself; the lesson reached one call site and not the shared helper
/// every other guard depends on. A *smarter* marker search — line-anchored,
/// or `\n#[cfg(test)]\nmod tests` — keeps the same shape: a guard that stops
/// looking where it happens to find a string. Dropping comments first makes
/// the marker unambiguous by construction, because prose is no longer part
/// of the text being searched.
///
/// Returns an owned `String` rather than a borrowed slice, since the result
/// is no longer a contiguous piece of the input.
pub(crate) fn production_code(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .take_while(|line| !line.trim_start().starts_with("#[cfg(test)]"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("a readable source directory") {
        let path = entry.expect("a readable directory entry").path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    /// One released section's entries must not reappear anywhere else.
    ///
    /// A PR branched before the last tag keeps its notes under `[Unreleased]`,
    /// and git merges them *cleanly* into whatever now occupies that position
    /// — which is the section that was just published. The repository has been
    /// bitten three times: twice by contributor branches, and once by a
    /// maintainer resolving a conflict with a script, which is what turned a
    /// documented manual check into this test.
    ///
    /// The manual check compares the newest section against its own tag and so
    /// needs git, which the AUR `check()` does not have. This needs only the
    /// file: an entry duplicated across two versions is the shape the accident
    /// takes, whichever direction it came from.
    #[test]
    fn no_changelog_entry_appears_under_two_versions() {
        let Some(changelog) = changelog() else { return };
        let mut seen: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        let mut version = "<before any version heading>";

        for line in changelog.lines() {
            if let Some(rest) = line.strip_prefix("## ") {
                version = rest.trim();
            } else if let Some(entry) = line.strip_prefix("- ") {
                let entry = entry.trim();
                // The first line of a bullet is its identity; continuation
                // lines are indented and never start a bullet.
                if entry.len() > 24 {
                    seen.entry(entry).or_default().push(version);
                }
            }
        }

        let duplicated: Vec<_> = seen
            .iter()
            .filter(|(_, versions)| {
                let mut distinct = versions.to_vec();
                distinct.sort_unstable();
                distinct.dedup();
                distinct.len() > 1
            })
            .map(|(entry, versions)| format!("{versions:?}: {}…", &entry[..40.min(entry.len())]))
            .collect();

        assert!(
            duplicated.is_empty(),
            "these entries appear under more than one version — an [Unreleased] \\
             note has merged into a released section:\n{}",
            duplicated.join("\n")
        );
    }

    /// Keep-A-Changelog allows one heading per category per release, and the
    /// release process reads those headings to pick the next version number. A
    /// second `### Fixed` in one section hides whatever is under it from that
    /// decision. Merging two branches that both add a category produces
    /// exactly this.
    #[test]
    fn no_changelog_section_repeats_a_category() {
        let Some(changelog) = changelog() else { return };
        let mut offenders = Vec::new();
        let mut version = "<before any version heading>";
        let mut categories: Vec<&str> = Vec::new();

        let mut flush = |version: &str, categories: &mut Vec<&str>| {
            let mut sorted = categories.clone();
            sorted.sort_unstable();
            let before = sorted.len();
            sorted.dedup();
            if sorted.len() != before {
                offenders.push(version.to_string());
            }
            categories.clear();
        };

        for line in changelog.lines() {
            if let Some(rest) = line.strip_prefix("## ") {
                flush(version, &mut categories);
                version = rest.trim();
            } else if let Some(rest) = line.strip_prefix("### ") {
                categories.push(rest.trim());
            }
        }
        flush(version, &mut categories);

        // Two released sections shipped this way. Their text is frozen — a
        // tag is immutable and the drift rule forbids editing a published
        // section — so they are named here rather than corrected. The point of
        // this guard is the next one, not the last two.
        const SHIPPED_THIS_WAY: [&str; 2] = ["[1.8.0] — 2026-08-28", "[0.14.0] — 2026-07-20"];
        offenders.retain(|version| !SHIPPED_THIS_WAY.contains(&version.as_str()));

        assert!(
            offenders.is_empty(),
            "these versions carry the same category heading twice, which hides \
             whatever is under the second one from the release's version \
             decision: {offenders:?}"
        );
    }

    /// The changelog, when this build has one.
    ///
    /// `nix/package.nix` filters the source down to what the binary needs, and
    /// `CHANGELOG.md` is not in it — correctly, since adding it would rebuild
    /// the package every time a release note changes. These guards protect the
    /// repository's changelog, so they have nothing to say in a build that
    /// ships without one and skip rather than fail.
    ///
    /// They still run everywhere it matters: a developer's `make test`, the
    /// Linux/macOS/Windows CI jobs, and the AUR `check()`, whose source is the
    /// release tarball and does include the file.
    fn changelog() -> Option<String> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("CHANGELOG.md");
        std::fs::read_to_string(&path).ok()
    }
}
