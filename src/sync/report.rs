//! The pure `sync status` / `sync push --dry-run` model and its renderers.
//! Owned by plan 2-01, extended with D4's dry-run by plan 2-07.
//!
//! Same split the rest of the project uses: [`build_status`] touches the
//! filesystem, [`render_status`] and [`render_dry_run`] are pure functions of
//! the model, so the wording is testable without a disk.
//!
//! Both commands print **one** table, because they answer the same question at
//! different moments. The third column — what a push would actually put on the
//! wire — is the one D4 says matters, and it is present only when a sync key
//! was available: file counts and raw bytes need no key, and a user checking
//! what is in scope should not have to authenticate. Where the key is missing
//! the column is absent and the reason is printed, never a zero.

use std::path::PathBuf;

use chrono::{DateTime, Utc};

use crate::config::{SyncCategory, SyncConfig};
use crate::sync::index::Index;
use crate::sync::plan::{CategoryPlan, SyncPlan};
use crate::sync::scope;
use crate::sync::{SyncRoots, scope::CategoryScan};

/// The one thing `sync status` can say about the index that is not a count.
///
/// A fixed vocabulary rather than free text, and deliberately carries no `{}`:
/// [`StatusReport::warnings`] is serialized into a machine-readable document
/// that promises to carry counts, labels and paths only, and a format string
/// with a caller-supplied hole in it is how a file's bytes would reach it
/// (T-6-01). A new warning is a new constant, added to [`WARNINGS`] too.
pub const WARN_INDEX_UNAVAILABLE: &str =
    "the local index is unavailable, so last-sync and pending changes are unknown";

/// The machine-bound half of `credentials` could not be counted.
///
/// A **third** answer, and it exists because the other two would both be
/// wrong: a locked Keychain is not "no credential here", and a count that
/// silently omitted it is the under-report this warning was added to end.
pub const WARN_KEYSTORE_UNAVAILABLE: &str = "the machine-bound credential store could not be read, so the credentials \
     count may be one short of what a push would carry";

/// Every string that may appear in [`StatusReport::warnings`].
pub const WARNINGS: [&str; 2] = [WARN_INDEX_UNAVAILABLE, WARN_KEYSTORE_UNAVAILABLE];

/// What a push would have to look at, counted without opening anything.
///
/// Files the local index does not vouch for, and the local bytes attributable
/// to them. Not a promise about what would go on the wire — that is the
/// dry-run's third column, and it costs a key and a read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PendingSummary {
    pub files: usize,
    pub bytes: u64,
}

/// One category's row in `sync status`.
#[derive(Debug, Clone)]
pub struct CategoryLine {
    pub category: SyncCategory,
    pub enabled: bool,
    pub files: usize,
    pub bytes: u64,
    /// True when the walk hit its entry cap — the counts are a floor, not a
    /// total, and saying so beats quietly under-reporting.
    pub capped: bool,
}

/// What `sync status` learned about the remote (plan 3-07).
///
/// **Best-effort with respect to the category lines, not with respect to the
/// exit code.** A user whose token expired should still see what would be sent,
/// so [`failure`](RepoSection::failure) suppresses nothing above it — but a
/// non-empty `failure` is a non-zero exit (D-06, REPO-05, T-3-41).
///
/// The token appears here only as a **source label**. There is no field that
/// could hold its value, which is a cheaper guarantee than remembering not to
/// print one (REPO-02).
#[derive(Debug, Clone, Default)]
pub struct RepoSection {
    /// `owner/name` from `[sync] repo`; `None` means the key is unset, which is
    /// an unconfigured machine rather than a failure.
    pub configured: Option<String>,
    /// What GitHub reported this run.
    pub visibility: Option<String>,
    /// [`TokenSource::label`](crate::sync::github::token::TokenSource::label).
    pub token_source: Option<&'static str>,
    /// The pairing record's `checked_at` — when this machine last verified it.
    pub last_verified: Option<DateTime<Utc>>,
    /// Drift and gate warnings, verbatim from plans 3-04 and 3-03.
    pub warnings: Vec<String>,
    /// Why the section could not be filled, or the incident that filled it.
    /// Verbatim; a status-flavoured paraphrase would make one event read as
    /// two.
    pub failure: Option<String>,
}

impl RepoSection {
    /// Record why this section could not be filled and stop. Returns `self` so
    /// a caller can `return section.failed(e.to_string())` on one line — the
    /// fields gathered before the failure stay visible, which is the point of
    /// the section being best-effort about its *contents*.
    pub fn failed(mut self, why: String) -> RepoSection {
        self.failure = Some(why);
        self
    }
}

/// Everything `sync status` prints.
#[derive(Debug, Clone)]
pub struct StatusReport {
    pub lines: Vec<CategoryLine>,
    pub last_sync: Option<DateTime<Utc>>,
    /// Empty when the index could not be opened — the caller has already said
    /// why, and the status is still worth printing without it.
    pub index_path: PathBuf,
    /// What a push would send right now. `None` before a plan was ever built,
    /// or when no sync key was available to build one.
    pub plan: Option<SyncPlan>,
    /// The repository half. `None` only for `push --dry-run`, which contacts no
    /// network at all.
    pub repo: Option<RepoSection>,
    /// What is not backed up yet. `None` means the index was not available,
    /// which is a **third** state and must never be flattened into "nothing
    /// pending" — a backup nobody can tell is stale is what D-04 exists to
    /// prevent.
    pub pending: Option<PendingSummary>,
    /// Drawn from [`WARNINGS`]. Only the JSON rendering reads it; the text
    /// rendering has already said the same thing on stderr, and a consumer that
    /// never saw it would draw "last sync: never" for a machine that syncs
    /// hourly.
    pub warnings: Vec<String>,
}

impl StatusReport {
    /// Enabled categories only. A disabled one contributes no files by
    /// construction, and summing it anyway would make that an accident.
    pub fn total_files(&self) -> usize {
        self.lines
            .iter()
            .filter(|l| l.enabled)
            .map(|l| l.files)
            .sum()
    }

    pub fn total_bytes(&self) -> u64 {
        self.lines
            .iter()
            .filter(|l| l.enabled)
            .map(|l| l.bytes)
            .sum()
    }

    fn category(&self, cat: SyncCategory) -> Option<&CategoryPlan> {
        self.plan
            .as_ref()?
            .categories
            .iter()
            .find(|c| c.category == cat)
    }
}

/// Everything `sync push --dry-run` prints.
///
/// A thin wrapper over [`StatusReport`] rather than a parallel model: the
/// dry-run and the status show the same figures, and the only thing the dry-run
/// adds is what to say when the would-upload column could not be computed.
#[derive(Debug, Clone)]
pub struct DryRunReport {
    pub status: StatusReport,
    /// Why `status.plan` is `None`, in the user's words. Rendered in place of
    /// the column — a wrong zero would read as "a push is free".
    pub no_key: Option<String>,
}

/// Scan every category in D1 order. `now` goes straight through to
/// [`scope::collect`] so the transcripts bounds have a reference point that no
/// test has to fake by moving the clock.
///
/// With a `plan`, the rows come from the walk the planner already did: a second
/// scan would cost another walk of the same tree and could disagree with the
/// first if a file appeared between them.
pub fn build_status(
    roots: &SyncRoots,
    cfg: &SyncConfig,
    index: Option<&Index>,
    now: DateTime<Utc>,
    plan: Option<SyncPlan>,
    repo: Option<RepoSection>,
) -> StatusReport {
    // The index is the only thing this builder can fail to have on the planned
    // path, so it is the only warning that path can raise — and raising it here
    // rather than at each call site is what keeps `pending: None` and the
    // explanation for it from ever disagreeing.
    let mut warnings = match index {
        Some(_) => Vec::new(),
        None => vec![WARN_INDEX_UNAVAILABLE.to_string()],
    };
    // `pending` is `None` whenever the index is, in both arms: without one
    // there is nothing to compare against, and a confident zero would be a lie.
    let (lines, pending) = match &plan {
        Some(p) => (
            p.categories.iter().map(|c| line_of(c, cfg)).collect(),
            index.map(|_| pending_of_plan(p)),
        ),
        None => {
            // Collected once and reused: a second walk of the same tree costs
            // as much as the first and could disagree with it.
            let scans: Vec<CategoryScan> = SyncCategory::ALL
                .iter()
                .map(|&category| scope::collect(category, roots, cfg, now))
                .collect();
            let pending = index.map(|i| pending_of_scans(&scans, i));
            let mut lines: Vec<CategoryLine> =
                scans.into_iter().map(|scan| line(scan, cfg)).collect();
            // The half a walk cannot see. Only on this path: a plan has already
            // counted the stores itself (`plan::build`'s third pass), and
            // adding them again here would double them.
            match count_stores(roots, cfg) {
                Ok(stores) => add_stores(&mut lines, stores),
                Err(_) => warnings.push(WARN_KEYSTORE_UNAVAILABLE.to_string()),
            }
            (lines, pending)
        }
    };
    StatusReport {
        lines,
        last_sync: index.and_then(Index::last_sync),
        // Taken from the opened index rather than re-resolved, so nothing in
        // this builder touches `$HOME`.
        index_path: index.map(|i| i.path().to_path_buf()).unwrap_or_default(),
        plan,
        repo,
        pending,
        warnings,
    }
}

/// How many machine-bound credential stores this machine holds — the half of
/// `credentials` that is not a file, and so is invisible to [`scope::collect`].
///
/// **Existence only, never a value.** `sync status --json` is what the macOS
/// menu bar runs on every menu open, so this path may not ask for a password,
/// open a network connection, or read a file body;
/// [`Stores::has`](crate::sync::keystore::Stores::has) does none of the three.
///
/// The `cfg.includes` guard mirrors the planner's third pass exactly: with
/// `credentials` switched off, `scope::collect` returns no files and the
/// planner reads no store, so this must contribute nothing either.
///
/// `Err` is "this machine could not say", which the caller turns into
/// [`WARN_KEYSTORE_UNAVAILABLE`]. Never a zero: an unreadable store reported as
/// absent is the under-count this whole function exists to fix.
fn count_stores(roots: &SyncRoots, cfg: &SyncConfig) -> crate::error::Result<usize> {
    if !cfg.includes(SyncCategory::Credentials) {
        return Ok(0);
    }
    let mut held = 0usize;
    for store in roots.stores.all()? {
        if roots.stores.has(&store)? {
            held += 1;
        }
    }
    Ok(held)
}

/// Add the stores to the row they belong to, which is `credentials` and only
/// `credentials` — the same category the planner attributes them to.
///
/// ponytail: the count moves and the byte total does not. A store's size
/// is its value's length, and reading a value is the thing this path may not
/// do; the shortfall is a few hundred bytes against a total rendered in
/// megabytes. If a store ever holds something big enough to see, the size has
/// to travel out of the planner rather than be measured here.
fn add_stores(lines: &mut [CategoryLine], stores: usize) {
    if let Some(line) = lines
        .iter_mut()
        .find(|l| l.category == SyncCategory::Credentials)
    {
        line.files += stores;
    }
}

/// **Never opens a file body.** `sync status` is advertised as costing a stat
/// sweep, and a status call that hashed a 50 MB transcript to draw a menu row
/// would break that promise silently — so this asks the same metadata-only
/// question the planner's short-circuit asks, and nothing more.
fn pending_of_scans(scans: &[CategoryScan], index: &Index) -> PendingSummary {
    let mut summary = PendingSummary::default();
    for entry in scans.iter().flat_map(|scan| &scan.files) {
        if index.lookup(entry).is_none() {
            summary.files += 1;
            summary.bytes += entry.size;
        }
    }
    summary
}

/// The same count, read off a plan the caller already built rather than by
/// re-asking the index: [`FilePlan::reused`](crate::sync::plan::FilePlan)
/// records that exact short-circuit hitting.
fn pending_of_plan(plan: &SyncPlan) -> PendingSummary {
    plan.file_plans.iter().filter(|f| !f.reused).fold(
        PendingSummary::default(),
        |mut summary, f| {
            summary.files += 1;
            summary.bytes += f.new_bytes;
            summary
        },
    )
}

/// The machine-readable rendering — the macOS menu bar's whole read of sync.
///
/// Pure, and derived from the same [`StatusReport`] [`render_status`] draws, so
/// the two cannot disagree. **Every key is present on every run**, so a
/// consumer never has to tell "absent" from "null", and the object stays open:
/// a reader that ignores unknown keys is what lets a later phase add one.
///
/// Carries counts, byte totals, category labels, the index path, and warnings
/// from [`WARNINGS`]. There is no key here that could hold a file's contents,
/// which is a cheaper guarantee than remembering not to add one (T-6-01).
pub fn status_json(report: &StatusReport) -> serde_json::Value {
    let categories: Vec<serde_json::Value> = report
        .lines
        .iter()
        .map(|l| {
            serde_json::json!({
                "category": l.category.label(),
                "enabled": l.enabled,
                "files": l.files,
                "bytes": l.bytes,
                "capped": l.capped,
            })
        })
        .collect();

    serde_json::json!({
        // Null, never the string "never": the wording belongs to whoever draws
        // the row, and this document is read by more than one surface.
        "last_sync": report.last_sync.map(|t| t.to_rfc3339()),
        "pending": report.pending.map(|p| p.files > 0),
        "pending_files": report.pending.map(|p| p.files),
        "pending_bytes": report.pending.map(|p| p.bytes),
        "categories": categories,
        "total_files": report.total_files(),
        "total_bytes": report.total_bytes(),
        "index": (!report.index_path.as_os_str().is_empty())
            .then(|| report.index_path.display().to_string()),
        "warnings": report.warnings,
    })
}

fn line(scan: CategoryScan, cfg: &SyncConfig) -> CategoryLine {
    CategoryLine {
        category: scan.category,
        enabled: cfg.includes(scan.category),
        files: scan.files.len(),
        bytes: scan.bytes,
        capped: scan.walk_capped,
    }
}

fn line_of(c: &CategoryPlan, cfg: &SyncConfig) -> CategoryLine {
    CategoryLine {
        category: c.category,
        enabled: cfg.includes(c.category),
        files: c.files,
        bytes: c.raw_bytes,
        capped: c.capped,
    }
}

// ---- styling ---------------------------------------------------------------
//
// Colour, without a crate. `widget::pretty` already writes `"\x1b[2m"` by hand
// and this milestone's discipline is zero new dependencies, so this is the same
// four escapes gathered behind one value instead of scattered through the
// renderers.

/// The terminal width every styled line is laid out against.
///
/// A constant rather than a query: asking the real terminal is an ambient read
/// this module must not make (see [`Style`]), it needs a crate or an ioctl, and
/// 80 is the width that is safe everywhere. A wider terminal renders a narrower
/// table, which is correct-looking; a narrower one is the case that wrapped
/// mid-word, and that is what this fixes.
const WIDTH: usize = 80;

/// Whether this rendering may use ANSI, and — since there is only one palette —
/// which colours it uses when it may.
///
/// **[`Style::PLAIN`] renders byte-for-byte what this module rendered before
/// colour existed**, and that is load-bearing rather than a courtesy: a pipe, a
/// log file and the macOS menu bar all take that path, `sync status --json` is
/// parsed by the Node contract suites, and every existing test in this crate
/// asserts against it. Colour, computed column widths and wrapping are all
/// styled-only for the same reason — a consumer that is not a human
/// does not want its columns to move when a number gets wider.
///
/// Restraint is the rule: [`dim`](Style::dim) for context, one accent for a
/// heading, [`bad`](Style::bad) only for a refusal, [`good`](Style::good) only
/// for something that actually succeeded, and [`bold`](Style::bold) for the
/// numbers a reader has to decide on. Someone about to hand their credentials
/// to a remote should find the screen calm, not decorated.
///
/// **Do not nest one styled string inside another.** Every helper closes with a
/// full reset, so an inner reset would end an outer attribute early. Style the
/// leaf fragments; assemble afterwards.
///
/// The colour *decision* — a tty, and `NO_COLOR` unset — is made by
/// [`crate::display::color_enabled`] and injected. It cannot be made here:
/// `src/sync/`'s structural guard forbids reading the process environment
/// anywhere in this subtree.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Style {
    on: bool,
}

impl Style {
    /// No escapes at all — today's bytes, exactly. The default, so a type that
    /// carries a `Style` and is never told one renders plain.
    pub const PLAIN: Style = Style { on: false };

    /// From the injected decision. There is deliberately no `COLOR` constant
    /// beside `PLAIN`: nothing in production would ever name it, and this
    /// milestone has shipped enough tested code with no call site.
    pub fn color(on: bool) -> Style {
        Style { on }
    }

    /// True when this rendering emits escapes — what the layout branches on.
    pub fn is_on(self) -> bool {
        self.on
    }

    /// `text` wrapped in an SGR sequence, and **`text` can never close it.**
    ///
    /// A manifest path, a GitHub error message and an asset name all come from
    /// a hostile remote and all reach these renderers. Styling text that still
    /// holds an `ESC` would let it terminate the sequence and start its own —
    /// so the payload goes through the crate's shared sanitizer first, which is
    /// what strips `ESC`, the other C0/C1 controls and the bidi overrides. It
    /// is applied here, once, rather than at each call site that remembered.
    ///
    /// Plain returns the argument untouched, which is what makes "the plain
    /// path is byte-identical" true by construction rather than by review.
    fn sgr(self, code: &str, text: &str) -> String {
        if !self.on {
            return text.to_owned();
        }
        format!(
            "\x1b[{code}m{}\x1b[0m",
            crate::display::sanitize_untrusted_field(text)
        )
    }

    /// Secondary text: labels, units, the figures that are context.
    pub fn dim(self, text: &str) -> String {
        self.sgr("2", text)
    }

    /// A number that decides something — bytes about to be sent, files at risk.
    pub fn bold(self, text: &str) -> String {
        self.sgr("1", text)
    }

    /// The one accent, and only for a heading.
    pub fn head(self, text: &str) -> String {
        self.sgr("1;36", text)
    }

    /// A refusal. Nothing else is red.
    pub fn bad(self, text: &str) -> String {
        self.sgr("31", text)
    }

    /// Something that actually succeeded. Nothing else is green.
    pub fn good(self, text: &str) -> String {
        self.sgr("32", text)
    }
}

/// Break `text` so no line exceeds [`WIDTH`], continuing at `indent` spaces.
///
/// **Only ever at a space.** `2141 files 1.7 GiB left out by the age and size
/// bounds` is 82 columns, and what the user actually saw was the terminal
/// cutting it in the middle of a word. A word longer than the budget goes on a
/// line of its own and overruns, which is visible and honest where a mid-word
/// cut is neither.
///
/// Applied to the **unstyled** text, always: an escape sequence occupies bytes
/// and no columns, so wrapping after styling measures the wrong thing.
/// [`wrap`], but only when styled.
///
/// A pipe, a log file and the macOS menu bar read the plain path and must keep
/// receiving the bytes they always did; a human at 80 columns is the one who
/// watched a line break in the middle of a word.
pub(crate) fn reflow(style: Style, text: &str, indent: usize) -> String {
    if style.is_on() {
        wrap(text, indent)
    } else {
        text.to_owned()
    }
}

fn wrap(text: &str, indent: usize) -> String {
    if text.len() <= WIDTH && !text.contains('\n') {
        return text.to_owned();
    }
    let pad = " ".repeat(indent);
    let mut out = String::with_capacity(text.len() + 8);
    for (n, para) in text.lines().enumerate() {
        if n > 0 {
            out.push('\n');
        }
        let mut column = 0usize;
        for (i, word) in para.split(' ').enumerate() {
            let width = word.chars().count();
            if i == 0 {
                out.push_str(word);
                column = width;
            } else if column + 1 + width > WIDTH {
                out.push('\n');
                out.push_str(&pad);
                out.push_str(word);
                column = indent + width;
            } else {
                out.push(' ');
                out.push_str(word);
                column += 1 + width;
            }
        }
    }
    out
}

/// Pure. Given the same struct it always renders the same string.
pub fn render_status(report: &StatusReport) -> String {
    render_status_styled(report, Style::PLAIN)
}

/// [`render_status`] with a palette. Pure in both arms.
pub fn render_status_styled(report: &StatusReport, style: Style) -> String {
    let mut out = table(report, style);
    out.push_str(&field(
        style,
        "last sync",
        &report
            .last_sync
            .map_or_else(|| "never".to_string(), |t| t.to_rfc3339()),
        true,
    ));
    if report.index_path.as_os_str().is_empty() {
        out.push_str(&field(style, "index", "unavailable", false));
    } else {
        out.push_str(&field(
            style,
            "index",
            &report.index_path.display().to_string(),
            false,
        ));
    }
    out.push_str(&rebuilt_note(report, style));
    if let Some(repo) = &report.repo {
        out.push_str(&render_repo(repo, style));
    }
    out
}

/// `  label:     value`, with the label dimmed — it is the thing the eye skips
/// once it knows the shape, and the value is what it came for.
///
/// The 11-column label field is the alignment `last sync:`, `index:`, `repo:`,
/// `visible:`, `token:` and `verified:` have always shared, restated once
/// instead of six times in six format strings. Only the weight is new.
fn field(style: Style, label: &str, value: &str, first: bool) -> String {
    format!(
        "{}  {}{}\n",
        if first { "\n" } else { "" },
        style.dim(&format!("{:<11}", format!("{label}:"))),
        value
    )
}

/// The repository half of `sync status`. Pure, like everything else here.
///
/// Prints the token's **source**, never the token — [`RepoSection`] has no
/// field that could carry one.
fn render_repo(repo: &RepoSection, style: Style) -> String {
    let Some(name) = &repo.configured else {
        return format!(
            "\n  {}{}\n\
             \x20            Name one in config.toml, after creating it yourself:\n\
             \x20              gh repo create <owner>/<name> --private\n\
             \n\
             \x20              [sync]\n\
             \x20              repo = \"<owner>/<name>\"\n",
            style.dim(&format!("{:<11}", "repo:")),
            style.bad("not configured — no sync repository is paired.")
        );
    };

    let mut out = field(style, "repo", name, true);
    // A private repository is the gate's whole precondition, so it is the one
    // thing on this screen that has genuinely *succeeded*. Anything else here
    // is a repository this tool will refuse to push to.
    let visible = repo.visibility.as_deref().unwrap_or("unknown");
    out.push_str(&field(
        style,
        "visible",
        &if visible == "private" {
            style.good(visible)
        } else {
            style.bad(visible)
        },
        false,
    ));
    out.push_str(&match repo.token_source {
        Some(source) => field(style, "token", &format!("present ({source})"), false),
        None => field(style, "token", "none found", false),
    });
    out.push_str(&match repo.last_verified {
        Some(at) => field(style, "verified", &at.to_rfc3339(), false),
        None => field(
            style,
            "verified",
            "never — this machine is not paired yet, run `ai-usagebar sync setup`",
            false,
        ),
    });
    for warning in &repo.warnings {
        out.push_str(&note("warning", warning, style, Weight::Warn));
    }
    if let Some(failure) = &repo.failure {
        out.push_str(&note("repo:      FAILED", failure, style, Weight::Bad));
    }
    out
}

/// D4's dry-run: the same table, then the totals, then what a push would
/// actually do. Pure — it takes the model and returns a string, and touches no
/// filesystem.
pub fn render_dry_run(report: &DryRunReport) -> String {
    render_dry_run_styled(report, Style::PLAIN)
}

/// [`render_dry_run`] with a palette. Pure in both arms.
pub fn render_dry_run_styled(report: &DryRunReport, style: Style) -> String {
    let status = &report.status;
    let mut out = table(status, style);

    out.push_str(&format!(
        "\n  {} {} files, {} of local state\n",
        style.dim("snapshot:"),
        style.bold(&status.total_files().to_string()),
        style.bold(&human_bytes(status.total_bytes()))
    ));

    match (&status.plan, &report.no_key) {
        (Some(plan), _) => {
            if plan.is_empty() {
                out.push_str(&sentence(
                    style,
                    &format!(
                        "a push would send nothing — every file matched the local index, \
                         {} opened",
                        plan.files_opened
                    ),
                ));
            } else {
                // The one figure this whole command exists to produce, so it is
                // the one thing on the line carrying weight.
                let sending = human_bytes(plan.total_new_stored_bytes);
                out.push_str(&sentence_with(
                    style,
                    &format!(
                        "a push would send {sending} in {} new chunks ({} of plaintext, \
                         from {} file{} read)",
                        plan.new_chunk_ids.len(),
                        human_bytes(plan.total_new_bytes),
                        plan.files_opened,
                        if plan.files_opened == 1 { "" } else { "s" },
                    ),
                    &sending,
                ));
            }
            if plan.append_check_miss_bytes > 0 {
                out.push_str(&sentence(
                    style,
                    &format!(
                        "{} was re-read by append checks that then failed",
                        human_bytes(plan.append_check_miss_bytes)
                    ),
                ));
            }
        }
        // Never a zero here: "0 bytes" and "not computed" are opposite answers
        // to "what will this cost me".
        (None, Some(why)) => out.push_str(&note(
            "would upload: not computed",
            why,
            style,
            Weight::Warn,
        )),
        (None, None) => out.push_str(&note(
            "would upload: not computed",
            "no plan was built",
            style,
            Weight::Warn,
        )),
    }

    out.push_str(&rebuilt_note(status, style));
    out.push_str(&sentence(
        style,
        "--dry-run uploads nothing and contacts no network.",
    ));
    out
}

/// One indented prose line, wrapped at a space when styled and dimmed — it is
/// the explanation, not the figure.
fn sentence(style: Style, body: &str) -> String {
    if !style.is_on() {
        return format!("  {body}\n");
    }
    format!("{}\n", style.dim(&reflow(style, &format!("  {body}"), 4)))
}

/// [`sentence`], with one fragment of it bold — the number the reader is
/// deciding on, against a dim explanation.
///
/// Wrapping runs over the **unstyled** sentence, because an escape sequence
/// costs bytes and no columns; the emphasis is applied afterwards, to the first
/// occurrence of `emphasis` in the wrapped text.
///
/// ponytail: first occurrence, not an offset. `emphasis` is the leading figure
/// of every sentence this is called with, so the first hit is it; if wrapping
/// ever splits the fragment across a line the match simply fails and the whole
/// sentence renders dim, which is a weight, not a wrong number.
fn sentence_with(style: Style, body: &str, emphasis: &str) -> String {
    if !style.is_on() {
        return format!("  {body}\n");
    }
    let wrapped = wrap(&format!("  {body}"), 4);
    match wrapped.find(emphasis) {
        Some(at) => format!(
            "{}{}{}\n",
            style.dim(&wrapped[..at]),
            style.bold(emphasis),
            style.dim(&wrapped[at + emphasis.len()..])
        ),
        None => format!("{}\n", style.dim(&wrapped)),
    }
}

/// The shared table: one row per category, then the totals. The third column
/// appears only when a plan was built.
fn table(report: &StatusReport, style: Style) -> String {
    let w = Widths::of(report, style);
    let has_plan = report.plan.is_some();

    let mut out = String::new();
    // The header used to appear only alongside the third column. A styled run
    // always draws it: "no organization" was half of what the user reported,
    // and three unlabelled columns of numbers are exactly that.
    if has_plan || style.is_on() {
        let head = format!(
            "  {:<lw$}  {:>fw$}  {:>rw$}{}",
            "",
            "files",
            "raw",
            if has_plan {
                format!("  {:>sw$}", "would send", sw = w.send)
            } else {
                String::new()
            },
            lw = w.label,
            fw = w.files + " files".len(),
            rw = w.raw,
        );
        out.push_str(&format!("{}\n", style.dim(&head)));
    }
    for l in &report.lines {
        // "off" and "0" are different facts and the user is choosing between
        // them: an off category has not been looked at, not found to be empty.
        if !l.enabled {
            out.push_str(&format!(
                "  {}  {}\n",
                pad_left(style, l.category.label(), w.label),
                style.dim(&format!("{:>fw$}", "off", fw = w.files + " files".len())),
            ));
            continue;
        }
        out.push_str(&row(
            style,
            &w,
            l.category.label(),
            l.files,
            l.bytes,
            report.category(l.category).map(|c| c.new_stored_bytes),
        ));
        if l.capped {
            out.push_str(&style.dim("  (capped)"));
        }
        out.push('\n');
        out.push_str(&excluded_note(report, l, &w, style));
    }

    out.push('\n');
    // The totals are the figures a decision is made against, so the whole row
    // carries weight where the per-category rows do not.
    out.push_str(&row_bold(
        style,
        &w,
        "total",
        report.total_files(),
        report.total_bytes(),
        report.plan.as_ref().map(|p| p.total_new_stored_bytes),
    ));
    out.push('\n');
    out
}

/// The table's column widths.
///
/// **Fixed when plain, measured when styled.** The fixed set is exactly what
/// the format strings used to hard-code, so a piped run still emits the bytes
/// it always did. Measuring is the fix for the reported defect: `{:>5}` for a
/// file count silently overflows at six digits and shoves every column right of
/// it out of line, and `{:>10}` does the same for `1023.9 MiB`.
struct Widths {
    label: usize,
    files: usize,
    raw: usize,
    send: usize,
}

impl Widths {
    fn of(report: &StatusReport, style: Style) -> Widths {
        let label = SyncCategory::ALL
            .iter()
            .map(|c| c.label().len())
            .max()
            .unwrap_or(0);
        if !style.is_on() {
            return Widths {
                label,
                files: 5,
                raw: 10,
                send: 10,
            };
        }
        // Every figure that will actually be drawn, including the totals row
        // and the transcripts exclusion line — a width computed from the
        // categories alone is the same bug one column over.
        let mut files = report.total_files().to_string().len();
        let mut raw = human_bytes(report.total_bytes()).len();
        let mut send = report
            .plan
            .as_ref()
            .map_or(0, |p| human_bytes(p.total_new_stored_bytes).len());
        for l in &report.lines {
            files = files.max(l.files.to_string().len());
            raw = raw.max(human_bytes(l.bytes).len());
            if let Some(c) = report.category(l.category) {
                send = send.max(human_bytes(c.new_stored_bytes).len());
                files = files.max(c.excluded_files.to_string().len());
                raw = raw.max(human_bytes(c.excluded_bytes).len());
            }
        }
        Widths {
            label: label.max("total".len()),
            files,
            // The header labels sit in these columns too. Measuring only the
            // figures is the same bug one row up: `would send` is ten
            // characters and overflows any narrower field it is given.
            raw: raw.max("raw".len()),
            send: send.max("would send".len()),
        }
    }
}

/// One category row: the label and the raw size are context, the would-send
/// figure is the one that decides whether to run the push.
fn row(
    style: Style,
    w: &Widths,
    label: &str,
    files: usize,
    bytes: u64,
    send: Option<u64>,
) -> String {
    let mut out = format!(
        "  {}  {:>fw$} {}  {}",
        pad_left(style, label, w.label),
        files,
        style.dim("files"),
        style.dim(&format!("{:>rw$}", human_bytes(bytes), rw = w.raw)),
        fw = w.files,
    );
    if let Some(send) = send {
        out.push_str("  ");
        out.push_str(&style.bold(&format!("{:>sw$}", human_bytes(send), sw = w.send)));
    }
    out
}

/// [`row`] for the totals, where every figure is a decision input.
fn row_bold(
    style: Style,
    w: &Widths,
    label: &str,
    files: usize,
    bytes: u64,
    send: Option<u64>,
) -> String {
    let mut out = format!(
        "  {}  {} {}  {}",
        pad_left(style, label, w.label),
        style.bold(&format!("{files:>fw$}", fw = w.files)),
        style.dim("files"),
        style.bold(&format!("{:>rw$}", human_bytes(bytes), rw = w.raw)),
    );
    if let Some(send) = send {
        out.push_str("  ");
        out.push_str(&style.bold(&format!("{:>sw$}", human_bytes(send), sw = w.send)));
    }
    out
}

/// Left-align `text` in `width` columns, padding **outside** any escape.
///
/// `format!("{:<w$}", styled)` counts the escape bytes as columns and under-pads
/// by exactly their length, which is how a coloured table stops lining up.
fn pad_left(style: Style, text: &str, width: usize) -> String {
    let pad = width.saturating_sub(text.chars().count());
    format!("{}{}", style.dim(text), " ".repeat(pad))
}

/// What D3's bounds left behind — **transcripts only**.
///
/// The other four categories have no bounds, so their `excluded_*` are
/// structurally zero and a column of zeros would invite the reader to look for
/// a meaning it does not have (2-CONTEXT, and plan 2-06's note).
///
/// Deliberately never phrased as "30 days": on this project's own measured
/// archive the byte budget binds first and reaches back ~21 days, so the day
/// window is a ceiling to report against, not a promise to make.
fn excluded_note(report: &StatusReport, l: &CategoryLine, w: &Widths, style: Style) -> String {
    if l.category != SyncCategory::Transcripts {
        return String::new();
    }
    let Some(c) = report.category(l.category) else {
        return String::new();
    };
    if c.excluded_files == 0 {
        return String::new();
    }
    if !style.is_on() {
        return format!(
            "  {:<width$}  {:>5} files  {:>10}   left out by the age and size bounds\n",
            "",
            c.excluded_files,
            human_bytes(c.excluded_bytes),
            width = w.label,
        );
    }
    // **This is the line the user watched wrap mid-word.** Padded into the
    // table's own columns it is 82 characters and there is no arrangement of
    // those columns that makes the trailing clause fit at 80. So it stops
    // pretending to be a table row: it is a sub-note of the row above it,
    // indented under it, dim, and short enough to fit whole.
    format!(
        "{}\n",
        style.dim(&wrap(
            &format!(
                "    {} files, {} left out by the age and size bounds",
                c.excluded_files,
                human_bytes(c.excluded_bytes)
            ),
            6,
        ))
    )
}

/// How loudly a [`note`] reads. A refusal is red; nothing else is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Weight {
    Warn,
    Bad,
}

/// `  {head} — {first line}`, with any further lines of `body` indented under
/// it. The reasons a column is missing run past a terminal width otherwise.
///
/// **`body` is frequently attacker-controlled** — a GitHub error message, a
/// manifest path — which is why the head carries the styling and the body is
/// only ever passed through [`Style::sgr`]'s sanitizer, never trusted to
/// terminate a sequence it was wrapped in.
fn note(head: &str, body: &str, style: Style, weight: Weight) -> String {
    let painted = |text: &str| match weight {
        Weight::Warn => style.bold(text),
        Weight::Bad => style.bad(text),
    };
    let mut lines = body.lines();
    let first = lines.next().unwrap_or_default();
    let plain = format!("  {head} — {first}");
    let mut out = if !style.is_on() {
        format!("{plain}\n")
    } else {
        // Wrapped as one line and *then* split, because the head is 30 columns
        // of the budget the body has to fit in — wrapping the body alone
        // measures it against a margin that is not there.
        let lead = format!("  {head} —");
        let wrapped = wrap(&plain, 4);
        match wrapped.strip_prefix(&lead) {
            Some(rest) => format!("{}{}\n", painted(&lead), style.dim(rest)),
            None => format!("{}\n", style.dim(&wrapped)),
        }
    };
    for line in lines {
        out.push_str(&format!(
            "    {}\n",
            if style.is_on() {
                style.dim(&reflow(style, line, 6))
            } else {
                line.to_owned()
            }
        ));
    }
    out
}

/// A rebuilt index makes everything read as changed, which is the difference
/// between a slow first run and a bug.
fn rebuilt_note(report: &StatusReport, style: Style) -> String {
    match &report.plan {
        Some(p) if p.index_rebuilt => sentence(
            style,
            "the local index was missing or unreadable and was rebuilt, so everything \
             reads as new — the next run will be cheap.",
        ),
        _ => String::new(),
    }
}

pub(crate) fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::plan;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn seed(dir: &Path, rel: &str, body: &str) {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, body).unwrap();
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

    #[test]
    fn a_seeded_tree_renders_every_category_in_d1_order_with_counts_and_bytes() {
        let dir = TempDir::new().unwrap();
        seed(dir.path(), "config.toml", "[sync]\n");
        seed(dir.path(), "accounts/work/.credentials.json", "{}");

        let report = build_status(
            &roots_at(&dir),
            &SyncConfig::default(),
            None,
            Utc::now(),
            None,
            None,
        );
        assert_eq!(
            report.lines.iter().map(|l| l.category).collect::<Vec<_>>(),
            SyncCategory::ALL.to_vec()
        );
        let config_line = &report.lines[0];
        assert_eq!(config_line.files, 2);
        assert_eq!(config_line.bytes, 9); // "[sync]\n" + "{}"

        let text = render_status(&report);
        for cat in SyncCategory::ALL {
            assert!(
                text.contains(cat.label()),
                "missing {}: {text}",
                cat.label()
            );
        }
        assert!(text.contains("2 files"), "{text}");
    }

    #[test]
    fn a_category_absent_from_the_configured_set_renders_off_not_zero() {
        let dir = TempDir::new().unwrap();
        seed(dir.path(), "config.toml", "[sync]\n");

        let report = build_status(
            &roots_at(&dir),
            &SyncConfig::default(),
            None,
            Utc::now(),
            None,
            None,
        );
        let transcripts = report.lines.last().unwrap();
        assert_eq!(transcripts.category, SyncCategory::Transcripts);
        assert!(!transcripts.enabled);

        let line = render_status(&report)
            .lines()
            .find(|l| l.contains("transcripts"))
            .unwrap()
            .to_string();
        assert!(line.contains("off"), "{line}");
        assert!(!line.contains("files"), "{line}");
    }

    #[test]
    fn no_last_sync_renders_as_never() {
        let dir = TempDir::new().unwrap();
        let report = build_status(
            &roots_at(&dir),
            &SyncConfig::default(),
            None,
            Utc::now(),
            None,
            None,
        );
        assert!(report.last_sync.is_none());
        assert!(render_status(&report).contains("last sync: never"));
    }

    fn report_of(lines: Vec<CategoryLine>, plan: Option<SyncPlan>) -> StatusReport {
        StatusReport {
            lines,
            last_sync: DateTime::parse_from_rfc3339("2026-08-19T12:00:00Z")
                .ok()
                .map(|t| t.with_timezone(&Utc)),
            index_path: PathBuf::from("/nowhere/index.sqlite3"),
            plan,
            repo: None,
            pending: None,
            warnings: Vec::new(),
        }
    }

    fn a_line(category: SyncCategory, files: usize, bytes: u64) -> CategoryLine {
        CategoryLine {
            category,
            enabled: true,
            files,
            bytes,
            capped: false,
        }
    }

    fn a_category(category: SyncCategory, files: usize, raw: u64, stored: u64) -> CategoryPlan {
        CategoryPlan {
            category,
            files,
            raw_bytes: raw,
            new_bytes: raw,
            new_stored_bytes: stored,
            excluded_files: 0,
            excluded_bytes: 0,
            capped: false,
        }
    }

    fn a_plan(categories: Vec<CategoryPlan>) -> SyncPlan {
        let total_raw = categories.iter().map(|c| c.raw_bytes).sum();
        let total_new = categories.iter().map(|c| c.new_bytes).sum();
        let total_stored = categories.iter().map(|c| c.new_stored_bytes).sum();
        SyncPlan {
            categories,
            new_chunk_ids: if total_stored == 0 {
                Vec::new()
            } else {
                vec![[7u8; 32]]
            },
            total_raw_bytes: total_raw,
            total_new_bytes: total_new,
            total_new_stored_bytes: total_stored,
            files_opened: 0,
            append_check_miss_bytes: 0,
            index_rebuilt: false,
            file_plans: Vec::new(),
        }
    }

    #[test]
    fn rendering_is_a_pure_function_of_the_report() {
        let report = report_of(vec![a_line(SyncCategory::Config, 3, 2048)], None);
        let once = render_status(&report);
        assert_eq!(once, render_status(&report));
        assert!(once.contains("2.0 KiB"), "{once}");
        assert!(once.contains("2026-08-19T12:00:00"), "{once}");
    }

    #[test]
    fn a_capped_walk_is_reported_rather_than_silently_under_counting() {
        let report = report_of(
            vec![CategoryLine {
                category: SyncCategory::Transcripts,
                enabled: true,
                files: 200_000,
                bytes: 1,
                capped: true,
            }],
            None,
        );
        assert!(render_status(&report).contains("capped"));
    }

    // ---- 2-07: D4's dry-run ---------------------------------------------

    /// The four figures D4 names, per category and in total.
    #[test]
    fn a_dry_run_renders_files_raw_bytes_and_would_send_per_category() {
        let report = DryRunReport {
            status: report_of(
                vec![
                    a_line(SyncCategory::Config, 2, 4096),
                    a_line(SyncCategory::Credentials, 107, 24 * 1024 * 1024),
                ],
                Some(a_plan(vec![
                    a_category(SyncCategory::Config, 2, 4096, 1024),
                    a_category(
                        SyncCategory::Credentials,
                        107,
                        24 * 1024 * 1024,
                        11 * 1024 * 1024,
                    ),
                ])),
            ),
            no_key: None,
        };

        let text = render_dry_run(&report);
        let config = text.lines().find(|l| l.contains("config")).unwrap();
        assert!(config.contains("2 files"), "{config}");
        assert!(config.contains("4.0 KiB"), "raw bytes: {config}");
        assert!(config.contains("1.0 KiB"), "would-send bytes: {config}");

        let total = text.lines().find(|l| l.contains("total")).unwrap();
        assert!(total.contains("109 files"), "{total}");
        assert!(total.contains("24.0 MiB"), "{total}");
        assert!(total.contains("11.0 MiB"), "{total}");
        assert!(text.contains("snapshot: 109 files"), "{text}");
        assert!(text.contains("would send 11.0 MiB"), "{text}");
        assert!(text.contains("uploads nothing"), "{text}");
    }

    #[test]
    fn a_disabled_category_renders_off_in_every_column_not_three_zeros() {
        let report = DryRunReport {
            status: report_of(
                vec![
                    a_line(SyncCategory::Config, 1, 100),
                    CategoryLine {
                        category: SyncCategory::Transcripts,
                        enabled: false,
                        files: 0,
                        bytes: 0,
                        capped: false,
                    },
                ],
                Some(a_plan(vec![a_category(SyncCategory::Config, 1, 100, 64)])),
            ),
            no_key: None,
        };

        let line = render_dry_run(&report)
            .lines()
            .find(|l| l.contains("transcripts"))
            .unwrap()
            .to_string();
        assert!(line.contains("off"), "{line}");
        assert!(!line.contains('0'), "not a row of zeros: {line}");
    }

    // ---- the styled path --------------------------------------------------
    //
    // Everything above drives `Style::PLAIN` and asserts on wording. These
    // drive the same renderers with `Style::color(true)` and assert on the things
    // only the styled path can get wrong.

    /// The whole contract the piped world depends on: the report a pipe, a log
    /// file and the macOS menu bar receive did not move one byte.
    #[test]
    fn styling_changes_nothing_at_all_when_it_is_off() {
        let report = a_dry_run();
        assert_eq!(
            render_dry_run_styled(&report, Style::PLAIN),
            render_dry_run(&report)
        );
        assert_eq!(
            render_status_styled(&report.status, Style::PLAIN),
            render_status(&report.status)
        );
        assert!(!render_dry_run(&report).contains('\x1b'));
    }

    /// `NO_COLOR` is the whole of the difference, and it arrives as a
    /// [`Style`], not as an environment read this module could make.
    #[test]
    fn no_color_yields_the_plain_bytes_and_not_one_escape() {
        let report = a_dry_run();
        let style = Style::color(crate::display::color_enabled_with(true, true));
        assert_eq!(
            render_dry_run_styled(&report, style),
            render_dry_run(&report)
        );
        assert!(!render_dry_run_styled(&report, style).contains('\x1b'));

        let styled = render_dry_run_styled(&report, Style::color(true));
        assert!(
            styled.contains('\x1b'),
            "and with NO_COLOR unset it is styled"
        );
    }

    /// **Every sequence this module opens, it closes.** A report that dies
    /// mid-render must not leave the next shell prompt tinted.
    #[test]
    fn every_styled_run_ends_every_sequence_it_starts() {
        for text in [
            render_dry_run_styled(&a_dry_run(), Style::color(true)),
            render_status_styled(&a_dry_run().status, Style::color(true)),
        ] {
            let closes = text.matches("\x1b[0m").count();
            assert_eq!(
                text.matches("\x1b[").count(),
                closes * 2,
                "unbalanced: {text:?}"
            );
        }
    }

    /// **The reported defect.** A six-digit file count and a ten-character byte
    /// figure both overflowed their hard-coded fields and shoved every column
    /// right of them out of line. Widths now come from the rows.
    #[test]
    fn the_columns_line_up_however_wide_the_numbers_get() {
        let report = DryRunReport {
            status: report_of(
                vec![
                    a_line(SyncCategory::Config, 1, 309),
                    a_line(SyncCategory::Credentials, 123_456, 1_099_511_627_776),
                ],
                Some(a_plan(vec![
                    a_category(SyncCategory::Config, 1, 309, 300),
                    a_category(
                        SyncCategory::Credentials,
                        123_456,
                        1_099_511_627_776,
                        1_073_741_824,
                    ),
                ])),
            ),
            no_key: None,
        };
        let text = render_dry_run_styled(&report, Style::color(true));
        let rows: Vec<usize> = text
            .lines()
            .take_while(|l| !l.contains("snapshot:"))
            .filter(|l| !strip(l).trim().is_empty())
            .map(visible)
            .collect();
        assert!(
            rows.len() >= 4,
            "header, two categories and the totals: {text}"
        );
        assert!(
            rows.windows(2).all(|w| w[0] == w[1]),
            "every table row is the same width: {rows:?}\n{text}"
        );
        assert!(
            rows[0] <= 80,
            "and the table fits the terminal it is drawn on: {}",
            rows[0]
        );
    }

    /// The other half of the same defect: `2141 files 1.7 GiB left out by the
    /// age and size bounds` is 82 columns padded into the table, and what the
    /// user saw was their terminal cutting it in the middle of a word.
    #[test]
    fn no_styled_line_runs_past_the_terminal_or_breaks_a_word() {
        let mut transcripts = a_category(SyncCategory::Transcripts, 2146, 2_136_746_229, 1_000);
        transcripts.excluded_files = 2141;
        transcripts.excluded_bytes = 1_782_579_527;
        let report = DryRunReport {
            status: report_of(
                vec![a_line(SyncCategory::Transcripts, 2146, 2_136_746_229)],
                Some(a_plan(vec![transcripts])),
            ),
            no_key: None,
        };
        // The other over-long line, which only renders without a plan: the
        // reason the would-send column is missing, in the user's own words.
        let unkeyed = DryRunReport {
            status: report_of(vec![a_line(SyncCategory::Transcripts, 2146, 1 << 31)], None),
            no_key: Some(
                "this bundle has no sync keyfile yet and the third column needs one, so \
                 the counts above are everything this run can honestly tell you about it"
                    .to_string(),
            ),
        };

        for report in [&report, &unkeyed] {
            let text = render_dry_run_styled(report, Style::color(true));
            for line in text.lines() {
                assert!(visible(line) <= 80, "{} columns: {line:?}", visible(line));
            }
            // Never mid-word: every word of the plain rendering survives the
            // wrapped one intact.
            let plain = strip(&render_dry_run(report));
            let wrapped = strip(&text);
            for word in plain.split_whitespace() {
                assert!(wrapped.contains(word), "{word:?} was broken up:\n{text}");
            }
        }

        assert!(
            render_dry_run_styled(&report, Style::color(true))
                .contains("left out by the age and size bounds"),
            "and the clause survives whole, on one line"
        );
    }

    /// A hostile remote writes the manifest paths and the GitHub error
    /// messages that reach these renderers. Wrapping one in a sequence it can
    /// close is how it would start its own.
    #[test]
    fn untrusted_text_cannot_close_the_sequence_it_is_styled_inside() {
        let hostile = "\x1b[0m\x1b]52;c;cGF5bG9hZA==\x07 sync succeeded\u{202e}drowssap";
        let mut section = RepoSection {
            configured: Some("owner/name".into()),
            visibility: Some(hostile.into()),
            ..RepoSection::default()
        };
        section.warnings.push(hostile.into());
        let section = section.failed(hostile.into());

        let mut status = a_dry_run().status;
        status.repo = Some(section);
        let text = render_status_styled(&status, Style::color(true));

        assert!(!text.contains("\x1b]"), "no OSC survives: {text:?}");
        assert!(!text.contains('\u{202e}'), "no bidi override survives");
        // The only escapes left are this module's own, and they still balance.
        let closes = text.matches("\x1b[0m").count();
        assert_eq!(text.matches("\x1b[").count(), closes * 2, "{text:?}");
        // And the payload's visible remains are inert text, not an instruction.
        assert!(text.contains("]52;c;cGF5bG9hZA=="), "{text:?}");
    }

    /// One `\n` in a remote error message is one forged report line, so the
    /// note indents continuation lines rather than letting them start at the
    /// left margin where a real field would.
    #[test]
    fn a_multi_line_untrusted_note_cannot_forge_a_field() {
        let status = {
            let mut s = a_dry_run().status;
            s.repo = Some(
                RepoSection {
                    configured: Some("owner/name".into()),
                    ..RepoSection::default()
                }
                .failed("first\nvisible:   private".into()),
            );
            s
        };
        for text in [
            render_status(&status),
            strip(&render_status_styled(&status, Style::color(true))),
        ] {
            let forged = text
                .lines()
                .filter(|l| l.starts_with("  visible:   private"))
                .count();
            assert_eq!(forged, 0, "no field was forged: {text}");
            assert!(
                text.lines().any(|l| l == "    visible:   private"),
                "the second line is indented under the note, not at field depth: {text}"
            );
        }
    }

    /// Restraint is the taste rule, and a rule nobody can check is a
    /// suggestion. Four attributes, and no 24-bit colour: the palette borrows
    /// the user's own terminal theme rather than overriding it.
    #[test]
    fn the_palette_is_four_attributes_and_never_a_literal_colour() {
        let text = format!(
            "{}{}",
            render_dry_run_styled(&a_dry_run(), Style::color(true)),
            render_status_styled(&a_dry_run().status, Style::color(true))
        );
        let mut seen: Vec<&str> = text
            .split("\x1b[")
            .skip(1)
            .filter_map(|s| s.split('m').next())
            .collect();
        seen.sort_unstable();
        seen.dedup();
        // The accent (`1;36`) is spent on the `sync setup` step markers, which
        // this module does not render — so the report itself is three.
        assert_eq!(seen, vec!["0", "1", "2"], "{seen:?}");
        assert!(!text.contains("\x1b[38;2;"), "no 24-bit colour");
        assert!(
            Style::color(true).head("x").contains("\x1b[1;36m"),
            "the accent exists"
        );
    }

    /// Columns, not bytes — the measurement every assertion above depends on.
    fn visible(line: &str) -> usize {
        strip(line).chars().count()
    }

    fn strip(text: &str) -> String {
        let mut out = String::new();
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for skip in chars.by_ref() {
                    if skip == 'm' {
                        break;
                    }
                }
                continue;
            }
            out.push(c);
        }
        out
    }

    fn a_dry_run() -> DryRunReport {
        DryRunReport {
            status: report_of(
                vec![
                    a_line(SyncCategory::Config, 2, 4096),
                    a_line(SyncCategory::Credentials, 107, 24 * 1024 * 1024),
                ],
                Some(a_plan(vec![
                    a_category(SyncCategory::Config, 2, 4096, 1024),
                    a_category(
                        SyncCategory::Credentials,
                        107,
                        24 * 1024 * 1024,
                        11 * 1024 * 1024,
                    ),
                ])),
            ),
            no_key: None,
        }
    }

    /// 2-CONTEXT: the excluded column counts bound-dropped files, which only
    /// transcripts have — and it must never be phrased as "30 days", because on
    /// the measured archive the byte budget binds first.
    #[test]
    fn only_transcripts_report_what_the_bounds_left_behind() {
        let mut transcripts = a_category(SyncCategory::Transcripts, 2077, 2_136_746_229, 1_000);
        transcripts.excluded_files = 2135;
        transcripts.excluded_bytes = 1_782_579_527;
        let config = a_category(SyncCategory::Config, 1, 183, 168);

        let report = DryRunReport {
            status: report_of(
                vec![
                    a_line(SyncCategory::Config, 1, 183),
                    a_line(SyncCategory::Transcripts, 2077, 2_136_746_229),
                ],
                Some(a_plan(vec![config, transcripts])),
            ),
            no_key: None,
        };

        let text = render_dry_run(&report);
        assert!(text.contains("2135 files"), "the excluded count: {text}");
        assert!(
            text.contains("left out by the age and size bounds"),
            "{text}"
        );
        assert!(
            !text.contains("30 days"),
            "the byte budget binds first, so the day window is not a promise: {text}"
        );
        // One excluded row, on the transcripts line only.
        assert_eq!(
            text.lines()
                .filter(|l| l.contains("left out by the"))
                .count(),
            1,
            "{text}"
        );
    }

    #[test]
    fn a_plan_with_nothing_new_says_a_push_would_send_nothing() {
        let report = DryRunReport {
            status: report_of(
                vec![a_line(SyncCategory::Config, 1649, 104_248_000)],
                Some(a_plan(vec![a_category(
                    SyncCategory::Config,
                    1649,
                    104_248_000,
                    0,
                )])),
            ),
            no_key: None,
        };

        let text = render_dry_run(&report);
        assert!(text.contains("would send nothing"), "{text}");
        assert!(text.contains("0 opened"), "SYNC-02's evidence: {text}");
        assert!(!text.contains("new chunks"), "{text}");
    }

    #[test]
    fn a_rebuilt_index_is_reported_because_it_explains_a_slow_run() {
        let mut plan = a_plan(vec![a_category(SyncCategory::Config, 1, 10, 64)]);
        plan.index_rebuilt = true;
        let report = DryRunReport {
            status: report_of(vec![a_line(SyncCategory::Config, 1, 10)], Some(plan)),
            no_key: None,
        };
        assert!(render_dry_run(&report).contains("rebuilt"));
    }

    /// SCOPE-04's answerable half: counts and raw bytes need no key at all.
    #[test]
    fn without_a_key_the_counts_still_render_and_the_missing_column_is_named() {
        let report = DryRunReport {
            status: report_of(
                vec![
                    a_line(SyncCategory::Config, 2, 4096),
                    a_line(SyncCategory::Credentials, 107, 24 * 1024 * 1024),
                ],
                None,
            ),
            no_key: Some("this needs the sync password".to_string()),
        };

        let text = render_dry_run(&report);
        assert!(text.contains("2 files"), "{text}");
        assert!(text.contains("4.0 KiB"), "{text}");
        assert!(text.contains("24.0 MiB"), "{text}");
        assert!(text.contains("sync password"), "{text}");
        assert!(text.contains("not computed"), "{text}");
        assert!(
            !text.contains("would send 0"),
            "a wrong zero reads as 'a push is free': {text}"
        );
    }

    #[test]
    fn rendering_a_dry_run_is_pure_and_touches_no_filesystem() {
        let report = DryRunReport {
            status: report_of(
                vec![a_line(SyncCategory::Config, 1, 10)],
                Some(a_plan(vec![a_category(SyncCategory::Config, 1, 10, 64)])),
            ),
            no_key: None,
        };
        assert_eq!(render_dry_run(&report), render_dry_run(&report));
    }

    /// The one integration-style test: a real plan through the real chunker,
    /// and the rendered totals are that plan's own fields.
    #[test]
    fn the_rendered_totals_are_the_plans_own_figures() {
        use crate::sync::crypto::{KdfParams, Keyfile};
        use crate::sync::index::Index;

        let dir = TempDir::new().unwrap();
        seed(dir.path(), "config.toml", "[sync]\n");
        seed(dir.path(), "accounts/work/.credentials.json", "{}");
        let roots = roots_at(&dir);
        let index = Index::at(&dir.path().join("index.sqlite3")).unwrap();
        // Microseconds, not a gibibyte: the AUR `check()` runs this.
        let cheap = KdfParams {
            m_kib: 8,
            t: 1,
            p: 1,
        };
        let keys = Keyfile::create_with_floor(b"a-test-passphrase", cheap, cheap.m_kib)
            .unwrap()
            .1;

        let cfg = SyncConfig {
            categories: vec![SyncCategory::Config],
            transcript_days: 30,
            transcript_max_bytes: 0,
            repo: None,
            ..SyncConfig::default()
        };
        // Fixed, not the wall clock: `now` is only a reference point for the
        // transcript bounds, and a test that reads the clock is a test that can
        // fail on a date nobody chose.
        let now = DateTime::from_timestamp(1_760_000_000, 0).unwrap();
        let plan = plan::build_with_keys(&roots, &cfg, &index, now, &keys).unwrap();
        assert_eq!(plan.files_opened, 2);
        assert!(plan.total_new_stored_bytes > 0);
        let stored = plan.total_new_stored_bytes;

        let report = DryRunReport {
            status: build_status(&roots, &cfg, Some(&index), now, Some(plan), None),
            no_key: None,
        };
        let text = render_dry_run(&report);
        assert!(text.contains(&human_bytes(stored)), "{stored}: {text}");
        assert!(text.contains("2 files"), "{text}");
        assert!(
            text.contains("9 B"),
            "the raw bytes of the two files: {text}"
        );

        // …and a second dry-run over the untouched tree sends nothing.
        let again = plan::build_with_keys(&roots, &cfg, &index, now, &keys).unwrap();
        assert_eq!(again.files_opened, 0);
        let report = DryRunReport {
            status: build_status(&roots, &cfg, Some(&index), now, Some(again), None),
            no_key: None,
        };
        assert!(render_dry_run(&report).contains("would send nothing"));
    }

    #[test]
    fn human_bytes_steps_through_the_binary_units() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1024), "1.0 KiB");
        assert_eq!(human_bytes(4 * 1024 * 1024), "4.0 MiB");
        assert_eq!(human_bytes(2 * 1024 * 1024 * 1024), "2.0 GiB");
    }

    // ---- 6-01: the machine-readable rendering ----------------------------

    /// Never the wall clock. `now` is only the transcripts bounds' reference
    /// point, and a test that reads the clock is a test that can fail on a date
    /// nobody chose.
    fn fixed_now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_760_000_000, 0).unwrap()
    }

    /// Microseconds, not a gibibyte: the AUR `check()` runs this.
    fn cheap_keys() -> crate::sync::crypto::Keys {
        use crate::sync::crypto::{KdfParams, Keyfile};
        let cheap = KdfParams {
            m_kib: 8,
            t: 1,
            p: 1,
        };
        Keyfile::create_with_floor(b"a-test-passphrase", cheap, cheap.m_kib)
            .unwrap()
            .1
    }

    fn walk_strings(v: &serde_json::Value, out: &mut Vec<String>) {
        match v {
            serde_json::Value::String(s) => out.push(s.clone()),
            serde_json::Value::Array(a) => a.iter().for_each(|x| walk_strings(x, out)),
            serde_json::Value::Object(o) => o.values().for_each(|x| walk_strings(x, out)),
            _ => {}
        }
    }

    /// The key set 6-02 parses. Named once, here, because renaming one of these
    /// after a menu bar has shipped against it is a compatibility break.
    #[test]
    fn status_json_carries_the_last_sync_the_pending_summary_and_one_line_per_category() {
        let mut report = report_of(vec![a_line(SyncCategory::Config, 3, 2048)], None);
        report.pending = Some(PendingSummary {
            files: 3,
            bytes: 4096,
        });

        let v = status_json(&report);
        assert_eq!(v["last_sync"], "2026-08-19T12:00:00+00:00");
        assert_eq!(v["index"], "/nowhere/index.sqlite3");
        assert_eq!(v["total_files"], 3);
        assert_eq!(v["total_bytes"], 2048);
        assert_eq!(v["pending"], true);
        assert_eq!(v["pending_files"], 3);
        assert_eq!(v["pending_bytes"], 4096);
        assert_eq!(v["warnings"], serde_json::json!([]));

        let cats = v["categories"].as_array().unwrap();
        assert_eq!(cats.len(), 1);
        assert_eq!(cats[0]["category"], "config");
        assert_eq!(cats[0]["enabled"], true);
        assert_eq!(cats[0]["files"], 3);
        assert_eq!(cats[0]["bytes"], 2048);
        assert_eq!(cats[0]["capped"], false);
    }

    /// Every key on every run, so a consumer never has to tell "absent" from
    /// "null" — and `last_sync` is JSON `null`, never the string "never": the
    /// wording belongs to whoever draws the row.
    #[test]
    fn every_key_is_present_even_when_its_value_is_null() {
        let mut report = report_of(Vec::new(), None);
        report.last_sync = None;
        report.index_path = PathBuf::new();

        let v = status_json(&report);
        for key in [
            "last_sync",
            "pending",
            "pending_files",
            "pending_bytes",
            "categories",
            "total_files",
            "total_bytes",
            "index",
            "warnings",
        ] {
            assert!(v.get(key).is_some(), "missing {key}: {v}");
        }
        assert_eq!(v["last_sync"], serde_json::Value::Null);
        assert_eq!(v["index"], serde_json::Value::Null);
    }

    /// An index that would not open is a warning the JSON consumer must see:
    /// without it a machine that syncs hourly renders as "never synced".
    #[test]
    fn an_unavailable_index_yields_a_null_index_and_says_so_in_warnings() {
        let dir = TempDir::new().unwrap();
        let report = build_status(
            &roots_at(&dir),
            &SyncConfig::default(),
            None,
            fixed_now(),
            None,
            None,
        );

        let v = status_json(&report);
        assert_eq!(v["index"], serde_json::Value::Null);
        assert_eq!(v["last_sync"], serde_json::Value::Null);
        assert_eq!(v["pending"], serde_json::Value::Null);
        assert_eq!(v["warnings"], serde_json::json!([WARN_INDEX_UNAVAILABLE]));
    }

    #[test]
    fn status_json_is_a_pure_function_of_the_report() {
        let report = report_of(vec![a_line(SyncCategory::Config, 1, 10)], None);
        assert_eq!(status_json(&report), status_json(&report));
    }

    /// D-04's three states. "Unknown" is not "nothing pending": a backup nobody
    /// can tell is stale is exactly what surfacing this exists to prevent.
    #[test]
    fn pending_is_true_false_or_unknown_and_never_flattens_the_third() {
        let mut report = report_of(vec![a_line(SyncCategory::Config, 3, 4096)], None);

        report.pending = Some(PendingSummary {
            files: 3,
            bytes: 4096,
        });
        assert_eq!(status_json(&report)["pending"], true);

        report.pending = Some(PendingSummary::default());
        let v = status_json(&report);
        assert_eq!(v["pending"], false);
        assert_eq!(v["pending_files"], 0);
        assert_eq!(v["pending_bytes"], 0);

        report.pending = None;
        let v = status_json(&report);
        assert_eq!(v["pending"], serde_json::Value::Null);
        assert_eq!(v["pending_files"], serde_json::Value::Null);
        assert_eq!(v["pending_bytes"], serde_json::Value::Null);
    }

    /// T-6-01. This document promises counts, byte totals, category labels, an
    /// index path and a fixed warning vocabulary — nothing else. Walk it and
    /// fail on any other string, so a future field that carries a file's
    /// contents has to break this test before it can ship.
    #[test]
    fn every_string_in_the_document_is_a_label_a_path_a_timestamp_or_a_known_warning() {
        let dir = TempDir::new().unwrap();
        seed(dir.path(), "config.toml", "a-secret-that-must-not-travel");
        seed(dir.path(), "accounts/work/.credentials.json", "{}");
        let index = Index::at(&dir.path().join("index.sqlite3")).unwrap();
        let report = build_status(
            &roots_at(&dir),
            &SyncConfig::default(),
            Some(&index),
            fixed_now(),
            None,
            None,
        );

        let document = status_json(&report);
        let rendered = document.to_string();
        assert!(
            !rendered.contains("a-secret-that-must-not-travel"),
            "{rendered}"
        );

        let mut leaves = Vec::new();
        walk_strings(&document, &mut leaves);
        assert!(!leaves.is_empty(), "nothing was walked: {document}");
        for leaf in leaves {
            let known = SyncCategory::ALL.iter().any(|c| c.label() == leaf)
                || WARNINGS.contains(&leaf.as_str())
                || DateTime::parse_from_rfc3339(&leaf).is_ok()
                || Path::new(&leaf).is_absolute();
            assert!(known, "unexpected string in the JSON: {leaf:?}");
        }
    }

    /// Where the first two pending states come from: the index either vouches
    /// for a file or it does not.
    #[test]
    fn pending_counts_the_files_the_index_does_not_vouch_for() {
        let dir = TempDir::new().unwrap();
        seed(dir.path(), "config.toml", "[sync]\n");
        seed(dir.path(), "accounts/work/.credentials.json", "{}");
        let roots = roots_at(&dir);
        let cfg = SyncConfig::default();
        let index = Index::at(&dir.path().join("index.sqlite3")).unwrap();

        let before = build_status(&roots, &cfg, Some(&index), fixed_now(), None, None);
        let p = before
            .pending
            .expect("an opened index is never the unknown state");
        assert_eq!((p.files, p.bytes), (2, 9));

        // Record them exactly as the planner does; then nothing is pending —
        // via the scan, and via a plan whose files were all short-circuited.
        let keys = cheap_keys();
        plan::build_with_keys(&roots, &cfg, &index, fixed_now(), &keys).unwrap();
        let after = build_status(&roots, &cfg, Some(&index), fixed_now(), None, None);
        assert_eq!(after.pending, Some(PendingSummary::default()));

        let reused = plan::build_with_keys(&roots, &cfg, &index, fixed_now(), &keys).unwrap();
        assert_eq!(reused.files_opened, 0);
        let with_plan = build_status(&roots, &cfg, Some(&index), fixed_now(), Some(reused), None);
        assert_eq!(with_plan.pending, Some(PendingSummary::default()));
    }

    /// `sync status` is advertised as costing a stat sweep. A file whose body
    /// cannot be read at all is still counted, because nothing on this path
    /// opens one — a status call that hashed a 50 MB transcript to draw a menu
    /// row would break that promise silently.
    #[cfg(unix)]
    #[test]
    fn the_pending_count_never_opens_a_file_body() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        seed(dir.path(), "config.toml", "[sync]\n");
        let unreadable = dir.path().join("config.toml");
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).unwrap();

        let index = Index::at(&dir.path().join("index.sqlite3")).unwrap();
        let report = build_status(
            &roots_at(&dir),
            &SyncConfig::default(),
            Some(&index),
            fixed_now(),
            None,
            None,
        );
        assert_eq!(report.pending.map(|p| p.files), Some(1));

        // …and give it back a mode the TempDir can clean up.
        fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o600)).unwrap();
    }

    // ---- 6-11: the half a filesystem walk cannot see ---------------------

    /// One credential file and one machine-bound store, under `credentials`.
    fn a_machine_with_a_keystore(dir: &TempDir) -> SyncRoots {
        seed(dir.path(), "claude-home/.credentials.json", "{}");
        let roots = roots_at(dir);
        roots.stores.edit().set(
            crate::sync::keystore::Store::ClaudeCodeOauth,
            r#"{"claudeAiOauth":{}}"#,
        );
        roots
    }

    fn only_credentials() -> SyncConfig {
        SyncConfig {
            categories: vec![SyncCategory::Credentials],
            repo: None,
            ..SyncConfig::default()
        }
    }

    fn credentials_files(report: &StatusReport) -> usize {
        report
            .lines
            .iter()
            .find(|l| l.category == SyncCategory::Credentials)
            .expect("every category has a row")
            .files
    }

    /// **The defect.** `sync status` walks the filesystem, and a machine-bound
    /// store is not a file — so it reported one credential fewer than
    /// `sync push --dry-run` planned, and the missing one was the Claude Code
    /// login: the single most sensitive item in the bundle. The two commands
    /// answer the same question at different moments and must agree.
    #[test]
    fn status_counts_the_keystore_the_way_a_push_would() {
        use crate::sync::crypto::{KdfParams, Keyfile};
        use crate::sync::index::Index;

        let dir = TempDir::new().unwrap();
        let roots = a_machine_with_a_keystore(&dir);
        let cfg = only_credentials();
        let index = Index::at(&dir.path().join("index.sqlite3")).unwrap();

        // No plan: the walk, plus the store the walk cannot see.
        let walked = build_status(&roots, &cfg, Some(&index), fixed_now(), None, None);
        assert_eq!(credentials_files(&walked), 2, "one file and one store");
        assert!(walked.warnings.is_empty(), "{:?}", walked.warnings);

        // A plan: the planner's own third pass, which is what a push sends.
        let cheap = KdfParams {
            m_kib: 8,
            t: 1,
            p: 1,
        };
        let keys = Keyfile::create_with_floor(b"a-test-passphrase", cheap, cheap.m_kib)
            .unwrap()
            .1;
        let plan = plan::build_with_keys(&roots, &cfg, &index, fixed_now(), &keys).unwrap();
        let planned = build_status(&roots, &cfg, Some(&index), fixed_now(), Some(plan), None);

        assert_eq!(
            credentials_files(&walked),
            credentials_files(&planned),
            "status and push --dry-run disagree about what would be carried"
        );
    }

    /// The planner reads no store with `credentials` switched off, so neither
    /// may this — otherwise turning the category off would still advertise the
    /// login as in scope.
    #[test]
    fn a_store_is_not_counted_when_credentials_is_switched_off() {
        let dir = TempDir::new().unwrap();
        let roots = a_machine_with_a_keystore(&dir);
        let cfg = SyncConfig {
            categories: vec![SyncCategory::Config],
            repo: None,
            ..SyncConfig::default()
        };
        let report = build_status(&roots, &cfg, None, fixed_now(), None, None);
        assert_eq!(credentials_files(&report), 0);
    }

    /// A locked Keychain is a **third** answer. Reporting it as "no credential
    /// here" is the under-count this path was fixed to end, so it is said out
    /// loud instead — in the fixed vocabulary the JSON document promises.
    #[test]
    fn an_unreadable_store_is_a_warning_rather_than_a_quietly_short_count() {
        let dir = TempDir::new().unwrap();
        let roots = a_machine_with_a_keystore(&dir);
        roots.stores.edit().set_unreadable(true);

        let report = build_status(&roots, &only_credentials(), None, fixed_now(), None, None);
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w == WARN_KEYSTORE_UNAVAILABLE),
            "{:?}",
            report.warnings
        );
        // The row still carries what the walk could see, and no invented store.
        assert_eq!(credentials_files(&report), 1);
        // And the warning is one the machine-readable document may carry.
        assert!(WARNINGS.contains(&WARN_KEYSTORE_UNAVAILABLE));
    }

    /// Nothing on this path may reach for the store's *value*: `--json` is what
    /// the macOS menu bar runs on every menu open, and a value read is what
    /// raises the Keychain ACL prompt.
    #[test]
    fn the_status_path_asks_whether_a_store_has_an_entry_and_never_what_it_holds() {
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/sync/report.rs"),
        )
        .expect("the crate's own source is readable");
        let production = crate::sync::guard::production_code(&source);
        assert!(production.contains("stores.has("), "the probe is the seam");
        assert!(
            !production.contains("stores.read("),
            "sync status must never read a credential's value"
        );
    }
}
