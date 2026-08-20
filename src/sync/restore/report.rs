//! What a restore tells the user, before and after.
//!
//! The only module under `restore/` that prints or reads from a terminal; the
//! other five return values. [`render_plan`] and [`render_outcome`] are pure —
//! a borrowed model in, a `String` out — and the two gates take their reader
//! and writer, so no test needs a TTY and no test can hang on a prompt.
//!
//! # This module is the product of a dry run
//!
//! On a dry run nothing else happens: the report *is* the deliverable, and it
//! is the only thing between the user and an irreversible overwrite of live
//! OAuth credentials. Every sentence below is therefore a claim about what the
//! merged code does, and each one is asserted in this file's tests.
//!
//! # What it never prints
//!
//! Paths, byte counts, timestamps and counts. Never a file's contents, never a
//! passphrase, never a token, never a URL — a signed asset URL's query string
//! is itself a credential. That is structural rather than remembered: no field
//! of [`ItemPlan`], [`RestorePlan`], [`RestoreOutcome`] or [`BackupRecord`]
//! carries plaintext or a URL, so there is nothing here to leak (T-5-56).
//!
//! Every string that came off the wire — a `manifest_path`, a
//! [`Disposition::RejectedPath`] reason — is attacker-chosen and goes through
//! [`safe`] before it reaches the terminal (T-5-50).
//!
//! # Two gates, and only two
//!
//! [`confirm_apply`] is the one interactive confirmation D6 allows, and it
//! comes after the whole report. [`confirm_credentials`] is D2's separate
//! consent, and `--yes` does not answer it — [`RestoreOptions::assume_yes`] is
//! never read there (T-5-52). There is no third prompt; a test asserts it.
//!
//! Owned by plan 5-06 (the tracer's first cut was plan 5-01's).

use std::io::{BufRead, Write};

use chrono::{DateTime, Utc};

use crate::config::SyncCategory;
use crate::display::sanitize_untrusted_line;
use crate::error::Result;
use crate::sync::report::human_bytes;

use super::{Disposition, ItemPlan, RestoreOptions, RestoreOutcome, RestorePlan};

/// How many ordinary create / update / unchanged paths one category prints
/// before it collapses to a count. One item is one line here, so this is a
/// line budget as well as an item budget.
pub const MAX_ITEM_LINES_PER_CATEGORY: usize = 10;

/// The budget for the two blocks that *lead* the report — the items whose
/// local copy is newer, and the refusals.
///
/// **Deliberately larger than [`MAX_ITEM_LINES_PER_CATEGORY`], and deliberately
/// not unified with it.** These are the lines a user actually has to read
/// before answering the gate; a path refused for escaping the sync roots that
/// got collapsed into "and N more" is a tampered bundle hiding an entry
/// (T-5-53). Only ordinary creates and updates are cheap enough to truncate
/// hard. A later tidy-up that merges the two constants re-opens that hole.
///
/// One of these items renders up to four lines — its path, what will happen to
/// it, both timestamps, and the flag that changes the answer — so a block's
/// ceiling is four times this.
pub const MAX_ATTENTION_ITEMS: usize = 20;

/// What a user runs to turn this dry run into writes.
///
/// **A contract on plan 5-07**, which owns `sync/cli.rs`: the subcommand must
/// be spelled exactly this. A report that names a command that does not exist
/// is worse than one that names none.
pub const APPLY_COMMAND: &str = "ai-usagebar sync pull --apply";

/// The report: what a restore would do, having written nothing yet.
///
/// `applying` is whether `--apply` was given. The plan is always computed with
/// `apply` off — that is how the report exists at all — so the renderer is the
/// only place that can know, and a report that calls an apply a dry run and
/// then tells the user to re-run the command they just ran is a report nobody
/// can act on.
///
/// Order is a requirement, not a preference. The items whose local copy is
/// newer and the refusals come *first*, because SAFE-03 is that the user is
/// told what would change before they are asked, and a warning printed below a
/// two-hundred-line table satisfies the letter of that and none of it.
pub fn render_plan(plan: &RestorePlan, applying: bool) -> String {
    let mut out = String::new();
    out.push_str(&headline(plan, applying));
    out.push_str(&attention_block(plan));
    out.push_str(&refusals_block(plan));
    out.push_str(&category_table(plan));
    out.push_str(&category_detail(plan));
    out.push_str(&footer(plan, applying));
    out
}

/// The post-restore summary.
///
/// On a dry run it *is* [`render_plan`] — there is nothing else to say about a
/// run that wrote nothing. On an applied run it names every overwritten path,
/// because SYNC-06 asks that the user be told what was overwritten and a count
/// tells nobody anything (T-5-51).
pub fn render_outcome(outcome: &RestoreOutcome) -> String {
    if !outcome.applied {
        return render_plan(&outcome.plan, false);
    }

    let mut out = match &outcome.failed_at {
        Some(at) => format!(
            "RESTORE INCOMPLETE — it stopped at {}\n\
             \x20 {} item(s) were written and {} skipped before it stopped,\n\
             \x20 so this machine's tree is part-restored.\n",
            safe(at),
            outcome.written,
            outcome.skipped
        ),
        None => format!(
            "RESTORED snapshot {} of {} — {} written, {} skipped\n",
            outcome.plan.counter,
            safe(&outcome.plan.repo_id),
            outcome.written,
            outcome.skipped
        ),
    };

    if outcome.overwritten.is_empty() {
        out.push_str("  Nothing that already existed was replaced.\n");
    } else {
        out.push_str(&format!(
            "\n  Overwrote {} item(s) — every one of them, by name:\n",
            outcome.overwritten.len()
        ));
        // Never truncated. A list the user cannot see the end of is a count
        // wearing a list's clothes, and a count is what SYNC-06 rejects.
        for path in &outcome.overwritten {
            out.push_str(&format!("    {}\n", safe(path)));
        }
    }

    match &outcome.backup {
        Some(backup) => {
            out.push_str(&format!(
                "\n  Archived {} item(s) ({}) first, to:\n    {}\n",
                backup.members,
                human_bytes(backup.bytes),
                backup.archive.display()
            ));
            out.push_str(&format!(
                "  Undo this restore with:\n    {}\n",
                backup.rollback_command()
            ));
        }
        // `backup::take` returns `None` only when nothing at the destinations
        // existed to archive. If something *was* overwritten there is no undo,
        // and saying so plainly beats printing no line at all.
        None if outcome.overwritten.is_empty() => {
            out.push_str(
                "\n  No archive was taken — every item was new, so there was \
                          nothing to replace.\n",
            );
        }
        None => {
            out.push_str(
                "\n  WARNING: items were overwritten but no archive was \
                          recorded, so there is no undo command for this run.\n",
            );
        }
    }

    // Again, at the bottom. The partial path is when the rollback is actually
    // needed, and the bottom of the output is where a user looks after a
    // failure.
    if let (Some(at), Some(backup)) = (&outcome.failed_at, &outcome.backup) {
        out.push_str(&format!(
            "\n  It stopped at {}. To put this machine back as it was:\n    {}\n",
            safe(at),
            backup.rollback_command()
        ));
    }
    out
}

/// The one interactive gate D6 allows: the whole report, then a single
/// question.
///
/// The reader and the writer are injected, so interactivity is the caller's
/// decision through `IsTerminal` exactly as `sync/cli.rs` already does it, and
/// no test needs a TTY. A non-interactive run reaches EOF immediately rather
/// than blocking on a stdin that will never arrive, and EOF is a refusal that
/// names the flag (T-5-55).
///
/// Only an explicit affirmative passes. A bare newline is a refusal; anything
/// unrecognised is a refusal. Consent for an irreversible overwrite is not the
/// default branch.
pub fn confirm_apply(
    plan: &RestorePlan,
    opts: &RestoreOptions,
    out: &mut dyn Write,
    input: &mut dyn BufRead,
) -> Result<bool> {
    write!(out, "{}", render_plan(plan, false))?;

    if opts.assume_yes {
        writeln!(
            out,
            "\n--yes was passed, so this restore was not asked about."
        )?;
        return Ok(true);
    }

    write!(
        out,
        "\nApply this restore, replacing the items above? [y/N] "
    )?;
    out.flush()?;

    let mut line = String::new();
    if input.read_line(&mut line)? == 0 {
        writeln!(
            out,
            "\nno answer arrived — nothing was written.\n\
             Pass --yes to apply without being asked."
        )?;
        return Ok(false);
    }

    let answer = line.trim().to_ascii_lowercase();
    if answer == "y" || answer == "yes" {
        return Ok(true);
    }
    writeln!(out, "not applied — nothing was written.")?;
    Ok(false)
}

/// D2's second consent, for the credentials whose local copy is newer.
///
/// **`--yes` does not answer this.** [`RestoreOptions::assume_yes`] is never
/// read in this function; only [`RestoreOptions::force_credentials`]
/// short-circuits it (T-5-52). The check lives here rather than in the caller
/// on purpose: a security control a caller has to remember to apply is a
/// control that eventually is not applied.
///
/// It is not a `[y/N]`. It asks for the word, because a confirmation that can
/// be cleared with one keystroke is a keystroke and not a decision.
pub fn confirm_credentials(
    items: &[&ItemPlan],
    opts: &RestoreOptions,
    out: &mut dyn Write,
    input: &mut dyn BufRead,
) -> Result<bool> {
    // Consent over an empty set. The caller only reaches here with a
    // `NeedsCredentialConfirm` item in hand; this arm keeps the function total.
    if items.is_empty() {
        return Ok(true);
    }
    if opts.force_credentials {
        writeln!(
            out,
            "--force-credentials was passed, so {} credential(s) will be replaced.",
            items.len()
        )?;
        return Ok(true);
    }

    writeln!(
        out,
        "\nCREDENTIALS — {} item(s) hold OAuth tokens and your local copy is newer \
         than the snapshot:",
        items.len()
    )?;
    for item in items.iter().take(MAX_ATTENTION_ITEMS) {
        writeln!(out, "    {}", safe(&item.manifest_path))?;
        if let Some((local, remote)) = mtimes(&item.disposition) {
            writeln!(
                out,
                "      local {}  ·  snapshot {}",
                local.to_rfc3339(),
                remote.to_rfc3339()
            )?;
        }
    }
    if items.len() > MAX_ATTENTION_ITEMS {
        writeln!(
            out,
            "    ... and {} more",
            items.len() - MAX_ATTENTION_ITEMS
        )?;
    }

    writeln!(
        out,
        "\n  Restoring these writes the snapshot's older token back over the one\n\
         \x20 this machine is using now. If that token has since been rotated, the\n\
         \x20 live one is gone and everything authenticated with it stops working\n\
         \x20 until you log in again.\n\
         \x20 --yes does not answer this question."
    )?;
    write!(
        out,
        "Type the word `overwrite` to replace them, anything else to leave them alone: "
    )?;
    out.flush()?;

    let mut line = String::new();
    if input.read_line(&mut line)? == 0 {
        writeln!(
            out,
            "\nno answer arrived — the credentials were left alone.\n\
             Pass --force-credentials to replace them without being asked."
        )?;
        return Ok(false);
    }
    if line.trim().eq_ignore_ascii_case("overwrite") {
        return Ok(true);
    }
    writeln!(out, "the credentials were left alone.")?;
    Ok(false)
}

// ---------------------------------------------------------------------------
// The report, in the order it prints
// ---------------------------------------------------------------------------

fn headline(plan: &RestorePlan, applying: bool) -> String {
    let counts = Counts::of(plan.items.iter());
    let mut out = format!(
        "{} — snapshot {} of {}, taken {}\n",
        if applying { "RESTORING" } else { "DRY RUN" },
        plan.counter,
        safe(&plan.repo_id),
        plan.created_at.to_rfc3339()
    );
    out.push_str(&format!(
        "  {} to create, {} to update, {} already identical\n\
         \x20 {} needing your decision, {} refused\n",
        counts.create, counts.update, counts.unchanged, counts.attention, counts.refused
    ));
    out.push_str(&format!(
        "  {} {} in {} pack(s) and write {} item(s)\n",
        if applying {
            "will fetch"
        } else {
            "would fetch"
        },
        human_bytes(plan.bytes_to_fetch),
        plan.packs_needed,
        counts.writes
    ));
    if counts.writes == 0 && counts.attention == 0 {
        out.push_str(
            "  already up to date — this machine matches the snapshot.\n\
             \x20 That is a success, not a failure.\n",
        );
    } else if counts.writes == 0 {
        out.push_str(&format!(
            "  nothing would be written; the {} item(s) below need your decision first.\n",
            counts.attention
        ));
    }
    out
}

/// Everything whose local copy is newer than the snapshot, first, in its own
/// block, visually separate from a plain skip — because these are not "nothing
/// to do", they are "this needs your decision", and the flag that resolves each
/// one is different.
fn attention_block(plan: &RestorePlan) -> String {
    let items: Vec<&ItemPlan> = plan
        .items
        .iter()
        .filter(|i| facing(&i.disposition).kind == Kind::Attention)
        .collect();
    if items.is_empty() {
        return String::new();
    }

    let mut out = format!(
        "\n  >> YOUR LOCAL COPY IS NEWER — {} item(s). Read these before answering.\n",
        items.len()
    );
    for item in items.iter().take(MAX_ATTENTION_ITEMS) {
        let f = facing(&item.disposition);
        out.push_str(&format!(
            "     {}\n       {}\n",
            safe(&item.manifest_path),
            f.verb
        ));
        if let Some((local, remote)) = mtimes(&item.disposition) {
            out.push_str(&format!(
                "       local {} · snapshot {}\n",
                local.to_rfc3339(),
                remote.to_rfc3339()
            ));
        }
        // Which flag resolves it — and it is a *different* flag for a skip than
        // for a credential. "Skipped" without the flag is a dead end.
        if let Some(flag) = f.flag {
            out.push_str(&format!("       pass {flag} to restore it\n"));
        }
    }
    if items.len() > MAX_ATTENTION_ITEMS {
        out.push_str(&format!(
            "     ... and {} more\n",
            items.len() - MAX_ATTENTION_ITEMS
        ));
    }
    out
}

/// The refusals. Usually empty; when it is not, it is the most interesting
/// thing on the screen, so it prints before the table and never gets folded
/// into a count. A path silently dropped is how a user concludes a restore was
/// complete when it was not.
fn refusals_block(plan: &RestorePlan) -> String {
    let items: Vec<&ItemPlan> = plan
        .items
        .iter()
        .filter(|i| facing(&i.disposition).kind == Kind::Refusal)
        .collect();
    if items.is_empty() {
        return String::new();
    }

    let mut out = format!(
        "\n  >> NOT RESTORED — {} item(s) in the snapshot were refused:\n",
        items.len()
    );
    for item in items.iter().take(MAX_ATTENTION_ITEMS) {
        out.push_str(&format!(
            "     {}\n       {}\n",
            safe(&item.manifest_path),
            facing(&item.disposition).verb
        ));
        if let Disposition::RejectedPath(why) = &item.disposition {
            out.push_str(&format!("       reason: {}\n", safe(why)));
        }
    }
    if items.len() > MAX_ATTENTION_ITEMS {
        out.push_str(&format!(
            "     ... and {} more\n",
            items.len() - MAX_ATTENTION_ITEMS
        ));
    }
    out
}

/// One row per category that has anything in it, in `SyncCategory::ALL` order —
/// the same canonical order `sync/report.rs` prints, and the same
/// [`human_bytes`] formatting. Categories the snapshot has nothing in are
/// omitted rather than shown as zeros; unlike `sync status` there is no "off"
/// state here that a zero could be confused with.
fn category_table(plan: &RestorePlan) -> String {
    let width = SyncCategory::ALL
        .iter()
        .map(|c| c.label().len())
        .max()
        .unwrap_or(0);

    let mut out = format!(
        "\n  {:<width$}  {:>7}  {:>7}  {:>10}  {:>9}  {:>8}  {:>10}\n",
        "", "create", "update", "identical", "needs you", "refused", "to write"
    );
    let mut total = Counts::default();
    for cat in SyncCategory::ALL {
        let counts = Counts::of(plan.items.iter().filter(|i| i.category == cat));
        if counts.total == 0 {
            continue;
        }
        out.push_str(&format!(
            "  {:<width$}  {:>7}  {:>7}  {:>10}  {:>9}  {:>8}  {:>10}\n",
            cat.label(),
            counts.create,
            counts.update,
            counts.unchanged,
            counts.attention,
            counts.refused,
            human_bytes(counts.bytes)
        ));
        total.merge(counts);
    }
    out.push_str(&format!(
        "  {:<width$}  {:>7}  {:>7}  {:>10}  {:>9}  {:>8}  {:>10}\n",
        "total",
        total.create,
        total.update,
        total.unchanged,
        total.attention,
        total.refused,
        human_bytes(total.bytes)
    ));
    out
}

/// The per-item detail for the ordinary dispositions, per category, under
/// [`MAX_ITEM_LINES_PER_CATEGORY`]. The attention and refusal items are not
/// repeated here — they had their own block, above, with their own budget.
fn category_detail(plan: &RestorePlan) -> String {
    let mut out = String::new();
    for cat in SyncCategory::ALL {
        let items: Vec<&ItemPlan> = plan
            .items
            .iter()
            .filter(|i| i.category == cat && facing(&i.disposition).kind.is_ordinary())
            .collect();
        if items.is_empty() {
            continue;
        }
        out.push_str(&format!("\n  {}:\n", cat.label()));
        for item in items.iter().take(MAX_ITEM_LINES_PER_CATEGORY) {
            let f = facing(&item.disposition);
            out.push_str(&format!("    {:<9}  {}", f.verb, safe(&item.manifest_path)));
            if item.disposition.writes() {
                out.push_str(&format!("  ({})", human_bytes(item.true_len)));
            }
            out.push('\n');
        }
        if items.len() > MAX_ITEM_LINES_PER_CATEGORY {
            out.push_str(&format!(
                "    ... and {} more\n",
                items.len() - MAX_ITEM_LINES_PER_CATEGORY
            ));
        }
    }
    out
}

/// What happens next, and — the half a dry run most often leaves out — what
/// gets archived first. `backup::take` is the user's undo and it is worthless
/// if they do not know it happened.
fn footer(plan: &RestorePlan, applying: bool) -> String {
    let counts = Counts::of(plan.items.iter());
    let mut out = String::from(if applying {
        "\n  Nothing has been written yet — this is the plan --apply is about to run.\n"
    } else {
        "\n  Nothing has been written. This is a dry run.\n"
    });
    if counts.replacing > 0 {
        out.push_str(&format!(
            "  {} of the {} item(s) to write replace a file that exists here now.\n\
             \x20 Their current contents are archived to a tar.gz in the backups\n\
             \x20 directory before the first write. That archive's path, and the one\n\
             \x20 command that undoes the whole restore, are printed when the run ends.\n",
            counts.replacing, counts.writes
        ));
    } else if counts.writes > 0 {
        out.push_str("  Every item to write is new here, so there is nothing to archive.\n");
    }
    if !applying {
        out.push_str(&format!("  To apply it, run:  {APPLY_COMMAND}\n"));
    }
    out
}

// ---------------------------------------------------------------------------
// Classification
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Create,
    Update,
    Unchanged,
    /// Local copy is newer. Not a plain skip: it needs the user's decision.
    Attention,
    /// In the snapshot, deliberately not restored.
    Refusal,
}

impl Kind {
    /// The dispositions that get a one-line entry in the per-category detail.
    /// [`Kind::Attention`] and [`Kind::Refusal`] lead the report instead.
    fn is_ordinary(self) -> bool {
        match self {
            Kind::Create | Kind::Update | Kind::Unchanged => true,
            Kind::Attention | Kind::Refusal => false,
        }
    }
}

/// How one disposition faces the user.
struct Facing {
    kind: Kind,
    verb: &'static str,
    /// The flag on the next run that changes this item's outcome, when one
    /// does. `SkipLocalNewer` and `NeedsCredentialConfirm` are resolved by
    /// *different* flags and the report has to say which.
    flag: Option<&'static str>,
    /// True when the item replaces a file that already exists locally — which
    /// is exactly the set `backup::take` archives.
    replaces_existing: bool,
}

/// The one exhaustive match over [`Disposition`] in this module.
///
/// No wildcard arm anywhere in this file, on purpose: a new disposition must be
/// a compile error here, not a case that renders as nothing and that the user
/// therefore never learns existed.
fn facing(disposition: &Disposition) -> Facing {
    match disposition {
        Disposition::Create => Facing {
            kind: Kind::Create,
            verb: "create",
            flag: None,
            replaces_existing: false,
        },
        Disposition::Update => Facing {
            kind: Kind::Update,
            verb: "update",
            flag: None,
            replaces_existing: true,
        },
        Disposition::SkipIdentical => Facing {
            kind: Kind::Unchanged,
            verb: "identical",
            flag: None,
            replaces_existing: false,
        },
        Disposition::SkipLocalNewer { .. } => Facing {
            kind: Kind::Attention,
            verb: "SKIPPED, needs your decision — the local file is newer",
            flag: Some("--force"),
            replaces_existing: false,
        },
        Disposition::NeedsCredentialConfirm { .. } => Facing {
            kind: Kind::Attention,
            verb: "NEEDS YOUR CONFIRMATION — a credential, and the local one is newer",
            flag: Some("--force-credentials"),
            replaces_existing: false,
        },
        Disposition::Overwrite { .. } => Facing {
            kind: Kind::Attention,
            verb: "WILL BE OVERWRITTEN — the local file is newer and will be replaced",
            flag: None,
            replaces_existing: true,
        },
        Disposition::ExcludedByPolicy => Facing {
            kind: Kind::Refusal,
            verb: "excluded by policy — not written to this machine",
            flag: None,
            replaces_existing: false,
        },
        Disposition::RejectedPath(_) => Facing {
            kind: Kind::Refusal,
            verb: "REFUSED — the path does not resolve inside the sync roots",
            flag: None,
            replaces_existing: false,
        },
        Disposition::ReplacesLiveCredential => Facing {
            kind: Kind::Attention,
            verb: "NEEDS YOUR CONFIRMATION — this machine already has a different login here,                    and it is not archived by the pre-restore backup",
            flag: Some("--force-credentials"),
            replaces_existing: false,
        },
        Disposition::ForeignSafeStorage => Facing {
            kind: Kind::Refusal,
            verb: "REFUSED — this Claude Desktop session is locked to the Mac that saved it \
                   and cannot be read here; sign in to Claude Desktop on this Mac",
            flag: None,
            replaces_existing: false,
        },
    }
}

/// The timestamp pair the three local-is-newer variants carry. Exhaustive, no
/// wildcard, for the same reason [`facing`] is.
fn mtimes(disposition: &Disposition) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    match disposition {
        Disposition::SkipLocalNewer {
            local_mtime,
            remote_mtime,
        }
        | Disposition::Overwrite {
            local_mtime,
            remote_mtime,
        }
        | Disposition::NeedsCredentialConfirm {
            local_mtime,
            remote_mtime,
        } => Some((*local_mtime, *remote_mtime)),
        Disposition::Create
        | Disposition::Update
        | Disposition::SkipIdentical
        | Disposition::ExcludedByPolicy
        | Disposition::RejectedPath(_)
        | Disposition::ForeignSafeStorage
        // A store has no mtime on either side; `decide_store` compares digests
        // instead of pretending otherwise.
        | Disposition::ReplacesLiveCredential => None,
    }
}

#[derive(Default, Clone, Copy)]
struct Counts {
    create: usize,
    update: usize,
    unchanged: usize,
    attention: usize,
    refused: usize,
    /// `Disposition::writes()`, the frozen predicate — not `create + update`,
    /// because `Overwrite` writes and is neither.
    writes: usize,
    /// The subset of `writes` that replaces an existing local file, i.e. what
    /// gets archived.
    replacing: usize,
    bytes: u64,
    total: usize,
}

impl Counts {
    fn of<'a>(items: impl Iterator<Item = &'a ItemPlan>) -> Counts {
        let mut c = Counts::default();
        for item in items {
            let f = facing(&item.disposition);
            match f.kind {
                Kind::Create => c.create += 1,
                Kind::Update => c.update += 1,
                Kind::Unchanged => c.unchanged += 1,
                Kind::Attention => c.attention += 1,
                Kind::Refusal => c.refused += 1,
            }
            if item.disposition.writes() {
                c.writes += 1;
                c.bytes += item.true_len;
                if f.replaces_existing {
                    c.replacing += 1;
                }
            }
            c.total += 1;
        }
        c
    }

    fn merge(&mut self, other: Counts) {
        self.create += other.create;
        self.update += other.update;
        self.unchanged += other.unchanged;
        self.attention += other.attention;
        self.refused += other.refused;
        self.writes += other.writes;
        self.replacing += other.replacing;
        self.bytes += other.bytes;
        self.total += other.total;
    }
}

/// Remote-chosen text on its way to a terminal.
///
/// A manifest path is data. A terminal that interprets it is a terminal the
/// bundle's author is scripting, so control bytes and bidirectional overrides
/// come out through the crate's existing sanitizer. The extra newline collapse
/// is what keeps one path from forging extra report lines — and from slipping
/// past the line budgets above (T-5-50, T-5-53).
///
/// `pub(super)` because `write`'s failure line needs exactly this rule and had
/// its own `{}` instead — the one output site T-5-50's mitigation missed (F-3).
pub(super) fn safe(value: &str) -> String {
    sanitize_untrusted_line(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::restore::BackupRecord;
    use std::path::PathBuf;

    /// Every test below renders a dry run unless it names the flag itself, so
    /// this shadows the two-argument one rather than repeating `, false`
    /// twelve times.
    fn render_plan(plan: &RestorePlan) -> String {
        super::render_plan(plan, false)
    }

    /// This file's own text, at compile time. Reading it costs no filesystem
    /// access at run time, so the "no third prompt" and "no wildcard" checks
    /// stay hermetic.
    const SELF_SOURCE: &str = include_str!("report.rs");

    const NOW: DateTime<Utc> = match DateTime::from_timestamp(1_700_000_000, 0) {
        Some(t) => t,
        None => panic!("a fixed timestamp"),
    };
    const LATER: DateTime<Utc> = match DateTime::from_timestamp(1_700_009_000, 0) {
        Some(t) => t,
        None => panic!("a fixed timestamp"),
    };

    fn every_disposition() -> Vec<Disposition> {
        vec![
            Disposition::Create,
            Disposition::Update,
            Disposition::SkipIdentical,
            Disposition::SkipLocalNewer {
                local_mtime: LATER,
                remote_mtime: NOW,
            },
            Disposition::Overwrite {
                local_mtime: LATER,
                remote_mtime: NOW,
            },
            Disposition::NeedsCredentialConfirm {
                local_mtime: LATER,
                remote_mtime: NOW,
            },
            Disposition::ExcludedByPolicy,
            Disposition::RejectedPath("it contains a `..` component".into()),
        ]
    }

    fn item(path: &str, disposition: Disposition) -> ItemPlan {
        ItemPlan {
            manifest_path: path.into(),
            dest: None,
            category: SyncCategory::Config,
            true_len: 1024,
            chunks: Vec::new(),
            disposition,
        }
    }

    fn plan_of(items: Vec<ItemPlan>) -> RestorePlan {
        RestorePlan {
            items,
            counter: 3,
            created_at: NOW,
            repo_id: "github:1".into(),
            packs_needed: 2,
            bytes_to_fetch: 4096,
        }
    }

    fn line_of(text: &str, needle: &str) -> usize {
        text.lines()
            .position(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} is missing from:\n{text}"))
    }

    // -- render_plan ------------------------------------------------------

    #[test]
    fn every_disposition_renders_and_names_its_own_item() {
        let items: Vec<ItemPlan> = every_disposition()
            .into_iter()
            .enumerate()
            .map(|(i, d)| item(&format!("config/item-{i}.json"), d))
            .collect();
        let count = items.len();
        let rendered = render_plan(&plan_of(items));

        for i in 0..count {
            assert!(
                rendered.contains(&format!("config/item-{i}.json")),
                "item {i} is missing from the report:\n{rendered}"
            );
        }
        assert!(rendered.contains("Nothing has been written"));
    }

    /// SAFE-03: the user is told what would change *first*. Not after a table.
    #[test]
    fn locally_newer_items_and_refusals_lead_the_report() {
        let rendered = render_plan(&plan_of(vec![
            item("config/plain.toml", Disposition::Create),
            item(
                "config/newer.toml",
                Disposition::SkipLocalNewer {
                    local_mtime: LATER,
                    remote_mtime: NOW,
                },
            ),
            item(
                "config/escapes.toml",
                Disposition::RejectedPath("it escapes the sync roots".into()),
            ),
        ]));

        let newer = line_of(&rendered, "config/newer.toml");
        let refused = line_of(&rendered, "config/escapes.toml");
        // The table's totals row. "identical" would match the headline's count
        // line and "needs you" the attention block's own wording — both above
        // the table by design.
        let table = line_of(&rendered, "total");
        let plain = line_of(&rendered, "config/plain.toml");

        assert!(
            newer < table,
            "the locally-newer block must precede the table"
        );
        assert!(refused < table, "the refusals must precede the table");
        assert!(newer < refused, "locally-newer leads");
        assert!(table < plain, "ordinary detail comes after the table");
    }

    /// They are not "nothing to do", and the flag that resolves each is
    /// different. Saying "skipped" without saying which flag is a dead end.
    #[test]
    fn a_skip_and_a_credential_are_distinct_and_name_their_different_flags() {
        let rendered = render_plan(&plan_of(vec![
            item(
                "config/newer.toml",
                Disposition::SkipLocalNewer {
                    local_mtime: LATER,
                    remote_mtime: NOW,
                },
            ),
            item(
                "config/accounts/work/.credentials.json",
                Disposition::NeedsCredentialConfirm {
                    local_mtime: LATER,
                    remote_mtime: NOW,
                },
            ),
        ]));

        assert!(rendered.contains("YOUR LOCAL COPY IS NEWER"));
        assert!(rendered.contains("needs your decision"));
        assert!(rendered.contains("NEEDS YOUR CONFIRMATION"));
        assert!(rendered.contains("pass --force to restore it"));
        assert!(rendered.contains("pass --force-credentials to restore it"));
        // Both timestamps travel with the row.
        assert!(rendered.contains(&LATER.to_rfc3339()));
        assert!(rendered.contains(&NOW.to_rfc3339()));
    }

    /// A path refused for escaping the roots must be *shown*, with its name and
    /// its reason. Silently dropping it is how a user concludes a restore was
    /// complete when it was not.
    #[test]
    fn a_rejected_path_is_shown_with_its_name_and_its_reason() {
        let rendered = render_plan(&plan_of(vec![
            item(
                "config/../../etc/passwd",
                Disposition::RejectedPath("it contains a `..` component".into()),
            ),
            item("desktop-data/blocked", Disposition::ExcludedByPolicy),
        ]));
        assert!(rendered.contains("config/../../etc/passwd"));
        assert!(rendered.contains("it contains a `..` component"));
        assert!(rendered.contains("REFUSED"));
        assert!(rendered.contains("desktop-data/blocked"));
        assert!(rendered.contains("excluded by policy"));
        assert!(
            rendered.contains("2 refused"),
            "the headline counts them too"
        );
    }

    #[test]
    fn a_five_thousand_item_plan_stays_under_the_line_ceiling() {
        let dispositions = every_disposition();
        let items: Vec<ItemPlan> = (0..5_000)
            .map(|i| {
                let mut it = item(
                    &format!("config/item-{i}.json"),
                    dispositions[i % dispositions.len()].clone(),
                );
                it.category = SyncCategory::ALL[i % SyncCategory::ALL.len()];
                it
            })
            .collect();
        let rendered = render_plan(&plan_of(items));
        let lines = rendered.lines().count();
        assert!(
            lines < 250,
            "a 5,000-item plan printed {lines} lines:\n{rendered}"
        );
        // A report is only bounded if it is bounded in both directions: a
        // 300-column paragraph is a wall too.
        for line in rendered.lines() {
            assert!(
                line.chars().count() <= 80,
                "this line runs past a terminal: {line:?}"
            );
        }
        // Bounded, but the attention and refusal blocks keep their larger
        // budget rather than being collapsed to the per-category one.
        assert!(rendered.contains("... and "));
        assert!(rendered.contains("YOUR LOCAL COPY IS NEWER"));
        assert!(rendered.contains("NOT RESTORED"));
    }

    #[test]
    fn a_plan_with_nothing_to_write_says_already_up_to_date_without_implying_failure() {
        let rendered = render_plan(&plan_of(vec![item(
            "config/config.toml",
            Disposition::SkipIdentical,
        )]));
        assert!(rendered.contains("already up to date"));
        assert!(rendered.contains("That is a success, not a failure."));
    }

    #[test]
    fn a_dry_run_says_it_is_one_what_to_run_and_what_would_be_archived() {
        let rendered = render_plan(&plan_of(vec![
            item("config/new.toml", Disposition::Create),
            item("config/existing.toml", Disposition::Update),
        ]));
        assert!(rendered.starts_with("DRY RUN"));
        assert!(rendered.contains("Nothing has been written. This is a dry run."));
        assert!(rendered.contains(APPLY_COMMAND));
        assert!(rendered.contains("1 of the 2 item(s) to write replace a file"));
        assert!(rendered.contains("archived to a tar.gz"));
    }

    #[test]
    fn an_all_new_plan_says_there_is_nothing_to_archive_rather_than_promising_one() {
        let rendered = render_plan(&plan_of(vec![item("config/new.toml", Disposition::Create)]));
        assert!(rendered.contains("nothing to archive"));
        assert!(!rendered.contains("archived to a tar.gz"));
    }

    /// T-5-50: a manifest path is attacker-chosen data, and a terminal that
    /// interprets it is a terminal the bundle's author is scripting.
    #[test]
    fn a_terminal_escape_in_a_remote_string_never_reaches_the_terminal() {
        let hostile = render_plan(&plan_of(vec![item(
            "config/\x1b]52;c;Y2xpcGJvYXJk\x07pwn\nDRY RUN — a forged line\n  and another",
            Disposition::RejectedPath("reason\x1b[31m\u{202e}spoof\nsecond line".into()),
        )]));
        assert!(!hostile.contains('\x1b'), "an ESC survived:\n{hostile:?}");
        assert!(!hostile.contains('\u{202e}'), "a bidi override survived");

        // The path may legitimately *contain* any printable text, so the
        // property is structural: a hostile string occupies exactly the lines a
        // benign one does, and so cannot forge a row or slip past a budget.
        let benign = render_plan(&plan_of(vec![item(
            "config/ordinary.toml",
            Disposition::RejectedPath("an ordinary reason".into()),
        )]));
        assert_eq!(
            hostile.lines().count(),
            benign.lines().count(),
            "a remote string forged report lines:\n{hostile}"
        );
    }

    /// The report carries paths, counts, timestamps and byte totals. Nothing
    /// else is representable — no model here has a field for plaintext or a
    /// URL — and this pins the byte formatting to the crate's one helper.
    #[test]
    fn byte_counts_render_through_the_crates_own_formatting() {
        let mut it = item("config/big.toml", Disposition::Create);
        it.true_len = 4096;
        let rendered = render_plan(&plan_of(vec![it]));
        assert!(rendered.contains(&human_bytes(4096)));
        assert!(rendered.contains("4.0 KiB"));
    }

    // -- render_outcome ---------------------------------------------------

    fn outcome_of(
        applied: bool,
        overwritten: Vec<String>,
        failed_at: Option<String>,
    ) -> RestoreOutcome {
        RestoreOutcome {
            plan: plan_of(vec![item("config/config.toml", Disposition::Update)]),
            applied,
            backup: Some(BackupRecord {
                archive: PathBuf::from("/backups/restore-3.tar.gz"),
                root: PathBuf::from("/roots"),
                members: 2,
                bytes: 2048,
            }),
            written: 2,
            overwritten,
            skipped: 1,
            failed_at,
        }
    }

    #[test]
    fn an_outcome_names_what_it_overwrote_rather_than_counting_it() {
        let rendered = render_outcome(&outcome_of(
            true,
            vec![
                "config/config.toml".into(),
                "config/accounts/work/.credentials.json".into(),
            ],
            None,
        ));
        assert!(rendered.contains("config/config.toml"));
        assert!(rendered.contains("config/accounts/work/.credentials.json"));
        assert!(rendered.contains("2 written"));
        assert!(rendered.contains("/backups/restore-3.tar.gz"));
        assert!(rendered.contains("tar -xzf"));
    }

    #[test]
    fn a_partial_failure_names_where_it_stopped_and_prints_the_rollback_at_the_bottom() {
        let rendered = render_outcome(&outcome_of(
            true,
            vec!["config/config.toml".into()],
            Some("config/half.toml".into()),
        ));
        assert!(rendered.starts_with("RESTORE INCOMPLETE"));
        assert!(rendered.contains("config/half.toml"));
        let rollbacks = rendered.matches("tar -xzf").count();
        assert_eq!(rollbacks, 2, "the rollback must repeat at the bottom");
        let last = rendered.trim_end().lines().last().unwrap();
        assert!(last.contains("tar -xzf"), "last line was {last:?}");
    }

    #[test]
    fn an_applied_run_with_no_archive_and_an_overwrite_says_there_is_no_undo() {
        let mut outcome = outcome_of(true, vec!["config/config.toml".into()], None);
        outcome.backup = None;
        let rendered = render_outcome(&outcome);
        assert!(rendered.contains("no archive was recorded"));
        assert!(!rendered.contains("tar -xzf"));
    }

    #[test]
    fn a_dry_run_outcome_renders_the_plan_and_nothing_about_writes() {
        let rendered = render_outcome(&outcome_of(false, Vec::new(), None));
        assert_eq!(
            rendered,
            render_plan(&plan_of(vec![item(
                "config/config.toml",
                Disposition::Update,
            )]))
        );
        assert!(rendered.starts_with("DRY RUN"));
        // Not one word about writes that did not happen — no counts, no
        // overwrite list, no archive, no rollback command.
        for absent in ["RESTORED", "Overwrote", "Archived", "tar -xzf"] {
            assert!(!rendered.contains(absent), "a dry run mentioned {absent:?}");
        }
    }

    // -- the gates --------------------------------------------------------

    fn gate(answer: &str, opts: RestoreOptions) -> (bool, String) {
        let plan = plan_of(vec![item("config/config.toml", Disposition::Update)]);
        let mut out = Vec::new();
        let mut src = answer.as_bytes();
        let ok = confirm_apply(&plan, &opts, &mut out, &mut src).unwrap();
        (ok, String::from_utf8(out).unwrap())
    }

    /// The report under `--apply` describes what is about to happen. The plan
    /// phase always runs with `apply` off — that is how the report exists at all
    /// — and printing "DRY RUN", then telling the user to re-run the very
    /// command they just ran, is the one thing it must not do.
    #[test]
    fn an_apply_report_names_neither_a_dry_run_nor_the_command_already_given() {
        let plan = plan_of(vec![item("config/new.toml", Disposition::Create)]);

        let applying = super::render_plan(&plan, true);
        assert!(applying.starts_with("RESTORING"), "{applying}");
        assert!(!applying.contains("DRY RUN"), "{applying}");
        assert!(!applying.contains("To apply it, run"), "{applying}");
        assert!(!applying.contains(APPLY_COMMAND), "{applying}");
        assert!(!applying.contains("dry run"), "{applying}");

        // …and a dry run still says both.
        let dry = render_plan(&plan);
        assert!(dry.starts_with("DRY RUN"), "{dry}");
        assert!(dry.contains(APPLY_COMMAND), "{dry}");
    }

    #[test]
    fn confirm_apply_accepts_only_an_explicit_affirmative() {
        for yes in ["y\n", "Y\n", "yes\n", " yes \n"] {
            assert!(
                gate(yes, RestoreOptions::default()).0,
                "{yes:?} was refused"
            );
        }
        // A bare newline, an EOF, a "no", and anything unrecognised are all
        // refusals. Consent to an irreversible overwrite is never the default.
        for no in ["\n", "", "n\n", "no\n", "sure\n", "yes please\n"] {
            assert!(
                !gate(no, RestoreOptions::default()).0,
                "{no:?} was accepted"
            );
        }
    }

    #[test]
    fn confirm_apply_shows_the_plan_before_it_asks() {
        let (_, text) = gate("y\n", RestoreOptions::default());
        assert!(text.contains("config/config.toml"));
        assert!(line_of(&text, "config/config.toml") < line_of(&text, "Apply this restore"));
    }

    #[test]
    fn assume_yes_returns_true_without_asking_a_question() {
        let (ok, text) = gate(
            "",
            RestoreOptions {
                assume_yes: true,
                ..RestoreOptions::default()
            },
        );
        assert!(ok);
        assert!(!text.contains("[y/N]"));
        assert!(text.contains("config/config.toml"), "the plan still prints");
    }

    /// T-5-55: a run with nothing on stdin never blocks and never assumes
    /// consent — it refuses and names the flag.
    #[test]
    fn an_empty_stdin_refuses_and_names_the_flag() {
        let (ok, text) = gate("", RestoreOptions::default());
        assert!(!ok);
        assert!(text.contains("--yes"));
        assert!(text.contains("nothing was written"));
    }

    fn cred_gate(answer: &str, opts: RestoreOptions) -> (bool, String) {
        let it = item(
            "config/accounts/work/.credentials.json",
            Disposition::NeedsCredentialConfirm {
                local_mtime: LATER,
                remote_mtime: NOW,
            },
        );
        let items = [&it];
        let mut out = Vec::new();
        let mut src = answer.as_bytes();
        let ok = confirm_credentials(&items, &opts, &mut out, &mut src).unwrap();
        (ok, String::from_utf8(out).unwrap())
    }

    /// T-5-52, the critical one: `--yes` must not be able to answer this gate.
    /// `assume_yes` is not read in `confirm_credentials` at all.
    #[test]
    fn assume_yes_alone_leaves_the_credential_gate_refusing() {
        let opts = RestoreOptions {
            assume_yes: true,
            ..RestoreOptions::default()
        };
        assert!(!cred_gate("", opts).0, "--yes answered the credential gate");
        assert!(!cred_gate("\n", opts).0);
        assert!(!cred_gate("y\n", opts).0, "[y/N] is not the question");
        // The needles are built at run time so this assertion is not itself
        // the text it is searching for.
        let body = SELF_SOURCE
            .split(&format!("pub {} confirm_credentials", "fn"))
            .nth(1)
            .expect("confirm_credentials is defined here")
            .split(&"-".repeat(20))
            .next()
            .expect("the section divider closes the function");
        assert!(
            !body.contains("assume_yes"),
            "confirm_credentials must not read assume_yes"
        );
    }

    #[test]
    fn the_credential_gate_requires_the_word_and_force_credentials_short_circuits() {
        assert!(cred_gate("overwrite\n", RestoreOptions::default()).0);
        assert!(cred_gate("OVERWRITE\n", RestoreOptions::default()).0);
        assert!(!cred_gate("y\n", RestoreOptions::default()).0);
        assert!(!cred_gate("yes\n", RestoreOptions::default()).0);

        let (ok, text) = cred_gate(
            "",
            RestoreOptions {
                force_credentials: true,
                ..RestoreOptions::default()
            },
        );
        assert!(ok);
        assert!(text.contains("--force-credentials"));
    }

    #[test]
    fn the_credential_gate_names_the_stale_token_failure_mode_and_the_paths() {
        let (_, text) = cred_gate("no\n", RestoreOptions::default());
        assert!(text.contains("config/accounts/work/.credentials.json"));
        assert!(text.contains("rotated"));
        assert!(text.contains("--yes does not answer this question"));
        assert!(text.contains(&LATER.to_rfc3339()));
        // The path and the times, and no contents: nothing here can carry any.
        assert!(!text.contains("token="));
    }

    #[test]
    fn an_empty_credential_set_is_vacuous_consent_and_prints_nothing() {
        let mut out = Vec::new();
        let mut src = &b""[..];
        assert!(confirm_credentials(&[], &RestoreOptions::default(), &mut out, &mut src).unwrap());
        assert!(out.is_empty());
    }

    // -- the shape of the module itself -----------------------------------

    /// D6: exactly one interactive gate, plus D2's separate credential
    /// consent. A third prompt anywhere here is the restore that nobody
    /// finishes.
    #[test]
    fn there_are_exactly_two_gates_and_no_third_prompt() {
        // Needles built at run time: a literal here would count itself.
        assert_eq!(
            SELF_SOURCE
                .matches(&format!("pub {} confirm", "fn"))
                .count(),
            2,
            "there must be exactly two gate functions"
        );
        assert_eq!(
            SELF_SOURCE
                .matches(&format!("input.{}(", "read_line"))
                .count(),
            2,
            "every terminal read must belong to one of the two gates"
        );
    }

    /// A wildcard arm would render a newly added disposition as nothing, and
    /// the user would never learn the item existed. Built at run time so this
    /// assertion is not itself the match it is looking for.
    #[test]
    fn no_match_over_a_disposition_falls_through_a_wildcard() {
        let wildcard = format!("_ {}", "=>");
        for (n, line) in SELF_SOURCE.lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with("//") {
                continue;
            }
            assert!(
                !code.contains(&wildcard),
                "line {} has a wildcard arm: {line}",
                n + 1
            );
        }
    }
}
