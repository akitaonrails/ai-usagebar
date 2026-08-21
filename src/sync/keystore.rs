//! Credential stores that are **bound to one machine** — read on push, written
//! on restore, and never carried across as opaque bytes.
//!
//! # The defect this module exists for
//!
//! On macOS, recent Claude Code builds keep their OAuth credential as a generic
//! password in the login Keychain rather than in `~/.claude/.credentials.json`
//! ([`crate::anthropic::keychain`] exists for exactly that reason). The file the
//! collectors look for therefore does not exist, so the `credentials` category
//! shipped tens of megabytes of Claude Desktop state to a second Mac **without
//! the one credential that makes the CLI — and this tool — work there**.
//!
//! Copying the bytes cannot fix it: a Keychain item is not a file, and Claude
//! Desktop's `config-tokenCache*` blobs are AES ciphertext under a secret in
//! *that machine's* login Keychain. The fix for all three shapes is the same —
//! **read the credential out on push, write it back in on restore**, rather
//! than moving whatever file happens to hold it:
//!
//! | store | push | restore |
//! |---|---|---|
//! | Claude Code OAuth | read the Keychain item | write the Keychain item |
//! | Cursor sign-in | read the `cursorAuth/*` rows | write those rows back |
//! | Claude Desktop token cache, one per profile per slot | decrypt with this Mac's Safe Storage key | re-encrypt with the *target's* key |
//!
//! Each is machine-bound for a different reason, and none of them is fixed by
//! carrying bytes:
//!
//! - The Keychain item is not a file at all.
//! - Cursor's token *is* plaintext and portable, but it lives in a 38 MB
//!   `state.vscdb` of live editor state — open tabs, history, workspace layout.
//!   Copying that file to move a 424-byte JWT would destroy the receiving
//!   machine's editor state, so the rows travel and the database does not (see
//!   [`crate::cursor::db::write_auth_rows`]).
//! - A Desktop token cache is AES-CBC under a key in the *pushing* Mac's login
//!   Keychain. No byte copy can ever open on a second Mac; only a decrypt on
//!   one side and a re-encrypt on the other can. Plan 6-09's
//!   [`Disposition::ForeignSafeStorage`](crate::sync::restore::Disposition)
//!   refusal remains, but as the **fallback** for a target with no key of its
//!   own and for bundles pushed before this existed — not the primary path.
//!
//! Because the store carries them properly, `scope`'s `Credentials` collector
//! deliberately no longer collects `config-tokenCache{,V2}` as files. Two
//! carriers for one credential is two carriers that disagree.
//!
//! # The wire spelling is not a path, and must never become one
//!
//! A store's manifest entry is `keystore/<store>` — a prefix deliberately
//! **absent** from [`crate::sync::restore::layout`]'s `ROOT_PREFIXES`, so
//! `from_manifest_path` refuses it with "it names a root this build does not
//! know" rather than resolving it under a root. That refusal is the property
//! that matters: a synthetic entry that resolved to a file would be a live
//! OAuth token written in plaintext into the user's home directory.
//! `restore::merge` intercepts these entries *before* it resolves anything, and
//! `restore::write` routes them to [`Stores::write`] instead of a tempfile.
//!
//! Two of the three wire paths are compile-time literals with **no component
//! from the bundle**. The third cannot be: a user with four Claude Desktop
//! accounts has four token caches, and the profile label is what tells them
//! apart. So `keystore/desktop-token-cache/<profile>/<slot>` carries exactly one
//! bundle-chosen component, and it is checked before it means anything —
//! [`plain_component`] admits a single ordinary file name and nothing else, so
//! `..`, an absolute path, a separator, a NUL, a control character and an
//! over-long name are all simply *not a store* rather than a store that
//! resolves somewhere surprising. The slot is one of two fixed names.
//!
//! A store identified by a name the remote chose is otherwise a name that
//! reaches a Keychain service, and there is nothing here worth that: neither
//! Keychain-backed store takes one.
//!
//! # Hermeticity: no test may touch the real login Keychain
//!
//! [`Stores`] is the seam, and it is reached through [`crate::sync::SyncRoots`]
//! — the struct whose entire job is already "every root is injected".
//! [`SyncRoots::at`](crate::sync::SyncRoots::at), which every test in this crate
//! constructs, yields [`Stores::fixture`]; [`SyncRoots::resolve`](crate::sync::SyncRoots::resolve),
//! the single production wrapper, is the **only** place [`Stores::Machine`] is
//! built, and `the_machine_store_is_constructed_in_exactly_one_place` is the
//! structural guard that keeps it so. A test therefore cannot reach a real
//! Keychain by forgetting a seam; it would have to name `Stores::Machine`, and
//! that fails a test in this file.
//!
//! This is not fussiness. The AUR `check()` runs `cargo test` during `makepkg`,
//! so a test that wrote the real `Claude Code-credentials` item would clobber an
//! installer's Claude login at install time — which a previous plan in this
//! milestone did, and the user had to delete the item by hand.
//!
//! # Ceilings
//!
//! ponytail: the *default* Claude Code item only. A
//! `CLAUDE_CONFIG_DIR`-scoped login lives under a per-account service name
//! ([`crate::anthropic::keychain::read_raw_for`]) and does not travel; adding it
//! means a bundle-chosen account name reaching a service-name hash, which wants
//! its own validation. `keystore/claude-code-oauth-account/<name>` is the
//! additive upgrade path — an unknown `keystore/…` entry is refused, never
//! fatal, by every build including the ones already shipped.
//!
//! ponytail: for Claude Desktop this moves the **login** — the OAuth token
//! cache [`crate::anthropic::desktop_creds`] itself reads to authenticate — and
//! not the Electron web-view session. A profile's `desktop-state/` still travels
//! as ordinary files, and the cookie values inside it stay sealed under the
//! pushing Mac's key, so they are inert on the target: the app signs in from the
//! restored token rather than resuming a browser session. Re-encrypting those
//! means walking a Chromium `Cookies` database row by row and a `leveldb`
//! local-storage tree, both of which have their own per-row formats; that is a
//! separate piece of work, and the account is signed in without it.
//!
//! ponytail: [`crate::anthropic::keychain::read_raw`] hands back a plain
//! `String`, so one un-zeroized copy exists inside it before this module wraps
//! the value. Narrowing that is a signature change across `creds.rs`,
//! `cli_account.rs` and the widget; everything *this* module holds is
//! [`Zeroizing`].

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use zeroize::Zeroizing;

use crate::error::{AppError, Result};
use crate::safe_storage;

/// The wire prefix for every machine-bound store. Not a [`crate::sync::SyncRoots`]
/// root, and deliberately not spelled like one.
pub const PREFIX: &str = "keystore";

/// The two wire spellings under [`PREFIX`] that carry no bundle-chosen
/// component at all.
const WIRE_CLAUDE_CODE: &str = "keystore/claude-code-oauth";
const WIRE_CURSOR: &str = "keystore/cursor-auth";
/// The one that does, and everything after it is checked by [`plain_component`]
/// and [`TokenSlot::from_wire`] before it means anything.
const WIRE_DESKTOP: &str = "keystore/desktop-token-cache/";

/// A profile label longer than this is not one claude-acc wrote. The bound is
/// `NAME_MAX` on every mainstream filesystem, matching
/// [`crate::sync::restore::layout`]'s own per-component ceiling.
const MAX_PROFILE_BYTES: usize = 255;

/// Which of Claude Desktop's two token-cache slots. Both are a bare base64
/// safeStorage value in a file of the same name, newest format first — the same
/// pair, and the same order, [`crate::anthropic::desktop_creds`] reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TokenSlot {
    /// `config-tokenCacheV2`.
    V2,
    /// `config-tokenCache`, the older format some profiles still carry.
    V1,
}

impl TokenSlot {
    pub const ALL: [TokenSlot; 2] = [TokenSlot::V2, TokenSlot::V1];

    /// The file name in a claude-acc profile directory — and, being fixed, also
    /// the wire spelling. One literal for both directions.
    pub fn file_name(self) -> &'static str {
        match self {
            TokenSlot::V2 => "config-tokenCacheV2",
            TokenSlot::V1 => "config-tokenCache",
        }
    }

    /// Byte-exact, so `config-tokencachev2` is simply not a slot. A spelling
    /// that does not match is *not written*, so strictness here can only ever
    /// refuse more.
    fn from_wire(s: &str) -> Option<TokenSlot> {
        TokenSlot::ALL.into_iter().find(|s2| s2.file_name() == s)
    }
}

/// One machine-bound credential store.
///
/// No longer `Copy`: the Desktop variant names a profile, and a user with four
/// Claude Desktop accounts has four of them.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Store {
    /// Claude Code's OAuth credential in the macOS login Keychain — generic
    /// password service `Claude Code-credentials`, the item
    /// [`crate::anthropic::keychain::read_raw`] reads.
    ClaudeCodeOauth,
    /// The `cursorAuth/*` rows of Cursor's `state.vscdb`.
    ///
    /// **The one cross-platform store**, and deliberately so: Cursor keeps a
    /// bare JWT in a SQLite key-value table on macOS, Linux and Windows alike,
    /// with no Keychain, no safeStorage and no platform-specific sealing
    /// anywhere in the path. The reason it is a store rather than a file is not
    /// the platform — it is that the credential is 424 bytes inside a 38 MB file
    /// of the *receiving* machine's editor state.
    CursorAuth,
    /// One Claude Desktop profile's token cache, in one of its two slots.
    ///
    /// `profile` is a claude-acc profile directory name. It is the only
    /// bundle-chosen component in this module's whole vocabulary, and
    /// [`plain_component`] has already accepted it wherever this exists.
    DesktopTokenCache { profile: String, slot: TokenSlot },
}

impl Store {
    /// The complete manifest entry for this store.
    pub fn manifest_path(&self) -> String {
        match self {
            Store::ClaudeCodeOauth => WIRE_CLAUDE_CODE.to_string(),
            Store::CursorAuth => WIRE_CURSOR.to_string(),
            Store::DesktopTokenCache { profile, slot } => {
                format!("{WIRE_DESKTOP}{profile}/{}", slot.file_name())
            }
        }
    }

    /// The store a manifest entry names, or `None` for an ordinary file path —
    /// **or for a `keystore/…` entry whose shape this build refuses**, which
    /// [`Store::is_store_path`] still reports as a store so it is skipped rather
    /// than handed to the path resolver.
    ///
    /// Byte-exact, and that is right here for the same reason
    /// [`crate::sync::restore::layout`]'s prefix table is: a spelling that does
    /// not match is *not written*, so folding could only ever admit more.
    pub fn from_manifest_path(s: &str) -> Option<Store> {
        match s {
            WIRE_CLAUDE_CODE => Some(Store::ClaudeCodeOauth),
            WIRE_CURSOR => Some(Store::CursorAuth),
            _ => {
                let (profile, slot) = s.strip_prefix(WIRE_DESKTOP)?.split_once('/')?;
                if !plain_component(profile) {
                    return None;
                }
                Some(Store::DesktopTokenCache {
                    profile: profile.to_string(),
                    slot: TokenSlot::from_wire(slot)?,
                })
            }
        }
    }

    /// Must a failure to *read* this store stop the whole push?
    ///
    /// `true` for the two single stores: a bundle that silently omitted the
    /// Claude Code or Cursor login is the exact defect this module exists to
    /// end, so it must not be reachable by a shrug.
    ///
    /// `false` per Desktop profile, and that is not a weaker rule but a
    /// narrower one. A user with four accounts has four independent
    /// credentials; one profile whose blob will not decrypt — captured under a
    /// key since rotated, or truncated by a half-finished account switch —
    /// must be named and skipped, not allowed to take the other three down with
    /// it. The failure is still reported; it is only not fatal.
    pub fn read_failure_is_fatal(&self) -> bool {
        !matches!(self, Store::DesktopTokenCache { .. })
    }

    /// Does this manifest entry name a store at all? Cheaper than
    /// [`Store::from_manifest_path`] for the "is this a file?" question, and it
    /// answers `true` for a `keystore/…` entry this build does **not** know, so
    /// an unknown one is refused rather than treated as a file path.
    pub fn is_store_path(s: &str) -> bool {
        s.split('/').next() == Some(PREFIX)
    }

    /// What the user is told this item is. No secret, no path, no service name.
    ///
    /// The profile label is bundle data, so it is rendered with `{:?}`, whose
    /// `Debug for str` escapes — the same rule
    /// [`crate::sync::restore::write`] applies to every manifest string it
    /// prints, and the reason a label carrying a terminal escape sequence
    /// cannot rewrite the report the user just consented to.
    pub fn describe(&self) -> String {
        match self {
            Store::ClaudeCodeOauth => {
                "the Claude Code login in this Mac's login Keychain".to_string()
            }
            Store::CursorAuth => "the Cursor sign-in in this machine's Cursor database".to_string(),
            Store::DesktopTokenCache { profile, slot } => format!(
                "the Claude Desktop login for profile {profile:?} ({})",
                slot.file_name()
            ),
        }
    }
}

/// Is `s` a single ordinary file name, and nothing else?
///
/// The check the one bundle-chosen component in this module has to pass before
/// it is joined onto a directory. Deliberately the *same* rule
/// [`crate::sync::restore::layout`] applies per component — `Path::components`
/// yielding exactly one [`Component::Normal`] — rather than a second hand-rolled
/// list of forbidden strings, because that is the rule that has already been
/// adversary-tested and the one that keeps `..`, `/`, `\`, a drive letter and an
/// absolute path out.
///
/// The extra conditions are the ones a `Path` does not have an opinion about
/// *on the platform the check happens to run on*, which is the trap here: a
/// bundle is portable, so a name must be refused on macOS for being dangerous
/// on Windows. Hence the explicit `\` and `:` — `Path::new("C:")` is one
/// ordinary component on Unix and a drive-relative path on Windows, exactly the
/// "absolute path in disguise" [`crate::sync::restore::layout::from_manifest_path`]
/// refuses by name.
///
/// The rest: a NUL or a control character (which reach a terminal, see
/// [`crate::sync::restore::report::safe`]), a leading `.` (a name claude-acc
/// never writes and the shape of `.desktop-state.previous`, which is local
/// rollback state), and a length no filesystem accepts.
fn plain_component(s: &str) -> bool {
    if s.is_empty()
        || s.len() > MAX_PROFILE_BYTES
        || s.starts_with('.')
        || s.contains(|c: char| c.is_control())
        || s.contains('\\')
        || s.contains(':')
    {
        return false;
    }
    let mut components = Path::new(s).components();
    matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(_)), None)
    )
}

/// Injected contents for a [`Stores::Fixture`]. Test-only data, and still no
/// `Debug`: it holds credential material.
#[derive(Default)]
pub struct Fixture {
    creds: BTreeMap<Store, Zeroizing<String>>,
    safe_key: Option<safe_storage::Key>,
    /// Injected "this machine cannot say" — a locked Keychain, a denied ACL.
    /// The only way to reach the failing arm of [`Stores::read`] and
    /// [`Stores::has`] without a real one, and therefore the only way to test
    /// that `sync status` says so rather than reporting a short count.
    unreadable: bool,
}

impl Fixture {
    /// What a store currently holds, as a test asserts on it.
    pub fn get(&self, store: &Store) -> Option<&str> {
        self.creds.get(store).map(|v| v.as_str())
    }

    /// Seed a store, as if the machine already had that credential.
    pub fn set(&mut self, store: Store, value: &str) {
        self.creds.insert(store, Zeroizing::new(value.to_string()));
    }

    /// Seed the Claude Safe Storage key this "machine" has, or `None` for one
    /// that has none.
    pub fn set_safe_key(&mut self, key: Option<safe_storage::Key>) {
        self.safe_key = key;
    }

    /// Make every read and existence check on this "machine" fail, as a locked
    /// login Keychain does.
    pub fn set_unreadable(&mut self, unreadable: bool) {
        self.unreadable = unreadable;
    }

    /// The injected failure, in the shape the real one arrives in.
    fn fail(&self) -> Result<()> {
        if self.unreadable {
            return Err(crate::error::AppError::Credentials(
                "this fixture's credential store is unreadable".into(),
            ));
        }
        Ok(())
    }
}

/// The two files-on-disk stores this machine reads and writes, plus its
/// memoized Claude Safe Storage key.
///
/// Paths rather than resolvers: everything under [`crate::sync`] takes its
/// filesystem roots by injection, and a store is a root like any other. They
/// are filled in exactly once, by [`crate::sync::SyncRoots::resolve`].
///
/// The key is memoized because reading it runs `security(1)`, and a restore
/// asks `writable` once per store — eight subprocess spawns for a user with
/// four Desktop profiles, to answer the same question eight times. `OnceLock`
/// rather than a `Mutex`: it is written once and read many times, and a failed
/// read is a cached `None` rather than a retry loop against a Keychain that has
/// already said no.
pub struct MachinePaths {
    /// Cursor's own `state.vscdb` — [`crate::cursor::db::default_db_path`].
    pub cursor_db: PathBuf,
    /// The claude-acc profile store, `~/.claude-acc/profiles`.
    pub desktop_profiles_dir: PathBuf,
    safe_key: OnceLock<Option<safe_storage::Key>>,
}

impl MachinePaths {
    /// The real machine's paths. Called from one place; see the guard test.
    pub fn new(cursor_db: PathBuf, desktop_profiles_dir: PathBuf) -> Self {
        Self {
            cursor_db,
            desktop_profiles_dir,
            safe_key: OnceLock::new(),
        }
    }

    /// This Mac's derived Claude Safe Storage key, read at most once.
    fn safe_key(&self) -> Option<safe_storage::Key> {
        *self.safe_key.get_or_init(machine_safe_key)
    }
}

/// Where the machine-bound halves are read and written.
///
/// `Clone` shares one [`Fixture`], so a clone of a [`crate::sync::SyncRoots`]
/// sees the writes another clone made — which is what lets a test drive a whole
/// restore and then read back what landed. The `Arc` on the machine side is for
/// the same reason in the other direction: every clone shares the one memoized
/// Safe Storage key rather than re-running `security(1)` per clone.
#[derive(Clone)]
pub enum Stores {
    /// This machine's real stores. **Only** [`crate::sync::SyncRoots::resolve`]
    /// constructs this; see the module docs and the guard test below.
    Machine(Arc<MachinePaths>),
    /// Injected contents. What every `SyncRoots::at` yields, empty by default.
    Fixture(Arc<Mutex<Fixture>>),
}

/// Never the contents, on any variant. Derived `Debug` on a `Stores` reached
/// through `SyncRoots`, which *is* `Debug`, would print credentials into any
/// `{:?}` of a restore context.
impl std::fmt::Debug for Stores {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Stores::Machine(_) => f.write_str("Stores::Machine"),
            Stores::Fixture(_) => f.write_str("Stores::Fixture(<redacted>)"),
        }
    }
}

impl Stores {
    /// An empty injected store set. The default for every test seam.
    pub fn fixture() -> Self {
        Stores::Fixture(Arc::new(Mutex::new(Fixture::default())))
    }

    /// Reach the injected contents. Panics on [`Stores::Machine`], which is
    /// unreachable from a test by construction.
    pub fn edit(&self) -> MutexGuard<'_, Fixture> {
        match self {
            Stores::Machine(_) => panic!("Stores::Machine has no fixture to edit"),
            // A poisoned lock still holds the data a test wants to see, and a
            // second panic here would hide the first one's message.
            Stores::Fixture(state) => state.lock().unwrap_or_else(|e| e.into_inner()),
        }
    }

    /// Every store this machine could carry, in a stable order.
    ///
    /// The push side's enumeration, and the reason a *four-account* Claude
    /// Desktop travels rather than one: the profile set is discovered from the
    /// store on disk, not from a constant. A fixture answers with whatever was
    /// seeded, which is exactly the same question asked of an injected machine.
    ///
    /// `Err` is "this machine could not say", never a short list — the same rule
    /// [`Stores::has`] follows, and for the same reason.
    pub fn all(&self) -> Result<Vec<Store>> {
        match self {
            Stores::Fixture(_) => {
                let fixture = self.edit();
                fixture.fail()?;
                Ok(fixture.creds.keys().cloned().collect())
            }
            Stores::Machine(paths) => Ok(machine_all(paths)),
        }
    }

    /// May this machine write `store` at all?
    ///
    /// `false` on a platform whose Claude Code writes a real file instead, so a
    /// bundle pushed from a Mac is refused rather than failing a restore
    /// part-way through. `false` for a Desktop token cache on a machine with no
    /// Safe Storage key of its own, for the same reason in the other direction:
    /// nothing here could seal the value, so the planner refuses it and says so
    /// rather than the write path stopping the restore at that item.
    pub fn writable(&self, store: &Store) -> bool {
        match self {
            Stores::Fixture(_) => match store {
                // A fixture's "this machine has a key" answer is its injected
                // one, so `set_safe_key(None)` is a target that cannot re-seal.
                Store::DesktopTokenCache { .. } => self.edit().safe_key.is_some(),
                _ => true,
            },
            Stores::Machine(paths) => machine_writable(paths, store),
        }
    }

    /// What `store` holds right now.
    ///
    /// `Ok(None)` is "there is no such credential here" and nothing else. A
    /// locked Keychain or a denied ACL is an `Err`, deliberately: pushing a
    /// bundle that silently omits the credential is the exact failure this
    /// module was written to end, so it must not be reachable by a shrug.
    pub fn read(&self, store: &Store) -> Result<Option<Zeroizing<String>>> {
        match self {
            Stores::Fixture(_) => {
                let fixture = self.edit();
                fixture.fail()?;
                Ok(fixture.get(store).map(|v| Zeroizing::new(v.to_string())))
            }
            Stores::Machine(paths) => machine_read(paths, store),
        }
    }

    /// [`Stores::read`], with a per-item failure folded into "nothing here" for
    /// the stores whose failure must not be fatal — see
    /// [`Store::read_failure_is_fatal`].
    ///
    /// The push side's entry point, so the rule lives in one place rather than
    /// in both the planner and the packer. The failure is still *told*: the
    /// user sees which profile was skipped and why. What it does not do is
    /// abandon the other three accounts, the Claude Code login and the Cursor
    /// session because one blob would not open.
    ///
    /// The message names [`Store::describe`] and the error, and neither of them
    /// is or contains a credential.
    pub fn read_or_skip(&self, store: &Store) -> Result<Option<Zeroizing<String>>> {
        match self.read(store) {
            Err(why) if !store.read_failure_is_fatal() => {
                eprintln!("sync: skipping {} — {why}", store.describe());
                Ok(None)
            }
            other => other,
        }
    }

    /// Does `store` hold a credential — established **without reading it**?
    ///
    /// The question `sync status` asks. `status` walks the filesystem, and a
    /// store is not a file, so before this existed it reported one credential
    /// fewer than `push --dry-run` planned — and the missing one was the Claude
    /// Code login, the most sensitive item in the bundle (the whole reason to
    /// look at what sync carries).
    ///
    /// Existence, not value, and the distinction is what makes it payable on
    /// that path: [`crate::anthropic::keychain::has_raw`] never asks the
    /// Keychain for the secret, so it cannot raise the ACL prompt a
    /// [`Stores::read`] can — and the macOS menu bar runs `sync status --json`
    /// on every menu open. Nothing here needs a password or a network.
    ///
    /// `Err` is "this machine could not say", never a shrug: `sync status`
    /// turns it into [`WARN_KEYSTORE_UNAVAILABLE`](crate::sync::report::WARN_KEYSTORE_UNAVAILABLE)
    /// rather than a quietly short count.
    pub fn has(&self, store: &Store) -> Result<bool> {
        match self {
            Stores::Fixture(_) => {
                let fixture = self.edit();
                fixture.fail()?;
                Ok(fixture.get(store).is_some_and(|v| !v.is_empty()))
            }
            Stores::Machine(paths) => machine_has(paths, store),
        }
    }

    /// Replace what `store` holds.
    ///
    /// Whole-value, never incremental: the underlying write either replaces the
    /// item or fails, so a failure leaves the existing credential exactly as it
    /// was. Nothing here is a read-modify-write.
    pub fn write(&self, store: &Store, value: &str) -> Result<()> {
        match self {
            Stores::Fixture(_) => {
                self.edit().set(store.clone(), value);
                Ok(())
            }
            Stores::Machine(paths) => machine_write(paths, store, value),
        }
    }

    /// This machine's Claude Safe Storage key, or `None` where there is none to
    /// read — no Keychain item, no Claude Desktop, or not macOS at all.
    ///
    /// Lives here rather than in `restore::merge` so the *one* rule holds for
    /// every machine-bound secret: production reads it, a fixture injects it,
    /// and no test can reach the real login Keychain.
    pub fn safe_key(&self) -> Option<safe_storage::Key> {
        match self {
            Stores::Fixture(_) => self.edit().safe_key,
            Stores::Machine(paths) => paths.safe_key(),
        }
    }
}

/// The platform half of [`Stores::writable`], named rather than inlined so a
/// test can assert it without constructing a [`Stores::Machine`] — which it may
/// not, and which is the whole hermeticity rule of this module.
///
/// Three different answers, for three different reasons:
///
/// - **Claude Code OAuth**: only where the credential is a Keychain item.
///   Elsewhere Claude Code writes `~/.claude/.credentials.json`, which `scope`
///   collects as an ordinary file, so a store arriving there is refused in the
///   planner rather than failing part-way through a restore.
/// - **Cursor**: everywhere Cursor has a database to write into. Nothing in that
///   path is macOS-shaped — a plaintext JWT in a SQLite key-value table — so
///   gating it on the platform would refuse a working restore for no reason.
///   The database's *existence* is the real condition: this build will not
///   fabricate one (see [`crate::cursor::db::write_auth_rows`]).
/// - **Desktop token cache**: only where there is a Safe Storage key to re-seal
///   the value with. Off-Mac there is none, and on a Mac without Claude Desktop
///   there is none either; both are the same refusal.
fn machine_writable(paths: &MachinePaths, store: &Store) -> bool {
    match store {
        Store::ClaudeCodeOauth => cfg!(target_os = "macos"),
        Store::CursorAuth => paths.cursor_db.exists(),
        Store::DesktopTokenCache { .. } => paths.safe_key().is_some(),
    }
}

/// Every store this machine could carry, sorted so the wire order is stable
/// across runs and machines.
fn machine_all(paths: &MachinePaths) -> Vec<Store> {
    // Cursor is the one store with no platform condition; see
    // [`machine_writable`].
    let mut out = vec![Store::CursorAuth];
    if cfg!(target_os = "macos") {
        out.push(Store::ClaudeCodeOauth);
        // Only where a Safe Storage key could exist. Elsewhere every one of
        // these would be enumerated, then read, then fail for want of a key,
        // then be skipped with a line on stderr — a warning about a credential
        // the platform never had.
        //
        // A *directory listing*, deliberately, and not `MachinePaths::safe_key`:
        // `sync status` reaches this, the macOS menu bar runs `sync status
        // --json` on every menu open, and reading the Safe Storage key runs
        // `security(1)` against an item whose ACL does not name it — which can
        // raise a Keychain prompt. Existence is answered with a stat.
        out.extend(desktop_caches(&paths.desktop_profiles_dir));
    }
    out.sort();
    out
}

/// One [`Store::DesktopTokenCache`] per profile per slot that actually exists.
///
/// Listed from the profile store rather than from a constant — that is what
/// makes four Claude Desktop accounts travel where one used to. A directory
/// whose name is not a [`plain_component`] is skipped: it cannot be spelled on
/// the wire, so carrying it would produce an entry no restore could read back.
fn desktop_caches(profiles_dir: &Path) -> Vec<Store> {
    let Ok(entries) = std::fs::read_dir(profiles_dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        // `read_dir`'s file type does not traverse a link, so a symlinked
        // profile is not followed — the same rule `scope`'s walker applies.
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let dir = entry.path();
        let Some(profile) = dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !plain_component(profile) {
            continue;
        }
        for slot in TokenSlot::ALL {
            if dir.join(slot.file_name()).is_file() {
                out.push(Store::DesktopTokenCache {
                    profile: profile.to_string(),
                    slot,
                });
            }
        }
    }
    out
}

fn machine_read(paths: &MachinePaths, store: &Store) -> Result<Option<Zeroizing<String>>> {
    match store {
        Store::ClaudeCodeOauth => claude_code_read(),
        Store::CursorAuth => cursor_read(&paths.cursor_db),
        Store::DesktopTokenCache { profile, slot } => desktop_read(
            &paths.desktop_profiles_dir,
            profile,
            *slot,
            paths.safe_key(),
        ),
    }
}

fn machine_has(paths: &MachinePaths, store: &Store) -> Result<bool> {
    match store {
        Store::ClaudeCodeOauth => claude_code_has(),
        Store::CursorAuth => crate::cursor::db::has_auth_rows(&paths.cursor_db),
        // A stat, and never a decrypt: `sync status` may not ask the Keychain
        // for a secret, and this answers without one.
        Store::DesktopTokenCache { profile, slot } => Ok(paths
            .desktop_profiles_dir
            .join(profile)
            .join(slot.file_name())
            .metadata()
            .is_ok_and(|md| md.is_file() && md.len() > 0)),
    }
}

fn machine_write(paths: &MachinePaths, store: &Store, value: &str) -> Result<()> {
    match store {
        Store::ClaudeCodeOauth => claude_code_write(store, value),
        Store::CursorAuth => cursor_write(&paths.cursor_db, value),
        Store::DesktopTokenCache { profile, slot } => desktop_write(
            &paths.desktop_profiles_dir,
            profile,
            *slot,
            paths.safe_key(),
            value,
        ),
    }
}

// ---------------------------------------------------------------------------
// Cursor. Path-injected and therefore platform-independent and testable
// everywhere, which is also why `CursorAuth` is the one store with no `cfg`.
// ---------------------------------------------------------------------------

/// The `cursorAuth/*` rows as one JSON object.
///
/// `BTreeMap` all the way through, so the same login serialises to the same
/// bytes twice and `merge::decide_store`'s digest comparison can answer
/// "identical" rather than asking the user about a credential that did not
/// change.
fn cursor_read(db: &Path) -> Result<Option<Zeroizing<String>>> {
    let rows = crate::cursor::db::read_auth_rows(db)?;
    if rows.is_empty() {
        return Ok(None);
    }
    let json = serde_json::to_string(&rows)
        .map_err(|e| AppError::Other(format!("could not encode the Cursor sign-in: {e}")))?;
    Ok(Some(Zeroizing::new(json)))
}

/// The same object, back into the target's own database.
///
/// The parse error names no value: a malformed store payload is a fact about
/// the bundle, and `serde_json`'s message for one can quote the input.
fn cursor_write(db: &Path, value: &str) -> Result<()> {
    let rows: BTreeMap<String, String> = serde_json::from_str(value).map_err(|_| {
        AppError::Credentials(
            "the snapshot's Cursor sign-in is not the object this build writes — refusing it"
                .into(),
        )
    })?;
    if rows.is_empty() {
        return Err(AppError::Credentials(
            "the snapshot's Cursor sign-in is empty — refusing to replace a live one with it"
                .into(),
        ));
    }
    crate::cursor::db::write_auth_rows(db, &rows)
}

// ---------------------------------------------------------------------------
// Claude Desktop token caches. The *transform* is pure — a key in, a key out —
// so only `machine_safe_key` is macOS-gated and these two are exercised by
// tests on every platform, exactly as `safe_storage`'s own round trip is.
// ---------------------------------------------------------------------------

/// Decrypt one profile's token cache with **this** machine's key.
///
/// A missing or empty file is `Ok(None)`: that profile simply has no login in
/// that slot. Anything else is an `Err`, which [`Stores::read_or_skip`] turns
/// into "this one profile is skipped, and here is why" rather than a failed
/// push.
///
/// Nothing here renders the plaintext, the ciphertext or the key. The `Err`
/// arms name the store's description and, for a decrypt, `safe_storage`'s own
/// message, which carries a padding failure and no data.
fn desktop_read(
    profiles_dir: &Path,
    profile: &str,
    slot: TokenSlot,
    key: Option<safe_storage::Key>,
) -> Result<Option<Zeroizing<String>>> {
    let path = profiles_dir.join(profile).join(slot.file_name());
    let Ok(raw) = std::fs::read_to_string(&path).map(Zeroizing::new) else {
        return Ok(None);
    };
    let value = raw.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if !safe_storage::looks_like_value(value) {
        return Err(AppError::Credentials(format!(
            "{} is not a Safe Storage value — refusing to carry it as one",
            profile_label(profile, slot)
        )));
    }
    let key = key.ok_or_else(|| {
        AppError::Credentials(format!(
            "{} is sealed, and this machine has no Claude Safe Storage key to open it with",
            profile_label(profile, slot)
        ))
    })?;
    let plain = Zeroizing::new(safe_storage::decrypt(&key, value)?);
    let text = std::str::from_utf8(&plain).map_err(|_| {
        AppError::Credentials(format!(
            "{} did not decrypt to text — refusing to carry it",
            profile_label(profile, slot)
        ))
    })?;
    Ok(Some(Zeroizing::new(text.to_string())))
}

/// Re-seal one profile's token cache with the **target's** key and put it where
/// Claude Desktop reads it.
///
/// No key is a refusal, and it is the refusal that matters: a plaintext token
/// cache written where the app expects ciphertext is both a leak and a file the
/// app cannot read. The existing file is untouched on every failure path — the
/// encryption happens before the write, and the write itself is
/// [`crate::cache::atomic_write`]'s tempfile-and-rename, whose tempfile is
/// created 0600 and keeps that mode across the rename.
fn desktop_write(
    profiles_dir: &Path,
    profile: &str,
    slot: TokenSlot,
    key: Option<safe_storage::Key>,
    value: &str,
) -> Result<()> {
    if !plain_component(profile) {
        return Err(AppError::Credentials(format!(
            "refusing to write a Claude Desktop login for the profile name {profile:?}"
        )));
    }
    let key = key.ok_or_else(|| {
        AppError::Credentials(format!(
            "this machine has no Claude Safe Storage key, so {} cannot be sealed for it — \
             install and sign in to Claude Desktop once, then restore again",
            profile_label(profile, slot)
        ))
    })?;
    let sealed = safe_storage::encrypt(&key, value.as_bytes());
    crate::cache::atomic_write(
        &profiles_dir.join(profile).join(slot.file_name()),
        sealed.as_bytes(),
    )
}

/// A Desktop cache named for a message. The profile label is bundle data on the
/// restore side, so `{:?}` escapes it.
fn profile_label(profile: &str, slot: TokenSlot) -> String {
    format!(
        "the Claude Desktop login for profile {profile:?} ({})",
        slot.file_name()
    )
}

// ---------------------------------------------------------------------------
// Claude Code's Keychain item. The one store that is macOS or nothing.
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn claude_code_read() -> Result<Option<Zeroizing<String>>> {
    Ok(crate::anthropic::keychain::read_raw()?.map(Zeroizing::new))
}

#[cfg(target_os = "macos")]
fn claude_code_has() -> Result<bool> {
    crate::anthropic::keychain::has_raw()
}

#[cfg(target_os = "macos")]
fn claude_code_write(_store: &Store, value: &str) -> Result<()> {
    crate::anthropic::keychain::write_raw(value)
}

#[cfg(target_os = "macos")]
fn machine_safe_key() -> Option<safe_storage::Key> {
    safe_storage::macos_key().ok()
}

/// Not macOS: Claude Code writes `~/.claude/.credentials.json`, which the
/// collectors carry as an ordinary file, and there is no Keychain to consult.
#[cfg(not(target_os = "macos"))]
fn claude_code_read() -> Result<Option<Zeroizing<String>>> {
    Ok(None)
}

#[cfg(not(target_os = "macos"))]
fn claude_code_has() -> Result<bool> {
    Ok(false)
}

#[cfg(not(target_os = "macos"))]
fn claude_code_write(store: &Store, _value: &str) -> Result<()> {
    Err(AppError::Credentials(format!(
        "this build has no {} to write",
        store.describe()
    )))
}

/// Chromium's safeStorage is a different scheme backed by a different key store
/// off-Mac, and [`safe_storage::macos_key`] does not exist there.
#[cfg(not(target_os = "macos"))]
fn machine_safe_key() -> Option<safe_storage::Key> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Two machines' Safe Storage keys, derived from fixed fake secrets. Never
    /// a real Keychain read — the hermeticity rule this module is built around.
    fn this_mac() -> safe_storage::Key {
        safe_storage::derive_key(b"this-mac-not-a-real-secret")
    }
    fn other_mac() -> safe_storage::Key {
        safe_storage::derive_key(b"other-mac-not-a-real-secret")
    }

    fn desktop(profile: &str, slot: TokenSlot) -> Store {
        Store::DesktopTokenCache {
            profile: profile.to_string(),
            slot,
        }
    }

    /// Seed one profile's sealed token cache, as claude-acc wrote it.
    fn seal_into(dir: &Path, profile: &str, slot: TokenSlot, key: &safe_storage::Key, plain: &str) {
        let path = dir.join(profile).join(slot.file_name());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, safe_storage::encrypt(key, plain.as_bytes())).unwrap();
    }

    /// Nothing about a store's wire name may collide with another's, and every
    /// one of them must round trip through the spelling it emits.
    #[test]
    fn every_store_has_its_own_wire_name_under_the_prefix() {
        let stores = [
            Store::ClaudeCodeOauth,
            Store::CursorAuth,
            desktop("gmail", TokenSlot::V2),
            desktop("gmail", TokenSlot::V1),
            desktop("toptal", TokenSlot::V2),
        ];
        let mut seen: Vec<String> = Vec::new();
        for store in &stores {
            let path = store.manifest_path();
            assert!(
                path.starts_with(&format!("{PREFIX}/")),
                "{path} is not under the keystore prefix"
            );
            assert!(Store::is_store_path(&path));
            assert_eq!(Store::from_manifest_path(&path).as_ref(), Some(store));
            assert!(!seen.contains(&path), "{path} names two stores");
            seen.push(path);
        }
        assert_eq!(
            seen[2], "keystore/desktop-token-cache/gmail/config-tokenCacheV2",
            "the wire spelling is part of the format"
        );
    }

    /// An ordinary manifest path is not a store, and a `keystore/…` entry this
    /// build does not know is still recognised as one — so `merge` refuses it
    /// rather than handing it to the path resolver as a file.
    #[test]
    fn an_unknown_keystore_entry_is_a_store_path_with_no_store() {
        assert!(!Store::is_store_path("config/config.toml"));
        assert!(Store::from_manifest_path("config/config.toml").is_none());

        assert!(Store::is_store_path("keystore/from-a-later-version"));
        assert!(Store::from_manifest_path("keystore/from-a-later-version").is_none());

        // A path that merely *starts with the letters* is not the prefix.
        assert!(!Store::is_store_path("keystores/claude-code-oauth"));
    }

    /// **The one bundle-chosen component in this module.** Every shape that
    /// could resolve somewhere other than one directory under the profile store
    /// must be *not a store at all*, so it is never written and never resolved.
    #[test]
    fn a_profile_name_that_is_not_one_plain_directory_name_is_not_a_store() {
        for hostile in [
            "..",
            ".",
            "../../.ssh",
            "a/b",
            "a\\b",
            "/etc",
            "C:",
            "",
            ".desktop-state.previous",
            "with\nnewline",
            "with\u{1b}[2Jescape",
        ] {
            let wire = format!("keystore/desktop-token-cache/{hostile}/config-tokenCacheV2");
            assert!(
                Store::from_manifest_path(&wire).is_none(),
                "{hostile:?} was accepted as a profile name"
            );
            assert!(
                Store::is_store_path(&wire),
                "{hostile:?} must still be recognised as a store entry, so it is skipped \
                 rather than resolved as a file path"
            );
        }
        assert!(!plain_component(&"x".repeat(MAX_PROFILE_BYTES + 1)));
        // And the ordinary ones still work.
        for ok in ["gmail", "hotmail", "struct", "toptal", "work-2", "a.b"] {
            assert!(plain_component(ok), "{ok:?} is a real claude-acc label");
        }
    }

    /// The slot is one of two fixed names and nothing else.
    #[test]
    fn only_the_two_real_token_cache_slots_name_a_store() {
        assert!(
            Store::from_manifest_path("keystore/desktop-token-cache/x/config-tokenCache").is_some()
        );
        assert!(
            Store::from_manifest_path("keystore/desktop-token-cache/x/config-tokencachev2")
                .is_none(),
            "byte-exact: a spelling that does not match is not written"
        );
        assert!(Store::from_manifest_path("keystore/desktop-token-cache/x/anything").is_none());
        assert!(Store::from_manifest_path("keystore/desktop-token-cache/x").is_none());
    }

    #[test]
    fn a_fixture_round_trips_a_value_and_shares_it_across_clones() {
        let stores = Stores::fixture();
        assert!(stores.read(&Store::ClaudeCodeOauth).unwrap().is_none());

        let clone = stores.clone();
        clone
            .write(&Store::ClaudeCodeOauth, r#"{"claudeAiOauth":{}}"#)
            .unwrap();
        assert_eq!(
            stores
                .read(&Store::ClaudeCodeOauth)
                .unwrap()
                .map(|v| v.to_string()),
            Some(r#"{"claudeAiOauth":{}}"#.to_string())
        );
    }

    /// The whole value is replaced, so a store never holds a splice of two
    /// credentials.
    #[test]
    fn a_second_write_replaces_the_value_rather_than_merging_it() {
        let stores = Stores::fixture();
        stores.write(&Store::ClaudeCodeOauth, "first").unwrap();
        stores.write(&Store::ClaudeCodeOauth, "second").unwrap();
        assert_eq!(stores.edit().get(&Store::ClaudeCodeOauth), Some("second"));
    }

    /// `SyncRoots` is `Debug`, so a derived `Debug` here would print a live
    /// OAuth token into any `{:?}` of a restore context.
    #[test]
    fn debug_never_renders_a_stored_credential() {
        let stores = Stores::fixture();
        stores
            .write(&Store::ClaudeCodeOauth, "sk-ant-oat01-NEVER-PRINT-ME")
            .unwrap();
        stores
            .write(&Store::CursorAuth, "eyJh-NEVER-PRINT-ME")
            .unwrap();
        let rendered = format!("{stores:?}");
        assert!(!rendered.contains("NEVER-PRINT-ME"), "{rendered}");
        assert!(!rendered.contains("sk-ant"), "{rendered}");
    }

    /// A store's own description is what every message about it is built from,
    /// so it may never be built from the value.
    #[test]
    fn a_description_names_the_store_and_escapes_a_bundle_chosen_label() {
        assert!(Store::CursorAuth.describe().contains("Cursor"));
        let label = desktop("wo\"rk", TokenSlot::V2).describe();
        assert!(label.contains("Claude Desktop"), "{label}");
        assert!(label.contains("\\\""), "the label is escaped: {label}");
    }

    /// Claude Code's Keychain item is macOS or nothing; Cursor's database is
    /// not macOS-shaped at all and must not be gated as if it were.
    #[test]
    fn writability_follows_the_reason_each_store_is_machine_bound() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("state.vscdb");
        let paths = MachinePaths::new(db.clone(), dir.path().join("profiles"));

        assert_eq!(
            machine_writable(&paths, &Store::ClaudeCodeOauth),
            cfg!(target_os = "macos")
        );
        // Cursor: the database's existence is the condition, not the platform.
        assert!(!machine_writable(&paths, &Store::CursorAuth));
        std::fs::write(&db, b"").unwrap();
        assert!(machine_writable(&paths, &Store::CursorAuth));

        // A fixture always is, for everything but a Desktop cache with no key —
        // that is what makes "the target cannot re-seal" testable.
        let stores = Stores::fixture();
        assert!(stores.writable(&Store::CursorAuth));
        assert!(!stores.writable(&desktop("gmail", TokenSlot::V2)));
        stores.edit().set_safe_key(Some(this_mac()));
        assert!(stores.writable(&desktop("gmail", TokenSlot::V2)));
    }

    /// Four accounts, both slots each, discovered from the store on disk —
    /// which is what makes a four-Claude Mac travel rather than one login.
    #[test]
    fn every_profile_and_slot_on_disk_is_enumerated() {
        let dir = TempDir::new().unwrap();
        let profiles = dir.path().join("profiles");
        for label in ["gmail", "hotmail", "struct", "toptal"] {
            seal_into(&profiles, label, TokenSlot::V2, &this_mac(), "{}");
        }
        seal_into(&profiles, "gmail", TokenSlot::V1, &this_mac(), "{}");
        // Not a profile: a file, and a name that cannot be spelled on the wire.
        std::fs::write(profiles.join("loose-file"), b"x").unwrap();
        seal_into(&profiles, ".hidden", TokenSlot::V2, &this_mac(), "{}");

        // `desktop_caches` rather than `machine_all`: the enumeration is pure
        // and runs the same everywhere, while `machine_all` gates the Keychain-
        // shaped stores on the platform (asserted separately below).
        let mut found = desktop_caches(&profiles);
        found.sort();
        let wire: Vec<String> = found.iter().map(Store::manifest_path).collect();
        for label in ["gmail", "hotmail", "struct", "toptal"] {
            assert!(
                wire.contains(&format!(
                    "keystore/desktop-token-cache/{label}/config-tokenCacheV2"
                )),
                "{label} did not travel: {wire:?}"
            );
        }
        assert!(wire.contains(&"keystore/desktop-token-cache/gmail/config-tokenCache".to_string()));
        assert!(
            !wire.iter().any(|w| w.contains(".hidden")),
            "a name that cannot be spelled on the wire must not be carried: {wire:?}"
        );
        // Stable order, so a bundle's manifest does not churn between runs.
        let mut sorted = found.clone();
        sorted.sort();
        assert_eq!(found, sorted);

        // And the platform gate on the whole enumeration: Cursor always, the
        // two Keychain-shaped kinds only on a Mac.
        let all = machine_all(&MachinePaths::new(dir.path().join("db"), profiles));
        assert!(all.contains(&Store::CursorAuth));
        assert_eq!(
            all.contains(&Store::ClaudeCodeOauth),
            cfg!(target_os = "macos")
        );
        assert_eq!(
            all.iter()
                .any(|s| matches!(s, Store::DesktopTokenCache { .. })),
            cfg!(target_os = "macos"),
            "off-Mac these would be read, fail for want of a key, and warn"
        );
    }

    /// **The whole Claude Desktop feature, in one assertion.** A blob sealed
    /// under one Mac's key opens on a second Mac that has never seen that key,
    /// because the value crossed as plaintext inside the (already encrypted)
    /// bundle and was re-sealed on arrival.
    #[test]
    fn a_desktop_cache_sealed_by_one_mac_opens_on_another() {
        let source = TempDir::new().unwrap();
        let target = TempDir::new().unwrap();
        let secret = r#"{"anthropic:org:aud:user:inference":{"accessToken":"sk-ant-oat01-x"}}"#;
        seal_into(source.path(), "gmail", TokenSlot::V2, &this_mac(), secret);

        // Push: decrypt with the machine that has the key.
        let carried = desktop_read(source.path(), "gmail", TokenSlot::V2, Some(this_mac()))
            .unwrap()
            .expect("the source Mac can open its own blob");
        assert_eq!(carried.as_str(), secret);

        // Restore: re-seal under a key the source machine never had.
        desktop_write(
            target.path(),
            "gmail",
            TokenSlot::V2,
            Some(other_mac()),
            &carried,
        )
        .unwrap();

        let landed = target.path().join("gmail").join("config-tokenCacheV2");
        let bytes = std::fs::read_to_string(&landed).unwrap();
        assert!(
            safe_storage::looks_like_value(&bytes),
            "it must land sealed, not in plaintext"
        );
        assert!(
            !bytes.contains("sk-ant"),
            "the token is not on disk in clear"
        );
        // The target opens it with its own key, and only with its own key.
        assert_eq!(
            desktop_read(target.path(), "gmail", TokenSlot::V2, Some(other_mac()))
                .unwrap()
                .map(|v| v.to_string()),
            Some(secret.to_string())
        );
        assert!(desktop_read(target.path(), "gmail", TokenSlot::V2, Some(this_mac())).is_err());
    }

    /// A target with no Safe Storage key refuses, and refusing costs the
    /// machine nothing it already had.
    #[test]
    fn a_target_with_no_key_refuses_and_leaves_the_existing_login_alone() {
        let target = TempDir::new().unwrap();
        seal_into(target.path(), "gmail", TokenSlot::V2, &other_mac(), "mine");
        let landed = target.path().join("gmail").join("config-tokenCacheV2");
        let before = std::fs::read(&landed).unwrap();

        let err = desktop_write(target.path(), "gmail", TokenSlot::V2, None, "theirs")
            .expect_err("nothing here can seal it");
        assert!(err.to_string().contains("Safe Storage key"), "{err}");
        assert!(!err.to_string().contains("theirs"), "no value in a message");
        assert_eq!(std::fs::read(&landed).unwrap(), before);
    }

    /// The read side's refusals, none of which may render a byte of the value.
    #[test]
    fn a_desktop_cache_that_is_absent_empty_or_not_sealed_is_told_apart() {
        let dir = TempDir::new().unwrap();
        // Absent: this profile simply has no login in this slot.
        assert!(
            desktop_read(dir.path(), "gmail", TokenSlot::V1, Some(this_mac()))
                .unwrap()
                .is_none()
        );
        // Empty: the same answer.
        seal_into(dir.path(), "gmail", TokenSlot::V1, &this_mac(), "");
        std::fs::write(dir.path().join("gmail").join("config-tokenCache"), "").unwrap();
        assert!(
            desktop_read(dir.path(), "gmail", TokenSlot::V1, Some(this_mac()))
                .unwrap()
                .is_none()
        );
        // Present but not a Safe Storage value: refused rather than carried as
        // one, because carrying it would write a plaintext token on the target.
        std::fs::write(
            dir.path().join("gmail").join("config-tokenCache"),
            "SECRET-IN-THE-CLEAR",
        )
        .unwrap();
        let err = desktop_read(dir.path(), "gmail", TokenSlot::V1, Some(this_mac())).unwrap_err();
        assert!(
            err.to_string().contains("not a Safe Storage value"),
            "{err}"
        );
        assert!(!err.to_string().contains("SECRET-IN-THE-CLEAR"), "{err}");
        // Sealed, but this machine has no key: told, not silently skipped.
        seal_into(dir.path(), "gmail", TokenSlot::V1, &other_mac(), "x");
        assert!(desktop_read(dir.path(), "gmail", TokenSlot::V1, None).is_err());
    }

    /// One profile that will not open must not take the other three down.
    #[test]
    fn a_failing_desktop_profile_is_skipped_while_a_failing_single_store_is_fatal() {
        assert!(!desktop("gmail", TokenSlot::V2).read_failure_is_fatal());
        assert!(Store::ClaudeCodeOauth.read_failure_is_fatal());
        assert!(Store::CursorAuth.read_failure_is_fatal());

        let stores = Stores::fixture();
        stores.edit().set(desktop("gmail", TokenSlot::V2), "v");
        stores.edit().set(Store::CursorAuth, "v");
        stores.edit().set_unreadable(true);

        assert!(
            stores
                .read_or_skip(&desktop("gmail", TokenSlot::V2))
                .unwrap()
                .is_none(),
            "one unreadable profile is skipped"
        );
        assert!(
            stores.read_or_skip(&Store::CursorAuth).is_err(),
            "a bundle that silently omitted the Cursor login is the defect this ends"
        );
    }

    /// The Cursor store is the rows, encoded once and decoded back — and an
    /// empty payload never replaces a live login with nothing.
    #[test]
    fn the_cursor_store_round_trips_the_rows_through_a_real_database() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("source.vscdb");
        let target = dir.path().join("target.vscdb");
        for path in [&source, &target] {
            let conn = rusqlite::Connection::open(path).unwrap();
            conn.execute(
                "CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO ItemTable (key, value) VALUES ('workbench.state', ?1)",
                [path.to_string_lossy().to_string()],
            )
            .unwrap();
        }
        let conn = rusqlite::Connection::open(&source).unwrap();
        for (k, v) in [
            ("cursorAuth/accessToken", "eyJhbGciOi.source"),
            ("cursorAuth/refreshToken", "refresh.source"),
            ("cursorAuth/cachedEmail", "person@example.com"),
        ] {
            conn.execute(
                "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
                rusqlite::params![k, v],
            )
            .unwrap();
        }
        drop(conn);

        let carried = cursor_read(&source).unwrap().expect("a signed-in machine");
        cursor_write(&target, &carried).unwrap();

        assert_eq!(
            crate::cursor::db::read_access_token(&target).unwrap(),
            "eyJhbGciOi.source"
        );
        let rows = crate::cursor::db::read_auth_rows(&target).unwrap();
        assert_eq!(rows.len(), 3);
        // The target's own editor state is exactly as it was.
        assert_eq!(
            rusqlite::Connection::open(&target)
                .unwrap()
                .query_row(
                    "SELECT value FROM ItemTable WHERE key = 'workbench.state'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            target.to_string_lossy()
        );

        // Same bytes twice: an unchanged login must hash the same, or every
        // push would look like a rotated credential.
        assert_eq!(
            cursor_read(&source).unwrap().map(|v| v.to_string()),
            Some(carried.to_string())
        );

        // A machine that never signed in has nothing to carry.
        assert!(
            cursor_read(&dir.path().join("absent.vscdb"))
                .unwrap()
                .is_none()
        );
        // And nothing is what a live login is never replaced with.
        assert!(cursor_write(&target, "{}").is_err());
        assert!(cursor_write(&target, "not json").is_err());
        assert_eq!(
            crate::cursor::db::read_access_token(&target).unwrap(),
            "eyJhbGciOi.source",
            "a refused write leaves the existing login exactly as it was"
        );
    }

    /// **The hermeticity guard.** `Stores::Machine` is the only door to a real
    /// login Keychain, a real Cursor database and a real profile store, and it
    /// may be opened in exactly one production place — `SyncRoots::resolve` —
    /// so no test can reach one by forgetting a seam.
    ///
    /// A list of files that may name it, rather than a list of files to check:
    /// the sibling guards in this module tree learned that lesson twice (see
    /// [`crate::sync::guard`]), and a new file naming `Stores::Machine` must
    /// fail here rather than quietly work.
    #[test]
    fn the_machine_store_is_constructed_in_exactly_one_place() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let allowed = [
            root.join("src/sync/mod.rs"),      // SyncRoots::resolve
            root.join("src/sync/keystore.rs"), // the definition and this guard
        ];
        let mut found = 0usize;
        for path in crate::sync::guard::rs_files_in("src/sync") {
            let source = std::fs::read_to_string(&path).expect("readable module");
            let production = crate::sync::guard::production_code(&source);
            if !production.contains("Stores::Machine") && !production.contains("MachinePaths::new")
            {
                continue;
            }
            found += 1;
            assert!(
                allowed.contains(&path),
                "{} names Stores::Machine — a real login Keychain is reachable from it",
                path.display()
            );
        }
        assert_eq!(found, 2, "the guard walked the wrong tree");
    }
}
