//! Encrypted sync bundle format — client-side encryption for pushing local
//! state to a private git remote that is treated as fully hostile.
//!
//! Everything under this module is **pure and offline**: no network, no `$HOME`,
//! no Keychain, no git — with the single exception of [`github`], which is the
//! transport and says so. The whole format is exercisable by `cargo test` on a
//! machine with none of those, which is what lets it be adversary-tested before
//! anything can transmit it.
//!
//! Layout — one file per plan, so parallel work never collides:
//! - [`crypto`] — key hierarchy and every AEAD call. The only module here that
//!   imports `argon2` or `chacha20poly1305`.
//! - [`chunk`] — fixed-size chunking, compression, framing.
//! - [`pack`] — packing sealed blobs into remote-sized files.
//! - [`model`] — snapshot root, manifest, and index objects.
//! - [`passphrase`] — passphrase generation and strength floor.
//! - [`anchor`] — the local monotonic rollback anchor.
//! - [`scope`] — the one bounded, symlink-refusing walker plus the D2
//!   exclusion predicate every category funnels through.
//! - [`transcripts`] — the bounded transcript selector.
//! - [`index`] — the local `(path, size, mtime_ns, inode)` change-detection db.
//! - [`keystore`] — the machine-bound credential stores that are *read* on push
//!   and *written* on restore, because their contents cannot be copied.
//! - [`plan`] — dry-run planning over a scan.
//! - [`github`] — the GitHub transport: auth, the private-repo gate, and the
//!   pairing record. The only module here that opens a socket — and in Phase 3
//!   it can only `GET`.
//! - [`push`] — the outbound path: packing, uploading, and the one
//!   compare-and-swap that publishes a snapshot. The only module that can
//!   *change* the remote.
//! - [`restore`] — the inbound path: reading a bundle back onto a machine that
//!   may not be the one that pushed it. The only module that turns a remote's
//!   claims into local writes, and the only one that resolves a manifest path.
//! - [`report`] — the pure `sync status` model and its renderer.
//! - [`cli`] — the `ai-usagebar sync …` entry point.
//!
//! The scanning half of this module touches the filesystem, but only through
//! [`SyncRoots`], whose every root is injected — no test here reads a real
//! `$HOME`.

pub mod anchor;
pub mod chunk;
pub mod cli;
pub mod crypto;
pub mod github;
pub mod index;
pub mod keystore;
pub mod model;
pub mod pack;
pub mod passphrase;
pub mod plan;
pub mod push;
pub mod report;
pub mod restore;
pub mod scope;
pub mod transcripts;

use std::path::PathBuf;

use crate::config::Config;
use crate::error::{AppError, Result};
use crate::sync::keystore::Stores;

/// Fixed chunk size. Fixed-size, *not* content-defined: CDC boundary positions
/// are visible as ciphertext lengths and fingerprint the plaintext
/// (arXiv:2504.02095). The payload here is append-only JSONL and page-aligned
/// SQLite, so fixed blocks aligned to each file's start dedup just as well.
pub const CHUNK_SIZE: usize = 256 * 1024;

/// Recorded in the snapshot header so a future chunker can be introduced
/// without guessing how existing bundles were split.
pub const CHUNKER_ID: &str = "fixed-256k";

// BLAKE3 `derive_key` context strings. BLAKE3's contract is that these are
// hardcoded, application-specific and globally unique. The `v1` token is
// load-bearing: a v2 key hierarchy must not be able to collide with v1's.
pub const CTX_CHUNK: &str = "ai-usagebar.sync.v1 chunk-encryption-key";
pub const CTX_NAME: &str = "ai-usagebar.sync.v1 chunk-name-key";
pub const CTX_ROOT: &str = "ai-usagebar.sync.v1 snapshot-root-key";
pub const CTX_NONCE: &str = "ai-usagebar.sync.v1 chunk-nonce";

// Versioning. Every versioned object carries *two* numbers: the version this
// build writes, and the highest version it can read. Readers accept anything at
// or below their ceiling and refuse only what is greater — see `check_version`.
// An equality check would mean a v2 client could not read a v1 bundle, which
// inverts the "raise the KDF parameters without breaking existing bundles"
// promise the format exists to keep.

/// Keyfile version written by this build.
pub const KEYFILE_VERSION: u32 = 1;
/// Highest keyfile version this build can read.
pub const MAX_SUPPORTED_KEYFILE: u32 = 1;

/// Manifest version written by this build.
///
/// v2 carries the manifest across as many chunks as it needs; v1 assumed one.
pub const MANIFEST_VERSION: u32 = 2;
/// Highest manifest version this build can read.
pub const MAX_SUPPORTED_MANIFEST: u32 = 2;

/// Snapshot-root version written by this build.
///
/// v2 names the manifest with an ordered list of chunk ids; v1 named a single
/// id, which could not express a manifest past [`CHUNK_SIZE`].
pub const ROOT_VERSION: u32 = 2;
/// Highest snapshot-root version this build can read.
pub const MAX_SUPPORTED_ROOT: u32 = 2;

/// Index-object version written by this build.
pub const INDEX_VERSION: u32 = 1;
/// Highest index-object version this build can read.
pub const MAX_SUPPORTED_INDEX: u32 = 1;

/// Pack-header version written by this build.
pub const PACK_HEADER_VERSION: u32 = 1;
/// Highest pack-header version this build can read.
pub const MAX_SUPPORTED_PACK_HEADER: u32 = 1;

/// Accept any format version at or below `ceiling`; refuse only what is above
/// it, and say plainly that the *client* is the old thing, not the data.
///
/// `object` names the thing being read ("keyfile", "manifest", …) and is a
/// compile-time string, never user or attacker data.
pub fn check_version(found: u32, ceiling: u32, object: &str) -> Result<()> {
    if found <= ceiling {
        return Ok(());
    }
    Err(AppError::Other(format!(
        "this {object} was written at format version {found}, but this build of \
         ai-usagebar reads at most version {ceiling} — upgrade ai-usagebar to read it"
    )))
}

/// A fixed file or directory name that a security predicate compares an
/// attacker-supplied name against — and that can **only** be compared the way
/// the filesystem itself compares it.
///
/// # Why this is a type and not a `to_lowercase()` at each call site
///
/// macOS ships APFS case-insensitive by default (HFS+ was too) and Windows is
/// case-insensitive everywhere, so `.Credentials.json` and `.credentials.json`
/// are **one file**. A byte-exact `==` against a fixed name is therefore a hole
/// a tampered bundle opens with one capital letter: the classifier answers "not
/// a credential" while `symlink_metadata` at the very same path finds the live
/// credential. Phase 5's audit defeated the credential gate (F-1) and D4's
/// machine-bound exclusion list (F-2) with exactly that byte, and neither
/// needed anything else.
///
/// Folding at each call site would be three patches and a fourth bug later —
/// which is how the sibling defect in `guard::production_code` came back after
/// being learned and fixed once already. So the fix is structural: a
/// `FixedName` has no `PartialEq<str>`, no `Deref<Target = str>` and no
/// accessor handing back a comparable `&str`, so `name == CREDENTIAL_FILE` and
/// `EXCLUDED_NAMES.contains(&name)` **do not compile**. The only way to ask the
/// question is [`FixedName::matches`], and it folds. A future byte-exact
/// comparison against one of these names is a build failure, not a test
/// failure, and not a shipped hole.
///
/// # Where it deliberately is not used
///
/// [`restore::layout`]'s root-prefix table is matched byte-exactly on purpose:
/// a prefix that does not match is a *refusal*, so folding there would only
/// admit more bundles, and the push side emits exactly one spelling.
///
/// # Ceiling
///
/// ponytail: this covers case folding, which is the defect. Windows'
/// *other* name canonicalisations — trailing dots and spaces, 8.3 short names,
/// `:` alternate data streams — are a separate class this does not close; the
/// sync feature's users are macOS and Linux. If Windows ever becomes a restore
/// target, that wants its own normalisation at [`restore::layout`]'s boundary,
/// not more cases here.
#[derive(Debug, Clone, Copy)]
pub struct FixedName(&'static str);

impl FixedName {
    /// The name must be written in its own folded spelling — asserted by
    /// [`FixedName::is_folded`] in each owning module's tests, because a
    /// `const fn` cannot check it.
    pub const fn new(name: &'static str) -> Self {
        Self(name)
    }

    /// Does `candidate` name this file, as the filesystem would decide?
    pub fn matches(self, candidate: &str) -> bool {
        if candidate.is_ascii() {
            candidate.eq_ignore_ascii_case(self.0)
        } else {
            fold(candidate) == self.0
        }
    }

    /// Does `candidate` begin with this name?
    pub fn is_prefix_of(self, candidate: &str) -> bool {
        if candidate.is_ascii() {
            // `candidate` is ASCII, so every byte index is a char boundary.
            candidate.len() >= self.0.len()
                && candidate[..self.0.len()].eq_ignore_ascii_case(self.0)
        } else {
            fold(candidate).starts_with(self.0)
        }
    }

    /// Does `candidate` end with this name?
    pub fn is_suffix_of(self, candidate: &str) -> bool {
        if candidate.is_ascii() {
            candidate.len() >= self.0.len()
                && candidate[candidate.len() - self.0.len()..].eq_ignore_ascii_case(self.0)
        } else {
            fold(candidate).ends_with(self.0)
        }
    }

    /// A name not written in its own folded spelling can never match anything.
    /// The guard against a future `FixedName::new(".Stale")`.
    pub fn is_folded(self) -> bool {
        fold(self.0) == self.0
    }
}

impl std::fmt::Display for FixedName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// Fold a name the way a case-insensitive filesystem folds it before comparing.
///
/// [`str::to_lowercase`] is full Unicode lowercasing, which already covers the
/// one exotic character that matters here: U+212A KELVIN SIGN lowercases to
/// `k`, and both `backups` and `.lock` contain one. U+017F LATIN SMALL LETTER
/// LONG S is the only other code point that case-*folds* onto a single ASCII
/// character while already being lowercase, so it is mapped explicitly.
///
/// ponytail: two characters hand-mapped rather than an ICU fold or a new
/// dependency. Every [`FixedName`] in this crate is ASCII, and those are the
/// only two non-ASCII code points whose fold is one ASCII character. A
/// non-ASCII fixed name would want `unicase` instead.
fn fold(name: &str) -> String {
    name.chars()
        .flat_map(char::to_lowercase)
        .map(|c| if c == 'ſ' { 's' } else { c })
        .collect()
}

/// Every filesystem root the collectors are allowed to look at.
///
/// The same seam as [`crate::claude_desktop::Paths`]: [`SyncRoots::at`] is what
/// every test constructs, [`SyncRoots::resolve`] is the one production wrapper
/// that touches `$HOME`. Nothing under [`scope`] resolves a path itself, so a
/// collector physically cannot wander outside what it was handed.
#[derive(Debug, Clone)]
pub struct SyncRoots {
    /// The effective `config.toml`.
    pub config_file: PathBuf,
    /// Its parent — where `accounts/<label>/.credentials.json` lives.
    pub config_dir: PathBuf,
    /// Claude Desktop's data dir, parent of `claude-code-sessions`.
    pub desktop_data_dir: PathBuf,
    /// The claude-acc profile store, `~/.claude-acc/profiles`.
    pub desktop_profiles_dir: PathBuf,
    /// `~/.claude`, parent of `scheduled-tasks/` and `projects/`.
    pub claude_home: PathBuf,
    /// The change-detection index db. In the *cache* dir in production — it is
    /// a wipeable hint, not durable state — and inside the injected tree under
    /// [`SyncRoots::at`], so no test writes to an installer's real `$XDG`.
    pub index_file: PathBuf,
    /// The machine-bound credential stores — the login Keychain, in production.
    ///
    /// A root like any other, and injected like any other: [`SyncRoots::at`]
    /// yields an empty [`Stores::fixture`] and [`SyncRoots::resolve`] is the one
    /// place the real machine is reached. That is what keeps `cargo test` — run
    /// by the AUR `check()` on an installer's own laptop — structurally unable
    /// to read or clobber their Claude login. See [`keystore`].
    pub stores: Stores,
}

impl SyncRoots {
    /// Test seam: every root explicit.
    ///
    /// [`index_file`](SyncRoots::index_file) is the one exception — derived
    /// from `config_dir` rather than passed, so the six existing callers keep
    /// compiling and every one of them still lands inside its own `TempDir`.
    /// Production never takes this path; [`resolve`](SyncRoots::resolve) puts
    /// the index in the cache directory where plan 2-01 put it.
    pub fn at(
        config_file: PathBuf,
        config_dir: PathBuf,
        desktop_data_dir: PathBuf,
        desktop_profiles_dir: PathBuf,
        claude_home: PathBuf,
    ) -> Self {
        Self {
            config_file,
            index_file: config_dir.join("sync").join("index.sqlite3"),
            config_dir,
            desktop_data_dir,
            desktop_profiles_dir,
            claude_home,
            stores: Stores::fixture(),
        }
    }

    /// Production paths, all derived from resolvers that already exist. No new
    /// config knob: a second path to the same tree is a second thing to get
    /// wrong.
    pub fn resolve(config: &Config) -> Result<Self> {
        let config_file = crate::config::resolved_path().ok_or_else(|| {
            AppError::Other(
                "could not resolve the ai-usagebar config directory (no HOME?) — \
                 sync needs to know where config.toml lives"
                    .into(),
            )
        })?;
        let config_dir = config_file
            .parent()
            .ok_or_else(|| {
                AppError::Other(format!(
                    "config path has no parent directory: {}",
                    config_file.display()
                ))
            })?
            .to_path_buf();
        let desktop = crate::claude_desktop::Paths::resolve(&config.anthropic)?;
        Ok(Self {
            config_file,
            config_dir,
            desktop_data_dir: desktop.data_dir,
            desktop_profiles_dir: desktop.profiles_dir,
            claude_home: crate::cache::home_dir()?.join(".claude"),
            index_file: index::default_path()?,
            // The one door to a real login Keychain in the whole crate's sync
            // tree, guarded by `keystore`'s own structural test.
            stores: Stores::Machine,
        })
    }
}

/// The one recursive source walk every structural guard in this module tree
/// uses.
///
/// **It exists because two guards were blind and nobody could see it.** Phase
/// 4's audit found `crypto.rs`'s crypto-import invariant walking `src/sync` with
/// a non-recursive `read_dir`, leaving `push/` and `github/` — 11,500 lines —
/// entirely outside the invariant whose stated value is "what lets a security
/// auditor read one file instead of six"; and `passphrase.rs`'s
/// environment-read guard iterating a **hand-maintained list** of three files
/// that had not been extended to `push/rekey.rs`, the file its own critical
/// threat is about. That second one was Phase 3's F-8 recurring verbatim, one
/// phase later, because the remediation had added a file to the list rather than
/// making the list unnecessary.
///
/// A guard that enumerates what to check fails open on everything added after
/// it. A guard that walks fails *closed*: a new file is scanned by default and
/// an exemption has to be written down.
#[cfg(test)]
pub(crate) mod guard {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_check_accepts_at_or_below_the_ceiling_and_refuses_only_above() {
        assert!(check_version(0, 1, "keyfile").is_ok());
        assert!(check_version(1, 1, "keyfile").is_ok());
        let err = check_version(2, 1, "keyfile").expect_err("above the ceiling must be refused");
        assert!(err.to_string().contains("upgrade ai-usagebar"));
    }

    /// The whole point of the type: the spellings a case-insensitive volume
    /// treats as one file compare as one name.
    #[test]
    fn a_fixed_name_matches_every_spelling_the_filesystem_folds_together() {
        let credential = FixedName::new(".credentials.json");
        for spelling in [
            ".credentials.json",
            ".Credentials.json",
            ".CREDENTIALS.JSON",
            ".cReDeNtIaLs.JsOn",
            // U+017F LATIN SMALL LETTER LONG S, which folds onto `s`.
            ".credential\u{17f}.json",
        ] {
            assert!(
                credential.matches(spelling),
                "{spelling:?} would have slipped past the credential gate"
            );
        }
        assert!(!credential.matches(".credentials.jsonx"));
        assert!(!credential.matches("credentials.json"));

        // U+212A KELVIN SIGN, which lowercases to `k` — and `backups` has one.
        let backups = FixedName::new("backups");
        assert!(backups.matches("Bac\u{212a}ups"));
        assert!(FixedName::new(".lock").is_suffix_of("index.LOC\u{212a}"));
        assert!(FixedName::new(".tmp.").is_prefix_of(".TMP.credentials"));
        assert!(!FixedName::new(".tmp.").is_prefix_of(".tmp"));
    }

    /// A name written in any other spelling can never match, so it is a dead
    /// entry in a security list — caught here rather than in production.
    #[test]
    fn a_fixed_name_written_unfolded_is_reported_as_such() {
        assert!(FixedName::new("bridge-state.json").is_folded());
        assert!(!FixedName::new("Bridge-State.json").is_folded());
    }

    /// F-4: prose is not code, so a doc comment naming the marker must not
    /// truncate the region every structural guard scans.
    #[test]
    fn a_marker_inside_a_comment_does_not_truncate_the_scanned_region() {
        let source = concat!(
            "fn before() {}\n",
            "/// The only production wrapper here, and nothing under `#[cfg(test)]` calls it.\n",
            "fn after_the_prose() {}\n",
            "    // #[cfg(test)] — indented prose, still prose\n",
            "fn also_after() {}\n",
            "#[cfg(test)]\n",
            "mod tests {\n",
            "    fn inside_the_test_module() {}\n",
            "}\n",
        );
        let production = guard::production_code(source);
        assert!(production.contains("fn after_the_prose"), "{production}");
        assert!(production.contains("fn also_after"), "{production}");
        assert!(
            !production.contains("inside_the_test_module"),
            "the test module is still excluded: {production}"
        );
        assert!(
            !production.contains("The only production wrapper"),
            "comments are not code: {production}"
        );
    }

    /// The file the blind spot was actually in, named rather than described.
    /// `pairing.rs`'s first textual `#[cfg(test)]` is prose near the top, and
    /// its last production function is hundreds of lines below it.
    #[test]
    fn every_production_function_in_pairing_rs_is_inside_the_scanned_region() {
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/sync/github/pairing.rs"),
        )
        .expect("the crate's own source is readable");
        let production = guard::production_code(&source);
        for needle in [
            "fn default_path",
            "fn read_from",
            "fn write_to",
            "fn check_drift",
            "fn went_public_incident",
        ] {
            assert!(
                production.contains(needle),
                "pairing.rs::{needle} is outside the region every structural guard scans"
            );
        }
    }
}
