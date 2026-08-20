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
//! *that machine's* login Keychain. The fix for both shapes is the same:
//!
//! | store | push | restore |
//! |---|---|---|
//! | Claude Code OAuth | read the Keychain item | write the Keychain item |
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
//! The whole path is a compile-time literal with **no component from the
//! bundle**. A store identified by a name the remote chose would be a name that
//! reaches a Keychain service, and there is nothing here worth that.
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
//! ponytail: one store, the *default* Claude Code item. A
//! `CLAUDE_CONFIG_DIR`-scoped login lives under a per-account service name
//! ([`crate::anthropic::keychain::read_raw_for`]) and does not travel; adding it
//! means a bundle-chosen account name reaching a service-name hash, which wants
//! its own validation. `keystore/claude-code-oauth-account/<name>` is the
//! additive upgrade path — an unknown `keystore/…` entry is refused, never
//! fatal, by every build including the ones already shipped.
//!
//! ponytail: [`crate::anthropic::keychain::read_raw`] hands back a plain
//! `String`, so one un-zeroized copy exists inside it before this module wraps
//! the value. Narrowing that is a signature change across `creds.rs`,
//! `cli_account.rs` and the widget; everything *this* module holds is
//! [`Zeroizing`].

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use zeroize::Zeroizing;

use crate::error::Result;
use crate::safe_storage;

/// The wire prefix for every machine-bound store. Not a [`crate::sync::SyncRoots`]
/// root, and deliberately not spelled like one.
pub const PREFIX: &str = "keystore";

/// One machine-bound credential store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Store {
    /// Claude Code's OAuth credential in the macOS login Keychain — generic
    /// password service `Claude Code-credentials`, the item
    /// [`crate::anthropic::keychain::read_raw`] reads.
    ClaudeCodeOauth,
}

impl Store {
    /// Every store this build knows, in wire order.
    pub const ALL: [Store; 1] = [Store::ClaudeCodeOauth];

    /// The complete manifest entry for this store. A fixed string: nothing in
    /// it is derived from anything a bundle carries.
    pub fn manifest_path(self) -> &'static str {
        match self {
            Store::ClaudeCodeOauth => "keystore/claude-code-oauth",
        }
    }

    /// The store a manifest entry names, or `None` for an ordinary file path.
    ///
    /// Byte-exact, and that is right here for the same reason
    /// [`crate::sync::restore::layout`]'s prefix table is: a spelling that does
    /// not match is *not written*, so folding could only ever admit more.
    pub fn from_manifest_path(s: &str) -> Option<Store> {
        Store::ALL.into_iter().find(|st| st.manifest_path() == s)
    }

    /// Does this manifest entry name a store at all? Cheaper than
    /// [`Store::from_manifest_path`] for the "is this a file?" question, and it
    /// answers `true` for a `keystore/…` entry this build does **not** know, so
    /// an unknown one is refused rather than treated as a file path.
    pub fn is_store_path(s: &str) -> bool {
        s.split('/').next() == Some(PREFIX)
    }

    /// What the user is told this item is. No secret, no path, no service name.
    pub fn describe(self) -> &'static str {
        match self {
            Store::ClaudeCodeOauth => "the Claude Code login in this Mac's login Keychain",
        }
    }
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
    pub fn get(&self, store: Store) -> Option<&str> {
        self.creds.get(&store).map(|v| v.as_str())
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

/// Where the machine-bound halves are read and written.
///
/// `Clone` shares one [`Fixture`], so a clone of a [`crate::sync::SyncRoots`]
/// sees the writes another clone made — which is what lets a test drive a whole
/// restore and then read back what landed.
#[derive(Clone)]
pub enum Stores {
    /// This machine's real stores. **Only** [`crate::sync::SyncRoots::resolve`]
    /// constructs this; see the module docs and the guard test below.
    Machine,
    /// Injected contents. What every `SyncRoots::at` yields, empty by default.
    Fixture(Arc<Mutex<Fixture>>),
}

/// Never the contents, on any variant. Derived `Debug` on a `Stores` reached
/// through `SyncRoots`, which *is* `Debug`, would print credentials into any
/// `{:?}` of a restore context.
impl std::fmt::Debug for Stores {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Stores::Machine => f.write_str("Stores::Machine"),
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
            Stores::Machine => panic!("Stores::Machine has no fixture to edit"),
            // A poisoned lock still holds the data a test wants to see, and a
            // second panic here would hide the first one's message.
            Stores::Fixture(state) => state.lock().unwrap_or_else(|e| e.into_inner()),
        }
    }

    /// May this machine write `store` at all?
    ///
    /// `false` on a platform whose Claude Code writes a real file instead, so a
    /// bundle pushed from a Mac is refused rather than failing a restore
    /// part-way through.
    pub fn writable(&self, store: Store) -> bool {
        match self {
            Stores::Fixture(_) => true,
            Stores::Machine => machine_writable(store),
        }
    }

    /// What `store` holds right now.
    ///
    /// `Ok(None)` is "there is no such credential here" and nothing else. A
    /// locked Keychain or a denied ACL is an `Err`, deliberately: pushing a
    /// bundle that silently omits the credential is the exact failure this
    /// module was written to end, so it must not be reachable by a shrug.
    pub fn read(&self, store: Store) -> Result<Option<Zeroizing<String>>> {
        match self {
            Stores::Fixture(_) => {
                let fixture = self.edit();
                fixture.fail()?;
                Ok(fixture.get(store).map(|v| Zeroizing::new(v.to_string())))
            }
            Stores::Machine => machine_read(store),
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
    pub fn has(&self, store: Store) -> Result<bool> {
        match self {
            Stores::Fixture(_) => {
                let fixture = self.edit();
                fixture.fail()?;
                Ok(fixture.get(store).is_some_and(|v| !v.is_empty()))
            }
            Stores::Machine => machine_has(store),
        }
    }

    /// Replace what `store` holds.
    ///
    /// Whole-value, never incremental: the underlying write either replaces the
    /// item or fails, so a failure leaves the existing credential exactly as it
    /// was. Nothing here is a read-modify-write.
    pub fn write(&self, store: Store, value: &str) -> Result<()> {
        match self {
            Stores::Fixture(_) => {
                self.edit().set(store, value);
                Ok(())
            }
            Stores::Machine => machine_write(store, value),
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
            Stores::Machine => machine_safe_key(),
        }
    }
}

/// The platform half of [`Stores::writable`], named rather than inlined so a
/// test can assert it without constructing a [`Stores::Machine`] — which it may
/// not, and which is the whole hermeticity rule of this module.
#[cfg(target_os = "macos")]
fn machine_writable(_store: Store) -> bool {
    true
}

#[cfg(not(target_os = "macos"))]
fn machine_writable(_store: Store) -> bool {
    false
}

#[cfg(target_os = "macos")]
fn machine_read(store: Store) -> Result<Option<Zeroizing<String>>> {
    match store {
        Store::ClaudeCodeOauth => Ok(crate::anthropic::keychain::read_raw()?.map(Zeroizing::new)),
    }
}

#[cfg(target_os = "macos")]
fn machine_has(store: Store) -> Result<bool> {
    match store {
        Store::ClaudeCodeOauth => crate::anthropic::keychain::has_raw(),
    }
}

#[cfg(target_os = "macos")]
fn machine_write(store: Store, value: &str) -> Result<()> {
    match store {
        Store::ClaudeCodeOauth => crate::anthropic::keychain::write_raw(value),
    }
}

#[cfg(target_os = "macos")]
fn machine_safe_key() -> Option<safe_storage::Key> {
    safe_storage::macos_key().ok()
}

/// Not macOS: Claude Code writes `~/.claude/.credentials.json`, which the
/// collectors carry as an ordinary file, and there is no Keychain to consult.
#[cfg(not(target_os = "macos"))]
fn machine_read(_store: Store) -> Result<Option<Zeroizing<String>>> {
    Ok(None)
}

#[cfg(not(target_os = "macos"))]
fn machine_has(_store: Store) -> Result<bool> {
    Ok(false)
}

#[cfg(not(target_os = "macos"))]
fn machine_write(store: Store, _value: &str) -> Result<()> {
    Err(crate::error::AppError::Credentials(format!(
        "this build has no {} to write",
        store.describe()
    )))
}

#[cfg(not(target_os = "macos"))]
fn machine_safe_key() -> Option<safe_storage::Key> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nothing about a store's wire name may come from a bundle, and no two
    /// stores may share one.
    #[test]
    fn every_store_has_its_own_fixed_wire_name_under_the_prefix() {
        let mut seen: Vec<&str> = Vec::new();
        for store in Store::ALL {
            let path = store.manifest_path();
            assert!(
                path.starts_with(&format!("{PREFIX}/")),
                "{path} is not under the keystore prefix"
            );
            assert!(Store::is_store_path(path));
            assert_eq!(Store::from_manifest_path(path), Some(store));
            assert!(!seen.contains(&path), "{path} names two stores");
            seen.push(path);
        }
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

    #[test]
    fn a_fixture_round_trips_a_value_and_shares_it_across_clones() {
        let stores = Stores::fixture();
        assert!(stores.read(Store::ClaudeCodeOauth).unwrap().is_none());

        let clone = stores.clone();
        clone
            .write(Store::ClaudeCodeOauth, r#"{"claudeAiOauth":{}}"#)
            .unwrap();
        assert_eq!(
            stores
                .read(Store::ClaudeCodeOauth)
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
        stores.write(Store::ClaudeCodeOauth, "first").unwrap();
        stores.write(Store::ClaudeCodeOauth, "second").unwrap();
        assert_eq!(stores.edit().get(Store::ClaudeCodeOauth), Some("second"));
    }

    /// `SyncRoots` is `Debug`, so a derived `Debug` here would print a live
    /// OAuth token into any `{:?}` of a restore context.
    #[test]
    fn debug_never_renders_a_stored_credential() {
        let stores = Stores::fixture();
        stores
            .write(Store::ClaudeCodeOauth, "sk-ant-oat01-NEVER-PRINT-ME")
            .unwrap();
        let rendered = format!("{stores:?}");
        assert!(!rendered.contains("NEVER-PRINT-ME"), "{rendered}");
        assert!(!rendered.contains("sk-ant"), "{rendered}");
    }

    /// Only where Claude Code keeps its credential in a Keychain. Elsewhere it
    /// writes `~/.claude/.credentials.json`, which `scope` collects as an
    /// ordinary file — so a store arriving there is refused in the planner
    /// rather than failing part-way through a restore.
    #[test]
    fn a_store_is_writable_exactly_where_this_platform_has_one() {
        for store in Store::ALL {
            assert_eq!(machine_writable(store), cfg!(target_os = "macos"));
        }
        // A fixture always is; that is what makes the seam usable.
        assert!(Stores::fixture().writable(Store::ClaudeCodeOauth));
    }

    /// **The hermeticity guard.** `Stores::Machine` is the only door to a real
    /// login Keychain, and it may be opened in exactly one production place —
    /// `SyncRoots::resolve` — so no test can reach one by forgetting a seam.
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
            if !production.contains("Stores::Machine") {
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
