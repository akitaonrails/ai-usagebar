//! `ai-usagebar sync …` entry point. Owned by plan 2-01, extended with
//! `push --dry-run` by plan 2-07 and `setup` by plan 3-01.
//!
//! Output carries paths and byte counts only — never a file's contents. This
//! command's whole job is telling the user what *would* leave the machine, so
//! printing any of it here would defeat the point.
//!
//! **Only `setup` opens a socket, and only to `GET`.** `--dry-run` measures a
//! push without performing one, `sync push` without it refuses rather than
//! half-executing, and `setup` verifies the remote is private without uploading
//! a byte — the client it uses has no method that can carry a request body.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::config::{Config, SyncCategory, SyncConfig};
use crate::sync::crypto::{KdfParams, Keyfile, Keys, content_address};
use crate::sync::github::setup::TtyPrompt;
use crate::sync::github::{
    self, Client, Endpoints, RepoRef, gate, pairing, token, token::TokenChain,
};
use crate::sync::index::{self, Index};
use crate::sync::push::progress;
use crate::sync::push::{self, PushCtx, PushOutcome};
use crate::sync::report::{DryRunReport, RepoSection, Style};
use crate::sync::restore::{self, RestoreOptions};
use crate::sync::{SyncRoots, passphrase, plan, report};
use crate::widget::cli::SyncAction;

/// Same shape as `account::run` / `tui::settings::run_cli`: an exit code, no
/// Waybar exit-0 contract — a script piping this deserves a real code. The
/// widget's exit-0 invariant is a property of `widget::run::fallback`; `sync`
/// reports failure honestly (D-06).
///
/// **The thin wrapper that resolves the real world.** No test calls it: it
/// reads `$HOME` through `Config::load` and `TokenChain::production`, and the
/// AUR `check()` runs `cargo test` on installers' machines. Everything below
/// hangs off [`run_with`].
pub fn run(action: &SyncAction) -> i32 {
    let config = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("sync: could not read the config file: {e}");
            return 1;
        }
    };
    let roots = match SyncRoots::resolve(&config) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("sync: {e}");
            return 1;
        }
    };
    run_with(
        action,
        &config,
        &roots,
        &Endpoints::default(),
        &TokenChain::production(),
        Utc::now(),
    )
}

/// Every dependency injected: the config, the roots, both GitHub hosts, the
/// token chain, and the clock. This is what tests drive.
pub fn run_with(
    action: &SyncAction,
    cfg: &Config,
    roots: &SyncRoots,
    endpoints: &Endpoints,
    chain: &TokenChain,
    now: DateTime<Utc>,
) -> i32 {
    match action {
        SyncAction::Status { json } => status(cfg, roots, endpoints, chain, now, *json),
        SyncAction::Setup => setup(cfg, roots, endpoints, chain, now),
        SyncAction::Push {
            dry_run: true,
            rebuild_index,
            force_rehash,
            ..
        } => dry_run(cfg, roots, now, recovery(*rebuild_index, *force_rehash)),
        SyncAction::Push {
            dry_run: false,
            allow_rollback,
            rebuild_index,
            force_rehash,
        } => push(
            cfg,
            roots,
            endpoints,
            chain,
            *allow_rollback,
            recovery(*rebuild_index, *force_rehash),
            now,
        ),
        SyncAction::Prune => prune(cfg, roots, endpoints, chain, now),
        SyncAction::Rekey => rekey(cfg, roots, endpoints, chain, now),
        SyncAction::Pull {
            apply,
            // `--dry-run` maps to nothing. A dry run is the *absence* of
            // `apply`, not a flag anything checks (D1) — it is accepted for
            // symmetry with `push --dry-run`, and clap refuses it alongside
            // `--apply` so a run that passes both is an error, not a guess.
            dry_run: _,
            force,
            force_credentials,
            allow_rollback,
            yes,
            rebuild_index,
        } => pull(
            cfg,
            roots,
            endpoints,
            chain,
            RestoreOptions {
                apply: *apply,
                force: *force,
                force_credentials: *force_credentials,
                allow_rollback: *allow_rollback,
                rebuild_index: *rebuild_index,
                // Nothing on this path reads it, and that is why `sync pull`
                // does not offer `--force-rehash`: restore hashes what is on
                // disk and never asks the index, so the flag would change
                // nothing here. It lives on `sync push`, where it changes what
                // the planner does — and `RestoreOptions` carries no field for
                // it, so there is nothing here to set wrong.
                assume_yes: *yes,
            },
            now,
        ),
    }
}

/// The palette for a stream, decided **here** and injected downward.
///
/// This is the one production place the two facts meet: `IsTerminal` for the
/// stream about to be written, and `NO_COLOR` — which is read by
/// [`crate::display::color_enabled`] rather than here, because `src/sync/`'s
/// structural guard forbids `std::env` anywhere in this subtree and a password
/// input path lives two files away.
///
/// Two streams, two answers: `sync status` writes its report to standard output
/// and `sync push` writes its progress to standard error, and `… | less` should
/// colour neither while `… 2>/dev/null` should still colour the report.
fn style_of(is_terminal: bool) -> Style {
    Style::color(crate::display::color_enabled(is_terminal))
}

fn stdout_style() -> Style {
    style_of(std::io::stdout().is_terminal())
}

fn stderr_style() -> Style {
    style_of(std::io::stderr().is_terminal())
}

/// One current-thread runtime, built only where one is actually needed.
///
/// `src/bin/ai-usagebar.rs` dispatches `Command::Sync` before it constructs a
/// runtime, which keeps `push --dry-run` and an unconfigured `status` paying
/// nothing for a reactor they never use.
fn runtime() -> std::result::Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("could not start the async runtime ({e})"))
}

/// The index is a hint (D5): if it will not open, the scan is still the truth
/// and only the last-sync line and the would-upload column are lost.
fn open_index(roots: &SyncRoots) -> Option<Index> {
    match Index::at(&roots.index_file) {
        Ok(i) => Some(i),
        Err(e) => {
            eprintln!("sync: local index unavailable, last-sync unknown ({e})");
            None
        }
    }
}

/// `sync status` — the category listing, plus what plan 3-07 knows about the
/// repository.
///
/// The two halves fail independently on purpose: a dead token still leaves the
/// category listing visible (a user should be able to see what *would* be sent
/// without authenticating), but a repository-section failure is still a
/// non-zero exit (D-06, REPO-05, T-3-41).
///
/// **`--json` resolves strictly less.** It builds no plan and asks GitHub
/// nothing, because the object it prints has no key for either: the whole
/// point of the machine-readable form is that a menu bar can call it on every
/// menu open. A plan would want the sync password on a stdin a subprocess has
/// no way to answer (D-02) and would open file bodies to get it; a repository
/// section would put a network round-trip behind a UI gesture (T-6-04).
///
/// It does ask the machine-bound credential store **whether it holds an
/// entry** — see [`report::build_status`]. That is none of the three things
/// above: no password, no socket, no file body, and not even the store's value,
/// so it cannot raise a Keychain prompt on a menu open either.
fn status(
    config: &Config,
    roots: &SyncRoots,
    endpoints: &Endpoints,
    chain: &TokenChain,
    now: DateTime<Utc>,
    json: bool,
) -> i32 {
    let index = open_index(roots);
    let (plan, repo) = if json {
        (None, None)
    } else {
        // UX-02 is "what would change now", so status builds a plan when it can
        // — and still prints plan 2-01's counts-only form when it cannot.
        let (plan, _) = try_plan(roots, config, index.as_ref(), now);
        (
            plan,
            Some(resolve_repo_section(config, roots, endpoints, chain, now)),
        )
    };
    let (code, out) = status_with(
        roots,
        &config.sync,
        index.as_ref(),
        now,
        plan,
        repo,
        json,
        stdout_style(),
    );
    print!("{out}");
    code
}

/// One built [`report::StatusReport`], rendered one of two ways — which is what
/// keeps the text and the JSON from ever disagreeing about the same run.
///
/// Everything the real world supplies arrives as an argument, so no test on
/// this path calls `Config::load`, `SyncRoots::resolve`, `index::default_path`
/// or `Utc::now`.
// Eight, and every one of them is a fact the real world supplies that a test
// has to be able to fake. Bundling them into a struct would move the same eight
// one line up and add a type nothing else uses.
#[allow(clippy::too_many_arguments)]
fn status_with(
    roots: &SyncRoots,
    cfg: &SyncConfig,
    index: Option<&Index>,
    now: DateTime<Utc>,
    plan: Option<plan::SyncPlan>,
    repo: Option<RepoSection>,
    json: bool,
    style: Style,
) -> (i32, String) {
    // D-06: the listing survives a repository incident; the exit code does not.
    let failed = repo.as_ref().is_some_and(|r| r.failure.is_some());
    let report = report::build_status(roots, cfg, index, now, plan, repo);
    let out = if json {
        // **Untouched by styling, deliberately.** This document is the macOS
        // menu bar's whole read of sync and is parsed by the Node contract
        // suites; one escape in it is a parse failure, not a decoration.
        format!("{}\n", report::status_json(&report))
    } else {
        report::render_status_styled(&report, style)
    };
    (i32::from(failed), out)
}

/// The repository section, or the reason there is none.
///
/// An **unconfigured** machine gets an empty section and no runtime at all — a
/// network error reported by a machine that never named a repository would be a
/// lie.
fn resolve_repo_section(
    config: &Config,
    roots: &SyncRoots,
    endpoints: &Endpoints,
    chain: &TokenChain,
    now: DateTime<Utc>,
) -> RepoSection {
    if config.sync.repo.is_none() {
        return RepoSection::default();
    }
    match runtime() {
        Ok(rt) => rt.block_on(repo_section(config, roots, endpoints, chain, now)),
        Err(why) => RepoSection {
            configured: config.sync.repo.clone(),
            failure: Some(why),
            ..RepoSection::default()
        },
    }
}

/// **Exactly one request to the repository endpoint.** `fetch_facts` is called
/// once and its result is handed to both the drift check and the report; the
/// gate is not re-run per consumer.
async fn repo_section(
    config: &Config,
    roots: &SyncRoots,
    endpoints: &Endpoints,
    chain: &TokenChain,
    now: DateTime<Utc>,
) -> RepoSection {
    let mut section = RepoSection {
        configured: config.sync.repo.clone(),
        ..RepoSection::default()
    };
    let Some(configured) = config.sync.repo.as_deref() else {
        return section;
    };
    let repo = match RepoRef::parse(configured) {
        Ok(repo) => repo,
        Err(e) => return section.failed(e.to_string()),
    };

    let record = match pairing::read_from(&pairing::default_path(roots)) {
        Ok(record) => record,
        Err(e) => return section.failed(e.to_string()),
    };
    section.last_verified = record.as_ref().map(|p| p.checked_at);

    let (value, source) = match token::resolve(chain) {
        Ok(pair) => pair,
        Err(e) => return section.failed(e.to_string()),
    };
    // The label, never the value. `RepoSection` has no field that could hold one.
    section.token_source = Some(source.label());

    let client = match Client::new(endpoints, value, source) {
        Ok(client) => client,
        Err(e) => return section.failed(e.to_string()),
    };
    let facts = match gate::fetch_facts(&client, &repo, now).await {
        Ok(facts) => facts,
        // Same 401 handling the setup flow performs, and `source` is what makes
        // it safe: a 401 on a token resolved from the environment or from `gh`
        // must not delete the Keychain item or the token file, neither of which
        // this run used (F-1).
        //
        // **`token::clear_source` deletes the real macOS login Keychain item**
        // when the source *is* the Keychain. No test here may mock a 401 against
        // this arm; `sync setup`'s version of this call goes through
        // `SetupPrompt::clear_token`, which a test double overrides. If `status`
        // ever needs a 401 test, give it the same seam first — do not reach for
        // the production function.
        Err(e) => {
            let e = github::setup::clear_if_dead(
                e,
                source,
                &github::setup::token_path(roots),
                &token::clear_source,
            );
            return section.failed(e.to_string());
        }
    };
    section.visibility = Some(facts.visibility.clone());

    // One source, both gate calls — deriving it twice reintroduces the
    // contradiction plan 3-04 removed.
    let credentials_in_bundle = config.sync.includes(SyncCategory::Credentials);
    match pairing::check_drift(record.as_ref(), &facts, credentials_in_bundle, now) {
        // The SAFE-02 incident renders verbatim: a repository going public is
        // the same event whichever command noticed it.
        Err(e) => return section.failed(e.to_string()),
        Ok(drift) => section.warnings.extend(drift.warnings),
    }
    match gate::assert_pushable(&facts, &repo, credentials_in_bundle, now) {
        Err(e) => return section.failed(e.to_string()),
        Ok((_clearance, warnings)) => section.warnings.extend(warnings),
    }
    section
}

fn dry_run(config: &Config, roots: &SyncRoots, now: DateTime<Utc>, recovery: Recovery) -> i32 {
    if let Err(why) = maybe_reset_index(roots, recovery.rebuild) {
        return refuse(&why);
    }
    let index = open_index(roots).map(|index| rehashing(index, recovery.rehash));
    let (plan, no_key) = try_plan(roots, config, index.as_ref(), now);
    let report = DryRunReport {
        status: report::build_status(roots, &config.sync, index.as_ref(), now, plan, None),
        no_key,
    };
    print!("{}", report::render_dry_run_styled(&report, stdout_style()));
    0
}

/// `sync setup` — pair with the configured private repository.
///
/// **The runtime is built here.** `src/bin/ai-usagebar.rs` dispatches
/// `Command::Sync` before it constructs one, which was correct when `sync` only
/// scanned the filesystem. Keeping the dispatch where it is means `status` and
/// `push --dry-run` still pay nothing for a runtime they do not use.
///
/// The success line reports the token's **source** and never its value; the
/// failure path prints the message and nothing else — no token, no prefix of
/// one, no header dump (T-3-01).
fn setup(
    cfg: &Config,
    roots: &SyncRoots,
    endpoints: &Endpoints,
    chain: &TokenChain,
    now: DateTime<Utc>,
) -> i32 {
    let rt = match runtime() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("sync: {e}");
            return 1;
        }
    };
    let mut prompt = TtyPrompt::new(stdout_style());
    match rt.block_on(github::setup::run(
        &cfg.sync,
        roots,
        endpoints,
        chain,
        &mut prompt,
        now,
    )) {
        Ok(outcome) => {
            print!("{}", render_setup(&outcome));
            0
        }
        Err(e) => {
            eprintln!("sync: {e}");
            1
        }
    }
}

/// Pure so a test can assert on it without capturing stdout.
///
/// Neither secret is representable here: `SetupOutcome` carries the token's
/// *source* and no passphrase at all (T-3-36).
fn render_setup(outcome: &github::setup::SetupOutcome) -> String {
    let mut out = format!(
        "\nrepo:       {}\nvisibility: {}\ntoken:      present ({}), saved to the {}\n",
        outcome.repo,
        outcome.visibility,
        outcome.token_source.label(),
        outcome.stored_at.label(),
    );
    out.push_str(&format!(
        "categories: {}\n",
        outcome
            .categories
            .iter()
            .map(|c| c.label())
            .collect::<Vec<_>>()
            .join(", ")
    ));
    out.push_str(&format!(
        "keyfile:    {}  (local only — keep the password, there is no recovery)\n",
        outcome.keyfile.display()
    ));
    out.push_str(&format!(
        "first push: {} of {} in {} files\n",
        report::human_bytes(outcome.would_send),
        report::human_bytes(outcome.raw_bytes),
        outcome.files,
    ));
    for warning in &outcome.warnings {
        out.push_str(&format!("warning:    {warning}\n"));
    }
    if outcome.reused_pairing {
        out.push_str("pairing:    reused — this machine was already paired.\n");
    }
    // The one thing setup can put in the repository, and only ever with an
    // explicit yes — so the closing line reports it rather than repeating a
    // "nothing was uploaded" that would no longer be true.
    if outcome.initialised {
        out.push_str("repo:       initialised — a README was added to the empty repository.\n");
    }
    out.push_str("\nThis machine is paired and ready to push.\n");
    out.push_str(if outcome.initialised {
        "The README you approved is the only thing there — `sync setup` uploads no bundle \
         data. Run `ai-usagebar sync push` when you are ready.\n"
    } else {
        "Nothing was uploaded — `sync setup` never uploads. Run `ai-usagebar sync push` when \
         you are ready.\n"
    });
    out
}

/// The would-upload half, or the reason there is none.
///
/// Never fatal: the file counts and raw bytes need no key at all, and a user
/// checking what is in scope should not have to authenticate (SCOPE-04).
fn try_plan(
    roots: &SyncRoots,
    config: &Config,
    index: Option<&Index>,
    now: DateTime<Utc>,
) -> (Option<plan::SyncPlan>, Option<String>) {
    let Some(index) = index else {
        return (None, Some("the local index could not be opened".into()));
    };
    let keys = match keys_at(&keyfile_path(roots)) {
        Ok(k) => k,
        Err(why) => return (None, Some(why)),
    };
    match plan::build_with_keys(roots, &config.sync, index, now, &keys) {
        Ok(p) => (Some(p), None),
        Err(e) => (None, Some(e.to_string())),
    }
}

/// The local copy of the bundle keyfile, beside `config.toml`.
///
/// Derived from the injected [`SyncRoots`] rather than resolved here, so a test
/// points it at a temp directory the same way every other collector is pointed.
///
/// **Read-only.** Creating this file is the job of the guided setup that pairs
/// the repo and sets the sync password; this command only uses one that already
/// exists, and says so plainly when it does not.
pub(crate) fn keyfile_path(roots: &SyncRoots) -> PathBuf {
    roots.config_dir.join("sync").join("keyfile.json")
}

/// Open the keyfile at `path` with a password read from stdin — **never
/// prompting**, because the only caller is [`try_plan`].
///
/// `sync status` and `sync push --dry-run` want the third column *if* a
/// password happens to be there and print the report without it if not
/// (`DryRunReport::no_key`). Asking for one would turn a read-only command that
/// answers instantly into a command that blocks a terminal, so `may_prompt` is
/// `false` here and the old "must be piped in on stdin" refusal is what a
/// terminal still gets.
///
/// The password arrives on stdin only — never argv, never an environment
/// variable (T-2-29) — is held in a `Zeroizing<String>`, and never reaches an
/// error message: a wrong one produces `crypto`'s own single indistinguishable
/// refusal. Neither the keyfile's bytes nor any derived key is formatted into
/// the `String` this returns.
fn keys_at(path: &Path) -> std::result::Result<Keys, String> {
    local_keyfile(path, false).map(|k| k.keys)
}

/// Everything the push path needs out of the local keyfile, from one read.
pub(crate) struct LocalKeyfile {
    pub keys: Keys,
    /// The parameters *this bundle* lives at, which every new snapshot root
    /// repeats. Never `KdfParams::default` — that is the whole point of the
    /// keyfile storing them.
    pub kdf: KdfParams,
    /// The asset name this keyfile would publish under, content-addressed over
    /// its canonical serialization — the same bytes `rekey` uploads, so a
    /// rewrapped keyfile and this one can never collide.
    pub asset: String,
}

/// The refusal `keys_at`'s optional read still gets on a terminal.
const NO_PROMPT_HERE: &str = "the sync password must be piped in on stdin; this command \
                              does not ask for one";

/// The local keyfile, opened with the sync password.
///
/// `may_prompt` is the difference between the two readers of this file. `sync
/// push`, `sync prune` and `sync rekey` **need** the password, so on a terminal
/// they ask for it through [`sync_password`] — the same read `sync pull` has
/// used since Phase 5, and the behaviour `docs/sync-github.md` has documented
/// for `ai-usagebar sync push` all along. [`keys_at`] passes `false`: it only
/// *wants* the password, and a read-only report must not block on a question.
///
/// Nothing here reads the password from anywhere but the caller's stdin.
fn local_keyfile(path: &Path, may_prompt: bool) -> std::result::Result<LocalKeyfile, String> {
    // Before any password is wanted: a machine with no keyfile is told so
    // rather than asked for a password it would then discard.
    let raw = std::fs::read_to_string(path).map_err(|_| {
        format!(
            "this bundle has no sync keyfile yet ({} is absent)\n\
             only the third column needs one; the counts and raw bytes above need \
             no password at all",
            path.display()
        )
    })?;
    let keyfile: Keyfile = serde_json::from_str(&raw)
        .map_err(|_| format!("{} is not a readable sync keyfile", path.display()))?;

    let interactive = std::io::stdin().is_terminal();
    if interactive && !may_prompt {
        return Err(NO_PROMPT_HERE.into());
    }
    let pw = sync_password(interactive)?;
    open_keyfile(keyfile, &pw)
}

/// [`local_keyfile`] with the password already in hand — the rekey arm's path,
/// which prompts for the old password through 3-07's seam rather than reading
/// stdin itself.
fn local_keyfile_with(
    path: &Path,
    pw: &zeroize::Zeroizing<String>,
) -> std::result::Result<LocalKeyfile, String> {
    let raw = std::fs::read_to_string(path).map_err(|_| {
        format!(
            "this bundle has no sync keyfile ({} is absent)",
            path.display()
        )
    })?;
    let keyfile: Keyfile = serde_json::from_str(&raw)
        .map_err(|_| format!("{} is not a readable sync keyfile", path.display()))?;
    open_keyfile(keyfile, pw)
}

/// The one place a password becomes keys. Neither the password nor any derived
/// key reaches the `String` this returns: a wrong one produces `crypto`'s own
/// single indistinguishable refusal.
fn open_keyfile(
    keyfile: Keyfile,
    pw: &zeroize::Zeroizing<String>,
) -> std::result::Result<LocalKeyfile, String> {
    // Argon2id at m = 1 GiB is a deliberate cost. Announce it before it starts —
    // a command that appears frozen for a second and a half reads as a hang.
    let style = stderr_style();
    eprintln!(
        "{} {}",
        style.dim("sync:"),
        style.dim("deriving the sync key (Argon2id — this takes a moment)…")
    );
    let keys = keyfile.open(pw.as_bytes()).map_err(|e| e.to_string())?;
    Ok(LocalKeyfile {
        kdf: keyfile.kdf.params(),
        asset: keyfile_asset_for(&keyfile)?,
        keys,
    })
}

/// The keyfile's asset name — `keyfile-<content address>.json`.
pub(crate) fn keyfile_asset_for(keyfile: &Keyfile) -> std::result::Result<String, String> {
    let canonical = serde_json::to_vec(keyfile)
        .map_err(|e| format!("the sync keyfile could not be serialized: {e}"))?;
    Ok(push::keyfile_asset_name(&content_address(&canonical)))
}

// ---- the write commands ----------------------------------------------------

/// Everything a `PushCtx` needs that is resolved from the local machine.
///
/// Held as a struct so the three write arms build it identically: a second
/// place that resolves a repository, a token and a pairing record is a second
/// place for them to disagree.
struct Resolved {
    repo: RepoRef,
    client: Client,
    index: Index,
    repo_id: String,
}

fn resolve(
    cfg: &Config,
    roots: &SyncRoots,
    endpoints: &Endpoints,
    chain: &TokenChain,
) -> std::result::Result<Resolved, String> {
    let configured = cfg.sync.repo.as_deref().ok_or_else(|| {
        "no repository is configured. Set `repo = \"owner/name\"` under [sync] in config.toml \
         and run `ai-usagebar sync setup` — this tool never creates a repository."
            .to_owned()
    })?;
    let repo = RepoRef::parse(configured).map_err(|e| e.to_string())?;

    // The pairing record is what supplies the bundle identifier bound into every
    // snapshot root. Reading it from the *record* rather than from a response is
    // the format's §5 rule: a reader binds its own identifier.
    let pairing = pairing::read_from(&pairing::default_path(roots))
        .map_err(|e| e.to_string())?
        .ok_or_else(|| {
            format!(
                "this machine is not paired with {repo} yet. Run `ai-usagebar sync setup` \
                 first — it verifies the repository is private and records which repository \
                 this bundle belongs to."
            )
        })?;

    let (value, source) = token::resolve(chain).map_err(|e| e.to_string())?;
    let client = Client::new(endpoints, value, source).map_err(|e| e.to_string())?;
    let index = Index::at(&roots.index_file).map_err(|e| e.to_string())?;
    Ok(Resolved {
        repo,
        client,
        index,
        repo_id: push::repo_id_for(pairing.repo_id),
    })
}

/// `ai-usagebar sync push`.
///
/// **Every refusal that does not need a password comes first.** The password is
/// read here, at the terminal, and never below — nothing under
/// `src/sync/push/` reads a password or an environment variable — so an
/// unconfigured or unpaired machine must be told so *before* it is asked for
/// one.
fn push(
    cfg: &Config,
    roots: &SyncRoots,
    endpoints: &Endpoints,
    chain: &TokenChain,
    allow_rollback: bool,
    recovery: Recovery,
    now: DateTime<Utc>,
) -> i32 {
    if let Err(why) = maybe_reset_index(roots, recovery.rebuild) {
        return refuse(&why);
    }
    let mut parts = match resolve(cfg, roots, endpoints, chain) {
        Ok(parts) => parts,
        Err(why) => return refuse(&why),
    };
    parts.index = rehashing(parts.index, recovery.rehash);
    let parts = &parts;
    match local_keyfile(&keyfile_path(roots), true) {
        Ok(keyfile) => push_with_parts(cfg, roots, parts, &keyfile, allow_rollback, now),
        Err(why) => refuse(&why),
    }
}

/// The tested seam: everything injected, and no terminal — `resolve` has already
/// turned the config, the token chain and the pairing record into values.
///
/// **A run whose only failure is the prune step exits 0.** That is D2 in the
/// exit code, and it is the one place where getting it wrong is silent: the
/// push already succeeded and the user's data is safe, so leaving a few stale
/// packs costs storage, not correctness.
fn push_with_parts(
    cfg: &Config,
    roots: &SyncRoots,
    parts: &Resolved,
    keyfile: &LocalKeyfile,
    allow_rollback: bool,
    now: DateTime<Utc>,
) -> i32 {
    let rt = match runtime() {
        Ok(rt) => rt,
        Err(why) => return refuse(&why),
    };
    let mut ctx = context(cfg, roots, keyfile, parts, now);
    ctx.allow_rollback = allow_rollback;
    // A progress line on a terminal, plain completed-asset lines when piped.
    // `is_terminal` is read here rather than inside the reporter so tests can
    // pin either shape without a tty.
    let mut progress = progress::reporter(std::io::stderr().is_terminal(), stderr_style());
    match rt.block_on(push::run(ctx, progress.as_mut())) {
        Ok(outcome) => {
            print!("{}", render_push(&outcome));
            0
        }
        Err(e) => refuse(&e.to_string()),
    }
}

/// `ai-usagebar sync prune`.
///
/// Unlike the automatic prune after a push, a failure here **is** a failure:
/// the user asked for exactly this.
fn prune(
    cfg: &Config,
    roots: &SyncRoots,
    endpoints: &Endpoints,
    chain: &TokenChain,
    now: DateTime<Utc>,
) -> i32 {
    let parts = match resolve(cfg, roots, endpoints, chain) {
        Ok(parts) => parts,
        Err(why) => return refuse(&why),
    };
    let keyfile = match local_keyfile(&keyfile_path(roots), true) {
        Ok(k) => k,
        Err(why) => return refuse(&why),
    };
    let rt = match runtime() {
        Ok(rt) => rt,
        Err(why) => return refuse(&why),
    };
    let ctx = context(cfg, roots, &keyfile, &parts, now);
    match rt.block_on(push::prune::run_on_demand(
        &ctx,
        cfg.sync.keep_snapshots as usize,
    )) {
        Ok(deleted) => {
            println!("pruned:     {deleted} pack(s) no kept snapshot still referenced");
            0
        }
        Err(e) => refuse(&e.to_string()),
    }
}

/// `ai-usagebar sync rekey`.
///
/// **This arm owns both password prompts**, and they come *after* every refusal
/// that does not need one — an unconfigured or unpaired machine is told so
/// before it is asked for a password it would then discard. They go through plan
/// 3-07's prompt seam: TTY or stdin, never a command-line argument and never an
/// environment variable, which is Phase 1's rule and is not relaxed. Phase 1's
/// strength floor is applied to the new password before the call, so a refused
/// password costs no network round trip.
fn rekey(
    cfg: &Config,
    roots: &SyncRoots,
    endpoints: &Endpoints,
    chain: &TokenChain,
    now: DateTime<Utc>,
) -> i32 {
    use crate::sync::github::setup::SetupPrompt;

    let parts = match resolve(cfg, roots, endpoints, chain) {
        Ok(parts) => parts,
        Err(why) => return refuse(&why),
    };
    let rt = match runtime() {
        Ok(rt) => rt,
        Err(why) => return refuse(&why),
    };

    let mut prompt = TtyPrompt::new(stdout_style());
    prompt.say(
        "Changing the sync password rewraps the master key. Not one pack byte moves — and \
         this is NOT revocation: anyone who already holds a copy of the old keyfile can still \
         open it with the old password.",
    );
    prompt.say("The CURRENT sync password:");
    let old_pw = match prompt.passphrase("") {
        Ok(pw) => pw,
        Err(e) => return refuse(&e.to_string()),
    };
    prompt.say("The NEW sync password:");
    let new_pw = match prompt.passphrase("") {
        Ok(pw) => pw,
        Err(e) => return refuse(&e.to_string()),
    };
    // Phase 1's floor, at the parameters the new keyfile will be written at.
    match passphrase::check(&new_pw, prompt.kdf()) {
        passphrase::Strength::Rejected(why) => return refuse(&format!("refused: {why}")),
        passphrase::Strength::Weak(why) => prompt.say(&format!("     {why}")),
        passphrase::Strength::Strong => {}
    }

    let keyfile = match local_keyfile_with(&keyfile_path(roots), &old_pw) {
        Ok(k) => k,
        Err(why) => return refuse(&why),
    };
    let ctx = context(cfg, roots, &keyfile, &parts, now);
    match rt.block_on(push::rekey::run(&ctx, &old_pw, &new_pw)) {
        Ok(asset) => {
            println!("keyfile:    republished as {asset}");
            println!(
                "note:       this is not revocation — an old copy of the keyfile still opens \
                 under the old password."
            );
            0
        }
        Err(e) => refuse(&e.to_string()),
    }
}

// ---- the inbound command ---------------------------------------------------

/// Where a pull talks, and whether it may ask.
///
/// **`gate` is `Some` only when stdin is a terminal**, and that is this
/// command's deliberate answer to a real collision. A pull reads the sync
/// password off stdin, and both of plan 5-06's confirmations read from stdin
/// too. Over a *pipe* there is one stream and no way to tell the two apart, so
/// whichever reads second eats the other's line. The password wins that
/// stream — it is the one input that cannot be supplied any other way, since
/// Phase 1's rule keeps it out of argv and out of the environment — and a piped
/// run therefore answers with `--apply`, `--yes` and `--force-credentials`
/// rather than with typed words. Over a *terminal* the two reads are
/// sequential, nothing is consumed twice, and both confirmations are offered
/// normally.
struct PullIo<'a> {
    out: &'a mut dyn std::io::Write,
    gate: Option<&'a mut dyn std::io::BufRead>,
}

/// Announced before an interactive read, because the password is echoed.
const ECHOED_PROMPT: &str = "The sync password for this bundle. It is echoed — this build \
                             has no hidden-input dependency.";

/// The message when the stream ended without one.
const NO_PASSWORD: &str = "no sync password arrived on stdin";

/// **The one place either arm of this command reads the sync password.**
///
/// A pull opens a keyfile that comes off the remote, not off this disk — a
/// second machine has none, which is the whole point of a restore — while a
/// push opens the local one through [`local_keyfile`]. Different keyfiles, one
/// password, and one read: they used to be two, and they had drifted. The pull
/// arm asked on a terminal (Phase 5) and the push arm refused on one (Phase 2),
/// so `ai-usagebar sync push` typed at a prompt — the invocation
/// `docs/sync-github.md` documents — answered "this build has no interactive
/// prompt" and could not be run by hand at all.
///
/// It arrives the same way it always has: stdin only, never argv, never an
/// environment variable (T-2-29, T-5-66).
fn sync_password(interactive: bool) -> std::result::Result<zeroize::Zeroizing<String>, String> {
    if interactive {
        eprintln!("{ECHOED_PROMPT}");
    }
    sync_password_from(std::io::stdin().lock())
}

/// [`sync_password`] over an injected reader — the tested half, so neither arm
/// needs a terminal or the process's real stdin to be covered.
fn sync_password_from(
    r: impl std::io::BufRead,
) -> std::result::Result<zeroize::Zeroizing<String>, String> {
    let pw = passphrase::read_line(r).map_err(|e| e.to_string())?;
    // Without this an unattended run with stdin on /dev/null would spend a
    // gibibyte and a second and a half hashing the empty string first.
    if pw.is_empty() {
        return Err(NO_PASSWORD.into());
    }
    Ok(pw)
}

/// `--rebuild-index` / `--force-rehash`, carried as one value because two
/// commands offer them and two implementations would be two sets of semantics.
///
/// Both live on `sync push`, where they change what the planner does, and
/// `--rebuild-index` is offered on `sync pull` as well: a user reaches for it
/// after losing a machine, and after a machine loss the command they are
/// running is `pull`. Neither can lose data — the worst either does is make one
/// run slow.
#[derive(Debug, Clone, Copy, Default)]
struct Recovery {
    rebuild: bool,
    rehash: bool,
}

const fn recovery(rebuild: bool, rehash: bool) -> Recovery {
    Recovery { rebuild, rehash }
}

/// `--rebuild-index`, applied before anything opens the database.
fn maybe_reset_index(roots: &SyncRoots, rebuild: bool) -> std::result::Result<(), String> {
    if !rebuild {
        return Ok(());
    }
    index::reset_at(&roots.index_file)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// `--force-rehash`, applied where the handle is made — every planner that
/// takes this index then re-reads every file, and none of them had to remember
/// to thread a flag through.
fn rehashing(index: Index, rehash: bool) -> Index {
    if rehash { index.rehashing() } else { index }
}

/// `~/.claude-acc/backups` — the account switcher's own archive directory, so a
/// user has one place to look for "undo" rather than two (D3).
///
/// Derived from the injected roots the same way `claude_desktop::Paths::resolve`
/// derives it from `$HOME`, and never from `$HOME` here: that is what keeps
/// every test's archive inside its own `TempDir`.
fn backups_dir(roots: &SyncRoots) -> PathBuf {
    roots
        .desktop_profiles_dir
        .parent()
        .unwrap_or(&roots.desktop_profiles_dir)
        .join("backups")
}

/// `ai-usagebar sync pull` — the command a second machine types.
///
/// **Every refusal that does not need a password comes first**, exactly as the
/// push arm orders them: an unconfigured or unpaired machine is told so before
/// it is asked for a password it would then discard.
fn pull(
    cfg: &Config,
    roots: &SyncRoots,
    endpoints: &Endpoints,
    chain: &TokenChain,
    opts: RestoreOptions,
    now: DateTime<Utc>,
) -> i32 {
    // Before `resolve` opens the index, never after.
    if let Err(why) = maybe_reset_index(roots, opts.rebuild_index) {
        return refuse(&why);
    }
    let parts = match resolve(cfg, roots, endpoints, chain) {
        Ok(parts) => parts,
        Err(why) => return refuse(&why),
    };
    // One read of the terminal, used for both the password and the gates, so
    // the two can never disagree about which stream they are sharing.
    let interactive = std::io::stdin().is_terminal();
    let pw = match sync_password(interactive) {
        Ok(pw) => pw,
        Err(why) => return refuse(&why),
    };
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    if interactive {
        let stdin = std::io::stdin();
        let mut gate = stdin.lock();
        let mut io = PullIo {
            out: &mut out,
            gate: Some(&mut gate),
        };
        pull_with_parts(roots, &parts, &pw, opts, &mut io, now)
    } else {
        let mut io = PullIo {
            out: &mut out,
            gate: None,
        };
        pull_with_parts(roots, &parts, &pw, opts, &mut io, now)
    }
}

/// The tested seam: no terminal, no password prompt, everything injected.
///
/// **The order below is the phase's safety property, not a style.**
///
/// 1. plan, always, with `apply` off — including under `--apply`, because the
///    report is what both gates and the summary are built from, and a dry run
///    fetches no file content at all (5-02).
/// 2. the one apply gate (D6), which renders the plan itself.
/// 3. the credential gate (D2), **before** `backup::take` and before the first
///    byte, so a user who stops here has cost the machine nothing at all — no
///    archive, no writes (T-5-60, T-5-61).
/// 4. `restore::run` with `apply` set, which takes the backup and writes in the
///    order `restore/mod.rs` froze.
///
/// Exit codes: 0 for a completed dry run, a declined apply gate, or a completed
/// apply; 1 for every error, one message to stderr. A declined *apply* gate is
/// a choice the tool honoured; a declined *credential* gate stopped a restore
/// the user had already asked for, and is reported as the failure it is.
fn pull_with_parts(
    roots: &SyncRoots,
    parts: &Resolved,
    passphrase: &zeroize::Zeroizing<String>,
    mut opts: RestoreOptions,
    io: &mut PullIo<'_>,
    now: DateTime<Utc>,
) -> i32 {
    let rt = match runtime() {
        Ok(rt) => rt,
        Err(why) => return refuse(&why),
    };
    // 4-08 wired the rollback anchor onto the push path and named the file.
    // Restore reads and advances the **same** one, through the same helper —
    // a second implementation is how a defence ends up with two copies that
    // disagree, which is the finding that put the anchor on the path at all.
    let anchor_path = push::anchor_path(roots, &parts.repo);
    let backups_dir = backups_dir(roots);
    let ctx = |opts| restore::RestoreCtx {
        client: &parts.client,
        repo: &parts.repo,
        roots,
        repo_id: &parts.repo_id,
        passphrase,
        anchor_path: &anchor_path,
        backups_dir: &backups_dir,
        opts,
        now,
    };

    // 1.
    let plan = match rt.block_on(restore::run(ctx(RestoreOptions {
        apply: false,
        ..opts
    }))) {
        Ok(outcome) => outcome.plan,
        Err(e) => return refuse(&e.to_string()),
    };

    // 2. `confirm_apply` writes the whole report before it asks, so the plan is
    //    rendered exactly once on every path through here — printing it and
    //    then calling the gate would show it twice.
    if !opts.apply {
        let Some(gate) = io.gate.as_deref_mut() else {
            // A dry run is a *success*: it did exactly what it was asked, and
            // the footer names `--apply` (`report::APPLY_COMMAND`).
            return match write!(io.out, "{}", restore::report::render_plan(&plan, false)) {
                Ok(()) => 0,
                Err(e) => refuse(&e.to_string()),
            };
        };
        match restore::report::confirm_apply(&plan, &opts, io.out, gate) {
            Ok(true) => {}
            Ok(false) => return 0,
            Err(e) => return refuse(&e.to_string()),
        }
    } else if let Err(e) = write!(io.out, "{}", restore::report::render_plan(&plan, true)) {
        return refuse(&e.to_string());
    }

    // 3.
    let credentials: Vec<&restore::ItemPlan> = plan
        .items
        .iter()
        .filter(|item| {
            // Both consents run through the one gate: a locally-newer
            // `.credentials.json`, and a machine-bound store this machine
            // already holds a different login in. Answering the prompt sets
            // `force_credentials`, which is what promotes either.
            matches!(
                item.disposition,
                restore::Disposition::NeedsCredentialConfirm { .. }
                    | restore::Disposition::ReplacesLiveCredential
            )
        })
        .collect();
    if !credentials.is_empty() {
        // Piped, stdin belongs to the password (see `PullIo`); an unattended
        // run answers with `--force-credentials`, which the gate reads itself.
        let mut unanswerable = std::io::empty();
        let answered = match io.gate.as_deref_mut() {
            Some(gate) => restore::report::confirm_credentials(&credentials, &opts, io.out, gate),
            None => {
                restore::report::confirm_credentials(&credentials, &opts, io.out, &mut unanswerable)
            }
        };
        match answered {
            // Set the option and re-plan below, rather than editing the
            // dispositions in hand: a hand-set disposition and its recorded
            // reason drift apart, and re-planning is one more pass over data
            // that has already been downloaded.
            Ok(true) => opts.force_credentials = true,
            Ok(false) => {
                let named: Vec<String> = credentials
                    .iter()
                    .map(|item| {
                        crate::display::sanitize_untrusted_field(&item.manifest_path).to_string()
                    })
                    .collect();
                return refuse(&format!(
                    "the restore stopped at the credential confirmation. Nothing was written \
                     and no backup was taken.\n\
                     \x20           left alone: {}\n\
                     \x20           Re-run with `--force --force-credentials` to replace them.",
                    named.join(", ")
                ));
            }
            Err(e) => return refuse(&e.to_string()),
        }
    }

    // 4.
    opts.apply = true;
    let outcome = match rt.block_on(restore::run(ctx(opts))) {
        Ok(outcome) => outcome,
        Err(e) => return refuse(&e.to_string()),
    };
    if let Err(e) = write!(io.out, "{}", restore::report::render_outcome(&outcome)) {
        return refuse(&e.to_string());
    }
    // A partial restore is reported, not rolled back — and it is not a success.
    i32::from(outcome.failed_at.is_some())
}

/// One non-zero exit, one message, and nothing else: no token, no prefix of one,
/// no header dump, no response body echoed unsanitized. Remote-supplied text
/// arrives already through `http::message_of`'s `sanitize_untrusted_field`.
fn refuse(why: &str) -> i32 {
    // The one place this tool says no. Red is reserved for exactly this.
    let style = stderr_style();
    eprintln!("{} {why}", style.bad("sync:"));
    1
}

fn context<'a>(
    cfg: &'a Config,
    roots: &'a SyncRoots,
    keyfile: &'a LocalKeyfile,
    parts: &'a Resolved,
    now: DateTime<Utc>,
) -> PushCtx<'a> {
    PushCtx {
        client: &parts.client,
        repo: &parts.repo,
        cfg: &cfg.sync,
        roots,
        keys: &keyfile.keys,
        kdf: keyfile.kdf,
        index: &parts.index,
        repo_id: parts.repo_id.clone(),
        keyfile_asset: keyfile.asset.clone(),
        // Filled by `push::run` from the remote, after the gate. A caller that
        // populated it would have had to make a request before the gate.
        previous: None,
        // Only `sync push` offers the escape; prune and rekey refuse a
        // rolled-back pointer outright, because neither is a command a user
        // reaches for when they mean to move the bundle backwards.
        allow_rollback: false,
        now,
    }
}

/// Pure, so the no-secret assertion is on a value rather than on captured
/// stdout. `PushOutcome` has no field that could hold a token or a passphrase.
fn render_push(outcome: &PushOutcome) -> String {
    let mut out = format!(
        "\nuploaded:   {} pack(s), {}\nskipped:    {} pack(s) already present\n\
         snapshots:  {} kept\npruned:     {} pack(s)\n",
        outcome.packs_uploaded,
        report::human_bytes(outcome.bytes_uploaded),
        outcome.packs_skipped,
        outcome.snapshots_kept,
        outcome.packs_deleted,
    );
    // D2: a prune failure is a warning on a successful push, never a failure.
    if let Some(warning) = &outcome.prune_warning {
        out.push_str(&format!(
            "warning:    the push succeeded and the snapshot is published, but cleaning up \
             superseded data did not: {warning}\n\
             \x20           This costs storage, not correctness. `ai-usagebar sync prune` \
             retries it.\n"
        ));
    }
    out.push_str("\nThe snapshot is published.\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::github::setup::{Double, Script};
    use base64::Engine as _;
    use std::fs;
    use std::sync::Mutex;
    use tempfile::TempDir;

    const TOKEN: &str = "github_pat_fixture_not_a_real_token";
    const PRIVATE_BODY: &str = r#"{"id":1,"private":true,"visibility":"private",
        "owner":{"login":"o","id":7},"archived":false,"fork":false}"#;

    fn roots_at(dir: &TempDir) -> SyncRoots {
        SyncRoots::at(
            dir.path().join("config.toml"),
            dir.path().to_path_buf(),
            dir.path().join("desktop"),
            dir.path().join("profiles"),
            dir.path().join("claude-home"),
        )
    }

    #[test]
    fn the_keyfile_sits_beside_the_config_and_is_never_resolved_from_home() {
        let dir = TempDir::new().unwrap();
        let path = keyfile_path(&roots_at(&dir));
        assert!(path.starts_with(dir.path()), "{}", path.display());
        assert!(path.ends_with("sync/keyfile.json"), "{}", path.display());
    }

    /// The common case in this build: no bundle has been set up, so the column
    /// is unavailable — and the message says the counts still are.
    #[test]
    fn a_missing_keyfile_explains_itself_without_failing_the_command() {
        let dir = TempDir::new().unwrap();
        let err = keys_at(&keyfile_path(&roots_at(&dir))).expect_err("no keyfile was written");
        assert!(err.contains("no sync keyfile"), "{err}");
        assert!(err.contains("need no password"), "{err}");
    }

    /// T-2-30: a refusal names the file, never its bytes.
    #[test]
    fn an_unreadable_keyfile_is_refused_without_echoing_its_contents() {
        let dir = TempDir::new().unwrap();
        let path = keyfile_path(&roots_at(&dir));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{\"wrapped_master_key\": \"not-a-keyfile-at-all\"}").unwrap();

        let err = keys_at(&path).expect_err("that is not a keyfile");
        assert!(err.contains("not a readable sync keyfile"), "{err}");
        assert!(!err.contains("not-a-keyfile-at-all"), "{err}");
    }

    /// Injected end to end. Nothing here reads a real `$HOME`, which is what
    /// keeps the AUR `check()` from failing on an installer's machine.
    fn drive(action: &SyncAction, cfg: &Config, dir: &TempDir, base: &str) -> i32 {
        run_with(
            action,
            cfg,
            &roots_at(dir),
            &Endpoints {
                api_base: base.into(),
                uploads_base: base.into(),
            },
            &TokenChain {
                env_value: Some(zeroize::Zeroizing::new(TOKEN.into())),
                ..TokenChain::default()
            },
            DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        )
    }

    const NOW: DateTime<Utc> = match DateTime::from_timestamp(1_700_000_000, 0) {
        Some(t) => t,
        None => panic!("a fixed timestamp"),
    };

    /// The repository section alone, injected end to end.
    fn section_for(cfg: &Config, dir: &TempDir, base: &str) -> RepoSection {
        resolve_repo_section(
            cfg,
            &roots_at(dir),
            &Endpoints {
                api_base: base.into(),
                uploads_base: base.into(),
            },
            &TokenChain {
                env_value: Some(zeroize::Zeroizing::new(TOKEN.into())),
                ..TokenChain::default()
            },
            NOW,
        )
    }

    fn cfg_with_repo(repo: Option<&str>) -> Config {
        Config {
            sync: crate::config::SyncConfig {
                repo: repo.map(str::to_owned),
                ..Default::default()
            },
            ..Config::default()
        }
    }

    #[test]
    fn a_push_on_an_unconfigured_machine_refuses_before_any_request() {
        let dir = TempDir::new().unwrap();
        assert_ne!(
            drive(
                &SyncAction::Push {
                    dry_run: false,
                    allow_rollback: false,
                    rebuild_index: false,
                    force_rehash: false,
                },
                &cfg_with_repo(None),
                &dir,
                "http://127.0.0.1:1",
            ),
            0
        );
    }

    /// D-01: a missing `[sync] repo` is a non-zero exit that names the fix, and
    /// no request is made at all — the endpoint here is a dead port. Safe to
    /// drive through `run_with`, which builds a real `TtyPrompt`, precisely
    /// because the refusal happens before any prompt method is reached.
    #[test]
    fn setup_without_a_configured_repo_exits_non_zero() {
        let dir = TempDir::new().unwrap();
        assert_ne!(
            drive(
                &SyncAction::Setup,
                &cfg_with_repo(None),
                &dir,
                "http://127.0.0.1:1"
            ),
            0
        );
    }

    /// T-3-01 and T-3-36. `render_setup` is pure precisely so this can be
    /// asserted rather than reasoned about.
    ///
    /// Drives `github::setup::run` with the scripted double rather than
    /// `run_with`'s `Setup` arm: that arm constructs a `TtyPrompt`, and a test
    /// that let it read stdin would also pay the shipped 1 GiB KDF.
    #[test]
    fn the_success_line_reports_the_token_source_and_never_the_token() {
        let dir = TempDir::new().unwrap();
        let mut server = mockito::Server::new();
        let _m = server
            .mock("GET", "/repos/o/n")
            .with_status(200)
            .with_body(PRIVATE_BODY)
            .create();
        // Nothing has ever been pushed here, so step 3 mints a key rather than
        // joining a published bundle. Without this the pointer read answers
        // with mockito's 501.
        let _p = server
            .mock("GET", "/repos/o/n/contents/sync/pointer.json")
            .with_status(404)
            .with_body(r#"{"message":"Not Found"}"#)
            .create();
        // …and it already has a commit, so setup makes no offer to add one.
        let _c = server
            .mock("GET", "/repos/o/n/commits")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_body("[]")
            .create();

        let script = Script::new();
        script
            .borrow_mut()
            .passphrases
            .push("a-supplied-passphrase-long-enough".into());
        let rt = runtime().unwrap();
        let outcome = rt
            .block_on(github::setup::run(
                &cfg_with_repo(Some("o/n")).sync,
                &roots_at(&dir),
                &Endpoints {
                    api_base: server.url(),
                    uploads_base: server.url(),
                },
                &TokenChain {
                    env_value: Some(zeroize::Zeroizing::new(TOKEN.into())),
                    ..TokenChain::default()
                },
                &mut Double(std::rc::Rc::clone(&script)),
                DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
            ))
            .unwrap();
        let rendered = render_setup(&outcome);

        assert!(rendered.contains("o/n"), "{rendered}");
        assert!(rendered.contains("private"), "{rendered}");
        assert!(rendered.contains("token:      present (env)"), "{rendered}");
        assert!(rendered.contains("ready to push"), "{rendered}");
        assert!(
            rendered.contains("Nothing was uploaded"),
            "D-05: {rendered}"
        );
        assert!(!rendered.contains(TOKEN), "{rendered}");
        assert!(!rendered.contains(&TOKEN[..8]), "{rendered}");
        assert!(
            !rendered.contains("a-supplied-passphrase-long-enough"),
            "{rendered}"
        );
        assert!(!rendered.contains("a-suppli"), "{rendered}");
        assert!(!outcome.initialised, "this repository already had a commit");

        // 6-11: and when setup *did* put the approved README in an empty
        // repository, the closing line reports it instead of claiming over it.
        let initialised = render_setup(&github::setup::SetupOutcome {
            initialised: true,
            ..outcome
        });
        assert!(
            initialised.contains("a README was added to the empty repository"),
            "{initialised}"
        );
        assert!(
            !initialised.contains("Nothing was uploaded"),
            "a README was: {initialised}"
        );
        assert!(
            initialised.contains("uploads no bundle data"),
            "D-05 still holds for everything else: {initialised}"
        );
    }

    // ---- 3-07: `sync status` learns about the repository -----------------

    /// The `[sync] repo` key is named, and Phase 2's category listing is still
    /// there. No request is made at all — the endpoint is a dead port.
    #[test]
    fn status_without_a_configured_repo_names_the_key_and_still_lists_categories() {
        let dir = TempDir::new().unwrap();
        let cfg = cfg_with_repo(None);
        let roots = roots_at(&dir);
        let repo = resolve_repo_section(
            &cfg,
            &roots,
            &Endpoints {
                api_base: "http://127.0.0.1:1".into(),
                uploads_base: "http://127.0.0.1:1".into(),
            },
            &TokenChain::default(),
            NOW,
        );
        assert!(repo.failure.is_none(), "unconfigured is not a failure");

        let text = report::render_status(&report::build_status(
            &roots,
            &cfg.sync,
            None,
            NOW,
            None,
            Some(repo),
        ));
        assert!(text.contains("not configured"), "{text}");
        assert!(text.contains("[sync]"), "{text}");
        for category in SyncCategory::ALL {
            assert!(text.contains(category.label()), "{text}");
        }
        assert_eq!(
            drive(
                &SyncAction::Status { json: false },
                &cfg,
                &dir,
                "http://127.0.0.1:1"
            ),
            0,
            "an unconfigured machine is not a failure"
        );
    }

    /// The four repository facts, and exactly one request for them.
    #[test]
    fn status_reports_the_repository_visibility_token_source_and_last_verified() {
        let dir = TempDir::new().unwrap();
        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/repos/o/n")
            .with_status(200)
            .with_body(PRIVATE_BODY)
            .expect(1)
            .create();

        let roots = roots_at(&dir);
        pairing::write_to(
            &pairing::default_path(&roots),
            &pairing::Pairing {
                repo_id: 1,
                owner_id: 7,
                private: true,
                checked_at: NOW,
            },
        )
        .unwrap();

        let code = drive(
            &SyncAction::Status { json: false },
            &cfg_with_repo(Some("o/n")),
            &dir,
            &server.url(),
        );
        assert_eq!(code, 0);
        // One invocation, one repository request — the gate is not run twice.
        m.assert();

        let repo = section_for(&cfg_with_repo(Some("o/n")), &dir, &server.url());
        let text = report::render_status(&report::build_status(
            &roots,
            &cfg_with_repo(Some("o/n")).sync,
            None,
            NOW,
            None,
            Some(repo),
        ));
        assert!(text.contains("repo:      o/n"), "{text}");
        assert!(text.contains("visible:   private"), "{text}");
        assert!(text.contains("token:     present (env)"), "{text}");
        assert!(text.contains(&NOW.to_rfc3339()), "{text}");
        assert!(!text.contains(TOKEN), "{text}");
        assert!(!text.contains(&TOKEN[..8]), "{text}");
    }

    /// SAFE-02: a repository that has turned public since pairing is the
    /// incident, verbatim — not a generic error, and not a paraphrase.
    #[test]
    fn a_repository_that_turned_public_is_reported_as_the_incident_and_exits_non_zero() {
        let dir = TempDir::new().unwrap();
        let mut server = mockito::Server::new();
        let _m = server
            .mock("GET", "/repos/o/n")
            .with_status(200)
            .with_body(
                r#"{"id":1,"private":false,"visibility":"public",
                    "owner":{"login":"o","id":7},"archived":false,"fork":false}"#,
            )
            .create();

        let roots = roots_at(&dir);
        pairing::write_to(
            &pairing::default_path(&roots),
            &pairing::Pairing {
                repo_id: 1,
                owner_id: 7,
                private: true,
                checked_at: NOW,
            },
        )
        .unwrap();

        let cfg = cfg_with_repo(Some("o/n"));
        let repo = section_for(&cfg, &dir, &server.url());
        let failure = repo.failure.clone().expect("the incident");
        assert!(
            failure.starts_with("STOP — the backup repository was private"),
            "{failure}"
        );
        assert!(
            failure.contains("Rotating is the only thing that undoes this"),
            "{failure}"
        );

        // …and the category listing survives it, while the exit code does not.
        let text = report::render_status(&report::build_status(
            &roots,
            &cfg.sync,
            None,
            NOW,
            None,
            Some(repo),
        ));
        assert!(
            text.contains("config"),
            "the categories are still there: {text}"
        );
        assert_ne!(
            drive(
                &SyncAction::Status { json: false },
                &cfg,
                &dir,
                &server.url()
            ),
            0
        );
    }

    // ---- 4-01: the push, end to end ---------------------------------------

    /// A seeded tree, a cheap keyfile, and a pairing record — everything a push
    /// resolves from the local machine, all inside the injected `TempDir`.
    ///
    /// Microsecond KDF parameters, never production ones: the AUR `check()` runs
    /// these tests on an installer's machine.
    fn seeded(dir: &TempDir) -> (SyncRoots, LocalKeyfile) {
        let roots = roots_at(dir);
        fs::create_dir_all(&roots.config_dir).unwrap();
        fs::write(&roots.config_file, b"[anthropic]\nenabled = true\n").unwrap();

        let cheap = crate::sync::crypto::KdfParams {
            m_kib: 8,
            t: 1,
            p: 1,
        };
        let (file, keys) =
            Keyfile::create_with_floor(b"correct horse battery staple", cheap, cheap.m_kib)
                .unwrap();
        let asset = keyfile_asset_for(&file).unwrap();
        // `ensure_keyfile` publishes the keyfile from disk, exactly as `sync
        // setup` writes it — so the fixture has to have written it. Holding the
        // `Keyfile` only in memory made every push fail at the keyfile hop.
        let keyfile_path = keyfile_path(&roots);
        fs::create_dir_all(keyfile_path.parent().unwrap()).unwrap();
        fs::write(&keyfile_path, serde_json::to_vec_pretty(&file).unwrap()).unwrap();
        pairing::write_to(
            &pairing::default_path(&roots),
            &pairing::Pairing {
                repo_id: 1,
                owner_id: 7,
                private: true,
                checked_at: NOW,
            },
        )
        .unwrap();
        (
            roots,
            LocalKeyfile {
                keys,
                kdf: cheap,
                asset,
            },
        )
    }

    /// Drives the push at `push_with_parts`, one step below the arm that reads
    /// the sync password off stdin — the same carve-out the `Setup` arm takes,
    /// and for the same reason: no test may drive a terminal.
    fn push_against(cfg: &Config, roots: &SyncRoots, keyfile: &LocalKeyfile, base: &str) -> i32 {
        let parts = resolve(
            cfg,
            roots,
            &Endpoints {
                api_base: base.into(),
                uploads_base: base.into(),
            },
            &TokenChain {
                env_value: Some(zeroize::Zeroizing::new(TOKEN.into())),
                ..TokenChain::default()
            },
        )
        .expect("the fixture is configured and paired");
        push_with_parts(cfg, roots, &parts, keyfile, false, NOW)
    }

    /// Every asset a fixture has accepted, in upload order: id `n` is index
    /// `n - 1`. Keyed by name because a push uploads several packs.
    type Sent = std::sync::Arc<Mutex<Vec<(String, Vec<u8>)>>>;

    /// `…/releases/9/assets?name=pack-<hex>.bin` — the name GitHub assigns is
    /// the one the caller asked for, so the fixture echoes it rather than
    /// inventing one.
    fn asset_name_in(path_and_query: &str) -> String {
        path_and_query
            .split_once("name=")
            .map(|(_, rest)| rest.split('&').next().unwrap_or(rest).to_string())
            .unwrap_or_else(|| panic!("an upload names its asset: {path_and_query}"))
    }

    fn asset_id_in(path: &str) -> usize {
        path.rsplit('/')
            .next()
            .and_then(|id| id.parse().ok())
            .unwrap_or_else(|| panic!("an asset path ends in its id: {path}"))
    }

    fn asset_json_sized(id: u64, name: &str, size: usize) -> String {
        format!(
            r#"{{"id":{id},"name":"{name}","size":{size},"state":"uploaded",
                "created_at":"2023-11-14T22:13:20Z"}}"#
        )
    }

    fn asset_json(id: u64, name: &str) -> String {
        format!(
            r#"{{"id":{id},"name":"{name}","size":1,"state":"uploaded",
                "created_at":"2023-11-14T22:13:20Z"}}"#
        )
    }

    /// Everything the outbound path touches: the release read, the pointer
    /// read, the resume listing, the uploads and the verifying downloads.
    ///
    /// **A push uploads more than one asset.** Plan 4-02 packs the manifest and
    /// the index object alongside the data chunks, so even a one-file bundle
    /// produces several packs. The fixture therefore keys everything by asset
    /// name and hands out ids in upload order, rather than assuming a single
    /// `pack-x.bin` with id 1 — an assumption that silently made the verifying
    /// download (D3) compare one pack's bytes against another's.
    ///
    /// The verification hop serves back the very bytes it was handed, so the
    /// recorder is per-test rather than a shared static: these tests run in
    /// parallel.
    fn mock_upload_path(server: &mut mockito::ServerGuard) -> Sent {
        let sent: Sent = std::sync::Arc::default();

        server
            .mock("GET", "/repos/o/n/releases/tags/ai-usagebar-sync-v1")
            .with_status(200)
            .with_body(r#"{"id":9}"#)
            .create();
        server
            .mock("GET", "/repos/o/n/contents/sync/pointer.json")
            .with_status(404)
            .with_body(r#"{"message":"Not Found"}"#)
            .create();
        // 4-03's resume scan lists the release's assets before the first
        // upload. Empty, because this fixture is a first push and nothing has
        // landed yet. `expect(1)` deliberately: the incident path in
        // `a_repository_that_turns_public_mid_push_deletes_and_does_not_flip`
        // lists a *second* time, and mockito prefers a matching mock that is
        // still missing hits — so that test's own listing answers the second
        // call without this one having to know about it.
        server
            .mock("GET", "/repos/o/n/releases/9/assets")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_body("[]")
            // How many listings a push makes depends on how far it gets: the
            // resume scan always, `ensure_keyfile`'s own listing only if the
            // re-gate passes. `expect_at_least(1)` so the incident path, which
            // stops before the keyfile hop, still leaves its own listing mock
            // unsatisfied — mockito prefers a matching mock still missing hits.
            .expect_at_least(1)
            .create();

        // The body is recorded from `with_body_from_request`, not from
        // `match_request`: mockito evaluates every mock's matcher against every
        // request clearing method and path, so recording in a matcher counts
        // requests this mock never answers.
        let recorder = std::sync::Arc::clone(&sent);
        server
            .mock("POST", mockito::Matcher::Regex("/releases/9/assets".into()))
            .with_status(201)
            .with_body_from_request(move |req| {
                let name = asset_name_in(req.path_and_query());
                let mut held = recorder.lock().unwrap();
                held.push((name.clone(), req.body().unwrap().clone()));
                asset_json_sized(held.len() as u64, &name, held.last().unwrap().1.len())
                    .into_bytes()
            })
            .create();

        let echo = std::sync::Arc::clone(&sent);
        server
            .mock(
                "GET",
                mockito::Matcher::Regex(r"/releases/assets/\d+$".into()),
            )
            .with_status(200)
            .with_body_from_request(move |req| {
                let id = asset_id_in(req.path());
                echo.lock()
                    .unwrap()
                    .get(id - 1)
                    .map(|(_, bytes)| bytes.clone())
                    .unwrap_or_default()
            })
            .create();

        sent
    }

    /// The whole outbound path: gate, plan, pack, upload, verify, re-gate, flip.
    /// One file, one pack, one asset, one compare-and-swap.
    #[test]
    fn a_push_uploads_one_asset_and_flips_the_pointer_with_a_precondition() {
        let dir = TempDir::new().unwrap();
        let (roots, keyfile) = seeded(&dir);
        let mut server = mockito::Server::new();

        // Two visibility reads: one before the first byte, one before the flip.
        // The gate is re-earned inside the push, never carried from `sync setup`.
        let gate = server
            .mock("GET", "/repos/o/n")
            .with_status(200)
            .with_body(PRIVATE_BODY)
            .expect(2)
            .create();
        let sent = mock_upload_path(&mut server);
        let flip = server
            .mock("PUT", "/repos/o/n/contents/sync/pointer.json")
            .with_status(201)
            .with_body(r#"{"content":{"sha":"blob1"}}"#)
            .expect(1)
            .create();

        assert_eq!(
            push_against(&cfg_with_repo(Some("o/n")), &roots, &keyfile, &server.url()),
            0
        );
        gate.assert();
        flip.assert();
        let uploaded = sent.lock().unwrap();
        assert!(
            uploaded.iter().any(|(name, _)| name.starts_with("pack-")),
            "pack bytes reached the uploads host: {:?}",
            uploaded.iter().map(|(n, _)| n).collect::<Vec<_>>()
        );
        // The call-site guard. `ensure_keyfile` shipped once with no caller at
        // all, its own tests green because they called it directly — the third
        // time this milestone has produced a tested function nothing invokes.
        // A first push that skips it publishes a pointer naming a keyfile asset
        // that does not exist, and no second machine can bootstrap from it.
        assert!(
            uploaded
                .iter()
                .any(|(name, _)| name.starts_with("keyfile-")),
            "the wrapped master key is published, not merely addressed: {:?}",
            uploaded.iter().map(|(n, _)| n).collect::<Vec<_>>()
        );
    }

    /// SAFE-02 through the push path: a repository that reads readable on the
    /// re-gate deletes what this run uploaded and never reaches the flip.
    #[test]
    fn a_repository_that_turns_public_mid_push_deletes_and_does_not_flip() {
        let dir = TempDir::new().unwrap();
        let (roots, keyfile) = seeded(&dir);
        let mut server = mockito::Server::new();

        // Private on the first read…
        server
            .mock("GET", "/repos/o/n")
            .with_status(200)
            .with_body(PRIVATE_BODY)
            .expect(1)
            .create();
        let sent = mock_upload_path(&mut server);
        // …and public on the second.
        let public = server
            .mock("GET", "/repos/o/n")
            .with_status(200)
            .with_body(
                r#"{"id":1,"private":false,"visibility":"public",
                    "owner":{"login":"o","id":7},"archived":false,"fork":false}"#,
            )
            .expect(1)
            .create();

        // The incident path lists, then deletes only what this run uploaded.
        // The listing echoes every asset the fixture accepted — a push packs the
        // manifest and index object as well as the data, so "what this run
        // uploaded" is several assets, not one.
        let named = std::sync::Arc::clone(&sent);
        let listing = server
            .mock("GET", mockito::Matcher::Regex("per_page=100".into()))
            .with_status(200)
            .with_body_from_request(move |_| {
                let held = named.lock().unwrap();
                let rows: Vec<String> = held
                    .iter()
                    .enumerate()
                    .map(|(i, (name, bytes))| asset_json_sized(i as u64 + 1, name, bytes.len()))
                    .collect();
                format!("[{}]", rows.join(",")).into_bytes()
            })
            .expect(1)
            .create();
        // Asserted as a *set equal to what was uploaded*, not as a count: the
        // number of packs is 4-02's business, and a fixture that pins it would
        // fail on any future packing change while proving nothing extra.
        let deleted: std::sync::Arc<Mutex<Vec<usize>>> = std::sync::Arc::default();
        let recorder = std::sync::Arc::clone(&deleted);
        server
            .mock(
                "DELETE",
                mockito::Matcher::Regex(r"/releases/assets/\d+$".into()),
            )
            .with_status(204)
            .with_body_from_request(move |req| {
                recorder.lock().unwrap().push(asset_id_in(req.path()));
                Vec::new()
            })
            .create();
        let flip = server
            .mock("PUT", "/repos/o/n/contents/sync/pointer.json")
            .with_status(201)
            .expect(0)
            .create();

        assert_ne!(
            push_against(&cfg_with_repo(Some("o/n")), &roots, &keyfile, &server.url()),
            0
        );
        public.assert();
        listing.assert();
        flip.assert();

        let mut destroyed = deleted.lock().unwrap().clone();
        destroyed.sort_unstable();
        let uploaded: Vec<usize> = (1..=sent.lock().unwrap().len()).collect();
        assert!(!uploaded.is_empty(), "the run uploaded before it re-gated");
        assert_eq!(
            destroyed, uploaded,
            "every asset this run uploaded is destroyed, and nothing else is"
        );
    }

    /// A failed flip is a failed push. `with_retry` still never retries a
    /// conflict; the **one** re-drive is plan 4-04's bounded compare-and-swap in
    /// `pointer::commit` — re-read, rebuild on whoever won, `PUT` once more —
    /// and it stops there rather than looping. Two `PUT`s and no third.
    #[test]
    fn a_failed_pointer_put_exits_non_zero_after_exactly_one_bounded_retry() {
        let dir = TempDir::new().unwrap();
        let (roots, keyfile) = seeded(&dir);
        let mut server = mockito::Server::new();

        server
            .mock("GET", "/repos/o/n")
            .with_status(200)
            .with_body(PRIVATE_BODY)
            .create();
        let _sent = mock_upload_path(&mut server);
        let flip = server
            .mock("PUT", "/repos/o/n/contents/sync/pointer.json")
            .with_status(409)
            .with_body(r#"{"message":"is at abc but expected def"}"#)
            .expect(2)
            .create();

        assert_ne!(
            push_against(&cfg_with_repo(Some("o/n")), &roots, &keyfile, &server.url()),
            0
        );
        flip.assert();
    }

    /// SYNC-04, the other half of D3: a pack that does not read back as what was
    /// sent fails the push **before** the flip, so no pointer can ever reference
    /// a pack that did not verify. Killing a run anywhere above the `PUT` leaves
    /// the remote pointer byte-identical to what it was.
    #[test]
    fn a_pack_that_does_not_verify_never_reaches_the_pointer_put() {
        let dir = TempDir::new().unwrap();
        let (roots, keyfile) = seeded(&dir);
        let mut server = mockito::Server::new();

        server
            .mock("GET", "/repos/o/n")
            .with_status(200)
            .with_body(PRIVATE_BODY)
            .create();
        server
            .mock("GET", "/repos/o/n/releases/tags/ai-usagebar-sync-v1")
            .with_status(200)
            .with_body(r#"{"id":9}"#)
            .create();
        server
            .mock("GET", "/repos/o/n/contents/sync/pointer.json")
            .with_status(404)
            .with_body(r#"{"message":"Not Found"}"#)
            .create();
        // 4-03's resume scan, with nothing landed: every pack uploads.
        server
            .mock("GET", "/repos/o/n/releases/9/assets")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_body("[]")
            .create();
        server
            .mock("POST", mockito::Matcher::Regex("/releases/9/assets".into()))
            .with_status(201)
            .with_body(asset_json(1, "pack-x.bin"))
            .create();
        // Serves something other than what was uploaded.
        //
        // `expect_at_least`, not `expect(1)`: uploads run in a bounded window,
        // so when the first verification fails the ones already in flight have
        // also been read back. Pinning the count would pin 4-03's concurrency
        // cap into an unrelated test. What this test asserts is the flip.
        let verify = server
            .mock(
                "GET",
                mockito::Matcher::Regex(r"/releases/assets/\d+$".into()),
            )
            .with_status(200)
            .with_body("not the bytes that were sent")
            .expect_at_least(1)
            .create();
        let flip = server
            .mock("PUT", "/repos/o/n/contents/sync/pointer.json")
            .with_status(201)
            .expect(0)
            .create();

        assert_ne!(
            push_against(&cfg_with_repo(Some("o/n")), &roots, &keyfile, &server.url()),
            0
        );
        verify.assert();
        flip.assert();
    }

    /// D2 in the exit code: a prune failure is a warning, never a failed push.
    #[test]
    fn a_prune_failure_is_a_warning_line_and_the_push_still_exits_zero() {
        let rendered = render_push(&PushOutcome {
            packs_uploaded: 2,
            bytes_uploaded: 4096,
            snapshots_kept: 3,
            prune_warning: Some("GitHub refused the delete (403)".into()),
            ..PushOutcome::default()
        });
        assert!(rendered.contains("warning:"), "{rendered}");
        assert!(rendered.contains("403"), "{rendered}");
        assert!(rendered.contains("storage, not correctness"), "{rendered}");
        assert!(rendered.contains("published"), "{rendered}");
    }

    /// T-4-10. `PushOutcome` has no field that *could* hold either secret, and
    /// this is what keeps it that way.
    #[test]
    fn the_rendered_outcome_carries_neither_the_token_nor_the_passphrase() {
        let rendered = render_push(&PushOutcome {
            packs_uploaded: 1,
            prune_warning: Some("nothing secret here".into()),
            ..PushOutcome::default()
        });
        assert!(!rendered.contains(TOKEN), "{rendered}");
        assert!(!rendered.contains(&TOKEN[..8]), "{rendered}");
        assert!(!rendered.contains("correct horse"), "{rendered}");
    }

    /// **The dispatch exists and is reached.** `run_with` routes a bare `push`, a
    /// `prune` and a `rekey` into the real arms; an unconfigured repository
    /// refuses before any prompt and before any request, which is what makes
    /// this safe to drive through the production entry point.
    #[test]
    fn the_three_write_actions_dispatch_and_refuse_an_unconfigured_machine() {
        let dir = TempDir::new().unwrap();
        for action in [
            SyncAction::Push {
                dry_run: false,
                allow_rollback: false,
                rebuild_index: false,
                force_rehash: false,
            },
            SyncAction::Prune,
            SyncAction::Rekey,
        ] {
            assert_ne!(
                drive(&action, &cfg_with_repo(None), &dir, "http://127.0.0.1:1"),
                0,
                "{action:?} must refuse an unconfigured machine"
            );
        }
    }

    /// The bundle identifier comes from the pairing record, so an unpaired
    /// machine is told to pair rather than handed a confusing failure after a
    /// network round trip.
    #[test]
    fn an_unpaired_machine_is_told_to_run_setup_rather_than_pushing() {
        let dir = TempDir::new().unwrap();
        let err = resolve(
            &cfg_with_repo(Some("o/n")),
            &roots_at(&dir),
            &Endpoints::default(),
            &TokenChain {
                env_value: Some(zeroize::Zeroizing::new(TOKEN.into())),
                ..TokenChain::default()
            },
        )
        .err()
        .expect("nothing was paired");
        assert!(err.contains("sync setup"), "{err}");
    }

    /// REPO-04: no token is a non-zero exit that names every place one is
    /// looked for — and the category listing is still printed.
    #[test]
    fn status_without_a_token_names_every_source_and_exits_non_zero() {
        let dir = TempDir::new().unwrap();
        let cfg = cfg_with_repo(Some("o/n"));
        let roots = roots_at(&dir);
        let repo = resolve_repo_section(
            &cfg,
            &roots,
            &Endpoints {
                api_base: "http://127.0.0.1:1".into(),
                uploads_base: "http://127.0.0.1:1".into(),
            },
            // Empty: nothing can answer.
            &TokenChain::default(),
            NOW,
        );
        let failure = repo.failure.clone().expect("no token resolved");
        assert!(failure.contains("AI_USAGEBAR_SYNC_TOKEN"), "{failure}");
        assert!(failure.contains("sync-token"), "{failure}");
        assert!(failure.contains("gh auth"), "{failure}");

        let text = report::render_status(&report::build_status(
            &roots,
            &cfg.sync,
            None,
            NOW,
            None,
            Some(repo),
        ));
        assert!(text.contains("token:     none found"), "{text}");
        assert!(text.contains("credentials"), "the listing survives: {text}");
        assert_ne!(
            run_with(
                &SyncAction::Status { json: false },
                &cfg,
                &roots,
                &Endpoints {
                    api_base: "http://127.0.0.1:1".into(),
                    uploads_base: "http://127.0.0.1:1".into(),
                },
                &TokenChain::default(),
                NOW,
            ),
            0
        );
    }

    /// A network failure fills the repository section with the reason and
    /// exits non-zero, without hiding what would be sent.
    #[test]
    fn an_unreachable_github_still_prints_the_listing_and_exits_non_zero() {
        let dir = TempDir::new().unwrap();
        let cfg = cfg_with_repo(Some("o/n"));
        assert_ne!(
            drive(
                &SyncAction::Status { json: false },
                &cfg,
                &dir,
                "http://127.0.0.1:1"
            ),
            0
        );
        let repo = section_for(&cfg, &dir, "http://127.0.0.1:1");
        let failure = repo.failure.clone().expect("a dead port");
        assert!(failure.contains("Could not reach GitHub"), "{failure}");
        assert!(
            failure.contains("Nothing was uploaded"),
            "D-06 names the fix: {failure}"
        );
        assert_eq!(
            repo.token_source,
            Some("env"),
            "resolved before the request"
        );
    }

    // ---- 5-07: `sync pull`, the command --------------------------------

    const PASSWORD: &str = "correct horse battery staple";
    const REPO_ID: &str = "github:1";
    const RELEASE: u64 = 9;

    const B64: base64::engine::general_purpose::GeneralPurpose =
        base64::engine::general_purpose::STANDARD;

    /// Microseconds instead of ~1.5 s and a gibibyte. The AUR `check()` runs
    /// these on an installer's machine.
    const CHEAP: crate::sync::crypto::KdfParams = crate::sync::crypto::KdfParams {
        m_kib: 8,
        t: 1,
        p: 1,
    };

    /// One credential and one routine, so both halves of D2 are reachable: the
    /// credential is what the second gate guards, the routine is what `--force`
    /// alone promotes.
    const CRED: &str = "accounts/work/.credentials.json";
    const ROUTINE: &str = "claude-home/scheduled-tasks/daily.json";

    fn roots_in(dir: &Path) -> SyncRoots {
        SyncRoots::at(
            dir.join("config.toml"),
            dir.to_path_buf(),
            dir.join("desktop"),
            dir.join("profiles"),
            dir.join("claude-home"),
        )
    }

    fn pull_cfg() -> Config {
        Config {
            sync: crate::config::SyncConfig {
                repo: Some("o/n".into()),
                categories: vec![SyncCategory::Config, SyncCategory::Routines],
                ..Default::default()
            },
            ..Config::default()
        }
    }

    fn client_at(base: &str) -> Client {
        Client::new(
            &Endpoints {
                api_base: base.into(),
                uploads_base: base.into(),
            },
            zeroize::Zeroizing::new(TOKEN.into()),
            github::token::TokenSource::Env,
        )
        .unwrap()
    }

    /// A bundle the **push** side really produced, so this exercises the pair
    /// rather than a hand-rolled remote that could agree with a broken reader.
    struct Bundle {
        pointer: push::Pointer,
        keyfile_name: String,
        keyfile_bytes: Vec<u8>,
        packs: Vec<(String, Vec<u8>)>,
    }

    fn pushed(seed: &Path, files: &[(&str, &[u8])]) -> Bundle {
        let roots = roots_in(seed);
        for (rel, body) in files {
            let path = seed.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, body).unwrap();
        }

        let (keyfile, keys) =
            Keyfile::create_with_floor(PASSWORD.as_bytes(), CHEAP, CHEAP.m_kib).unwrap();
        let keyfile_bytes = serde_json::to_vec(&keyfile).unwrap();
        let keyfile_name = push::keyfile_asset_name(&content_address(&keyfile_bytes));

        let cfg = pull_cfg();
        let index = Index::at(&roots.index_file).unwrap();
        let plan = plan::build_with_keys(&roots, &cfg.sync, &index, NOW, &keys).unwrap();
        // Parked at a dead port: `packer::build` is pure, and a regression that
        // made it dial should fail rather than reach anything real.
        let client = client_at("http://127.0.0.1:1");
        let repo = RepoRef::parse("o/n").unwrap();
        let ctx = PushCtx {
            client: &client,
            repo: &repo,
            cfg: &cfg.sync,
            roots: &roots,
            keys: &keys,
            kdf: CHEAP,
            index: &index,
            repo_id: REPO_ID.into(),
            keyfile_asset: keyfile_name.clone(),
            previous: None,
            allow_rollback: false,
            now: NOW,
        };
        let bundle = push::packer::build(&ctx, &plan).unwrap();
        let (root, _counter) = push::packer::root_for(&ctx, None, &bundle.manifest_chunks).unwrap();

        Bundle {
            pointer: push::Pointer {
                format: push::POINTER_VERSION,
                repo_id: REPO_ID.into(),
                keyfile: keyfile_name.clone(),
                snapshots: vec![push::SnapshotRecord {
                    root: B64.encode(&root),
                    index_chunks: bundle.index_chunks.clone(),
                    packs: bundle.referenced_packs.clone(),
                }],
            },
            keyfile_name,
            keyfile_bytes,
            packs: bundle
                .packs
                .iter()
                .map(|p| (push::pack_asset_name(&p.id), p.bytes.clone()))
                .collect(),
        }
    }

    /// Serve `bundle` from `server` exactly as GitHub would.
    fn serve(server: &mut mockito::ServerGuard, bundle: &Bundle) {
        let pointer_json = serde_json::to_vec(&bundle.pointer).unwrap();
        server
            .mock("GET", "/repos/o/n/contents/sync/pointer.json")
            .with_status(200)
            .with_body(format!(
                r#"{{"sha":"deadbeef","content":"{}"}}"#,
                B64.encode(&pointer_json)
            ))
            .create();
        server
            .mock(
                "GET",
                format!("/repos/o/n/releases/tags/{}", push::RELEASE_TAG).as_str(),
            )
            .with_status(200)
            .with_body(format!(r#"{{"id":{RELEASE}}}"#))
            .create();

        let mut listing = Vec::new();
        let mut assets: Vec<(&str, &[u8])> =
            vec![(bundle.keyfile_name.as_str(), &bundle.keyfile_bytes)];
        for (name, bytes) in &bundle.packs {
            assets.push((name.as_str(), bytes));
        }
        for (i, (name, bytes)) in assets.iter().enumerate() {
            let id = 100 + i as u64;
            listing.push(format!(
                r#"{{"id":{id},"name":"{name}","size":{},"state":"uploaded",
                    "created_at":"2023-11-14T22:13:20Z"}}"#,
                bytes.len()
            ));
            server
                .mock("GET", format!("/repos/o/n/releases/assets/{id}").as_str())
                .with_status(200)
                .with_body(*bytes)
                .create();
        }
        server
            .mock(
                "GET",
                mockito::Matcher::Regex(format!("/releases/{RELEASE}/assets")),
            )
            .with_status(200)
            .with_body(format!("[{}]", listing.join(",")))
            .create();
    }

    /// Everything `resolve` needs on the restoring machine: a repository, a
    /// token and a pairing record. **No keyfile** — a second machine has none,
    /// and that is exactly the case a restore exists for.
    fn paired(dir: &Path) -> SyncRoots {
        let roots = roots_in(dir);
        fs::create_dir_all(&roots.config_dir).unwrap();
        // A machine that has run `sync status` once already has one. In
        // production it lives under `~/.cache`, outside every sync root;
        // `SyncRoots::at` only lands it inside `config_dir` because the test
        // seam derives it from there, so opening it here keeps the
        // "wrote nothing" assertions about restored *data*.
        Index::at(&roots.index_file).unwrap();
        pairing::write_to(
            &pairing::default_path(&roots),
            &pairing::Pairing {
                repo_id: 1,
                owner_id: 7,
                private: true,
                checked_at: NOW,
            },
        )
        .unwrap();
        roots
    }

    /// Drives the pull at `pull_with_parts`, one step below the arm that reads
    /// the sync password off stdin — the same carve-out `push_against` takes,
    /// and for the same reason: no test may drive a terminal.
    ///
    /// `answers` is `Some` exactly when the production run would have a
    /// terminal to ask at.
    fn pull_at(
        roots: &SyncRoots,
        base: &str,
        opts: RestoreOptions,
        answers: Option<&str>,
    ) -> (i32, String) {
        pull_as(roots, base, opts, answers, PASSWORD)
    }

    fn pull_as(
        roots: &SyncRoots,
        base: &str,
        opts: RestoreOptions,
        answers: Option<&str>,
        password: &str,
    ) -> (i32, String) {
        let cfg = pull_cfg();
        let parts = resolve(
            &cfg,
            roots,
            &Endpoints {
                api_base: base.into(),
                uploads_base: base.into(),
            },
            &TokenChain {
                env_value: Some(zeroize::Zeroizing::new(TOKEN.into())),
                ..TokenChain::default()
            },
        )
        .expect("the fixture is configured and paired");
        let pw = zeroize::Zeroizing::new(password.to_owned());
        let mut out: Vec<u8> = Vec::new();
        let code = match answers {
            Some(typed) => {
                let mut reader = std::io::Cursor::new(typed.as_bytes().to_vec());
                pull_with_parts(
                    roots,
                    &parts,
                    &pw,
                    opts,
                    &mut PullIo {
                        out: &mut out,
                        gate: Some(&mut reader),
                    },
                    NOW,
                )
            }
            None => pull_with_parts(
                roots,
                &parts,
                &pw,
                opts,
                &mut PullIo {
                    out: &mut out,
                    gate: None,
                },
                NOW,
            ),
        };
        (code, String::from_utf8_lossy(&out).into_owned())
    }

    /// Every regular file anywhere under `root`, sorted.
    fn files_under(root: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    out.push(path);
                }
            }
        }
        out.sort();
        out
    }

    /// What a machine that already has *newer* copies of both items looks like.
    /// The mtime is stamped past the snapshot's, deterministically — never from
    /// the wall clock.
    fn seed_newer(roots: &SyncRoots, items: &[(&str, &[u8])]) {
        let newer = std::time::SystemTime::UNIX_EPOCH
            + std::time::Duration::from_secs(u64::try_from(NOW.timestamp()).unwrap() + 3_600);
        for (rel, body) in items {
            let path = roots.config_dir.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, body).unwrap();
            fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(newer)
                .unwrap();
        }
    }

    /// **The defect this whole plan exists to close.** Every dry run prints
    /// `report::APPLY_COMMAND`; if the subcommand is spelled anything else, the
    /// report names a command that does not exist.
    #[test]
    fn the_command_every_dry_run_prints_is_one_the_binary_actually_accepts() {
        use clap::Parser;
        let argv: Vec<&str> = restore::report::APPLY_COMMAND.split_whitespace().collect();
        let cli = crate::widget::cli::Cli::try_parse_from(&argv)
            .unwrap_or_else(|e| panic!("`{}` must parse: {e}", restore::report::APPLY_COMMAND));
        assert!(
            matches!(
                cli.command,
                Some(crate::widget::cli::Command::Sync {
                    action: SyncAction::Pull { apply: true, .. }
                })
            ),
            "the printed command must be the applying pull, not something else"
        );
    }

    /// The one read both arms share, driven over an injected reader so neither
    /// a terminal nor the process's real stdin is involved.
    ///
    /// The empty case is the one that matters: without it an unattended run
    /// with stdin on `/dev/null` spends a gibibyte and a second and a half
    /// hashing the empty string before being told it was wrong.
    #[test]
    fn the_sync_password_is_one_line_off_the_reader_and_an_empty_stream_is_refused() {
        assert_eq!(
            *sync_password_from(&b"correct horse battery\n"[..]).unwrap(),
            "correct horse battery"
        );
        // Only the first line. The second belongs to whichever gate reads next.
        assert_eq!(
            *sync_password_from(&b"first\nsecond\n"[..]).unwrap(),
            "first"
        );

        for empty in [&b""[..], &b"\n"[..]] {
            let err = sync_password_from(empty).expect_err("an empty stream is not a password");
            assert_eq!(err, NO_PASSWORD);
        }

        // No refusal on this path echoes what it read.
        let secret = "a-password-that-must-not-travel";
        let err = sync_password_from(format!("{secret}\n").as_bytes())
            .map(|_| String::new())
            .unwrap_or_else(|e| e);
        assert!(!err.contains(secret));
    }

    /// **Every refusal that does not need a password comes first.** A machine
    /// with no keyfile is told so rather than asked for a password it would
    /// then discard — which is also why this test cannot hang: neither arm
    /// reaches a read.
    #[test]
    fn a_missing_or_unreadable_keyfile_is_refused_before_any_password_is_wanted() {
        let dir = TempDir::new().unwrap();
        let absent = dir.path().join("keyfile.json");
        for may_prompt in [true, false] {
            // `.err()` rather than `expect_err`: `LocalKeyfile` holds live keys and
            // must never gain a `Debug` a panic message could print.
            let err = local_keyfile(&absent, may_prompt)
                .err()
                .expect("no keyfile, no open");
            assert!(err.contains("has no sync keyfile yet"), "{err}");
        }

        let junk = dir.path().join("junk.json");
        std::fs::write(&junk, b"not json at all").unwrap();
        for may_prompt in [true, false] {
            let err = local_keyfile(&junk, may_prompt)
                .err()
                .expect("junk is not a keyfile");
            assert!(err.contains("not a readable sync keyfile"), "{err}");
        }
    }

    /// `sync status` and `sync push --dry-run` want the third column and do not
    /// need it, so they must never block a terminal on a question. The refusal
    /// they keep is deliberately *not* the one a push gets.
    #[test]
    fn the_read_only_report_says_it_does_not_ask_rather_than_asking() {
        assert!(NO_PROMPT_HERE.contains("does not ask"));
        assert!(!NO_PROMPT_HERE.contains("no interactive prompt"));
        // The push/prune/rekey arm announces the echo instead of refusing.
        assert!(ECHOED_PROMPT.contains("echoed"));
    }

    /// `--apply` and `--dry-run` together are an error, never a guess.
    #[test]
    fn apply_and_dry_run_together_are_refused_by_clap() {
        use clap::Parser;
        assert!(
            crate::widget::cli::Cli::try_parse_from([
                "ai-usagebar",
                "sync",
                "pull",
                "--apply",
                "--dry-run",
            ])
            .is_err()
        );
        // SAFE-03: the second consent is an *addition* to `--force`, and clap
        // says so before a byte is read.
        assert!(
            crate::widget::cli::Cli::try_parse_from([
                "ai-usagebar",
                "sync",
                "pull",
                "--force-credentials",
            ])
            .is_err()
        );
    }

    /// D1: no flags, no terminal — plan, report, exit 0, and **nothing on
    /// disk**. A dry run is a success: it did exactly what it was asked.
    #[test]
    fn a_pull_with_no_flags_and_no_terminal_writes_nothing_and_names_the_flag() {
        let push_dir = TempDir::new().unwrap();
        let bundle = pushed(push_dir.path(), &[(CRED, b"{\"token\":\"fixture\"}")]);
        let mut server = mockito::Server::new();
        serve(&mut server, &bundle);

        let dir = TempDir::new().unwrap();
        let roots = paired(dir.path());
        let before = files_under(dir.path());

        let (code, out) = pull_at(&roots, &server.url(), RestoreOptions::default(), None);
        assert_eq!(code, 0, "a dry run is a completed run");
        assert!(out.contains(restore::report::APPLY_COMMAND), "{out}");
        assert_eq!(files_under(dir.path()), before, "a dry run wrote something");
        assert!(
            !push::anchor_path(&roots, &RepoRef::parse("o/n").unwrap()).exists(),
            "a dry run advanced the rollback anchor"
        );
    }

    /// The other way to write: a terminal, and an affirmative.
    #[test]
    fn an_affirmative_at_the_terminal_applies_and_a_refusal_writes_nothing() {
        let push_dir = TempDir::new().unwrap();
        let bundle = pushed(push_dir.path(), &[(CRED, b"{\"token\":\"fixture\"}")]);
        let mut server = mockito::Server::new();
        serve(&mut server, &bundle);

        let no = TempDir::new().unwrap();
        let refused = paired(no.path());
        let before = files_under(no.path());
        let (code, _) = pull_at(
            &refused,
            &server.url(),
            RestoreOptions::default(),
            Some("n\n"),
        );
        assert_eq!(code, 0, "a declined gate is a choice, not a failure");
        assert_eq!(files_under(no.path()), before);

        let yes = TempDir::new().unwrap();
        let applied = paired(yes.path());
        let (code, out) = pull_at(
            &applied,
            &server.url(),
            RestoreOptions::default(),
            Some("y\n"),
        );
        assert_eq!(code, 0, "{out}");
        assert!(applied.config_dir.join(CRED).is_file(), "{out}");
    }

    /// `--apply` on a machine with no terminal at all: no gate, and the file
    /// lands. This is the unattended path.
    #[test]
    fn apply_without_a_terminal_writes_the_file_and_never_asks() {
        let push_dir = TempDir::new().unwrap();
        let bundle = pushed(push_dir.path(), &[(CRED, b"{\"token\":\"fixture\"}")]);
        let mut server = mockito::Server::new();
        serve(&mut server, &bundle);

        let dir = TempDir::new().unwrap();
        let roots = paired(dir.path());
        let (code, out) = pull_at(
            &roots,
            &server.url(),
            RestoreOptions {
                apply: true,
                ..RestoreOptions::default()
            },
            None,
        );
        assert_eq!(code, 0, "{out}");
        assert_eq!(
            fs::read(roots.config_dir.join(CRED)).unwrap(),
            b"{\"token\":\"fixture\"}"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(roots.config_dir.join(CRED))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "{mode:o}");
        }
    }

    /// SAFE-03 / D2, both halves in one fixture: `--force` promotes the
    /// locally-newer **routine** and names it, and stops dead at the
    /// locally-newer **credential** — with no archive taken and nothing
    /// written, because the credential gate runs before `backup::take`
    /// (T-5-60, T-5-61).
    #[test]
    fn force_alone_stops_at_a_locally_newer_credential_before_the_backup_exists() {
        let push_dir = TempDir::new().unwrap();
        let bundle = pushed(
            push_dir.path(),
            &[
                (CRED, b"{\"token\":\"from-the-snapshot\"}"),
                (ROUTINE, b"{\"routine\":\"from-the-snapshot\"}"),
            ],
        );
        let mut server = mockito::Server::new();
        serve(&mut server, &bundle);

        let dir = TempDir::new().unwrap();
        let roots = paired(dir.path());
        seed_newer(
            &roots,
            &[
                (CRED, b"{\"token\":\"live\"}"),
                (ROUTINE, b"{\"r\":\"mine\"}"),
            ],
        );

        let forced = RestoreOptions {
            apply: true,
            force: true,
            ..RestoreOptions::default()
        };
        let (code, out) = pull_at(&roots, &server.url(), forced, None);
        assert_ne!(code, 0, "a stopped restore is not a success: {out}");
        assert!(out.contains(CRED) || out.contains("CREDENTIALS"), "{out}");
        assert_eq!(
            fs::read(roots.config_dir.join(CRED)).unwrap(),
            b"{\"token\":\"live\"}",
            "the live token survived"
        );
        assert_eq!(
            fs::read(roots.config_dir.join(ROUTINE)).unwrap(),
            b"{\"r\":\"mine\"}",
            "the run stopped before any write, not part way through"
        );
        assert!(
            files_under(&backups_dir(&roots)).is_empty(),
            "the gate runs before backup::take, so a decline leaves no archive"
        );

        // …and with the second consent, both are replaced and both are named.
        let (code, out) = pull_at(
            &roots,
            &server.url(),
            RestoreOptions {
                force_credentials: true,
                ..forced
            },
            None,
        );
        assert_eq!(code, 0, "{out}");
        assert_eq!(
            fs::read(roots.config_dir.join(CRED)).unwrap(),
            b"{\"token\":\"from-the-snapshot\"}"
        );
        assert!(out.contains(CRED), "SYNC-06 names what it replaced: {out}");
        assert!(out.contains(ROUTINE), "{out}");
        assert!(
            !files_under(&backups_dir(&roots)).is_empty(),
            "D3: the archive is taken before the first byte"
        );
        assert!(out.contains("tar -xzf"), "the undo is printed: {out}");
    }

    /// The credential gate is answerable at a terminal, and only by the word.
    #[test]
    fn the_credential_gate_takes_the_typed_word_and_nothing_else() {
        let push_dir = TempDir::new().unwrap();
        let bundle = pushed(push_dir.path(), &[(CRED, b"{\"token\":\"snapshot\"}")]);
        let mut server = mockito::Server::new();
        serve(&mut server, &bundle);

        let forced = RestoreOptions {
            apply: true,
            force: true,
            // --yes deliberately does not answer this question (T-5-52).
            assume_yes: true,
            ..RestoreOptions::default()
        };

        let no = TempDir::new().unwrap();
        let refused = paired(no.path());
        seed_newer(&refused, &[(CRED, b"{\"token\":\"live\"}")]);
        let (code, _) = pull_at(&refused, &server.url(), forced, Some("yes\n"));
        assert_ne!(code, 0, "`yes` is not the word");
        assert_eq!(
            fs::read(refused.config_dir.join(CRED)).unwrap(),
            b"{\"token\":\"live\"}"
        );

        let ok = TempDir::new().unwrap();
        let accepted = paired(ok.path());
        seed_newer(&accepted, &[(CRED, b"{\"token\":\"live\"}")]);
        let (code, out) = pull_at(&accepted, &server.url(), forced, Some("overwrite\n"));
        assert_eq!(code, 0, "{out}");
        assert_eq!(
            fs::read(accepted.config_dir.join(CRED)).unwrap(),
            b"{\"token\":\"snapshot\"}"
        );
    }

    /// T-5-62: `--allow-rollback` opens an older snapshot of the **same**
    /// bundle and never one borrowed from a different bundle by renaming. The
    /// CLI adds no bypass of its own.
    #[test]
    fn allow_rollback_never_rescues_a_bundle_identity_mismatch() {
        let push_dir = TempDir::new().unwrap();
        let bundle = pushed(push_dir.path(), &[(CRED, b"{\"token\":\"fixture\"}")]);
        let mut server = mockito::Server::new();
        serve(&mut server, &bundle);

        let dir = TempDir::new().unwrap();
        let roots = paired(dir.path());

        // This machine has already seen counter 5 of *another* bundle.
        crate::sync::anchor::write_to(
            &push::anchor_path(&roots, &RepoRef::parse("o/n").unwrap()),
            &crate::sync::anchor::Anchor {
                repo_id: "github:999".into(),
                counter: 5,
            },
        )
        .unwrap();

        for opts in [
            RestoreOptions::default(),
            RestoreOptions {
                allow_rollback: true,
                ..RestoreOptions::default()
            },
        ] {
            let (code, _) = pull_at(&roots, &server.url(), opts, None);
            assert_ne!(code, 0, "a repo_id mismatch is refused under {opts:?}");
        }
    }

    /// A lower counter of the *same* bundle is the case the flag is for: named
    /// without it, accepted with it.
    #[test]
    fn a_lower_counter_names_the_flag_and_then_is_accepted_under_it() {
        let push_dir = TempDir::new().unwrap();
        let bundle = pushed(push_dir.path(), &[(CRED, b"{\"token\":\"fixture\"}")]);
        let mut server = mockito::Server::new();
        serve(&mut server, &bundle);

        let dir = TempDir::new().unwrap();
        let roots = paired(dir.path());
        crate::sync::anchor::write_to(
            &push::anchor_path(&roots, &RepoRef::parse("o/n").unwrap()),
            &crate::sync::anchor::Anchor {
                repo_id: REPO_ID.into(),
                counter: 5,
            },
        )
        .unwrap();

        let (code, _) = pull_at(&roots, &server.url(), RestoreOptions::default(), None);
        assert_ne!(code, 0, "an older snapshot is refused by default");

        let (code, _) = pull_at(
            &roots,
            &server.url(),
            RestoreOptions {
                allow_rollback: true,
                ..RestoreOptions::default()
            },
            None,
        );
        assert_eq!(code, 0, "…and accepted when the user says so");
    }

    /// T-5-67, one failure per test so each message is attributable: every one
    /// is non-zero, distinct, and puts no byte on disk.
    #[test]
    fn a_wrong_password_is_refused_and_writes_nothing() {
        let push_dir = TempDir::new().unwrap();
        let bundle = pushed(push_dir.path(), &[(CRED, b"{\"token\":\"fixture\"}")]);
        let mut server = mockito::Server::new();
        serve(&mut server, &bundle);

        let dir = TempDir::new().unwrap();
        let roots = paired(dir.path());
        let before = files_under(dir.path());
        let (code, _) = pull_as(
            &roots,
            &server.url(),
            RestoreOptions {
                apply: true,
                ..RestoreOptions::default()
            },
            None,
            "not the password",
        );
        assert_ne!(code, 0);
        assert_eq!(files_under(dir.path()), before);
    }

    /// Nothing has ever been pushed: a refusal, not a crash and not a success.
    #[test]
    fn a_bundle_with_no_pointer_is_refused_and_writes_nothing() {
        let mut empty = mockito::Server::new();
        empty
            .mock("GET", "/repos/o/n/contents/sync/pointer.json")
            .with_status(404)
            .with_body(r#"{"message":"Not Found"}"#)
            .create();

        let dir = TempDir::new().unwrap();
        let roots = paired(dir.path());
        let before = files_under(dir.path());
        let (code, _) = pull_at(&roots, &empty.url(), RestoreOptions::default(), None);
        assert_ne!(code, 0);
        assert_eq!(files_under(dir.path()), before);
    }

    /// A remote that refuses the read.
    ///
    /// **Not a dead port**, deliberately: a transport failure is retryable, so
    /// `pointer::load` spends the production 60/120/240-second backoff before
    /// giving up — seven minutes inside the AUR `check()`. That policy is
    /// `github::write::with_retry`'s to test, and it has its own. A 403 is
    /// returned immediately, which is what this arm is about.
    #[test]
    fn a_remote_that_refuses_the_read_is_refused_back_and_writes_nothing() {
        let mut server = mockito::Server::new();
        server
            .mock("GET", "/repos/o/n/contents/sync/pointer.json")
            .with_status(403)
            .with_body(r#"{"message":"Forbidden"}"#)
            .create();

        let dir = TempDir::new().unwrap();
        let roots = paired(dir.path());
        let before = files_under(dir.path());
        let (code, _) = pull_at(&roots, &server.url(), RestoreOptions::default(), None);
        assert_ne!(code, 0);
        assert_eq!(files_under(dir.path()), before);
    }

    /// One flipped byte in a bundle the push side really produced.
    #[test]
    fn a_tampered_pack_is_refused_and_writes_nothing() {
        let push_dir = TempDir::new().unwrap();
        let mut broken = pushed(push_dir.path(), &[(CRED, b"{\"token\":\"fixture\"}")]);
        broken.packs[0].1[64] ^= 0x01;
        let mut server = mockito::Server::new();
        serve(&mut server, &broken);

        let dir = TempDir::new().unwrap();
        let roots = paired(dir.path());
        let before = files_under(dir.path());
        let (code, _) = pull_at(
            &roots,
            &server.url(),
            RestoreOptions {
                apply: true,
                ..RestoreOptions::default()
            },
            None,
        );
        assert_ne!(code, 0);
        assert_eq!(files_under(dir.path()), before);
    }

    /// T-5-63: the local index is a cache and must never be able to decide what
    /// a restore writes. Deleting it changes the outcome not at all.
    #[test]
    fn a_deleted_index_produces_an_identical_restore() {
        let push_dir = TempDir::new().unwrap();
        let bundle = pushed(
            push_dir.path(),
            &[
                (CRED, b"{\"token\":\"fixture\"}"),
                (ROUTINE, b"{\"routine\":\"one\"}"),
            ],
        );
        let mut server = mockito::Server::new();
        serve(&mut server, &bundle);

        let warm_dir = TempDir::new().unwrap();
        let warm = paired(warm_dir.path());
        Index::at(&warm.index_file).unwrap();
        let (warm_code, _) = pull_at(
            &warm,
            &server.url(),
            RestoreOptions {
                apply: true,
                ..RestoreOptions::default()
            },
            None,
        );

        let cold_dir = TempDir::new().unwrap();
        let cold = paired(cold_dir.path());
        index::reset_at(&cold.index_file).unwrap();
        fs::remove_file(&cold.index_file).unwrap();
        let (cold_code, _) = pull_at(
            &cold,
            &server.url(),
            RestoreOptions {
                apply: true,
                rebuild_index: true,
                ..RestoreOptions::default()
            },
            None,
        );

        assert_eq!(warm_code, 0);
        assert_eq!(cold_code, warm_code);
        for rel in [CRED, ROUTINE] {
            assert_eq!(
                fs::read(warm.config_dir.join(rel)).unwrap(),
                fs::read(cold.config_dir.join(rel)).unwrap(),
                "{rel}"
            );
        }
    }

    /// D7: applying the same snapshot twice changes nothing the second time,
    /// and the second run is not reported as a conflict.
    #[test]
    fn a_second_apply_writes_nothing_and_reports_no_conflict() {
        let push_dir = TempDir::new().unwrap();
        let bundle = pushed(push_dir.path(), &[(CRED, b"{\"token\":\"fixture\"}")]);
        let mut server = mockito::Server::new();
        serve(&mut server, &bundle);

        let dir = TempDir::new().unwrap();
        let roots = paired(dir.path());
        let applying = RestoreOptions {
            apply: true,
            ..RestoreOptions::default()
        };
        assert_eq!(pull_at(&roots, &server.url(), applying, None).0, 0);
        let stamp = fs::metadata(roots.config_dir.join(CRED))
            .unwrap()
            .modified()
            .unwrap();

        let (code, out) = pull_at(&roots, &server.url(), applying, None);
        assert_eq!(code, 0, "{out}");
        assert_eq!(
            fs::metadata(roots.config_dir.join(CRED))
                .unwrap()
                .modified()
                .unwrap(),
            stamp,
            "an already-identical file must not be rewritten"
        );
    }

    /// The backups directory is derived from the injected roots, never from
    /// `$HOME` — which is what keeps every test's archive inside its own
    /// `TempDir` — and it is the account switcher's own directory, so there is
    /// one place a user looks for undo rather than two (D3).
    #[test]
    fn the_restore_archive_lands_beside_the_account_switchers_own() {
        let dir = TempDir::new().unwrap();
        let roots = roots_in(dir.path());
        let archives = backups_dir(&roots);
        assert!(archives.starts_with(dir.path()), "{}", archives.display());
        assert!(archives.ends_with("backups"), "{}", archives.display());
        assert_eq!(
            archives.parent(),
            roots.desktop_profiles_dir.parent(),
            "the same directory `claude_desktop::Paths` puts its rollbacks in"
        );
    }

    /// **The piped case, decided deliberately.** A pull reads the sync password
    /// off stdin and both gates read from stdin too; over one pipe whichever
    /// reads second eats the other's line. The password wins the stream, so a
    /// piped run is never asked anything — it prints the plan, names `--apply`,
    /// and stops. Nothing here consumes a second line that was never there.
    #[test]
    fn a_piped_run_is_never_asked_anything_so_the_password_keeps_the_pipe() {
        let push_dir = TempDir::new().unwrap();
        let bundle = pushed(push_dir.path(), &[(CRED, b"{\"token\":\"fixture\"}")]);
        let mut server = mockito::Server::new();
        serve(&mut server, &bundle);

        let dir = TempDir::new().unwrap();
        let roots = paired(dir.path());
        let (code, out) = pull_at(&roots, &server.url(), RestoreOptions::default(), None);

        assert_eq!(code, 0);
        assert!(!out.contains("[y/N]"), "a piped run must not ask: {out}");
        assert!(
            !out.contains("Type the word"),
            "nor reach the credential gate's reader: {out}"
        );
        assert!(out.contains(restore::report::APPLY_COMMAND), "{out}");
        // …and the same run on a terminal is asked, so the assertion above is
        // about the pipe rather than about the plan being empty.
        let (_, asked) = pull_at(
            &roots,
            &server.url(),
            RestoreOptions::default(),
            Some("n\n"),
        );
        assert!(asked.contains("[y/N]"), "{asked}");
    }

    /// The plan is rendered **once**. `confirm_apply` writes the whole report
    /// before it asks, so a caller that also printed it would show it twice.
    #[test]
    fn the_plan_is_printed_exactly_once_on_the_path_that_asks() {
        let push_dir = TempDir::new().unwrap();
        let bundle = pushed(push_dir.path(), &[(CRED, b"{\"token\":\"fixture\"}")]);
        let mut server = mockito::Server::new();
        serve(&mut server, &bundle);

        let dir = TempDir::new().unwrap();
        let roots = paired(dir.path());
        let (_, out) = pull_at(
            &roots,
            &server.url(),
            RestoreOptions::default(),
            Some("n\n"),
        );
        assert_eq!(out.matches("DRY RUN").count(), 1, "{out}");
    }

    /// The two push-side recovery flags reach the arm and do what they say:
    /// the index is discarded and the run re-reads every file. A flag nothing
    /// reads is this milestone's most repeated defect, so this is the call-site
    /// guard for both of them.
    #[test]
    fn the_index_recovery_flags_reach_the_push_arm_and_empty_the_index() {
        let dir = TempDir::new().unwrap();
        let (roots, keyfile) = seeded(&dir);
        let cfg = cfg_with_repo(Some("o/n"));

        // Warm it: a row and a last-sync line, both of which a reset drops.
        let index = Index::at(&roots.index_file).unwrap();
        plan::build_with_keys(&roots, &cfg.sync, &index, NOW, &keyfile.keys).unwrap();
        index.set_last_sync(NOW).unwrap();
        assert!(Index::at(&roots.index_file).unwrap().last_sync().is_some());
        drop(index);

        assert_eq!(
            dry_run(&cfg, &roots, NOW, recovery(true, true)),
            0,
            "`sync push --dry-run --rebuild-index --force-rehash`"
        );
        let after = Index::at(&roots.index_file).unwrap();
        assert!(after.last_sync().is_none(), "--rebuild-index discarded it");
        assert_eq!(
            plan::build_with_keys(&roots, &cfg.sync, &after.rehashing(), NOW, &keyfile.keys)
                .unwrap()
                .files_opened,
            plan::build_with_keys(
                &roots,
                &cfg.sync,
                &Index::at(&dir.path().join("cold.sqlite3")).unwrap(),
                NOW,
                &keyfile.keys,
            )
            .unwrap()
            .files_opened,
            "--force-rehash reads what a cold index reads"
        );
    }

    /// And the clap surface exists for both, on the command where they change
    /// what the planner does.
    #[test]
    fn push_accepts_both_recovery_flags_together() {
        use clap::Parser;
        let cli = crate::widget::cli::Cli::parse_from([
            "ai-usagebar",
            "sync",
            "push",
            "--rebuild-index",
            "--force-rehash",
        ]);
        assert!(matches!(
            cli.command,
            Some(crate::widget::cli::Command::Sync {
                action: SyncAction::Push {
                    rebuild_index: true,
                    force_rehash: true,
                    ..
                }
            })
        ));
    }

    // ---- 6-01: `sync status --json`, the menu bar's read ------------------

    /// One JSON object on stdout and nothing else, terminated by one newline,
    /// exit zero. This is the whole contract the macOS menu bar parses.
    #[test]
    fn status_json_prints_one_line_of_json_and_exits_zero() {
        let dir = TempDir::new().unwrap();
        let cfg = cfg_with_repo(None);
        let (code, out) = status_with(
            &roots_at(&dir),
            &cfg.sync,
            None,
            NOW,
            None,
            None,
            true,
            Style::color(true),
        );

        assert_eq!(code, 0);
        assert_eq!(out.lines().count(), 1, "{out:?}");
        assert!(out.ends_with('\n'), "{out:?}");
        let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
        assert!(v.is_object(), "{v}");
        assert_eq!(
            v["warnings"],
            serde_json::json!([report::WARN_INDEX_UNAVAILABLE]),
            "an index that would not open reaches the JSON consumer too"
        );
    }

    /// The text rendering is the renderer it always was, byte for byte: one
    /// built report, two renderings, so they cannot drift.
    #[test]
    fn the_text_rendering_is_byte_identical_to_what_it_always_printed() {
        let dir = TempDir::new().unwrap();
        let cfg = cfg_with_repo(None);
        let roots = roots_at(&dir);

        let (code, out) = status_with(
            &roots,
            &cfg.sync,
            None,
            NOW,
            None,
            None,
            false,
            Style::PLAIN,
        );
        assert_eq!(code, 0);
        assert_eq!(
            out,
            report::render_status(&report::build_status(
                &roots, &cfg.sync, None, NOW, None, None
            ))
        );
    }

    /// D-02 and T-6-04: `--json` answers from the stat sweep alone. It builds
    /// no plan — which would want the sync password on a stdin a menu-bar
    /// subprocess has no way to answer — and it makes no request, so a
    /// configured repository behind a dead port still exits zero. The human
    /// rendering of the same command still reports the repository, and still
    /// fails when it cannot be reached.
    #[test]
    fn status_json_wants_no_password_and_makes_no_request() {
        let dir = TempDir::new().unwrap();
        let cfg = cfg_with_repo(Some("o/n"));

        assert_eq!(
            drive(
                &SyncAction::Status { json: true },
                &cfg,
                &dir,
                "http://127.0.0.1:1"
            ),
            0
        );
        assert_ne!(
            drive(
                &SyncAction::Status { json: false },
                &cfg,
                &dir,
                "http://127.0.0.1:1"
            ),
            0
        );
    }
}
