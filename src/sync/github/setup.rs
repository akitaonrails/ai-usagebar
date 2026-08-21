//! `ai-usagebar sync setup` — pair this machine with the private repository
//! named in `[sync] repo`, in the five steps UX-03 asks for.
//!
//! **Uploads no bundle data** (D-05). The flow is: the categories, then the
//! repository and the gate, then the passphrase, then the size, then everything
//! that persists.
//!
//! It can write exactly one thing, and only when asked: a `README.md` into a
//! repository that has **no commits at all**, which is what
//! `gh repo create --private` leaves behind and which GitHub will not let a
//! release be tagged against. That offer sits inside step 2, *after* the gate,
//! because it is the first thing that would put a byte in the repository and
//! the private-repo refusal has to have run against it first. A decline leaves
//! the repository untouched and prints the `gh api …` line instead. Nothing
//! here creates a repository: the token holds no `Administration: write`, so
//! REPO-03's "cannot bring a public repository into existence" is structural
//! and a `Contents` write does not touch it.
//!
//! **The ordering is the substance**, and it changed for a security reason.
//! The categories come *first* because the credentials category is an input to
//! the gate — the one deciding D-04's public-repo carve-out — and a gate
//! answered before the question was settled was answered a question it was no
//! longer being asked (F-2). Everything else still refuses as early as it can:
//! the local preconditions and the whole gate run before a passphrase is
//! generated, so nobody is asked to choose one for a repository that is about to
//! be refused.
//!
//! **Nothing persists until the confirmation passes** (F-10). The keyfile, the
//! `config.toml` write-back, the token and the pairing record are all written in
//! step 5, after the last thing that can abort. A keyfile written earlier
//! survived the decline and then refused the re-run, stranding the user behind a
//! passphrase they were shown once.
//!
//! Every test drives a scripted prompt double that records which methods were
//! reached, so that ordering is asserted rather than reviewed.
//!
//! `roots` is the **only** way this module reaches a filesystem path: the
//! keyfile, the pairing record, the token file, the index, and the
//! `config.toml` write-back all resolve from it, so no path here is ever
//! derived from a real `$HOME` — which the AUR `check()` would run against on
//! an installer's machine.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use toml_edit::DocumentMut;
use zeroize::Zeroizing;

use crate::config::{SyncCategory, SyncConfig};
use crate::error::{AppError, Result};
use crate::sync::crypto::{KdfParams, Keyfile, Keys};
use crate::sync::index::Index;
use crate::sync::passphrase::{self, Strength};
use crate::sync::push::{self, Pointer, pointer};
use crate::sync::report::{self, DryRunReport, Style};
use crate::sync::restore::fetch;
use crate::sync::{SyncRoots, plan};

use super::gate;
use super::pairing;
use super::token::{self, TokenSource};
use super::write;
use super::{Client, Endpoints, RepoRef};

/// Everything the guided flow needs from outside itself.
///
/// One trait rather than direct `stdin`/`stdout` calls, so the *order* the
/// steps run in is observable: every test passes a double that records which
/// methods were reached, and a refusal that reaches [`SetupPrompt::passphrase`]
/// fails the test rather than the review.
///
/// **Nothing outside [`TtyPrompt`] may read stdin.**
pub trait SetupPrompt {
    /// Narrate. Everything the user reads goes through here.
    fn say(&mut self, line: &str);

    /// A yes/no with a default — step 4's size confirmation, and anything else
    /// that needs one. It is the last thing that can abort: nothing persists
    /// until it has passed.
    fn confirm(&mut self, question: &str, default_yes: bool) -> Result<bool>;

    /// Step 3. `generated` has already been displayed with Phase 1's
    /// no-recovery warning; the implementation either accepts it or supplies
    /// its own. Called again when the strength floor refuses a supplied one.
    fn passphrase(&mut self, generated: &str) -> Result<Zeroizing<String>>;

    /// Step 3 on a machine **joining** a bundle this repository already holds.
    ///
    /// A method of its own rather than [`SetupPrompt::passphrase`] with an
    /// empty `generated`, because there is nothing to offer: the password that
    /// opens the published keyfile was chosen on another machine and this build
    /// cannot mint an alternative to it. Reused, [`TtyPrompt`] would print
    /// *"Press Enter to take the generated passphrase"* over `""` — an
    /// instruction to submit no password at all.
    ///
    /// Called again while the keyfile refuses to open, up to
    /// [`JOIN_ATTEMPTS`] times, the way the generate path re-prompts on a
    /// passphrase under the strength floor.
    fn existing_passphrase(&mut self) -> Result<Zeroizing<String>>;

    /// Step 1, and the gate's own input: whether `credentials` is in the answer
    /// is what decides D-04's public-repo carve-out, so this is asked before the
    /// gate rather than after it (F-2). Returns the categories to keep, in any
    /// order.
    fn categories(&mut self, current: &[SyncCategory]) -> Result<Vec<SyncCategory>>;

    /// The palette this flow narrates in.
    ///
    /// A seam exactly like [`kdf`](SetupPrompt::kdf) and for the same reason:
    /// the decision needs a terminal and the process environment, neither of
    /// which anything under `src/sync/` may reach for. `TtyPrompt` carries what
    /// `sync::cli` resolved; every test double takes the default and therefore
    /// asserts against the unstyled wording it always did.
    fn style(&self) -> Style {
        Style::PLAIN
    }

    /// The KDF cost a new keyfile is written at.
    ///
    /// A seam, not a question: the shipped default is 1 GiB and takes about a
    /// second and a half, which every test that reaches step 3 would otherwise
    /// pay — and the AUR `check()` runs those tests during `makepkg`. Tests
    /// override it with [`crate::sync::crypto::MIN_KDF_MEMORY_KIB`].
    fn kdf(&self) -> KdfParams {
        KdfParams::default()
    }

    /// Step 5's persistence. Also a seam, and a mandatory one: on macOS
    /// [`token::store`] writes the **real** login Keychain item, so a test that
    /// reached step 5 through the production call would clobber the user's own
    /// sync token — and the AUR `check()` runs these tests on installers'
    /// machines. The default *is* production; the double overrides it.
    fn store_token(&self, token: &str, file: &Path) -> Result<TokenSource> {
        token::store(token, file)
    }

    /// The 401 path, for the same reason: [`token::clear_source`] deletes the
    /// real Keychain item on macOS.
    ///
    /// `source` is not decoration — it is what decides *which* store is touched,
    /// and passing the wrong one destroys a credential the run never used.
    fn clear_token(&self, source: TokenSource, file: &Path) -> Result<()> {
        token::clear_source(source, file)
    }
}

/// The production implementation. Untested by construction — it is the only
/// thing in this file that reads stdin.
#[derive(Debug, Default)]
pub struct TtyPrompt {
    style: Style,
    /// Whether stdin is a terminal, injected by `sync::cli`.
    ///
    /// Separate from `style` on purpose: `NO_COLOR` at a real keyboard makes
    /// `style` plain and must not take the prompt marker with it, and a piped
    /// `sync setup` must emit byte-identical output to what it always has.
    interactive: bool,
}

impl TtyPrompt {
    /// With the palette `sync::cli` resolved for standard output — which is
    /// where every line below is printed — and stdin's own terminal fact.
    pub fn new(style: Style, interactive: bool) -> TtyPrompt {
        TtyPrompt { style, interactive }
    }

    /// The stream [`passphrase::read_line`] draws its marker on: **stdout**,
    /// which is where `say` and every prompt in this file already print. `None`
    /// when stdin is not a terminal — there is nobody to prompt.
    fn marker<'a>(&self, out: &'a mut std::io::Stdout) -> Option<&'a mut dyn std::io::Write> {
        if self.interactive { Some(out) } else { None }
    }

    fn line(&self) -> Result<String> {
        let mut buf = String::new();
        std::io::stdin()
            .read_line(&mut buf)
            .map_err(|e| AppError::Other(format!("could not read your answer: {e}")))?;
        Ok(buf.trim().to_owned())
    }
}

impl SetupPrompt for TtyPrompt {
    fn style(&self) -> Style {
        self.style
    }

    fn say(&mut self, line: &str) {
        println!("{line}");
    }

    fn confirm(&mut self, question: &str, default_yes: bool) -> Result<bool> {
        let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
        println!("{question} {hint}");
        let answer = self.line()?;
        Ok(match answer.to_ascii_lowercase().as_str() {
            "" => default_yes,
            "y" | "yes" => true,
            _ => false,
        })
    }

    fn passphrase(&mut self, _generated: &str) -> Result<Zeroizing<String>> {
        println!(
            "Press Enter to take the generated passphrase, or type your own now.\n\
             A typed passphrase is echoed — this build has no hidden-input dependency, \
             which is one more reason to take the generated one."
        );
        // Never argv, never an environment variable (T-3-37, Phase 1's rule).
        let mut out = std::io::stdout();
        let typed = passphrase::read_line(std::io::stdin().lock(), self.marker(&mut out))?;
        Ok(typed)
    }

    fn existing_passphrase(&mut self) -> Result<Zeroizing<String>> {
        println!(
            "Type the sync password this bundle was created with.\n\
             It is echoed — this build has no hidden-input dependency."
        );
        // Never argv, never an environment variable (T-3-37, Phase 1's rule).
        let mut out = std::io::stdout();
        passphrase::read_line(std::io::stdin().lock(), self.marker(&mut out))
    }

    fn categories(&mut self, current: &[SyncCategory]) -> Result<Vec<SyncCategory>> {
        let mut chosen = current.to_vec();
        for category in SyncCategory::ALL {
            let on = chosen.contains(&category);
            let keep = self.confirm(&format!("  include `{}`?", category.label()), on)?;
            match (keep, on) {
                (true, false) => chosen.push(category),
                (false, true) => chosen.retain(|c| *c != category),
                _ => {}
            }
        }
        // Canonical D1 order, whatever order the answers arrived in.
        Ok(SyncCategory::ALL
            .into_iter()
            .filter(|c| chosen.contains(c))
            .collect())
    }
}

/// What setup learned.
///
/// **No [`PushClearance`](gate::PushClearance).** Setup has no consumer for one
/// — the field was minted, moved in, and dropped — and carrying it would hand
/// Phase 4 a stale capability that is easy to reach for (F-3). A push mints its
/// own, immediately before its first byte, and spends it.
///
/// Nothing here is a secret: the token is reported as a *source*, the
/// passphrase is not represented at all, and no keyfile byte is carried.
#[derive(Debug)]
pub struct SetupOutcome {
    pub repo: RepoRef,
    /// Where the token was *resolved* from; the value is never rendered.
    pub token_source: TokenSource,
    /// Where it was *stored* for next time — the Keychain on macOS, the
    /// mode-0600 file elsewhere.
    pub stored_at: TokenSource,
    pub visibility: String,
    pub warnings: Vec<String>,
    pub categories: Vec<SyncCategory>,
    /// The local keyfile. Still local — Phase 4 uploads it.
    pub keyfile: PathBuf,
    /// True when an existing pairing record was reused rather than issued.
    pub reused_pairing: bool,
    /// Phase 2's own figures for the chosen categories, not a second estimate.
    pub files: usize,
    pub raw_bytes: u64,
    pub would_send: u64,
    /// True when the user accepted the empty-repository offer and this run put
    /// a README in the repository. The only thing this flow can upload, and the
    /// reason the closing line cannot say "nothing was uploaded" unconditionally.
    pub initialised: bool,
}

/// `N/5  …` — the step markers were always there and simply did not stand out.
///
/// The accent on the marker is the only one in the whole flow, which is what
/// makes five steps read as five steps rather than as one wall of text; the
/// body is dim because the marker is the thing being scanned for.
///
/// **Narration only.** It reformats the line it is handed and knows nothing
/// about which step it is or what comes next.
fn step(style: Style, marker: &str, text: &str) -> String {
    format!(
        "{}  {}",
        style.head(marker),
        style.dim(&report::reflow(style, text, 5))
    )
}

/// An indented continuation under a step marker, at the same depth the flow has
/// always used.
fn under(style: Style, text: &str) -> String {
    format!("     {}", style.dim(&report::reflow(style, text, 5)))
}

/// The five steps, in order.
///
/// A refusal after step 1 has cost the user the category question and nothing
/// else — no passphrase, no file, no remote write. That is the price of asking
/// the gate a settled question, and it is the right one: the alternative is a
/// public repository cleared for a bundle it was never assessed against.
pub async fn run(
    cfg: &SyncConfig,
    roots: &SyncRoots,
    endpoints: &Endpoints,
    chain: &token::TokenChain,
    prompt: &mut dyn SetupPrompt,
    now: DateTime<Utc>,
) -> Result<SetupOutcome> {
    // Resolved once, before the first line is narrated. Nothing below branches
    // on it except for weight.
    let style = prompt.style();

    // ---- Preconditions: local, and reached before any prompt -------------
    // T-3-38 first, and before anything asks the user for anything: this is a
    // fact about *this machine*, not about the repository, and a run that is
    // going to refuse should refuse before it costs a question. Overwriting a
    // keyfile makes every bundle written under the old password permanently
    // unreadable.
    let keyfile_path = crate::sync::cli::keyfile_path(roots);
    if keyfile_path.exists() {
        return Err(AppError::Other(existing_keyfile_message(&keyfile_path)));
    }

    let Some(configured) = cfg.repo.as_deref() else {
        return Err(AppError::Other(no_repo_message()));
    };
    let repo = RepoRef::parse(configured)?;

    let token_file = token_path(roots);
    let (value, source) = token::resolve(chain)?;
    // Kept so step 5 can persist it. `Zeroizing`, so the copy dies with it.
    let keep = value.clone();
    let client = Client::new(endpoints, value, source)?;

    let facts = match gate::fetch_facts(&client, &repo, now).await {
        Ok(facts) => facts,
        Err(e) => {
            return Err(clear_if_dead(e, source, &token_file, &|src, p| {
                prompt.clear_token(src, p)
            }));
        }
    };

    // ---- Step 1: the categories — the question the gate is asked ---------
    //
    // **This runs before the gate, and that ordering is the whole point.**
    // `credentials_in_bundle` is the only gate input the user can change, and it
    // is the one deciding D-04's public-repo carve-out. Asked afterwards, a
    // public repository with `categories = ["config"]` took the carve-out, minted
    // a clearance, and *then* had `credentials` added at this prompt — the dry
    // run enumerated the credential files, the pairing record was written at
    // `private: false`, and setup printed "paired and ready to push" (F-2).
    // Nothing re-gated.
    //
    // Reordering rather than re-asserting is deliberate: a recompute-and-check
    // closes the window but leaves two evaluations to keep in agreement, which
    // is the shape that produced the hole. There is now exactly one.
    prompt.say(&step(
        style,
        "1/5",
        "What gets bundled. `credentials` is the deliberate one — turning it on \
         syncs saved logins.",
    ));
    let categories = prompt.categories(&cfg.categories)?;
    let chosen_cfg = SyncConfig {
        categories: categories.clone(),
        ..cfg.clone()
    };

    // ---- Step 2: the repository, and the gate ----------------------------
    // Computed **once**, from the categories just chosen, and passed to both
    // gate calls. Derived twice, plan 3-04's credentials-off carve-out dies:
    // `check_drift` carves it out and `assert_pushable` would then overrule it.
    let credentials_in_bundle = chosen_cfg.includes(SyncCategory::Credentials);

    let pairing_file = pairing::default_path(roots);
    let record = pairing::read_from(&pairing_file)?;
    // check_drift first, then assert_pushable — that order, always.
    let drift = pairing::check_drift(record.as_ref(), &facts, credentials_in_bundle, now)?;
    // Kept only as far as the empty-repository offer a few lines below, which is
    // the one write this flow can make and is inside the same handful of round
    // trips as the check that authorised it. If the repository is not empty the
    // clearance is dropped unspent, which uploads nothing. A push mints and
    // spends its own — see `gate`'s Phase 4 contract; nothing here is carried
    // into one (F-3).
    let (clearance, gate_warnings) =
        gate::assert_pushable(&facts, &repo, credentials_in_bundle, now)?;

    let mut warnings = drift.warnings.clone();
    warnings.extend(gate_warnings);

    prompt.say(&format!(
        "\n{}  {repo} is {} — the gate {}.",
        style.head("2/5"),
        // Remote-supplied, so it is sanitized on the way through `Style` before
        // it is allowed anywhere near an escape sequence.
        style.good(&facts.visibility),
        style.good("passed"),
    ));
    for warning in &warnings {
        prompt.say(&format!(
            "     {} {}",
            style.bold("warning:"),
            style.dim(&report::reflow(style, warning, 5))
        ));
    }
    if drift.first_contact {
        // F-5. Deleting the pairing record makes `check_drift` skip the
        // owner_id/repo_id comparison entirely and report first contact — and
        // with no positive line for it, a silently reset pairing looked exactly
        // like a first-ever setup. Say it, with the ids, and say what it means
        // if it is a surprise.
        prompt.say(&format!(
            "     {} {}",
            style.bold("first contact:"),
            style.dim(&report::reflow(
                style,
                &format!(
                    "pairing with repository id {} owned by {} (id {}). Nothing was \
                     compared, because there was no pairing record to compare against.",
                    facts.id, facts.owner_login, facts.owner_id
                ),
                5,
            ))
        ));
        prompt.say(&under(
            style,
            "If this machine was already paired, that record did not remove itself — \
             treat its disappearance as an incident, and confirm those ids are the \
             repository you mean before going on.",
        ));
    } else {
        prompt.say(&under(
            style,
            "this machine is already paired with it; reusing that pairing rather than \
             issuing a second one.",
        ));
    }

    // Still step 2, and deliberately **after** the gate: this is the first
    // thing that would put a byte in the repository, so the private-repo
    // refusal has to have already run against it. `gh repo create --private`
    // leaves an empty repository behind, so this is the ordinary first-run
    // state — and an empty one cannot be tagged, so a push would be answered
    // with GitHub's bare 422 several minutes in.
    let initialised = if client.repo_has_no_commits(&repo).await {
        offer_first_commit(prompt, style, &client, &repo, clearance, now).await?
    } else {
        // Dropped unspent. `#[must_use]` is about a clearance nothing acted on;
        // here the thing it would have authorised turned out not to be needed.
        false
    };

    // ---- Step 3: the passphrase, and the keyfile -------------------------
    //
    // **Which of the two shapes this is comes off the remote, not off this
    // machine.** A repository that already holds a published bundle already has
    // a password, and minting a second master key for it produced a machine
    // that could pull and never push: `restore` opens the keyfile the *pointer*
    // names, so a fresh local one was never consulted, while
    // `upload::assert_keyfile_is_current` correctly refused to republish a
    // divergent wrapper. Read-only was the whole failure — two machines
    // continuing each other's work is what this milestone is for.
    //
    // The read sits here rather than beside the gate on purpose: step 2 is the
    // private-repo refusal and must stay the first thing that touches this
    // repository, and steps 1, 2 and 4 do not move.
    let published = pointer::load(
        &client,
        &repo,
        &push::repo_id_for(drift.record.repo_id),
        now,
    )
    .await?
    .0;
    let (keyfile_bytes, keys) = match published {
        Some(bundle) => join(prompt, &client, &repo, &bundle, now).await?,
        None => generate(prompt, style)?,
    };

    // ---- Step 4: the size ------------------------------------------------
    let index = Index::at(&roots.index_file)?;
    let sync_plan = plan::build_with_keys(roots, &chosen_cfg, &index, now, &keys)?;
    let (files, raw_bytes, would_send) = (
        sync_plan
            .categories
            .iter()
            .filter(|c| chosen_cfg.includes(c.category))
            .map(|c| c.files)
            .sum(),
        sync_plan
            .categories
            .iter()
            .filter(|c| chosen_cfg.includes(c.category))
            .map(|c| c.raw_bytes)
            .sum(),
        sync_plan.total_new_stored_bytes,
    );

    prompt.say(&format!(
        "\n{}",
        step(style, "4/5", "What a first push would send:")
    ));
    // The dry-run's own renderer over the dry-run's own plan — so the number
    // here and the number `sync push --dry-run` prints cannot disagree.
    prompt.say(&report::render_dry_run_styled(
        &DryRunReport {
            status: report::build_status(
                roots,
                &chosen_cfg,
                Some(&index),
                now,
                Some(sync_plan),
                None,
            ),
            no_key: None,
        },
        style,
    ));
    if !prompt.confirm("     Pair this machine with that scope?", true)? {
        return Err(AppError::Other(format!(
            "setup stopped at the size confirmation. No bundle data was uploaded — this \
             command never uploads any — {}, and nothing was written here: no keyfile, no \
             config change, no stored token. Re-run when you are ready.",
            if initialised {
                "the README you approved is the only thing in the repository"
            } else {
                "the repository was not touched"
            }
        )));
    }

    // ---- Step 5: everything that persists --------------------------------
    // Nothing above this line writes. The confirmation is the last thing that
    // can abort, so it is the last thing before the first write (F-10).
    write_keyfile(&keyfile_path, &keyfile_bytes)?;
    prompt.say(&format!(
        "\n{}",
        step(
            style,
            "5/5",
            &format!("keyfile written: {}", keyfile_path.display())
        )
    ));
    if categories != cfg.categories {
        write_categories(&roots.config_file, &categories)?;
        prompt.say(&under(
            style,
            &format!("saved to {}", roots.config_file.display()),
        ));
    }
    let stored_at = prompt.store_token(&keep, &token_file).map_err(|e| {
        AppError::Other(format!(
            "could not save the GitHub sync token: {e}\n\
             The secret being saved here is the *sync token*, not a Claude credential — \
             the wording above comes from the shared Keychain helper."
        ))
    })?;
    pairing::write_to(&pairing_file, &drift.record)?;

    Ok(SetupOutcome {
        repo,
        token_source: source,
        stored_at,
        visibility: facts.visibility,
        warnings,
        categories,
        keyfile: keyfile_path,
        reused_pairing: !drift.first_contact,
        files,
        raw_bytes,
        would_send,
        initialised,
    })
}

/// The empty-repository offer, and the one write this flow can make.
///
/// **Asked, never assumed.** It is the user's repository and this is the first
/// thing the tool would ever put in it, so the answer comes from
/// [`SetupPrompt::confirm`] — a decline leaves the repository exactly as it was
/// and prints what to run instead. Setup then carries on: the pairing, the
/// password and the scope are all still valid, and the push that eventually
/// needs the commit says so again in the same words.
///
/// **This adds a commit; it does not create a repository.** The token holds no
/// `Administration: write`, which is what makes bringing a *public* repository
/// into existence structurally impossible rather than merely disallowed
/// (REPO-03), and a `Contents` write does not touch that. A repository that is
/// not there never reaches this function — [`gate::fetch_facts`] has already
/// refused with the `gh repo create --private` line.
///
/// The clearance is taken **by value** and spent here, which is the whole
/// reason it survived step 2: this is the byte the private-repo check was run
/// for, a few round trips after it.
async fn offer_first_commit(
    prompt: &mut dyn SetupPrompt,
    style: Style,
    client: &Client,
    repo: &RepoRef,
    clearance: gate::PushClearance,
    now: DateTime<Utc>,
) -> Result<bool> {
    prompt.say(&under(
        style,
        &format!(
            "{repo} has no commits yet — which is what `gh repo create --private` leaves \
             behind. GitHub will not tag an empty repository, so a push would be refused \
             by it.",
        ),
    ));
    if !prompt.confirm(
        "     Add a README.md to it now, so a push has something to hang off?",
        true,
    )? {
        prompt.say(&under(
            style,
            "left untouched. Give it a commit yourself before the first push:",
        ));
        prompt.say(&write::first_commit_command(repo));
        return Ok(false);
    }
    client
        .init_first_commit(repo, &clearance.spend(now)?, now)
        .await?;
    prompt.say(&under(style, "README.md written."));
    Ok(true)
}

/// Passphrase attempts a joining machine gets before setup gives up.
///
/// A cap rather than the generate path's unbounded re-prompt, because the two
/// loops end differently: a weak passphrase is refused by a pure check the user
/// can satisfy by typing a longer one, while a wrong password against a
/// published keyfile is an Argon2id derivation and an AEAD unwrap per attempt,
/// driven by whatever is on stdin. Three is enough for a typo and not enough
/// for a script.
const JOIN_ATTEMPTS: u32 = 3;

/// Step 3, the first-machine shape: mint a master key under a password this
/// machine chooses.
///
/// Returns the **bytes to persist** rather than a [`Keyfile`], so step 5 has one
/// write for both shapes. Nothing here writes: see step 5 and F-10.
fn generate(prompt: &mut dyn SetupPrompt, style: Style) -> Result<(Vec<u8>, Keys)> {
    let kdf = prompt.kdf();
    let generated = passphrase::generate()?;
    prompt.say(&format!(
        "\n{}",
        step(style, "3/5", "A sync password protects the bundle.")
    ));
    // **Bold, and the only prose on the screen that is.** Of everything setup
    // says, this is the sentence whose cost is unrecoverable, and the user
    // reported it reading at exactly the same weight as the rest.
    prompt.say(&style.bold(&report::reflow(style, passphrase::NO_RECOVERY, 0)));
    prompt.say(&report::reflow(style, passphrase::OFFLINE_ATTACK_NOTE, 0));
    // Shown exactly once — not re-displayed on a re-prompt below.
    //
    // The accent goes on the passphrase itself: it is the one thing on this
    // screen the user has to act on, and the only place in the whole tool that
    // prints a secret — deliberately, once.
    prompt.say(&format!(
        "\n     {}  {}\n",
        style.dim("generated passphrase:"),
        style.head(&generated)
    ));

    let chosen_pw = loop {
        let candidate = prompt.passphrase(&generated)?;
        let candidate = if candidate.is_empty() {
            generated.clone()
        } else {
            candidate
        };
        match passphrase::check(&candidate, kdf) {
            Strength::Rejected(why) => prompt.say(&format!(
                "     {} {}",
                style.bad("refused:"),
                style.dim(&report::reflow(style, why, 5))
            )),
            Strength::Weak(why) => {
                prompt.say(&under(style, why));
                break candidate;
            }
            Strength::Strong => break candidate,
        }
    };

    // Created, **not written**: see step 5. F-10 — the file used to land here,
    // before the confirmation that can abort, and its own existence then refused
    // the re-run, stranding a user who declined behind a passphrase they were
    // shown once and told there is no recovery for.
    let (keyfile, keys) = Keyfile::create(chosen_pw.as_bytes(), kdf)?;
    Ok((serde_json::to_vec_pretty(&keyfile)?, keys))
}

/// Step 3, the second-machine shape: adopt the keyfile the published pointer
/// names, under the password it was created with.
///
/// **[`Keyfile::create`] is not reachable from here, deliberately.** A generated
/// passphrase shown to a user whose bundle it cannot open is worse than a
/// refusal: they are told there is no recovery for a string that was never a
/// key to anything.
///
/// # These bytes are hostile until they open
///
/// They came off a remote the format treats as such, and the pointer that named
/// them is the one unauthenticated link in the chain. The **only** thing that
/// authenticates them is that the passphrase unwraps the master key — an AEAD
/// tag over the keyfile's own `{format, kdf}` associated data — so a failure
/// here is refused and nothing is returned to persist. It is deliberately one
/// message for "wrong password" and "not this bundle's keyfile" alike;
/// separating them is the oracle Phase 1 refused to build.
///
/// The bytes are returned **verbatim**, so step 5 writes a file byte-identical
/// to the published asset. That is what makes this machine's first push
/// *accepted*: `upload::assert_keyfile_is_current` compares the content address
/// of the local keyfile's canonical form against `Pointer.keyfile`, and a
/// re-serialization here would be a second place for the two to diverge.
async fn join(
    prompt: &mut dyn SetupPrompt,
    client: &Client,
    repo: &RepoRef,
    published: &Pointer,
    now: DateTime<Utc>,
) -> Result<(Vec<u8>, Keys)> {
    let bytes = fetch::published_keyfile(client, repo, &published.keyfile, now).await?;
    let keyfile: Keyfile = serde_json::from_slice(&bytes).map_err(|_| {
        AppError::Other(format!(
            "the keyfile asset {:?} this repository's snapshot pointer names is not a \
             readable sync keyfile, so this machine cannot join the bundle. Nothing was \
             written here.",
            published.keyfile
        ))
    })?;

    prompt.say(
        "\n3/5  This repository already holds a bundle, so this machine joins it rather than \
         starting a second one.",
    );
    prompt.say(
        "     Type the sync password that bundle was created with. It is not stored anywhere \
         this tool can read, and there is no recovery for it by design.",
    );

    for attempt in 1..=JOIN_ATTEMPTS {
        let candidate = prompt.existing_passphrase()?;
        if let Ok(keys) = keyfile.open(candidate.as_bytes()) {
            return Ok((bytes, keys));
        }
        prompt.say(&format!(
            "     refused: that password does not open this bundle's keyfile \
             (attempt {attempt} of {JOIN_ATTEMPTS})."
        ));
    }

    Err(AppError::Other(format!(
        "the sync password did not open this bundle's keyfile in {JOIN_ATTEMPTS} attempts.\n\
         Nothing was written on this machine — no keyfile, no config change, no stored token — \
         so re-running setup with the right password is all this takes. It is the password the \
         bundle was created with on the first machine; it cannot be reset from here, and this \
         tool has no copy of it."
    )))
}

/// `<config_dir>/sync-token` — the same file [`token::TokenChain::production`]
/// computes from the resolved config path, but taken from the injected roots
/// so nothing here reads a real `$HOME`.
pub(crate) fn token_path(roots: &SyncRoots) -> PathBuf {
    roots.config_dir.join("sync-token")
}

/// Take a rejected token out of circulation — the *right* one, and say which.
///
/// Two predicates were wrong here, and they compounded (F-1):
///
/// - **The trigger** was `matches!(err, AppError::Credentials(_))`, which also
///   catches `Client::get_json`'s "this token is not a legal HTTP header value".
///   A token with one illegal byte silently deleted the Keychain item while
///   printing a message about header validity. The trigger is now
///   [`gate::FetchError::token_rejected`] — a 401 from GitHub, nothing else.
/// - **The action** ignored the [`TokenSource`] it was handed. It is now the
///   only thing that decides which store is touched; see
///   [`token::clear_source`] for the sequence that destroyed a working token
///   with no attacker involved.
///
/// The message follows the action rather than preceding it: `http::actionable`'s
/// 401 arm no longer promises a clear it cannot know about, and
/// [`token::clear_note`] states what was actually done, per source.
///
/// `clear` is a parameter because the production function deletes the **real**
/// login Keychain item on macOS, which no unit test may touch. Production passes
/// `token::clear_source`; the test passes a recorder, which is what makes the
/// pairing assertable rather than reviewable.
///
/// A failure to clear is deliberately swallowed: the original 401 is the thing
/// the user needs to read, and burying it under a filesystem error would be
/// worse than a token file that outlived its usefulness.
pub(crate) fn clear_if_dead(
    err: gate::FetchError,
    source: TokenSource,
    token_file: &Path,
    clear: &dyn Fn(TokenSource, &Path) -> Result<()>,
) -> AppError {
    if !err.token_rejected {
        return err.error;
    }
    // The seam is not even reached for a source this tool has no store for —
    // belt to `token::clear_source`'s braces, and what makes "the environment's
    // token 401'd and nothing was deleted" assertable through the test double
    // rather than only inside the production function.
    if token::owns_a_store(source) {
        let _ = clear(source, token_file);
    }
    AppError::Credentials(format!(
        "{}\n{}",
        err.error,
        token::clear_note(source, token_file)
    ))
}

/// D-01: nothing is guessed, and the failure names the exact fix.
fn no_repo_message() -> String {
    "[sync] repo is unset, and ai-usagebar never guesses or creates one.\n\
     Create a private repository, then name it in config.toml:\n\
     \x20   gh repo create <owner>/<name> --private\n\
     \n\
     \x20   [sync]\n\
     \x20   repo = \"<owner>/<name>\""
        .to_owned()
}

/// T-3-38. Overwriting a keyfile makes every bundle written under the old one
/// permanently unreadable, and there is no recovery by design.
fn existing_keyfile_message(path: &Path) -> String {
    format!(
        "a sync keyfile already exists at {} and setup will not overwrite it.\n\
         Overwriting it would make every bundle written under the old password permanently \
         unreadable — there is no recovery, by design.\n\
         This machine is already set up. To start over with a new password you must delete \
         that file yourself, knowing the existing bundle becomes unreadable.",
        path.display()
    )
}

/// Atomically, then mode 0600 — the same belt-and-braces `anchor::write_to` and
/// the Settings overlay apply. The temp file lands in the destination's own
/// directory, never `/tmp`.
///
/// Takes **bytes**, not a [`Keyfile`]: the two step-3 shapes disagree about what
/// the file should contain. [`generate`] pretty-prints its own new keyfile;
/// [`join`] writes the published asset verbatim, because byte-identity with
/// `Pointer.keyfile`'s content address is what makes this machine's first push
/// accepted rather than refused as a superseded wrapper.
fn write_keyfile(path: &Path, bytes: &[u8]) -> Result<()> {
    crate::cache::atomic_write(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| AppError::io_at(path, e))?;
    }
    Ok(())
}

/// Write `categories` back into `[sync]`, preserving comments and key order the
/// way `tui::settings::save_to_path` already does (T-3-39).
fn write_categories(config_file: &Path, categories: &[SyncCategory]) -> Result<()> {
    let original = match std::fs::read_to_string(config_file) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(AppError::io_at(config_file, e)),
    };
    let mut doc: DocumentMut = if original.trim().is_empty() {
        DocumentMut::new()
    } else {
        original.parse().map_err(|e: toml_edit::TomlError| {
            AppError::Other(format!("config.toml not parseable: {e}"))
        })?
    };

    let mut array = toml_edit::Array::new();
    for category in categories {
        array.push(category.label());
    }
    doc.entry("sync")
        .or_insert_with(toml_edit::table)
        .as_table_mut()
        .ok_or_else(|| AppError::Other("config.toml has a non-table [sync]".into()))?["categories"] =
        toml_edit::value(array);

    crate::cache::atomic_write(config_file, doc.to_string().as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(config_file) {
            let mut perms = meta.permissions();
            perms.set_mode(0o600);
            let _ = std::fs::set_permissions(config_file, perms);
        }
    }
    Ok(())
}

/// The scripted [`SetupPrompt`] every test in this crate drives.
///
/// `reached` is the whole point: it records which prompt methods ran, in order,
/// so "the gate refused before the password step" is an assertion rather than a
/// claim. Lives at module scope (rather than inside `mod tests`) because
/// `sync::cli`'s tests drive the same flow — the same reuse `cache::temp_file`
/// already makes.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct Script {
    pub reached: Vec<String>,
    pub said: Vec<String>,
    /// Defaults to [`Style::PLAIN`], which is why every existing test in this
    /// crate still asserts against the wording it always did.
    pub style: Style,
    /// Answers for `passphrase`, in order. Exhausted ⇒ the generated one.
    pub passphrases: Vec<String>,
    /// Answers for `existing_passphrase`, in order — the join path's ask, which
    /// has no generated alternative to fall back to. Exhausted ⇒ `""`, which
    /// opens nothing, so a script that runs out fails loudly rather than
    /// silently taking a password it was never given.
    pub existing: Vec<String>,
    /// `None` keeps whatever the config already had.
    pub categories: Option<Vec<SyncCategory>>,
    pub confirm: bool,
    /// Answer `false` to any question containing this, whatever `confirm` says.
    ///
    /// The flow has two yes/no questions that a test needs to answer
    /// *differently* — the empty-repository offer and the size confirmation —
    /// and one `bool` cannot say "no to the README, yes to the pairing".
    pub decline_matching: Option<String>,
    /// Token-file paths the flow asked to store / clear. Recorded rather than
    /// acted on — no test may reach a real Keychain. `cleared` carries the
    /// `TokenSource` too: *which* store a 401 reaches is the whole question
    /// (F-1), and a recorder that dropped it could not tell right from wrong.
    pub stored: Vec<PathBuf>,
    pub cleared: Vec<(TokenSource, PathBuf)>,
}

#[cfg(test)]
impl Script {
    pub fn new() -> std::rc::Rc<std::cell::RefCell<Script>> {
        std::rc::Rc::new(std::cell::RefCell::new(Script {
            confirm: true,
            ..Script::default()
        }))
    }
}

#[cfg(test)]
pub(crate) struct Double(pub std::rc::Rc<std::cell::RefCell<Script>>);

#[cfg(test)]
impl SetupPrompt for Double {
    fn style(&self) -> Style {
        self.0.borrow().style
    }
    fn say(&mut self, line: &str) {
        self.0.borrow_mut().said.push(line.to_owned());
    }
    fn confirm(&mut self, question: &str, _default_yes: bool) -> Result<bool> {
        let mut s = self.0.borrow_mut();
        s.reached.push(format!("confirm:{question}"));
        if let Some(needle) = &s.decline_matching
            && question.contains(needle.as_str())
        {
            return Ok(false);
        }
        Ok(s.confirm)
    }
    fn passphrase(&mut self, _generated: &str) -> Result<Zeroizing<String>> {
        let mut s = self.0.borrow_mut();
        s.reached.push("passphrase".into());
        let next = if s.passphrases.is_empty() {
            String::new()
        } else {
            s.passphrases.remove(0)
        };
        Ok(Zeroizing::new(next))
    }
    fn existing_passphrase(&mut self) -> Result<Zeroizing<String>> {
        let mut s = self.0.borrow_mut();
        s.reached.push("existing_passphrase".into());
        let next = if s.existing.is_empty() {
            String::new()
        } else {
            s.existing.remove(0)
        };
        Ok(Zeroizing::new(next))
    }
    fn categories(&mut self, current: &[SyncCategory]) -> Result<Vec<SyncCategory>> {
        let mut s = self.0.borrow_mut();
        s.reached.push("categories".into());
        Ok(s.categories.clone().unwrap_or_else(|| current.to_vec()))
    }
    /// 8 MiB rather than a gibibyte: the AUR `check()` runs these.
    fn kdf(&self) -> KdfParams {
        KdfParams {
            m_kib: crate::sync::crypto::MIN_KDF_MEMORY_KIB,
            t: 1,
            p: 1,
        }
    }
    /// Recorded, never performed. The production default would write the real
    /// macOS login Keychain.
    fn store_token(&self, _token: &str, file: &Path) -> Result<TokenSource> {
        self.0.borrow_mut().stored.push(file.to_path_buf());
        Ok(TokenSource::File)
    }
    fn clear_token(&self, source: TokenSource, file: &Path) -> Result<()> {
        self.0
            .borrow_mut()
            .cleared
            .push((source, file.to_path_buf()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use base64::Engine as _;
    use std::cell::RefCell;
    use std::fs;
    use std::rc::Rc;
    use tempfile::TempDir;

    const FIXTURE: &str = "github_pat_fixture_not_a_real_token";
    const PRIVATE: &str = r#"{"id":1,"private":true,"visibility":"private",
        "owner":{"login":"o","id":7},"archived":false,"fork":false}"#;
    const PUBLIC: &str = r#"{"id":1,"private":false,"visibility":"public",
        "owner":{"login":"o","id":7},"archived":false,"fork":false}"#;

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    fn roots_at(dir: &TempDir) -> SyncRoots {
        SyncRoots::at(
            dir.path().join("config.toml"),
            dir.path().to_path_buf(),
            dir.path().join("desktop"),
            dir.path().join("profiles"),
            dir.path().join("claude-home"),
        )
    }

    fn chain() -> token::TokenChain {
        token::TokenChain {
            env_value: Some(Zeroizing::new(FIXTURE.into())),
            ..token::TokenChain::default()
        }
    }

    fn endpoints_at(base: &str) -> Endpoints {
        Endpoints {
            api_base: base.into(),
            uploads_base: base.into(),
        }
    }

    fn cfg_for(repo: Option<&str>) -> SyncConfig {
        SyncConfig {
            repo: repo.map(str::to_owned),
            ..SyncConfig::default()
        }
    }

    /// One private-repo mock, an empty repository, one scripted run.
    ///
    /// The 404 on `sync/pointer.json` is what makes this the **first** machine:
    /// step 3 reads the pointer to decide whether to mint a master key or join
    /// an existing bundle, so a fixture without it would answer that question
    /// with mockito's 501.
    async fn drive(
        cfg: &SyncConfig,
        dir: &TempDir,
        body: &str,
        status: usize,
        script: &Rc<RefCell<Script>>,
    ) -> Result<SetupOutcome> {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/repos/o/n")
            .with_status(status)
            .with_body(body)
            .create_async()
            .await;
        let _p = no_pointer(&mut server).await;
        let _c = has_commits(&mut server).await;
        run(
            cfg,
            &roots_at(dir),
            &endpoints_at(&server.url()),
            &chain(),
            &mut Double(Rc::clone(script)),
            now(),
        )
        .await
    }

    /// "This repository already has a commit" — the ordinary case, and the one
    /// every test but the empty-repository ones wants. Only the status is read.
    async fn has_commits(server: &mut mockito::ServerGuard) -> mockito::Mock {
        server
            .mock("GET", "/repos/o/n/commits")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_body("[]")
            .create_async()
            .await
    }

    /// GitHub's own answer for a repository with no commits at all, verbatim.
    async fn no_commits(server: &mut mockito::ServerGuard) -> mockito::Mock {
        server
            .mock("GET", "/repos/o/n/commits")
            .match_query(mockito::Matcher::Any)
            .with_status(409)
            .with_body(r#"{"message":"Git Repository is empty."}"#)
            .create_async()
            .await
    }

    /// "Nothing has ever been pushed here."
    async fn no_pointer(server: &mut mockito::ServerGuard) -> mockito::Mock {
        server
            .mock("GET", "/repos/o/n/contents/sync/pointer.json")
            .with_status(404)
            .with_body(r#"{"message":"Not Found"}"#)
            .create_async()
            .await
    }

    /// A published bundle: the pointer, the release it hangs off, the asset
    /// listing, and the keyfile bytes themselves.
    ///
    /// `asset` is served verbatim, so a corrupted-keyfile case is this same
    /// fixture with different bytes — and the byte-identity assertion compares
    /// against exactly what the fake handed over.
    async fn publish(server: &mut mockito::ServerGuard, name: &str, asset: Vec<u8>) {
        let pointer = serde_json::json!({
            "format": 1,
            "repo_id": "github:1",
            "keyfile": name,
            "snapshots": [],
        })
        .to_string();
        server
            .mock("GET", "/repos/o/n/contents/sync/pointer.json")
            .with_status(200)
            .with_body(format!(
                r#"{{"sha":"blob1","content":"{}"}}"#,
                base64::engine::general_purpose::STANDARD.encode(&pointer)
            ))
            .create_async()
            .await;
        server
            .mock("GET", "/repos/o/n/releases/tags/ai-usagebar-sync-v1")
            .with_status(200)
            .with_body(r#"{"id":9}"#)
            .create_async()
            .await;
        server
            .mock("GET", mockito::Matcher::Regex("/releases/9/assets".into()))
            .with_status(200)
            .with_body(format!(
                r#"[{{"id":42,"name":"{name}","size":{},"state":"uploaded",
                     "created_at":"2023-11-14T22:13:20Z"}}]"#,
                asset.len()
            ))
            .create_async()
            .await;
        server
            .mock("GET", "/repos/o/n/releases/assets/42")
            .with_status(200)
            .with_body(asset)
            .create_async()
            .await;
    }

    /// A keyfile the fake publishes, at parameters a test can afford, plus the
    /// canonical bytes and asset name that go with it.
    ///
    /// `Keyfile::create` is the production verb and enforces
    /// `MIN_KDF_MEMORY_KIB`; 8 MiB of Argon2id is milliseconds, which is what
    /// keeps the AUR `check()` inside its budget on an installer's machine.
    fn published_keyfile(pw: &str) -> (Vec<u8>, String) {
        let (keyfile, _keys) = Keyfile::create(
            pw.as_bytes(),
            KdfParams {
                m_kib: crate::sync::crypto::MIN_KDF_MEMORY_KIB,
                t: 1,
                p: 1,
            },
        )
        .unwrap();
        let bytes = serde_json::to_vec(&keyfile).unwrap();
        let name =
            crate::sync::push::keyfile_asset_name(&crate::sync::crypto::content_address(&bytes));
        (bytes, name)
    }

    /// One already-published-bundle mock set, one scripted run.
    async fn drive_joining(
        dir: &TempDir,
        asset_name: &str,
        asset: Vec<u8>,
        script: &Rc<RefCell<Script>>,
    ) -> Result<SetupOutcome> {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/repos/o/n")
            .with_status(200)
            .with_body(PRIVATE)
            .create_async()
            .await;
        publish(&mut server, asset_name, asset).await;
        let _c = has_commits(&mut server).await;
        run(
            &cfg_for(Some("o/n")),
            &roots_at(dir),
            &endpoints_at(&server.url()),
            &chain(),
            &mut Double(Rc::clone(script)),
            now(),
        )
        .await
    }

    // ---- the happy path --------------------------------------------------

    #[tokio::test]
    async fn the_five_steps_run_in_order_and_end_at_ready_to_push() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("config.toml"), "[sync]\n").unwrap();
        let script = Script::new();

        let out = drive(&cfg_for(Some("o/n")), &dir, PRIVATE, 200, &script)
            .await
            .unwrap();

        // Categories first: the gate cannot be asked before the question it is
        // being asked is settled (F-2).
        let reached = script.borrow().reached.clone();
        assert_eq!(reached[0], "categories", "{reached:?}");
        assert_eq!(reached[1], "passphrase", "{reached:?}");
        assert!(reached[2].starts_with("confirm:"), "{reached:?}");
        assert_eq!(reached.len(), 3, "{reached:?}");

        assert_eq!(out.visibility, "private");
        assert_eq!(out.token_source, TokenSource::Env);
        assert!(!out.reused_pairing);
        assert!(out.keyfile.starts_with(dir.path()));
        assert!(out.keyfile.exists());
        // Phase 2's own number, and the same number the user was shown: step 4
        // renders the dry-run's own report over the dry-run's own plan, so the
        // figure here and the figure `sync push --dry-run` prints cannot
        // disagree.
        assert!(out.files >= 1, "{out:?}");
        assert!(out.would_send > 0, "{out:?}");
        let said = script.borrow().said.join("\n");
        assert!(
            said.contains(&crate::sync::report::human_bytes(out.would_send)),
            "the confirmed size is the plan builder's own total: {said}"
        );
        assert!(said.contains("uploads nothing"), "{said}");
        // The token never reaches the narration either, not even a prefix.
        assert!(!said.contains(FIXTURE), "{said}");
        assert!(!said.contains(&FIXTURE[..8]), "{said}");
    }

    /// The reported defect: five step headers that read as one wall, with the
    /// two consequential lines at the same weight as everything else.
    ///
    /// **Narration only.** Nothing below asserts on the order or on which
    /// methods ran — that is the test above — only on which words carry weight.
    #[tokio::test]
    async fn the_steps_and_the_two_lines_that_matter_carry_the_only_weight() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("config.toml"), "[sync]\n").unwrap();
        let script = Script::new();
        script.borrow_mut().style = Style::color(true);

        drive(&cfg_for(Some("o/n")), &dir, PRIVATE, 200, &script)
            .await
            .unwrap();
        let said = script.borrow().said.join("\n");

        // Every step marker wears the one accent, and nothing else does.
        for marker in ["1/5", "2/5", "3/5", "4/5", "5/5"] {
            assert!(
                said.contains(&format!("\x1b[1;36m{marker}\x1b[0m")),
                "{marker} does not stand out:\n{said}"
            );
        }
        assert_eq!(
            said.matches("\x1b[1;36m").count(),
            6,
            "five markers and the generated passphrase, and nothing else"
        );

        // The two lines that matter most, at a weight nothing else has.
        assert!(
            said.contains(&format!("\x1b[1m{}", &passphrase::NO_RECOVERY[..40])),
            "the no-recovery warning is not bold:\n{said}"
        );
        let generated = said
            .split("generated passphrase:\x1b[0m  \x1b[1;36m")
            .nth(1)
            .and_then(|rest| rest.split("\x1b[0m").next())
            .expect("the generated passphrase is shown once, in the accent");
        assert!(generated.len() >= 20, "{generated:?}");

        // …and it is still shown exactly once, and is still the only secret on
        // the screen.
        assert_eq!(said.matches(generated).count(), 1, "shown once");
        assert!(!said.contains(FIXTURE), "no token: {said}");
        assert!(!said.contains(&FIXTURE[..8]), "no token prefix: {said}");

        // Nothing here opens a sequence it does not close.
        let closes = said.matches("\x1b[0m").count();
        assert_eq!(
            said.matches("\x1b[").count(),
            closes * 2,
            "unbalanced:\n{said}"
        );
    }

    /// The wording is the contract every other test in this crate asserts
    /// against, so the unstyled flow must still say exactly what it said.
    #[tokio::test]
    async fn the_unstyled_flow_says_the_same_words_the_styled_one_does() {
        let plain = {
            let dir = TempDir::new().unwrap();
            fs::write(dir.path().join("config.toml"), "[sync]\n").unwrap();
            let script = Script::new();
            script
                .borrow_mut()
                .passphrases
                .push("a-supplied-passphrase-long-enough".into());
            drive(&cfg_for(Some("o/n")), &dir, PRIVATE, 200, &script)
                .await
                .unwrap();
            script.borrow().said.join("\n")
        };
        // Deliberately never printed on failure: the narration holds the
        // generated passphrase, and a panic message is a transcript.
        assert!(
            !plain.contains('\x1b'),
            "an escape reached the unstyled flow"
        );
        for wording in [
            "1/5  What gets bundled.",
            "     first contact: pairing with repository id ",
            "\n3/5  A sync password protects the bundle.",
            "     generated passphrase:  ",
            "\n4/5  What a first push would send:",
            "\n5/5  keyfile written: ",
        ] {
            assert!(plain.contains(wording), "{wording:?} moved");
        }
    }

    /// T-3-36. Neither secret, nor an eight-character prefix of either, reaches
    /// the value the CLI renders.
    #[tokio::test]
    async fn the_outcome_carries_neither_the_token_nor_the_passphrase() {
        let dir = TempDir::new().unwrap();
        let script = Script::new();
        script
            .borrow_mut()
            .passphrases
            .push("a-supplied-passphrase-long-enough".into());

        let out = drive(&cfg_for(Some("o/n")), &dir, PRIVATE, 200, &script)
            .await
            .unwrap();

        let rendered = format!("{out:?}");
        assert!(!rendered.contains(FIXTURE), "{rendered}");
        assert!(!rendered.contains(&FIXTURE[..8]), "{rendered}");
        assert!(!rendered.contains("a-supplied-passphrase-long-enough"));
        assert!(!rendered.contains("a-suppli"), "{rendered}");
        // …and no keyfile byte either.
        let keyfile = fs::read_to_string(&out.keyfile).unwrap();
        let wrapped: serde_json::Value = serde_json::from_str(&keyfile).unwrap();
        let wrapped = wrapped["wrapped_master_key"].as_str().unwrap();
        assert!(!rendered.contains(wrapped), "{rendered}");
    }

    /// T-3-35, the whole ordering claim: a refusal never reaches the password
    /// step, and never leaves a keyfile behind.
    ///
    /// The category question is the one thing a gate refusal now costs, and it
    /// has to: it is the gate's own input (F-2). Everything that is not a gate
    /// input still refuses with no prompt at all — which is why the two
    /// token/repository failures are asserted separately and more strictly.
    #[tokio::test]
    async fn every_refusal_stops_before_the_password_step_is_reached() {
        for (body, status) in [(PUBLIC, 200), ("{}", 404), ("{}", 401)] {
            let dir = TempDir::new().unwrap();
            let script = Script::new();
            let err = drive(&cfg_for(Some("o/n")), &dir, body, status, &script)
                .await
                .expect_err("this repository is not pairable");
            let reached = script.borrow().reached.clone();
            assert!(
                !reached.iter().any(|r| r == "passphrase"),
                "status {status} reached {reached:?}"
            );
            if status != 200 {
                assert!(
                    reached.is_empty(),
                    "a failure that is not a gate decision asks nothing: {reached:?}"
                );
            }
            assert!(!crate::sync::cli::keyfile_path(&roots_at(&dir)).exists());
            let _ = err;
        }
    }

    /// D-01, and the 404 arm both carry the create command; neither offers to
    /// create anything.
    #[tokio::test]
    async fn a_missing_repository_names_the_create_command_and_never_offers_to_run_it() {
        let dir = TempDir::new().unwrap();
        let script = Script::new();
        let err = drive(&cfg_for(Some("o/n")), &dir, "{}", 404, &script)
            .await
            .expect_err("404");
        let text = err.to_string();
        assert!(text.contains("gh repo create"), "{text}");

        // …and with no repository configured at all, against a dead port.
        let err = run(
            &cfg_for(None),
            &roots_at(&dir),
            &endpoints_at("http://127.0.0.1:1"),
            &chain(),
            &mut Double(Rc::clone(&script)),
            now(),
        )
        .await
        .expect_err("no repository is configured");
        let text = err.to_string();
        assert!(
            text.contains("gh repo create <owner>/<name> --private"),
            "{text}"
        );
        assert!(text.contains("[sync]"), "{text}");
        assert!(script.borrow().reached.is_empty());
    }

    // ---- the 401 promise -------------------------------------------------

    /// F-1, both halves, on the function that decides them.
    ///
    /// The trigger used to be `matches!(err, AppError::Credentials(_))`, which
    /// also catches `Client::get_json`'s "not a valid HTTP header value" — so a
    /// malformed token deleted the Keychain item while printing about header
    /// validity. And the action ignored the `TokenSource` entirely, deleting
    /// both stores whatever had answered.
    #[test]
    fn only_a_401_clears_and_it_clears_only_the_store_it_came_from() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sync-token");
        let cleared: RefCell<Vec<(TokenSource, PathBuf)>> = RefCell::new(Vec::new());
        let record = |source: TokenSource, p: &Path| {
            cleared.borrow_mut().push((source, p.to_path_buf()));
            Ok(())
        };
        let dead = || gate::FetchError {
            error: AppError::Credentials("GitHub rejected the sync token (401)".into()),
            token_rejected: true,
        };

        // (a) A `Credentials` error that is not a 401 clears nothing — the
        // malformed-token path, which produces the same `AppError` arm.
        let returned = clear_if_dead(
            gate::FetchError {
                error: AppError::Credentials(
                    "the stored sync token is not a valid HTTP header value".into(),
                ),
                token_rejected: false,
            },
            TokenSource::Keychain,
            &path,
            &record,
        );
        assert!(
            cleared.borrow().is_empty(),
            "a token this build could not send is not a token GitHub rejected"
        );
        assert!(returned.to_string().contains("header value"), "{returned}");

        // (b) Neither store belongs to this tool, so neither is touched — and
        // the message names the thing the user must change instead.
        for source in [TokenSource::Env, TokenSource::GhCli] {
            let returned = clear_if_dead(dead(), source, &path, &record);
            assert!(cleared.borrow().is_empty(), "{source:?} cleared something");
            let text = returned.to_string();
            assert!(text.contains("Nothing was cleared"), "{text}");
            assert!(
                text.contains("401"),
                "the 401 still reaches the user: {text}"
            );
        }
        assert!(
            clear_if_dead(dead(), TokenSource::Env, &path, &record)
                .to_string()
                .contains("AI_USAGEBAR_SYNC_TOKEN"),
            "the env arm names the variable that outranks every stored token"
        );

        // (c) The store that produced the rejected value, and only it.
        for source in [TokenSource::Keychain, TokenSource::File] {
            cleared.borrow_mut().clear();
            let returned = clear_if_dead(dead(), source, &path, &record);
            assert_eq!(
                cleared.borrow().as_slice(),
                &[(source, path.clone())],
                "{source:?}"
            );
            assert!(returned.to_string().contains("removed"), "{returned}");
        }

        // Nothing else clears, whatever arm it lands on.
        cleared.borrow_mut().clear();
        for keep in [
            AppError::Http {
                status: 403,
                body: "forbidden".into(),
            },
            AppError::Http {
                status: 404,
                body: "not found".into(),
            },
            AppError::Transport("connection reset".into()),
        ] {
            clear_if_dead(
                gate::FetchError {
                    error: keep,
                    token_rejected: false,
                },
                TokenSource::Keychain,
                &path,
                &record,
            );
        }
        assert!(
            cleared.borrow().is_empty(),
            "only a 401 clears — a 403 must keep a working token"
        );
    }

    /// The documented configuration F-1 destroyed a credential in, end to end:
    /// the value came from `AI_USAGEBAR_SYNC_TOKEN` (`chain()` sets it), so a
    /// 401 takes out neither the Keychain item nor the token file — both of
    /// which this run never sent — and says which variable to change.
    #[tokio::test]
    async fn a_401_on_an_environment_token_clears_nothing_and_names_the_variable() {
        let dir = TempDir::new().unwrap();
        let script = Script::new();
        let err = drive(&cfg_for(Some("o/n")), &dir, "{}", 401, &script)
            .await
            .expect_err("401");
        assert!(matches!(err, AppError::Credentials(_)), "{err:?}");
        let text = err.to_string();
        assert!(text.contains("That token is dead"), "{text}");
        assert!(text.contains("Nothing was cleared"), "{text}");
        assert!(text.contains("AI_USAGEBAR_SYNC_TOKEN"), "{text}");
        assert!(
            script.borrow().cleared.is_empty(),
            "the stored token was never sent and is not this 401's to delete"
        );
        assert!(
            script.borrow().stored.is_empty(),
            "a dead token is not stored"
        );
    }

    /// …and the other side of the same wiring: a token that *did* come from the
    /// file is cleared, at the injected path, and nowhere else.
    #[tokio::test]
    async fn a_401_on_a_file_token_clears_that_file_and_says_so() {
        let dir = TempDir::new().unwrap();
        let roots = roots_at(&dir);
        let token_file = token_path(&roots);
        std::fs::create_dir_all(token_file.parent().unwrap()).unwrap();
        fs::write(&token_file, format!("{FIXTURE}\n")).unwrap();

        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/repos/o/n")
            .with_status(401)
            .with_body("{}")
            .create_async()
            .await;

        let script = Script::new();
        let err = run(
            &cfg_for(Some("o/n")),
            &roots,
            &endpoints_at(&server.url()),
            &token::TokenChain {
                file_path: Some(token_file.clone()),
                ..token::TokenChain::default()
            },
            &mut Double(Rc::clone(&script)),
            now(),
        )
        .await
        .expect_err("401");

        assert_eq!(
            script.borrow().cleared.as_slice(),
            &[(TokenSource::File, token_file.clone())]
        );
        let text = err.to_string();
        assert!(text.contains(&token_file.display().to_string()), "{text}");
        assert!(text.contains("has been removed"), "{text}");
    }

    /// A 403 keeps a working token (T-3-16): the message says so, and the flow
    /// agrees with the message.
    #[tokio::test]
    async fn a_403_does_not_clear_the_token_the_message_told_the_user_to_keep() {
        let dir = TempDir::new().unwrap();
        let script = Script::new();
        let err = drive(&cfg_for(Some("o/n")), &dir, "{}", 403, &script)
            .await
            .expect_err("403");
        assert!(err.to_string().contains("keep it"), "{err}");
        assert!(script.borrow().cleared.is_empty(), "a 403 must not clear");
    }

    // ---- step 3: the passphrase and the keyfile --------------------------

    #[tokio::test]
    async fn the_keyfile_is_written_owner_only_inside_the_injected_directory() {
        let dir = TempDir::new().unwrap();
        let script = Script::new();
        let out = drive(&cfg_for(Some("o/n")), &dir, PRIVATE, 200, &script)
            .await
            .unwrap();

        assert!(out.keyfile.starts_with(dir.path()), "{:?}", out.keyfile);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&out.keyfile).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // Phase 1's no-recovery warning, shown once alongside the passphrase.
        let said = script.borrow().said.join("\n");
        assert!(said.contains("There is no recovery"), "{said}");
        assert_eq!(
            said.matches("There is no recovery").count(),
            1,
            "shown exactly once"
        );
        assert!(said.contains("guess at it as long as they like"), "{said}");
    }

    #[tokio::test]
    async fn a_passphrase_under_the_floor_is_refused_and_re_prompted_not_accepted() {
        let dir = TempDir::new().unwrap();
        let script = Script::new();
        script.borrow_mut().passphrases = vec![
            "short".into(),
            "also-too-short".into(),
            "a-long-enough-supplied-passphrase".into(),
        ];

        let out = drive(&cfg_for(Some("o/n")), &dir, PRIVATE, 200, &script)
            .await
            .unwrap();
        assert!(out.keyfile.exists());

        let reached = script.borrow().reached.clone();
        assert_eq!(
            reached.iter().filter(|r| *r == "passphrase").count(),
            3,
            "two refusals then an accepted one: {reached:?}"
        );
        let said = script.borrow().said.join("\n");
        assert!(said.contains("refused:"), "{said}");
    }

    /// T-3-38: an unrecoverable overwrite is refused, and refused *before* a
    /// new passphrase is generated.
    #[tokio::test]
    async fn an_existing_keyfile_stops_the_flow_rather_than_being_overwritten() {
        let dir = TempDir::new().unwrap();
        let existing = crate::sync::cli::keyfile_path(&roots_at(&dir));
        fs::create_dir_all(existing.parent().unwrap()).unwrap();
        fs::write(&existing, "{\"already\":\"here\"}").unwrap();

        let script = Script::new();
        let err = drive(&cfg_for(Some("o/n")), &dir, PRIVATE, 200, &script)
            .await
            .expect_err("a keyfile is already there");
        assert!(err.to_string().contains("will not overwrite"), "{err}");
        assert!(
            script.borrow().reached.is_empty(),
            "before any prompt at all — it is a local precondition, not a gate decision"
        );
        assert_eq!(
            fs::read_to_string(&existing).unwrap(),
            "{\"already\":\"here\"}"
        );
    }

    // ---- step 1: the categories ------------------------------------------

    #[tokio::test]
    async fn a_toggled_category_lands_in_the_injected_config_and_reads_back() {
        let dir = TempDir::new().unwrap();
        let config = dir.path().join("config.toml");
        fs::write(&config, "# keep me\n[sync]\ntranscript_days = 7\n").unwrap();

        let script = Script::new();
        script.borrow_mut().categories = Some(vec![SyncCategory::Config, SyncCategory::Routines]);

        let out = drive(&cfg_for(Some("o/n")), &dir, PRIVATE, 200, &script)
            .await
            .unwrap();
        assert_eq!(
            out.categories,
            vec![SyncCategory::Config, SyncCategory::Routines]
        );

        let written = fs::read_to_string(&config).unwrap();
        assert!(written.contains("# keep me"), "comments survive: {written}");
        assert!(written.contains("transcript_days = 7"), "{written}");

        let reloaded = Config::load_from(&config).unwrap();
        assert_eq!(
            reloaded.sync.categories,
            vec![SyncCategory::Config, SyncCategory::Routines]
        );
        assert!(!reloaded.sync.includes(SyncCategory::Credentials));
    }

    // ---- D-04's carve-out, through both gate calls ------------------------

    /// Credentials **off**: a public repository proceeds, with the warning.
    #[tokio::test]
    async fn a_public_repository_with_credentials_off_proceeds_with_the_warning() {
        let dir = TempDir::new().unwrap();
        let cfg = SyncConfig {
            repo: Some("o/n".into()),
            categories: vec![SyncCategory::Config],
            ..SyncConfig::default()
        };
        let script = Script::new();
        let out = drive(&cfg, &dir, PUBLIC, 200, &script).await.unwrap();

        assert!(!out.warnings.is_empty(), "the public warning");
        assert!(
            out.warnings.iter().any(|w| w.contains("public")),
            "{:?}",
            out.warnings
        );
        assert!(!script.borrow().reached.is_empty(), "the flow continued");
    }

    /// Credentials **on**: the same repository stops at the gate, having asked
    /// only the one question the gate needed answered.
    #[tokio::test]
    async fn a_public_repository_with_credentials_on_stops_before_the_passphrase() {
        let dir = TempDir::new().unwrap();
        let script = Script::new();
        let err = drive(&cfg_for(Some("o/n")), &dir, PUBLIC, 200, &script)
            .await
            .expect_err("credentials are in the bundle by default");
        assert!(err.to_string().contains("REFUSING TO PUSH"), "{err}");
        assert_eq!(script.borrow().reached, ["categories"]);
    }

    /// F-2, the composed path, exactly as it was reachable with default
    /// answers: config says `categories = ["config"]`, so the *old* order took
    /// D-04's carve-out and minted a clearance — and the user then added
    /// `credentials` at a prompt nothing re-gated. Setup stored the token, wrote
    /// the pairing record at `private: false`, and printed "ready to push".
    ///
    /// The gate is now asked the question the user actually answered.
    #[tokio::test]
    async fn adding_credentials_at_the_prompt_re_decides_the_public_repository() {
        let dir = TempDir::new().unwrap();
        let roots = roots_at(&dir);
        // The carve-out's own configuration, so a gate reading `cfg` rather than
        // the answer would clear this run.
        let cfg = SyncConfig {
            repo: Some("o/n".into()),
            categories: vec![SyncCategory::Config],
            ..SyncConfig::default()
        };
        let script = Script::new();
        script.borrow_mut().categories =
            Some(vec![SyncCategory::Config, SyncCategory::Credentials]);

        let err = drive(&cfg, &dir, PUBLIC, 200, &script)
            .await
            .expect_err("a public repository does not hold credentials");
        assert!(err.to_string().contains("REFUSING TO PUSH"), "{err}");
        assert!(err.to_string().contains("rotate"), "{err}");

        // …and none of the things that made it look cleared happened.
        assert!(script.borrow().stored.is_empty(), "no token was stored");
        assert!(!pairing::default_path(&roots).exists(), "no pairing record");
        assert!(
            !crate::sync::cli::keyfile_path(&roots).exists(),
            "no keyfile"
        );
        let said = script.borrow().said.join("\n");
        assert!(!said.contains("the gate passed"), "{said}");
    }

    /// The carve-out still lives, from the other direction: config carries
    /// `credentials`, the user turns it *off* at the prompt, and the same public
    /// repository proceeds with the warning. One evaluation, of the answer.
    #[tokio::test]
    async fn removing_credentials_at_the_prompt_re_decides_it_too() {
        let dir = TempDir::new().unwrap();
        let script = Script::new();
        script.borrow_mut().categories = Some(vec![SyncCategory::Config]);

        let out = drive(&cfg_for(Some("o/n")), &dir, PUBLIC, 200, &script)
            .await
            .expect("credentials are off, so public is a warning");
        assert_eq!(out.categories, vec![SyncCategory::Config]);
        assert!(out.warnings.iter().any(|w| w.contains("public")), "{out:?}");
    }

    // ---- pairing reuse, and the filesystem boundary -----------------------

    #[tokio::test]
    async fn a_second_run_reuses_the_pairing_rather_than_issuing_another() {
        let dir = TempDir::new().unwrap();
        let script = Script::new();
        let first = drive(&cfg_for(Some("o/n")), &dir, PRIVATE, 200, &script)
            .await
            .unwrap();
        assert!(!first.reused_pairing);

        // The keyfile now exists, so a second full run stops there — remove it
        // to exercise the pairing branch on its own.
        fs::remove_file(&first.keyfile).unwrap();
        let again = Script::new();
        let second = drive(&cfg_for(Some("o/n")), &dir, PRIVATE, 200, &again)
            .await
            .unwrap();
        assert!(second.reused_pairing);
        assert!(
            again
                .borrow()
                .said
                .iter()
                .any(|s| s.contains("already paired")),
            "{:?}",
            again.borrow().said
        );
    }

    /// Every path this flow writes resolves from the injected `SyncRoots`.
    #[tokio::test]
    async fn nothing_is_written_outside_the_injected_temp_directory() {
        let dir = TempDir::new().unwrap();
        let roots = roots_at(&dir);
        let script = Script::new();
        let out = drive(&cfg_for(Some("o/n")), &dir, PRIVATE, 200, &script)
            .await
            .unwrap();

        for path in [
            out.keyfile.clone(),
            pairing::default_path(&roots),
            token_path(&roots),
            roots.config_file.clone(),
            roots.index_file.clone(),
        ] {
            assert!(
                path.starts_with(dir.path()),
                "escaped the TempDir: {}",
                path.display()
            );
        }
        assert!(
            pairing::default_path(&roots).exists(),
            "the record persisted"
        );
        assert!(roots.index_file.exists(), "the index is inside too");
        assert_eq!(
            script.borrow().stored.as_slice(),
            std::slice::from_ref(&token_path(&roots)),
            "the token was stored at the injected path and nowhere else"
        );
    }

    /// D-05: declining the size confirmation leaves the remote untouched, and
    /// says so.
    #[tokio::test]
    async fn declining_the_size_confirmation_stops_without_pairing() {
        let dir = TempDir::new().unwrap();
        let script = Script::new();
        script.borrow_mut().confirm = false;

        let config = dir.path().join("config.toml");
        fs::write(&config, "[sync]\n").unwrap();
        let before = fs::read_to_string(&config).unwrap();
        script.borrow_mut().categories = Some(vec![SyncCategory::Config]);

        let err = drive(&cfg_for(Some("o/n")), &dir, PRIVATE, 200, &script)
            .await
            .expect_err("the user declined");
        assert!(
            err.to_string().contains("No bundle data was uploaded"),
            "{err}"
        );
        assert!(
            err.to_string().contains("the repository was not touched"),
            "this repository already had commits, so nothing was written to it: {err}"
        );
        assert!(!pairing::default_path(&roots_at(&dir)).exists());

        // F-10: the keyfile used to be written before this confirmation, and
        // then refused the re-run — stranding the user behind a passphrase they
        // were shown once, with no recovery by design. Nothing persists until
        // the last thing that can abort has passed.
        let keyfile = crate::sync::cli::keyfile_path(&roots_at(&dir));
        assert!(!keyfile.exists(), "a declined setup leaves no keyfile");
        assert_eq!(
            fs::read_to_string(&config).unwrap(),
            before,
            "and no config write-back either"
        );
        assert!(script.borrow().stored.is_empty(), "and no stored token");

        // …so the flow can simply be re-run, which is the whole point.
        let again = Script::new();
        drive(&cfg_for(Some("o/n")), &dir, PRIVATE, 200, &again)
            .await
            .expect("a declined setup is re-runnable");
    }

    // ---- 6-07: the second machine ----------------------------------------

    /// The gap, and the reason it made a second machine read-only: setup always
    /// called `Keyfile::create`, so the local keyfile's content address never
    /// matched `Pointer.keyfile`. Pull worked (it opens the keyfile the pointer
    /// names), push was refused as a superseded wrapper.
    ///
    /// Byte-identity is the assertion rather than "opens with the same
    /// password", because it is byte-identity that
    /// `upload::assert_keyfile_is_current` compares: two keyfiles wrapping the
    /// same master key under the same password are still two different assets.
    #[tokio::test]
    async fn a_repository_that_already_holds_a_bundle_is_joined_not_re_keyed() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("config.toml"), "[sync]\n").unwrap();
        let (asset, name) = published_keyfile("the first machine's password");
        let script = Script::new();
        script.borrow_mut().existing = vec!["the first machine's password".into()];

        let out = drive_joining(&dir, &name, asset.clone(), &script)
            .await
            .unwrap();

        assert_eq!(
            fs::read(&out.keyfile).unwrap(),
            asset,
            "the adopted keyfile is the published asset, byte for byte"
        );
        let reached = script.borrow().reached.clone();
        assert_eq!(reached[0], "categories", "{reached:?}");
        assert_eq!(reached[1], "existing_passphrase", "{reached:?}");
        assert!(reached[2].starts_with("confirm:"), "{reached:?}");
        assert!(
            !reached.iter().any(|r| r == "passphrase"),
            "the generate path's ask has no answer here: {reached:?}"
        );
        let said = script.borrow().said.join("\n");
        assert!(
            !said.contains("generated passphrase:"),
            "nothing was generated, so nothing was shown as generated: {said}"
        );
        assert!(said.contains("already holds a bundle"), "{said}");
        assert!(!said.contains("the first machine's password"), "{said}");
    }

    /// A wrong password must leave **no** keyfile: `existing_keyfile_message`
    /// then refuses the re-run, which would strand the user behind a file that
    /// opens with a password nobody has.
    #[tokio::test]
    async fn a_wrong_passphrase_on_join_refuses_and_writes_nothing() {
        let dir = TempDir::new().unwrap();
        let roots = roots_at(&dir);
        let (asset, name) = published_keyfile("the first machine's password");
        let script = Script::new();
        script.borrow_mut().existing = vec!["wrong".into(), "also wrong".into(), "still".into()];

        let err = drive_joining(&dir, &name, asset, &script)
            .await
            .expect_err("that password opens nothing");

        let text = err.to_string();
        assert!(text.contains("3 attempts"), "{text}");
        assert!(text.contains("Nothing was written"), "{text}");
        assert!(
            !text.contains("wrong"),
            "the attempts are not echoed: {text}"
        );
        assert_eq!(
            script
                .borrow()
                .reached
                .iter()
                .filter(|r| *r == "existing_passphrase")
                .count(),
            3,
            "capped, so a script cannot spin: {:?}",
            script.borrow().reached
        );
        assert!(
            !crate::sync::cli::keyfile_path(&roots).exists(),
            "a wrong password leaves no keyfile to refuse the re-run"
        );
        assert!(!pairing::default_path(&roots).exists(), "no pairing record");
        assert!(script.borrow().stored.is_empty(), "no stored token");
    }

    /// The asset is remote-chosen bytes. Unreadable ones are refused before the
    /// user is asked for anything — there is nothing a password could do.
    #[tokio::test]
    async fn a_corrupted_published_keyfile_is_refused_before_the_password_ask() {
        let dir = TempDir::new().unwrap();
        let script = Script::new();
        let err = drive_joining(
            &dir,
            "sync-keyfile-deadbeef.json",
            b"not a keyfile".to_vec(),
            &script,
        )
        .await
        .expect_err("that asset is not a keyfile");

        assert!(
            err.to_string().contains("not a readable sync keyfile"),
            "{err}"
        );
        assert!(
            !script
                .borrow()
                .reached
                .iter()
                .any(|r| r == "existing_passphrase"),
            "{:?}",
            script.borrow().reached
        );
        assert!(!crate::sync::cli::keyfile_path(&roots_at(&dir)).exists());
    }

    /// The first machine is untouched: nothing published, so step 3 generates
    /// exactly as it did before, and shows the passphrase once.
    #[tokio::test]
    async fn an_empty_repository_still_generates_and_shows_the_passphrase_once() {
        let dir = TempDir::new().unwrap();
        let script = Script::new();
        let out = drive(&cfg_for(Some("o/n")), &dir, PRIVATE, 200, &script)
            .await
            .unwrap();

        assert!(out.keyfile.exists());
        let said = script.borrow().said.join("\n");
        assert_eq!(
            said.matches("generated passphrase:").count(),
            1,
            "shown exactly once: {said}"
        );
        assert!(
            !script
                .borrow()
                .reached
                .iter()
                .any(|r| r == "existing_passphrase"),
            "there is no existing passphrase to ask for"
        );
    }

    // ---- 6-11: the repository that has no commits yet --------------------

    /// A private repository with **no commits at all** — what
    /// `gh repo create --private` leaves behind — plus the one write the offer
    /// can make.
    ///
    /// Every mock is returned because dropping one un-registers it; `readme` is
    /// handed back separately so a test can ask whether it was hit.
    async fn empty_repo(server: &mut mockito::ServerGuard) -> (Vec<mockito::Mock>, mockito::Mock) {
        let repo = server
            .mock("GET", "/repos/o/n")
            .with_status(200)
            .with_body(PRIVATE)
            .create_async()
            .await;
        let commits = no_commits(server).await;
        let pointer = no_pointer(server).await;
        let readme = server
            .mock("PUT", "/repos/o/n/contents/README.md")
            .with_status(201)
            .with_body(r#"{"content":{"sha":"a-blob-sha"}}"#)
            .create_async()
            .await;
        (vec![repo, commits, pointer], readme)
    }

    /// The fix. Before it, setup paired a machine against a repository GitHub
    /// would refuse to tag, and the refusal arrived several minutes into the
    /// first push as a bare 422 with a command to paste.
    #[tokio::test]
    async fn an_empty_repository_is_offered_a_first_commit_and_a_yes_writes_one() {
        let dir = TempDir::new().unwrap();
        let mut server = mockito::Server::new_async().await;
        let (_keep, readme) = empty_repo(&mut server).await;

        let script = Script::new();
        let out = run(
            &cfg_for(Some("o/n")),
            &roots_at(&dir),
            &endpoints_at(&server.url()),
            &chain(),
            &mut Double(Rc::clone(&script)),
            now(),
        )
        .await
        .unwrap();

        readme.assert_async().await;
        assert!(out.initialised, "the outcome must report the one write");

        // Asked, never assumed — it is the user's repository.
        let reached = script.borrow().reached.clone();
        assert!(
            reached.iter().any(|r| r.contains("Add a README.md")),
            "{reached:?}"
        );
        let said = script.borrow().said.join("\n");
        assert!(said.contains("has no commits yet"), "{said}");
        assert!(said.contains("README.md written"), "{said}");
    }

    /// **The ordering the whole offer hangs on.** Step 2 is the private-repo
    /// gate and stays the first thing that touches the repository: a public one
    /// is refused before anything asks about a commit, let alone writes one.
    #[tokio::test]
    async fn a_public_repository_is_refused_before_the_offer_is_ever_reached() {
        let dir = TempDir::new().unwrap();
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/repos/o/n")
            .with_status(200)
            .with_body(PUBLIC)
            .create_async()
            .await;
        let commits = no_commits(&mut server).await;
        let readme = server
            .mock("PUT", "/repos/o/n/contents/README.md")
            .with_status(201)
            .with_body(r#"{"content":{"sha":"a-blob-sha"}}"#)
            .create_async()
            .await;

        let script = Script::new();
        run(
            &cfg_for(Some("o/n")),
            &roots_at(&dir),
            &endpoints_at(&server.url()),
            &chain(),
            &mut Double(Rc::clone(&script)),
            now(),
        )
        .await
        .expect_err("a public repository is refused");

        assert!(
            !commits.matched_async().await,
            "emptiness was probed before the gate refused"
        );
        assert!(
            !readme.matched_async().await,
            "a byte was written to a repository the gate had not cleared"
        );
        assert!(
            !script.borrow().reached.iter().any(|r| r.contains("README")),
            "the offer was made about a repository that was about to be refused"
        );
    }

    /// A decline leaves the repository exactly as it was and hands back the
    /// command — the same one the push path prints, from the same function.
    /// Setup then carries on: the pairing, the password and the scope are all
    /// still valid without the commit.
    #[tokio::test]
    async fn declining_the_offer_leaves_the_repository_untouched_and_names_the_command() {
        let dir = TempDir::new().unwrap();
        let mut server = mockito::Server::new_async().await;
        let (_keep, readme) = empty_repo(&mut server).await;

        let script = Script::new();
        script.borrow_mut().decline_matching = Some("README".into());

        let out = run(
            &cfg_for(Some("o/n")),
            &roots_at(&dir),
            &endpoints_at(&server.url()),
            &chain(),
            &mut Double(Rc::clone(&script)),
            now(),
        )
        .await
        .expect("a declined offer is not a failed setup");

        assert!(!readme.matched_async().await, "the decline wrote something");
        assert!(!out.initialised);

        let said = script.borrow().said.join("\n");
        assert!(said.contains("left untouched"), "{said}");
        assert!(
            said.contains(&write::first_commit_command(&out.repo)),
            "the decline must print the command the push path prints: {said}"
        );
    }

    /// The size confirmation still reports the truth about the remote, and the
    /// truth changed: an approved README is in the repository, and saying
    /// "the repository was not touched" over it would be a lie.
    #[tokio::test]
    async fn declining_the_size_after_approving_the_readme_says_what_is_there() {
        let dir = TempDir::new().unwrap();
        let mut server = mockito::Server::new_async().await;
        let (_keep, readme) = empty_repo(&mut server).await;

        let script = Script::new();
        script.borrow_mut().decline_matching = Some("Pair this machine".into());

        let err = run(
            &cfg_for(Some("o/n")),
            &roots_at(&dir),
            &endpoints_at(&server.url()),
            &chain(),
            &mut Double(Rc::clone(&script)),
            now(),
        )
        .await
        .expect_err("the user declined the scope");

        readme.assert_async().await;
        let text = err.to_string();
        assert!(text.contains("No bundle data was uploaded"), "{text}");
        assert!(
            text.contains("the README you approved is the only thing in the repository"),
            "{text}"
        );
        assert!(!text.contains("the repository was not touched"), "{text}");

        // F-10 is untouched: a README does not strand a re-run the way a
        // keyfile did, and nothing local was written.
        assert!(!crate::sync::cli::keyfile_path(&roots_at(&dir)).exists());
        assert!(!pairing::default_path(&roots_at(&dir)).exists());
        assert!(script.borrow().stored.is_empty());
    }
}
