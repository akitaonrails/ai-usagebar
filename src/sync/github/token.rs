//! Where the sync token comes from, in D-02's order:
//!
//! 1. `AI_USAGEBAR_SYNC_TOKEN` — the explicit override, and the one that works
//!    over SSH with no session keyring, which is exactly the headless-restore
//!    case this feature exists for.
//! 2. the macOS Keychain item `ai-usagebar-sync-token`
//! 3. `~/.config/ai-usagebar/sync-token`, mode 0600
//! 4. `gh auth token`, when `gh` happens to be installed — convenience only
//!
//! All four are injected as fields on [`TokenChain`], so a test exercises every
//! source without a Keychain, a real path, or a subprocess anywhere near it.
//! [`TokenChain::production`] is the single place that decides which of them are
//! live on this platform, and no test calls it.
//!
//! [`store`] and [`clear_source`] are the other half: where a token collected by
//! `sync setup` goes, and how a revoked one is taken back out. Neither can
//! reach `config.toml` — a `Contents: write` GitHub token is a different class
//! of secret from the read-only provider keys that file may hold inline.
//!
//! The value is a [`Zeroizing<String>`] end to end, and **no type here derives
//! `Debug` while holding it**. It is never logged, not even a prefix: only its
//! [`TokenSource`] is ever reported. "End to end" is the literal claim: every
//! field, every injected closure's return type, and every buffer a source reads
//! into is a `Zeroizing<String>`, so a value that entered the chain is wiped
//! when it leaves it — the sentence used to be true only of `resolve`'s return
//! value, with four plain `String`s behind it (F-9).
//!
//! The one boundary this module cannot own is `security(1)`'s output, which
//! [`crate::anthropic::keychain`] hands back as a plain `String`;
//! [`keychain::read_raw`](super::keychain::read_raw) wraps it on the first line
//! it can, so the plain copy lives for one move and no allocation is copied.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use zeroize::Zeroizing;

use crate::error::{AppError, Result};

/// Source 1, and the name every failure message has to print.
const ENV_VAR: &str = "AI_USAGEBAR_SYNC_TOKEN";

/// How long source 4 gets before it is killed. `gh` shells out to whatever
/// credential helper the user configured, and one of those blocked on a locked
/// keyring would otherwise wedge `sync setup` with no output at all.
const GH_TIMEOUT: Duration = Duration::from_secs(5);

/// Which of D-02's four the token actually came from. This — never the value —
/// is what gets printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSource {
    Env,
    Keychain,
    File,
    GhCli,
}

impl TokenSource {
    pub fn label(&self) -> &'static str {
        match self {
            TokenSource::Env => "env",
            TokenSource::Keychain => "Keychain",
            TokenSource::File => "file",
            TokenSource::GhCli => "gh",
        }
    }
}

/// The four sources, injected. The two closures are the seam that keeps every
/// test off the real Keychain and out of a subprocess.
///
/// No `Debug`: `env_value` holds the token itself.
#[derive(Default)]
pub struct TokenChain {
    pub env_value: Option<Zeroizing<String>>,
    #[allow(clippy::type_complexity)]
    pub keychain: Option<Box<dyn Fn() -> Result<Option<Zeroizing<String>>>>>,
    pub file_path: Option<PathBuf>,
    #[allow(clippy::type_complexity)]
    pub gh: Option<Box<dyn Fn() -> Result<Option<Zeroizing<String>>>>>,
}

impl TokenChain {
    /// The only function in this module that touches the environment or a real
    /// path. **No test calls it** — the AUR `check()` runs `cargo test` on
    /// installers' machines.
    pub fn production() -> TokenChain {
        TokenChain {
            env_value: std::env::var(ENV_VAR).ok().map(Zeroizing::new),
            // Source 2 exists on macOS only. D-02 rejects `keyring`/
            // `secret-service` for the Linux half: it needs a live D-Bus
            // session and fails over SSH, which is precisely the headless
            // restore this feature exists to serve. Source 3 covers it.
            #[cfg(target_os = "macos")]
            keychain: Some(Box::new(super::keychain::read_raw)),
            #[cfg(not(target_os = "macos"))]
            keychain: None,
            file_path: crate::config::resolved_path()
                .and_then(|p| p.parent().map(|d| d.join("sync-token"))),
            gh: Some(Box::new(gh_auth_token)),
        }
    }
}

/// Walk the chain in D-02's order and return the first non-empty value with its
/// source. Trailing whitespace is trimmed: an editor-written token file ends in
/// a newline, and a newline in a header value is a request-splitting bug.
pub fn resolve(chain: &TokenChain) -> Result<(Zeroizing<String>, TokenSource)> {
    if let Some(found) = usable(chain.env_value.clone()) {
        return Ok((found, TokenSource::Env));
    }
    if let Some(read) = &chain.keychain
        && let Some(found) = usable(read()?)
    {
        return Ok((found, TokenSource::Keychain));
    }
    if let Some(path) = &chain.file_path {
        match std::fs::read_to_string(path) {
            Ok(raw) => {
                if let Some(found) = usable(Some(Zeroizing::new(raw))) {
                    return Ok((found, TokenSource::File));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            // An unreadable-but-present token file is a misconfiguration the
            // user must see, not a source to silently skip past.
            Err(e) => return Err(AppError::io_at(path, e)),
        }
    }
    if let Some(read) = &chain.gh
        && let Some(found) = usable(read()?)
    {
        return Ok((found, TokenSource::GhCli));
    }

    Err(AppError::Credentials(format!(
        "no GitHub token for sync. Supply one of:\n\
         \x20 - {ENV_VAR} in the environment\n\
         {}\
         \x20 - {}, mode 0600\n\
         \x20 - `gh auth login`, if you already use the GitHub CLI\n\
         It must be a fine-grained PAT scoped to the single sync repository, \
         with Contents: read/write and Metadata: read — and nothing else.",
        keychain_hint(),
        chain
            .file_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "~/.config/ai-usagebar/sync-token".into())
    )))
}

/// Trim, and treat an empty result as "this source had nothing".
fn usable(raw: Option<Zeroizing<String>>) -> Option<Zeroizing<String>> {
    let raw = raw?;
    let trimmed = Zeroizing::new(raw.trim().to_owned());
    (!trimmed.is_empty()).then_some(trimmed)
}

/// The Keychain line of the exhausted-chain message, on the one platform where
/// source 2 exists. D-06: a failure names its fix, and naming a Keychain item
/// that cannot exist on this machine is not a fix.
#[cfg(target_os = "macos")]
fn keychain_hint() -> String {
    format!(
        "\x20 - the macOS Keychain item `{}`, which `ai-usagebar sync setup` writes\n",
        super::keychain::SERVICE
    )
}

#[cfg(not(target_os = "macos"))]
fn keychain_hint() -> String {
    String::new()
}

/// Source 4: `gh auth token`, when `gh` happens to be installed.
///
/// `Ok(None)` for a missing binary and for a non-zero exit alike — this source
/// is convenience and never a requirement, so "not installed" and "not logged
/// in" are both just "this source had nothing".
///
/// Two rules govern the spawn. The token travels back on the child's
/// **stdout**; nothing is passed *to* it. And the child's environment is
/// stripped of this project's provider secrets and of the sync token itself, so
/// a subprocess this tool spawns inherits no credential it has no business
/// seeing (T-3-09).
///
/// The bound is `spawn` + a watchdog thread + [`std::process::Child::kill`],
/// because [`Command::output`] has no timeout of its own, nothing in this
/// dependency tree adds one, and `resolve` is synchronous — so `tokio::process`
/// is not reachable from here either.
///
/// **Every blocking step is bounded, not just the process** (F-6). Killing `gh`
/// does not close a stdout pipe that a credential helper `gh` spawned still
/// holds open, so a `read_to_string` on this thread waited for an EOF that
/// never came — the watchdog fired, the command hung anyway. The read now runs
/// on its own thread behind a [`mpsc::Receiver::recv_timeout`], and the exit
/// status is collected with [`std::process::Child::try_wait`] rather than
/// `wait`: `wait` holds the mutex the watchdog needs in order to kill, so the
/// one call that was supposed to be bounded was the one blocking the killer.
fn gh_auth_token() -> Result<Option<Zeroizing<String>>> {
    let mut cmd = Command::new("gh");
    cmd.args(["auth", "token"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        // `gh`'s diagnostics are not ours to relay, and "not logged in" is not
        // a failure at this layer.
        .stderr(Stdio::null());
    for var in crate::vendor::vendor_secret_env_vars_to_remove(&[]) {
        cmd.env_remove(var);
    }
    cmd.env_remove(ENV_VAR);

    let Ok(mut child) = cmd.spawn() else {
        return Ok(None);
    };
    // Taken out before the child is shared: the read below must not hold the
    // lock the watchdog needs, and a killed child closes this pipe, which is
    // what ends that read.
    let stdout = child.stdout.take();
    let child = Arc::new(Mutex::new(child));

    let watched = Arc::clone(&child);
    let (finished, wake) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        // `Disconnected` means the call below finished and dropped its end;
        // only `Timeout` means the child is still running.
        if matches!(
            wake.recv_timeout(GH_TIMEOUT),
            Err(mpsc::RecvTimeoutError::Timeout)
        ) && let Ok(mut child) = watched.lock()
        {
            let _ = child.kill();
        }
    });

    let (read, out) = mpsc::channel::<Zeroizing<String>>();
    std::thread::spawn(move || {
        let mut buf = Zeroizing::new(String::new());
        if let Some(mut pipe) = stdout {
            let _ = pipe.read_to_string(&mut buf);
        }
        // A dropped receiver means the timeout below already gave up; the
        // buffer is zeroed here either way.
        let _ = read.send(buf);
    });
    let Ok(out) = out.recv_timeout(GH_TIMEOUT) else {
        if let Ok(mut child) = child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
        return Ok(None);
    };

    // stdout is at EOF, so `gh` has closed it and is exiting; poll rather than
    // block, and stop at the same bound. The watchdog above kills anything
    // still running when this deadline passes, and the kill lands as a status
    // the next poll collects.
    let deadline = std::time::Instant::now() + GH_TIMEOUT;
    let exited_zero = loop {
        match child
            .lock()
            .ok()
            .and_then(|mut c| c.try_wait().ok())
            .flatten()
        {
            Some(status) => break status.success(),
            None if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10))
            }
            None => break false,
        }
    };
    drop(finished);

    // `resolve` trims and treats an empty result as "nothing here".
    Ok(exited_zero.then_some(out))
}

/// Persist a token collected by `sync setup`, and report where it went.
///
/// macOS stores it in the Keychain and never touches `file_path`; every other
/// target writes `file_path` at mode 0600. There is no third option and no
/// config path: nothing here can write a token into `config.toml` (REPO-02).
pub fn store(token: &str, file_path: &Path) -> Result<TokenSource> {
    #[cfg(target_os = "macos")]
    {
        // The Keychain *is* the store on macOS (D-02); the file remains
        // readable by `resolve` so a token copied from another machine still
        // works, but this is not where a new one is written.
        let _ = file_path;
        super::keychain::write_raw(token)?;
        Ok(TokenSource::Keychain)
    }
    #[cfg(not(target_os = "macos"))]
    {
        store_file(token, file_path)?;
        Ok(TokenSource::File)
    }
}

/// The file half of [`store`], separated so it is testable on macOS too — where
/// `store` itself would reach the real login Keychain and no unit test may.
///
/// [`crate::cache::atomic_write`] puts the temp file in the destination's own
/// directory and creates the parent, so this inherits the project's no-torn-
/// state rule rather than reinventing it — and never lands in `/tmp`, which is
/// world-readable, is often a different filesystem (making `persist` a copy
/// that leaves the original behind), and may be swap-backed tmpfs. The mode is
/// then set explicitly rather than trusting the temp file's inherited one, the
/// same belt-and-braces `anchor::write_to` applies.
#[cfg(any(not(target_os = "macos"), test))]
fn store_file(token: &str, file_path: &Path) -> Result<()> {
    crate::cache::atomic_write(file_path, token.trim().as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(file_path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| AppError::io_at(file_path, e))?;
    }
    Ok(())
}

/// Remove the stored token, idempotently — **and only from the store that
/// produced the value that was rejected**.
///
/// This used to clear the Keychain item *and* the token file unconditionally,
/// with the resolved [`TokenSource`] bound and never consulted (F-1). A 401 says
/// the value that was *sent* is dead; it says nothing about the other three
/// sources. The realistic sequence needs no attacker and is the documented
/// configuration: a macOS user pairs (token → Keychain), later a shell exports
/// `AI_USAGEBAR_SYNC_TOKEN` — CI, an `.envrc`, or a 90-day PAT that expired,
/// both of which this project's own docs recommend — and the next `sync status`
/// resolves `Env`, 401s, and deletes the still-valid Keychain token that was
/// never used. The env var still holds the dead one, so the next run fails
/// identically, and the message sends the user off to mint a *third*.
///
/// `Env` and `GhCli` therefore clear nothing: neither store is this tool's to
/// delete. [`clear_note`] is the other half — what the user is told instead.
pub fn clear_source(source: TokenSource, file_path: &Path) -> Result<()> {
    if !owns_a_store(source) {
        return Ok(());
    }
    match source {
        #[cfg(target_os = "macos")]
        TokenSource::Keychain => super::keychain::delete_raw(),
        // Source 2 does not exist off macOS, so nothing can have resolved from
        // it — and if it somehow did, there is no item here to delete.
        #[cfg(not(target_os = "macos"))]
        TokenSource::Keychain => Ok(()),
        TokenSource::File => clear_file(file_path),
        // Unreachable behind the guard above, and stated anyway: the two
        // partitions must not disagree.
        TokenSource::Env | TokenSource::GhCli => Ok(()),
    }
}

/// Whether this tool has a store of its own for `source` — the partition
/// [`clear_source`] acts on, asked rather than restated.
///
/// `Env` and `GhCli` are values the caller's shell and the GitHub CLI own; a 401
/// on one of them is not permission to reach into ai-usagebar's own stores,
/// which the failing request never sent (F-1). A caller that wants to skip the
/// destructive seam entirely — a guided flow behind a test double, say — asks
/// this rather than writing the rule a second time.
pub fn owns_a_store(source: TokenSource) -> bool {
    matches!(source, TokenSource::Keychain | TokenSource::File)
}

/// What [`clear_source`] just did, in the user's words — kept beside it so the
/// message and the action cannot drift apart, which is the half of F-1 that made
/// the other half invisible.
///
/// The two "nothing was cleared" arms are the ones that matter: they name the
/// thing the user has to change, because a replacement written anywhere else
/// will not be reached while the environment or `gh` still answers first.
pub fn clear_note(source: TokenSource, file_path: &Path) -> String {
    match source {
        TokenSource::Keychain => format!(
            "The rejected token came from {}, and that entry has been removed. \
             Nothing else was touched.",
            keychain_label()
        ),
        TokenSource::File => format!(
            "The rejected token came from {}, and that file has been removed. \
             Nothing else was touched.",
            file_path.display()
        ),
        TokenSource::Env => format!(
            "Nothing was cleared: the rejected token came from {ENV_VAR}, which is not this \
             tool's to delete and which outranks every stored token. Unset or replace it — \
             a new token in the Keychain or in the token file is never reached while it is set."
        ),
        TokenSource::GhCli => {
            "Nothing was cleared: the rejected token came from `gh auth token`, which is the \
             GitHub CLI's credential and not this tool's. Run `gh auth login` again, or set \
             AI_USAGEBAR_SYNC_TOKEN to a token of ai-usagebar's own so `gh` is not consulted."
                .to_owned()
        }
    }
}

/// Named through the same constant the read and the write select on, so this
/// cannot point at an item that does not exist.
#[cfg(target_os = "macos")]
fn keychain_label() -> String {
    format!("the macOS Keychain item `{}`", super::keychain::SERVICE)
}

#[cfg(not(target_os = "macos"))]
fn keychain_label() -> String {
    "the macOS Keychain".to_owned()
}

fn clear_file(file_path: &Path) -> Result<()> {
    match std::fs::remove_file(file_path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(AppError::io_at(file_path, e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const FIXTURE: &str = "github_pat_fixture_not_a_real_token";

    /// What the two injected fields hold. Nothing in this module's tests goes
    /// near the real Keychain or spawns `gh`.
    type Source = Option<Box<dyn Fn() -> Result<Option<Zeroizing<String>>>>>;

    fn answering(value: &str) -> Source {
        let value = value.to_owned();
        Some(Box::new(move || Ok(Some(Zeroizing::new(value.clone())))))
    }

    fn empty() -> Source {
        Some(Box::new(|| Ok(None)))
    }

    /// A token file that exists and answers, so a later source cannot pass a
    /// precedence test by being the only one able to speak.
    fn seeded_file(dir: &TempDir, value: &str) -> PathBuf {
        let path = dir.path().join("sync-token");
        std::fs::write(&path, format!("{value}\n")).unwrap();
        path
    }

    /// Precedence is asserted, not assumed: every later source can also answer,
    /// with a value the assertion would notice.
    #[test]
    fn the_environment_wins_and_reports_itself_as_the_source() {
        let dir = TempDir::new().unwrap();
        let chain = TokenChain {
            env_value: Some(Zeroizing::new(format!("{FIXTURE}\n"))),
            keychain: answering("from-the-keychain"),
            file_path: Some(seeded_file(&dir, "from-the-file")),
            gh: answering("from-gh"),
        };
        let (token, source) = resolve(&chain).unwrap();
        assert_eq!(token.as_str(), FIXTURE);
        assert_eq!(source, TokenSource::Env);
    }

    /// Second in D-02's order: ahead of the file and of `gh`, both of which
    /// answer here.
    #[test]
    fn the_keychain_outranks_the_file_and_gh() {
        let dir = TempDir::new().unwrap();
        let chain = TokenChain {
            env_value: None,
            keychain: answering(FIXTURE),
            file_path: Some(seeded_file(&dir, "from-the-file")),
            gh: answering("from-gh"),
        };
        let (token, source) = resolve(&chain).unwrap();
        assert_eq!(token.as_str(), FIXTURE);
        assert_eq!(source, TokenSource::Keychain);
    }

    /// Fourth and last, and only once the other three had nothing.
    #[test]
    fn gh_answers_when_nothing_ahead_of_it_did() {
        let dir = TempDir::new().unwrap();
        let chain = TokenChain {
            env_value: None,
            keychain: empty(),
            file_path: Some(dir.path().join("sync-token")),
            gh: answering(&format!("  {FIXTURE}  ")),
        };
        let (token, source) = resolve(&chain).unwrap();
        assert_eq!(token.as_str(), FIXTURE);
        assert_eq!(source, TokenSource::GhCli);
    }

    /// T-3-11. An error from the Keychain means "the Keychain could not
    /// answer", not "there is no token": falling through to a later source
    /// would send the user off to re-issue a token they already have.
    #[test]
    fn a_keychain_error_stops_the_chain_instead_of_falling_through() {
        let dir = TempDir::new().unwrap();
        let err = resolve(&TokenChain {
            env_value: None,
            keychain: Some(Box::new(|| {
                Err(AppError::Credentials("the login Keychain is locked".into()))
            })),
            file_path: Some(seeded_file(&dir, "from-the-file")),
            gh: answering("from-gh"),
        })
        .expect_err("a locked Keychain is not an absent token");
        assert!(err.to_string().contains("locked"), "{err}");
    }

    #[test]
    fn a_token_file_answers_when_the_environment_is_unset() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sync-token");
        std::fs::write(&path, format!("  {FIXTURE}  \n")).unwrap();

        let chain = TokenChain {
            env_value: None,
            keychain: empty(),
            file_path: Some(path),
            gh: answering("from-gh"),
        };
        let (token, source) = resolve(&chain).unwrap();
        assert_eq!(token.as_str(), FIXTURE);
        assert_eq!(source, TokenSource::File);
    }

    /// An empty file is not a token — fall through, then say how to supply one.
    #[test]
    fn an_exhausted_chain_names_every_way_to_supply_a_token() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sync-token");
        std::fs::write(&path, "\n\n").unwrap();

        let err = resolve(&TokenChain {
            env_value: None,
            keychain: empty(),
            file_path: Some(path),
            gh: empty(),
        })
        .expect_err("nothing in the chain answered");
        let text = err.to_string();
        assert!(text.contains("AI_USAGEBAR_SYNC_TOKEN"), "{text}");
        assert!(text.contains("sync-token"), "{text}");
        assert!(text.contains("gh auth login"), "{text}");
        assert!(text.contains("Contents: read/write"), "{text}");
        // The Keychain is only one of D-02's four where it can exist.
        #[cfg(target_os = "macos")]
        assert!(text.contains(super::super::keychain::SERVICE), "{text}");
    }

    /// T-3-10. `store`'s file half, exercised on every platform — on macOS
    /// `store` itself would write the real login Keychain, which no unit test
    /// may touch.
    #[test]
    fn a_stored_token_file_is_created_at_mode_0600_in_its_own_directory() {
        let dir = TempDir::new().unwrap();
        // A directory that does not exist yet: `sync setup` may run before
        // anything else has written to the config dir.
        let path = dir.path().join("nested").join("sync-token");
        store_file(&format!("{FIXTURE}\n"), &path).unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), FIXTURE);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "{mode:o}");
        }
        // And nothing else was left in the directory at any mode — a `persist`
        // that degraded into a copy would show up here.
        let left: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, vec![std::ffi::OsString::from("sync-token")]);

        clear_file(&path).unwrap();
        assert!(!path.exists());
        clear_file(&path).expect("removing what is already gone is not a failure");
    }

    /// The platform half of [`store`]. On macOS it is the Keychain, which is
    /// why that arm is exercised by the `#[ignore]`d round trip in
    /// `tests/live.rs` instead of here.
    #[test]
    #[cfg(not(target_os = "macos"))]
    fn store_reports_the_file_as_the_destination_off_macos() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sync-token");
        assert_eq!(store(FIXTURE, &path).unwrap(), TokenSource::File);
        assert_eq!(
            resolve(&TokenChain {
                file_path: Some(path.clone()),
                ..TokenChain::default()
            })
            .unwrap()
            .1,
            TokenSource::File
        );
        clear(&path).unwrap();
        assert!(!path.exists());
    }

    /// F-1's inner guard, independent of the caller's: even asked directly, a
    /// source this tool does not own removes nothing. The token file is seeded
    /// so a wrong answer has something to destroy.
    #[test]
    fn clearing_an_env_or_gh_source_removes_nothing_this_tool_stores() {
        let dir = TempDir::new().unwrap();
        let path = seeded_file(&dir, FIXTURE);

        for source in [TokenSource::Env, TokenSource::GhCli] {
            assert!(!owns_a_store(source), "{source:?}");
            clear_source(source, &path).unwrap();
            assert!(path.exists(), "{source:?} removed the token file");
        }

        assert!(owns_a_store(TokenSource::File));
        clear_source(TokenSource::File, &path).unwrap();
        assert!(!path.exists(), "the file source clears the file");
        clear_source(TokenSource::File, &path).expect("clearing twice is not a failure");
    }

    /// T-3-01: the value is never rendered, not even through a derived `Debug`.
    #[test]
    fn no_rendering_of_the_resolved_token_type_contains_the_token() {
        let chain = TokenChain {
            env_value: Some(Zeroizing::new(FIXTURE.into())),
            ..TokenChain::default()
        };
        let (token, source) = resolve(&chain).unwrap();
        assert!(!format!("{source:?}").contains(FIXTURE));
        assert!(!format!("{:?}", source.label()).contains(FIXTURE));
        // `Zeroizing<String>` is `Debug` by way of `String`, which is exactly
        // why nothing that *holds* one derives `Debug` — see `Client`.
        drop(token);
    }
}
