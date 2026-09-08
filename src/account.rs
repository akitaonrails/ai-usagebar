//! Administrative commands for named Claude accounts.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use crate::anthropic::cli_account::{self, CliSwitchOpts, CliSwitchOutcome, KeychainStore};
use crate::claude_desktop::{self, Paths, SwitchOpts, SwitchPlan};
use crate::config::Config;
use crate::error::{AppError, Result};
use crate::widget::cli::AccountAction;

struct Registered {
    config_path: PathBuf,
    credential_file: PathBuf,
    account_dir: PathBuf,
    credential_display: String,
    already_existed: bool,
    anthropic_enabled: bool,
}

impl Registered {
    fn supports_scoped_login(&self) -> bool {
        self.credential_file
            .file_name()
            .and_then(|name| name.to_str())
            == Some(".credentials.json")
    }
}

/// Run an administrative account command and return its process exit code.
#[must_use]
pub fn run(action: &AccountAction) -> i32 {
    match action {
        AccountAction::Add {
            label,
            no_login,
            desktop,
            email,
            yes,
        } => {
            if *desktop {
                add_desktop(label, email.as_deref(), *yes)
            } else {
                add(label, !no_login)
            }
        }
        AccountAction::Status { json } => status(*json),
        AccountAction::Switch {
            label,
            desktop,
            cli,
            dry_run,
            yes,
            force,
            keep_bridge,
            backup_sessions,
            keep_backups,
            delete_conflict,
        } => switch(&SwitchArgs {
            label,
            desktop: *desktop,
            cli: *cli,
            dry_run: *dry_run,
            yes: *yes,
            force: *force,
            delete_conflict: delete_conflict.clone(),
            opts: SwitchOpts {
                keep_bridge: *keep_bridge,
                backup_sessions: *backup_sessions,
                keep_backups: *keep_backups,
            },
        }),
    }
}

/// A config that fails to parse must not blank out the account report: the
/// menu bar polls `account status` while the file is being edited. Read-only
/// status can safely fall back to the conventional account locations.
fn config_or_default() -> Config {
    Config::load().unwrap_or_else(|error| {
        eprintln!("ai-usagebar account: using defaults, config.toml did not parse: {error}");
        Config::default()
    })
}

/// Read-only status can degrade while a config is mid-edit; commands that
/// change credentials or Desktop state cannot. Falling back there could select
/// the default profile store instead of the user's configured one.
fn config_for_mutation(loaded: Result<Config>) -> Result<Config> {
    loaded.map_err(|error| {
        AppError::Other(format!(
            "refusing to change account state because config.toml did not parse: {error}"
        ))
    })
}

fn status(json: bool) -> i32 {
    let config = config_or_default();
    let accounts = config.anthropic.all_accounts();
    let paths = Paths::resolve(&config.anthropic)
        .ok()
        .filter(Paths::available);

    // A Desktop profile captured without an e-mail can still be named: the
    // same account signed into the CLI records one, keyed by the same UUID.
    let emails_by_uuid: Vec<(String, String)> = accounts
        .iter()
        .filter_map(|account| {
            let marker = cli_account::marker_path(&account.config_dir());
            Some((
                cli_account::account_uuid_in(&marker)?,
                cli_account::account_email_in(&marker)?,
            ))
        })
        .collect();
    let email_for = |uuid: &str| -> Option<&str> {
        emails_by_uuid
            .iter()
            .find(|(known, _)| known == uuid)
            .map(|(_, email)| email.as_str())
    };

    let desktop = paths.as_ref().map(|paths| {
        let profiles = claude_desktop::load_profiles(&paths.profiles_dir);
        let sessions_root = paths.sessions_root();
        let active_uuid = claude_desktop::active_account_uuid(&paths.config_json());
        let active_label = active_uuid
            .as_deref()
            .and_then(|uuid| claude_desktop::label_for_uuid(&profiles, uuid))
            .map(str::to_string);
        let rows: Vec<serde_json::Value> = profiles
            .iter()
            .map(|profile| {
                serde_json::json!({
                    "label": profile.label,
                    "email": profile
                        .email
                        .as_deref()
                        .or_else(|| email_for(&profile.account_uuid)),
                    "account_uuid": profile.account_uuid,
                    "org_uuid": profile.org_uuid,
                    "has_credentials": profile.has_credentials,
                    "has_desktop_state": profile.has_desktop_state,
                    "sessions": claude_desktop::session_count(&sessions_root, profile),
                    "active": Some(&profile.label) == active_label.as_ref(),
                })
            })
            .collect();
        // Routines one account deleted that another still holds. Reported here
        // so the macOS menu bar can ask before it starts a switch, then pass
        // the answer back with `--delete-conflict`.
        let named = |uuid: &str| {
            claude_desktop::label_for_uuid(&profiles, uuid)
                .unwrap_or(uuid)
                .to_string()
        };
        let conflicts: Vec<serde_json::Value> = claude_desktop::merge::deletion_candidates(
            &sessions_root,
            &claude_desktop::load_synced(&paths.synced_path()),
        )
        .iter()
        .map(|candidate| {
            serde_json::json!({
                "key": candidate.external_key(),
                "id": candidate.id,
                "kind": candidate.kind.label(),
                "summary": candidate.summary,
                "deleted_by": named(&candidate.deleted_by),
                "still_in": candidate.still_in.iter().map(|uuid| named(uuid)).collect::<Vec<_>>(),
            })
        })
        .collect();
        serde_json::json!({
            "available": true,
            "data_dir": paths.data_dir,
            "profiles_dir": paths.profiles_dir,
            "active_label": active_label,
            "active_account_uuid": active_uuid,
            "profiles": rows,
            "deletion_conflicts": conflicts,
        })
    });

    let home = cli_account::home_claude_json().ok();
    let cli_active = home
        .as_deref()
        .and_then(|home| cli_account::resolve_active_label(home, &accounts));
    let cli_rows: Vec<serde_json::Value> = accounts
        .iter()
        .map(|account| {
            let marker = cli_account::marker_path(&account.config_dir());
            serde_json::json!({
                "label": account.label,
                "email": cli_account::account_email_in(&marker),
                "account_uuid": cli_account::account_uuid_in(&marker),
                "config_dir": account.config_dir(),
                "active": Some(&account.label) == cli_active.as_ref(),
            })
        })
        .collect();
    let cli = serde_json::json!({
        "active_label": cli_active,
        "active_account_uuid": home.as_deref().and_then(cli_account::account_uuid_in),
        "accounts": cli_rows,
    });

    // The menu bar consumes this exact TUI enumeration instead of duplicating
    // profile paths and CLI/Desktop dedup policy in Swift.
    let usage_accounts: Vec<serde_json::Value> = crate::tui::app::tabs_with_desktop(&config)
        .into_iter()
        .filter(|tab| tab.vendor_id() == Some(crate::vendor::VendorId::Anthropic))
        .filter_map(|tab| {
            Some(serde_json::json!({
                "label": tab.account?,
                "desktop": tab.desktop,
            }))
        })
        .collect();
    let report = serde_json::json!({
        "desktop": desktop,
        "cli": cli,
        "usage_accounts": usage_accounts,
    });
    if json {
        println!("{report}");
        return 0;
    }
    print_status(&report);
    0
}

fn print_status(report: &serde_json::Value) {
    for line in status_lines(report) {
        println!("{line}");
    }
}

/// Render the status report. Separate from printing it so the rendering — the
/// half with the fallbacks for absent fields, and the note about a live login
/// nothing here accounts for — can be read back in a test.
fn status_lines(report: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    match &report["desktop"] {
        serde_json::Value::Null => {
            out.push("Claude Desktop   not found (macOS only)".to_string());
        }
        desktop => {
            out.push(format!(
                "Claude Desktop   {}",
                desktop["data_dir"].as_str().unwrap_or("?")
            ));
            let profiles = desktop["profiles"]
                .as_array()
                .map_or(&[][..], Vec::as_slice);
            if profiles.is_empty() {
                out.push(format!(
                    "  no saved accounts in {} — capture one with \
                     `ai-usagebar account add <label> --desktop`",
                    desktop["profiles_dir"].as_str().unwrap_or("?")
                ));
            }
            for profile in profiles {
                out.push(format!(
                    "  {:<12} {:<28} sessions={:<5} creds={:<4} state={:<4}{}",
                    profile["label"].as_str().unwrap_or("?"),
                    profile["email"].as_str().unwrap_or("(email unknown)"),
                    profile["sessions"].as_u64().unwrap_or(0),
                    yes_no(&profile["has_credentials"]),
                    yes_no(&profile["has_desktop_state"]),
                    active_tag(&profile["active"]),
                ));
            }
        }
    }

    out.push(String::new());
    out.push("Claude Code      the `claude` CLI's default login".to_string());
    let accounts = report["cli"]["accounts"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    if accounts.is_empty() {
        out.push(
            "  no named accounts — add one with `ai-usagebar account add <label>`".to_string(),
        );
    }
    for account in accounts {
        out.push(format!(
            "  {:<12} {:<28}{}",
            account["label"].as_str().unwrap_or("?"),
            account["email"].as_str().unwrap_or("(email unknown)"),
            active_tag(&account["active"]),
        ));
    }
    if report["cli"]["active_label"].is_null() && !accounts.is_empty() {
        out.push(
            "  note: the live CLI login belongs to no account listed here, so its \
             saved copies may be stale."
                .to_string(),
        );
    }
    out
}

fn yes_no(value: &serde_json::Value) -> &'static str {
    if value.as_bool().unwrap_or(false) {
        "yes"
    } else {
        "no"
    }
}

fn active_tag(value: &serde_json::Value) -> &'static str {
    if value.as_bool().unwrap_or(false) {
        "  [active]"
    } else {
        ""
    }
}

/// Capture a Claude Desktop account. Unlike the CLI half, this cannot happen
/// quietly in the background: the app has one login slot, so the only way to
/// obtain a second account's credential is to sign it out and have the user
/// sign back in as the account being saved.
fn add_desktop(label: &str, email: Option<&str>, assume_yes: bool) -> i32 {
    if let Err(error) = crate::config::validate_account_label(label) {
        eprintln!("ai-usagebar account add: {error}");
        return 1;
    }
    let config = match config_for_mutation(Config::load()) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("ai-usagebar account add: {error}");
            return 1;
        }
    };
    let paths = match Paths::resolve(&config.anthropic) {
        Ok(paths) if paths.available() => paths,
        Ok(paths) => {
            eprintln!(
                "ai-usagebar account add: no Claude Desktop app data at {} (macOS only)",
                paths.data_dir.display()
            );
            return 1;
        }
        Err(error) => {
            eprintln!("ai-usagebar account add: {error}");
            return 1;
        }
    };

    let profiles = claude_desktop::load_profiles(&paths.profiles_dir);
    let active = claude_desktop::active_account_uuid(&paths.config_json())
        .and_then(|uuid| claude_desktop::label_for_uuid(&profiles, &uuid).map(str::to_string));

    println!("Capturing a Claude Desktop account as {label:?}.");
    println!("  The app will close and reopen at its login screen, where you sign in as the");
    println!("  account you want to save. Nothing else on this machine is touched.");
    match &active {
        Some(previous) => println!(
            "  Your current account ({previous:?}) is saved first, so switching back is intact."
        ),
        None => println!(
            "  Your current login is copied to {} first and restored if you cancel.",
            paths.prelogin_dir().display()
        ),
    }
    if !assume_yes && !confirm("  Close Claude and start the login?") {
        println!("Aborted (pass -y to skip this prompt).");
        return 1;
    }
    // Cosmetic, and the only chance to ask: once the app is signed out this
    // process is polling, not reading stdin.
    let email = email
        .map(str::to_string)
        .or_else(|| ask("  E-mail for this account (optional, for display): "));
    println!("  Waiting for sign-in (up to five minutes); press Ctrl-C to cancel and restore.");

    let mut notes = Vec::new();
    let outcome = claude_desktop::capture::capture_profile(
        &paths,
        label,
        email.as_deref(),
        &claude_desktop::app::DesktopApp,
        claude_desktop::capture::WaitOpts::default(),
        &mut notes,
    );
    for note in &notes {
        println!("  note: {note}");
    }
    match outcome {
        Err(error) => {
            eprintln!("ai-usagebar account add: {error}");
            1
        }
        Ok(claude_desktop::capture::CaptureOutcome::TimedOut) => {
            eprintln!(
                "No login detected in time — your previous account was restored. \
                 Re-run when you are ready to sign in."
            );
            1
        }
        Ok(claude_desktop::capture::CaptureOutcome::Cancelled) => {
            eprintln!("Capture cancelled — your previous account was restored.");
            1
        }
        Ok(claude_desktop::capture::CaptureOutcome::AlreadySaved(existing)) => {
            println!(
                "That account is already saved as {existing:?} — nothing new to add. \
                 You are left signed into it."
            );
            0
        }
        Ok(claude_desktop::capture::CaptureOutcome::Captured(captured)) => {
            println!(
                "  seeded         {} session(s) and {} routine(s) from your other accounts",
                captured.seeded_sessions, captured.seeded_routines
            );
            println!();
            println!(
                "Added Claude Desktop account {label:?}. Switch to it with:\n\
                 \n  ai-usagebar account switch {label} --desktop\n"
            );
            0
        }
    }
}

struct SwitchArgs<'a> {
    label: &'a str,
    desktop: bool,
    cli: bool,
    dry_run: bool,
    yes: bool,
    force: bool,
    /// Routine ids an already-answered dialog confirmed for deletion. Non-empty
    /// means the decision is made and nothing is prompted.
    delete_conflict: Vec<String>,
    opts: SwitchOpts,
}

fn switch(args: &SwitchArgs) -> i32 {
    let config = match config_for_mutation(Config::load()) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("ai-usagebar account switch: {error}");
            return 1;
        }
    };
    // Neither flag means both surfaces. A label that only exists on one side is
    // then a skip, not a failure: the two namespaces are independent and may
    // legitimately hold different sets of accounts.
    let both = !args.desktop && !args.cli;
    let mut failed = false;
    let mut acted = false;

    if args.desktop || both {
        match switch_desktop(&config, args, both) {
            Ok(true) => acted = true,
            Ok(false) => {}
            Err(error) => {
                eprintln!("ai-usagebar account switch: {error}");
                failed = true;
            }
        }
    }
    if args.cli || both {
        if acted {
            println!();
        }
        match switch_cli(&config, args, both) {
            Ok(true) => acted = true,
            Ok(false) => {}
            Err(error) => {
                eprintln!("ai-usagebar account switch: {error}");
                failed = true;
            }
        }
    }

    if !acted && !failed {
        eprintln!(
            "ai-usagebar account switch: nothing to do for {:?}. \
             Run `ai-usagebar account status` to see the known accounts.",
            args.label
        );
        return 1;
    }
    i32::from(failed)
}

/// `Ok(false)` means "this surface does not know that label, and the other one
/// might" — reported as a note rather than an error.
fn switch_desktop(config: &Config, args: &SwitchArgs, tolerant: bool) -> Result<bool> {
    let paths = Paths::resolve(&config.anthropic)?;
    if !paths.available() {
        if !tolerant {
            return Err(AppError::Other(format!(
                "no Claude Desktop app data at {} (macOS only)",
                paths.data_dir.display()
            )));
        }
        return Ok(false);
    }

    // Keep planning and mutation in one process-wide transaction. Two menu or
    // terminal switches planned against the same live identity must not race.
    let _lock = crate::cache::acquire_lock(
        &paths.account_switch_lock(),
        claude_desktop::ACCOUNT_LOCK_TIMEOUT,
    )?;

    let plan = match claude_desktop::plan_switch(&paths, args.label, args.opts.clone()) {
        Ok(plan) => plan,
        Err(error) if tolerant => {
            println!("Claude Desktop   skipped: {error}");
            return Ok(false);
        }
        Err(error) => return Err(error),
    };
    let mut plan = plan;
    print_plan(&plan);

    if args.dry_run {
        // Report the conflicts, but never ask: a dry run must not leave the
        // user having made a decision that was then thrown away.
        resolve_deletions(&paths, &mut plan, args, false);
        println!("  (dry run — nothing was changed)");
        return Ok(true);
    }
    resolve_deletions(&paths, &mut plan, args, std::io::stdin().is_terminal());
    if !args.yes
        && !confirm(&format!(
            "  Quit and relaunch the Claude Desktop app as {:?}?",
            plan.target.label
        ))
    {
        println!("  aborted (pass -y to skip this prompt)");
        return Ok(false);
    }

    let notes = claude_desktop::apply_switch(&paths, &plan, &claude_desktop::app::DesktopApp)?;
    for note in &notes {
        println!("  note: {note}");
    }
    println!(
        "  switched — the app is reopening as {:?}.",
        plan.target.label
    );
    Ok(true)
}

/// How many conflicts to spell out before summarising the rest. A switch after
/// a big clean-up should not scroll the decision off the screen.
const DELETION_PREVIEW: usize = 10;

/// Ask what to do about schedules an account deleted that others still hold.
///
/// Deleting is deliberately reachable only from an answered prompt: `-y` skips
/// the quit confirmation, but must not silently erase routines, and a
/// non-interactive run (the menu bar's subprocess, a pipe) keeps everything and
/// says so. Keeping is always the recoverable choice — the task simply comes
/// back, and the prompt returns next time.
fn resolve_deletions(paths: &Paths, plan: &mut SwitchPlan, args: &SwitchArgs, interactive: bool) {
    if plan.deletions.is_empty() {
        return;
    }
    if !args.delete_conflict.is_empty() {
        // The caller already asked — the macOS menu bar's dialog, or a script
        // that listed `deletion_conflicts` from `account status --json`.
        plan.confirmed_deletions = authorized_deletions(&plan.deletions, &args.delete_conflict);
        println!(
            "  deletions       deleting {} of {} conflict(s) everywhere, as chosen",
            plan.confirmed_deletions.len(),
            plan.deletions.len()
        );
        return;
    }
    let profiles = claude_desktop::load_profiles(&paths.profiles_dir);
    let name = |uuid: &str| {
        claude_desktop::label_for_uuid(&profiles, uuid)
            .unwrap_or(uuid)
            .to_string()
    };

    println!();
    println!(
        "Deleted elsewhere  {} item(s) deleted in one account, still present in another",
        plan.deletions.len()
    );
    for (index, candidate) in plan.deletions.iter().take(DELETION_PREVIEW).enumerate() {
        let others: Vec<String> = candidate.still_in.iter().map(|uuid| name(uuid)).collect();
        println!(
            "  {:>2}. {:<7} {:<34} deleted in {}, still in {}",
            index + 1,
            candidate.kind.label(),
            candidate.summary,
            name(&candidate.deleted_by),
            others.join(", ")
        );
    }
    if plan.deletions.len() > DELETION_PREVIEW {
        println!(
            "      … and {} more",
            plan.deletions.len() - DELETION_PREVIEW
        );
    }

    if !interactive {
        println!(
            "  Keeping all of them (no terminal to ask). Re-run the switch from a \
             terminal to decide."
        );
        return;
    }

    println!();
    println!("  [k] keep them all (default)      — they stay, and this asks again next time");
    println!("  [d] delete them everywhere       — removed from every account");
    println!("  [c] choose individually");
    println!(
        "      (a deleted chat loses only its index — the transcript in \
         ~/.claude/projects stays)"
    );
    let answer = ask("> ").unwrap_or_default().to_ascii_lowercase();
    match answer.as_str() {
        "d" | "delete" => {
            plan.confirmed_deletions = plan.deletions.iter().map(|c| c.key()).collect();
            println!(
                "  deleting {} item(s) everywhere",
                plan.confirmed_deletions.len()
            );
        }
        "c" | "choose" => choose_deletions_individually(plan, &name),
        _ => println!("  keeping all of them"),
    }
}

/// Which of this plan's deletions the caller's `--delete-conflict` keys
/// authorize.
///
/// Only keys naming a conflict *this* plan actually found are honoured. The
/// keys are opaque and carry the deleting account and the holders, so an answer
/// given against an older observation — a menu-bar dialog left open, a script
/// working from a previous `account status --json` — cannot authorize the
/// deletion of a later item that happens to reuse the same id. An unrecognised
/// key is dropped rather than treated as a match.
fn authorized_deletions(
    deletions: &[claude_desktop::merge::DeletionCandidate],
    requested: &[String],
) -> BTreeSet<claude_desktop::merge::DeletionKey> {
    let known: BTreeMap<String, claude_desktop::merge::DeletionKey> = deletions
        .iter()
        .map(|candidate| (candidate.external_key(), candidate.key()))
        .collect();
    requested
        .iter()
        .filter_map(|key| known.get(key).cloned())
        .collect()
}

/// Read a "numbers to KEEP" answer as a set of 1-based indices.
///
/// `Err` carries every token that names nothing in the list, and the caller
/// keeps everything when it does. Acting on the readable half of a mistyped
/// answer would delete whatever the unreadable half meant to save, and guessing
/// which item that was is not worth erasing one over.
fn parse_keep_list(
    answer: &str,
    count: usize,
) -> std::result::Result<BTreeSet<usize>, Vec<String>> {
    let valid = 1..=count;
    let unknown: Vec<String> = answer
        .split_whitespace()
        .filter(|token| !matches!(token.parse::<usize>(), Ok(n) if valid.contains(&n)))
        .map(str::to_string)
        .collect();
    if !unknown.is_empty() {
        return Err(unknown);
    }
    Ok(answer
        .split_whitespace()
        .filter_map(|token| token.parse::<usize>().ok())
        .collect())
}

/// Pick which to keep by number; everything unlisted goes. Asking for the
/// keepers rather than the doomed makes the destructive answer the one you have
/// to type deliberately, and a blank line the harmless one.
fn choose_deletions_individually(plan: &mut SwitchPlan, name: &impl Fn(&str) -> String) {
    println!();
    for (index, candidate) in plan.deletions.iter().enumerate() {
        println!(
            "  {:>2}. {:<34} deleted in {}",
            index + 1,
            candidate.summary,
            name(&candidate.deleted_by)
        );
    }
    println!();
    println!("  Numbers to KEEP, space separated (blank = keep none, deletes all):");
    let picked = ask("> ").unwrap_or_default();
    let keep = match parse_keep_list(&picked, plan.deletions.len()) {
        Ok(keep) => keep,
        Err(unknown) => {
            println!("  {unknown:?} is not in the list — keeping everything instead.");
            return;
        }
    };
    plan.confirmed_deletions = plan
        .deletions
        .iter()
        .enumerate()
        .filter(|(index, _)| !keep.contains(&(index + 1)))
        .map(|(_, candidate)| candidate.key())
        .collect();
    println!(
        "  keeping {}, deleting {} everywhere",
        keep.len(),
        plan.confirmed_deletions.len()
    );
}

fn print_plan(plan: &SwitchPlan) {
    for line in plan_lines(plan) {
        println!("{line}");
    }
}

/// Render what a switch would do, before anything is touched. Separate from
/// printing it because this is the text a user reads to decide whether to go
/// ahead with a destructive operation: it has to be exact about what is
/// skipped, what is only kept locally, and whether a rollback exists.
fn plan_lines(plan: &SwitchPlan) -> Vec<String> {
    let mut out = Vec::new();
    let email = plan.target.email.as_deref().unwrap_or("email unknown");
    out.push(format!(
        "Claude Desktop   → {} ({email})",
        plan.target.label
    ));
    if let Some(outgoing) = &plan.outgoing {
        out.push(format!(
            "  saving          {outgoing}'s credential and browser state first"
        ));
    }
    match plan.target.org_uuid {
        Some(_) => {
            let (new_routines, edited_routines, conflicts) =
                plan.scheduled.as_ref().map_or((0, 0, 0), |merge| {
                    (merge.added, merge.updated, merge.conflicts)
                });
            out.push(format!(
                "  history         {} new + {} refreshed session(s)",
                plan.sessions.copied.len(),
                plan.sessions.updated.len(),
            ));
            out.push(format!(
                "  routines        {new_routines} new + {edited_routines} edited elsewhere"
            ));
            if conflicts > 0 {
                out.push(format!(
                    "  routine conflicts {conflicts} kept local (edit the desired copy to resolve)"
                ));
            }
        }
        None => {
            out.push("  history         skipped (no org recorded for this account yet)".to_string())
        }
    }
    out.push("  credential      swap to the saved Desktop login".to_string());
    out.push(format!(
        "  browser state   {}",
        if plan.restores_desktop_state {
            "restore this account's cookies and local storage"
        } else {
            "none saved yet"
        }
    ));
    if !plan.opts.keep_bridge {
        out.push("  remote bridge   clear (a stale session id breaks /remote-control)".to_string());
    }
    if !plan.archive_members.is_empty() {
        out.push(format!("  rollback        {}", plan.archive.display()));
    }
    out
}

fn switch_cli(config: &Config, args: &SwitchArgs, tolerant: bool) -> Result<bool> {
    let accounts = config.anthropic.all_accounts();
    if tolerant && !accounts.iter().any(|a| a.label == args.label) {
        println!(
            "Claude Code      skipped: no account {:?} configured",
            args.label
        );
        return Ok(false);
    }
    let home = cli_account::home_claude_json()?;
    let outcome = cli_account::switch_cli_account(
        &home,
        &accounts,
        args.label,
        CliSwitchOpts {
            force: args.force,
            dry_run: args.dry_run,
        },
        &KeychainStore,
    )?;

    println!("Claude Code      → {}", args.label);
    match outcome {
        CliSwitchOutcome::AlreadyActive => {
            println!("  already the CLI's default login; nothing to do");
        }
        CliSwitchOutcome::RemovedDuplicate => {
            println!("  already active; removed its redundant named credential copy");
        }
        CliSwitchOutcome::WouldRemoveDuplicate => {
            println!("  would remove its redundant named credential copy");
            println!("  (dry run — nothing was changed)");
        }
        CliSwitchOutcome::RepairedActive => {
            println!("  repaired the empty default credential slot from its saved login");
        }
        CliSwitchOutcome::WouldSwitch { outgoing } => {
            print_cli_capture(outgoing.as_deref());
            println!("  (dry run — nothing was changed)");
        }
        CliSwitchOutcome::Switched { outgoing } => {
            print_cli_capture(outgoing.as_deref());
            println!(
                "  switched — plain `claude` now signs in as {:?}.",
                args.label
            );
        }
    }
    Ok(true)
}

fn print_cli_capture(outgoing: Option<&str>) {
    match outgoing {
        Some(label) => println!("  saving          {label}'s credential back into its own account"),
        None => println!("  saving          nothing to save (--force discarded the live login)"),
    }
}

fn confirm(prompt: &str) -> bool {
    matches!(
        ask(&format!("{prompt} [y/N] "))
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "y" | "yes"
    )
}

/// One line from the terminal, or `None` when there isn't one — piped input and
/// the menu bar's subprocess both land here, and neither can answer.
fn ask(prompt: &str) -> Option<String> {
    use std::io::IsTerminal;

    if !std::io::stdin().is_terminal() {
        return None;
    }
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).ok()?;
    let answer = answer.trim().to_string();
    (!answer.is_empty()).then_some(answer)
}

fn add(label: &str, login: bool) -> i32 {
    let registration = match register(label) {
        Ok(registration) => registration,
        Err(error) => {
            eprintln!("ai-usagebar account: could not add {label:?}: {error}");
            return 1;
        }
    };

    if registration.already_existed {
        println!(
            "Claude account {label:?} is already configured in {}.",
            registration.config_path.display()
        );
    } else {
        println!(
            "Added Claude account {label:?} to {}.",
            registration.config_path.display()
        );
        println!("  credentials_path = {}", registration.credential_display);
    }
    if !registration.anthropic_enabled {
        println!("  note: [anthropic] is disabled; set enabled = true for this account to appear.");
    }
    println!();

    if !registration.supports_scoped_login() {
        if login {
            eprintln!(
                "Automatic login requires this account's credentials_path to end in \
                 .credentials.json; it currently points to {}. Update that entry or \
                 keep managing its credential file manually.",
                registration.credential_file.display()
            );
            return 1;
        }
        println!("Registration kept unchanged; interactive login was not requested.");
        return 0;
    }

    if !login {
        print!("{}", manual_login_hint(&registration.account_dir));
        return 0;
    }

    // After a CLI account has been moved into the default Keychain slot, a
    // scoped login for that same active label would recreate a second copy of
    // its rotating refresh-token lineage. Reauthenticate it in the one live
    // slot instead, or switch away before recreating its named slot.
    #[cfg(target_os = "macos")]
    if cli_label_is_active(&registration.config_path, label) {
        eprintln!(
            "Cannot open an isolated login for {label:?} while it is the active plain `claude` \
             login; that would duplicate its rotating credential. Run plain `claude` to \
             reauthenticate it, or switch the CLI to another account first."
        );
        return 1;
    }

    println!(
        "Opening `claude` for {label:?} with an isolated CLAUDE_CONFIG_DIR; your \
         default Claude login is untouched."
    );
    println!();
    match login_claude_account(&registration.account_dir) {
        LoginOutcome::NotFound => {
            eprintln!("`claude` was not found on PATH. The account remains registered.\n");
            eprint!("{}", manual_login_hint(&registration.account_dir));
            1
        }
        LoginOutcome::Failed(code) => {
            eprintln!(
                "`claude` exited with status {code}. The account remains registered; retry with:\n"
            );
            eprint!("{}", manual_login_hint(&registration.account_dir));
            1
        }
        LoginOutcome::Ok => {
            // Touch metadata only: rewriting the pre-login contents here would
            // clobber config edits made while the interactive command ran.
            if let Err(error) = restamp_config(&registration.config_path) {
                eprintln!(
                    "warning: login finished, but config.toml could not be touched for live reload: {error}"
                );
            }
            println!();
            println!(
                "`claude` finished for {label:?}; the menu bar / TUI will refresh momentarily."
            );
            0
        }
    }
}

#[cfg(target_os = "macos")]
fn cli_label_is_active(config_path: &Path, label: &str) -> bool {
    let Ok(config) = Config::load_from(config_path) else {
        return false;
    };
    let Ok(home) = cli_account::home_claude_json() else {
        return false;
    };
    cli_account::resolve_active_label(&home, &config.anthropic.all_accounts()).as_deref()
        == Some(label)
}

enum LoginOutcome {
    Ok,
    Failed(i32),
    NotFound,
}

fn login_claude_account(account_dir: &Path) -> LoginOutcome {
    let mut command = std::process::Command::new("claude");
    command.env("CLAUDE_CONFIG_DIR", account_dir);
    for var in crate::vendor::vendor_secret_env_vars_to_remove(&[]) {
        command.env_remove(var);
    }

    match command.status() {
        Ok(status) if status.success() => LoginOutcome::Ok,
        Ok(status) => LoginOutcome::Failed(status.code().unwrap_or(-1)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => LoginOutcome::NotFound,
        Err(_) => LoginOutcome::Failed(-1),
    }
}

fn register(label: &str) -> Result<Registered> {
    let config_path = crate::config::resolved_path()
        .or_else(crate::config::default_path)
        .ok_or_else(|| {
            AppError::Other("could not resolve a config.toml path (no home directory?)".into())
        })?;
    let home = crate::cache::home_dir().ok();
    register_at(&config_path, label, home.as_deref())
}

fn register_at(config_path: &Path, label: &str, home: Option<&Path>) -> Result<Registered> {
    let original = match std::fs::read_to_string(config_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(AppError::io_at(config_path, error)),
    };
    let mut doc: toml_edit::DocumentMut = if original.trim().is_empty() {
        toml_edit::DocumentMut::new()
    } else {
        original.parse().map_err(|error: toml_edit::TomlError| {
            AppError::Other(format!("config.toml is not valid TOML: {error}"))
        })?
    };

    // Honor an existing explicit or accounts_dir-discovered account. In
    // particular, do not send a re-login to a different default directory.
    let (existing, anthropic_enabled) = if config_path.exists() {
        let config = Config::load_from(config_path)?;
        let existing = config
            .anthropic
            .all_accounts()
            .into_iter()
            .find(|account| account.label == label);
        (existing, config.anthropic.enabled)
    } else {
        (None, true)
    };
    let already_existed = existing.is_some();
    let credential_file = existing.map_or_else(
        || crate::config::default_account_credentials_path(config_path, label),
        |account| account.credentials_path,
    );
    let account_dir = credential_file
        .parent()
        .ok_or_else(|| AppError::Other("account credentials path has no parent directory".into()))?
        .to_path_buf();
    let credential_display = home.map_or_else(
        || credential_file.display().to_string(),
        |home| crate::config::tildify(&credential_file, home),
    );

    if !already_existed {
        crate::config::add_anthropic_account_to_doc(&mut doc, label, &credential_display)?;
    }

    let supports_scoped_login =
        credential_file.file_name().and_then(|name| name.to_str()) == Some(".credentials.json");
    if !already_existed || supports_scoped_login {
        let directory_was_missing = !account_dir.exists();
        std::fs::create_dir_all(&account_dir)
            .map_err(|error| AppError::io_at(&account_dir, error))?;
        if !already_existed || directory_was_missing {
            restrict_account_dir(&account_dir)?;
        }
    }
    if !already_existed {
        crate::cache::atomic_write(config_path, doc.to_string().as_bytes())?;
    }

    Ok(Registered {
        config_path: config_path.to_path_buf(),
        credential_file,
        account_dir,
        credential_display,
        already_existed,
        anthropic_enabled,
    })
}

fn restamp_config(path: &Path) -> Result<()> {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|error| AppError::io_at(path, error))?;
    let times = std::fs::FileTimes::new().set_modified(std::time::SystemTime::now());
    file.set_times(times)
        .map_err(|error| AppError::io_at(path, error))
}

#[cfg(unix)]
fn restrict_account_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| AppError::io_at(path, error))
}

#[cfg(not(unix))]
fn restrict_account_dir(_path: &Path) -> Result<()> {
    Ok(())
}

fn manual_login_hint(account_dir: &Path) -> String {
    format!(
        "Sign in later with:\n\n  {}\n\nThe account appears automatically after login.\n",
        shell_login_command(account_dir)
    )
}

#[cfg(not(windows))]
fn shell_login_command(account_dir: &Path) -> String {
    let escaped = account_dir.display().to_string().replace('\'', "'\\''");
    format!("CLAUDE_CONFIG_DIR='{escaped}' claude")
}

#[cfg(windows)]
fn shell_login_command(account_dir: &Path) -> String {
    let escaped = account_dir.display().to_string().replace('\'', "''");
    format!("$env:CLAUDE_CONFIG_DIR = '{escaped}'; claude")
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::claude_desktop::merge::{ConflictKind, DeletionCandidate};

    fn candidate(id: &str, deleted_by: &str, still_in: &[&str]) -> DeletionCandidate {
        DeletionCandidate {
            kind: ConflictKind::Chat,
            id: id.to_string(),
            deleted_by: deleted_by.to_string(),
            still_in: still_in.iter().map(|s| s.to_string()).collect(),
            summary: format!("chat {id}"),
        }
    }

    fn rendered(report: serde_json::Value) -> String {
        status_lines(&report).join("\n")
    }

    fn plan_fixture() -> SwitchPlan {
        SwitchPlan {
            target: claude_desktop::ProfileMeta {
                label: "work".into(),
                email: Some("w@example.test".into()),
                account_uuid: "acct-1".into(),
                org_uuid: Some("org-1".into()),
                has_credentials: true,
                has_desktop_state: true,
            },
            outgoing: Some("personal".into()),
            sessions: Default::default(),
            scheduled: None,
            tokens: claude_desktop::SavedTokens {
                token_cache: "{}".into(),
                token_cache_v2: "{}".into(),
            },
            archive: std::path::PathBuf::from("/archives/rollback.tar"),
            archive_members: vec!["a".into()],
            restores_desktop_state: true,
            opts: Default::default(),
            deletions: Vec::new(),
            confirmed_deletions: Default::default(),
            prior_synced: Default::default(),
        }
    }

    #[test]
    fn a_plan_names_the_target_the_outgoing_account_and_the_rollback() {
        let out = plan_lines(&plan_fixture()).join("\n");

        assert!(out.contains("work (w@example.test)"), "{out}");
        assert!(
            out.contains("saving          personal's credential"),
            "{out}"
        );
        assert!(out.contains("restore this account's cookies"), "{out}");
        assert!(
            out.contains("rollback        /archives/rollback.tar"),
            "{out}"
        );
    }

    /// Without an org there is no history folder to merge into, so the plan
    /// must say the history step is skipped rather than quietly claim zero
    /// sessions were copied — those read the same to a user and mean very
    /// different things.
    #[test]
    fn a_plan_without_an_org_says_history_is_skipped_not_empty() {
        let mut plan = plan_fixture();
        plan.target.org_uuid = None;

        let out = plan_lines(&plan).join("\n");

        assert!(out.contains("skipped (no org recorded"), "{out}");
        assert!(!out.contains("new + 0 refreshed"), "{out}");
    }

    /// Nothing saved yet, no rollback to offer, and no previous account to
    /// preserve: every optional line has to be absent rather than empty.
    #[test]
    fn a_plan_omits_the_lines_that_do_not_apply() {
        let mut plan = plan_fixture();
        plan.outgoing = None;
        plan.restores_desktop_state = false;
        plan.archive_members.clear();

        let out = plan_lines(&plan).join("\n");

        assert!(!out.contains("saving"), "{out}");
        assert!(!out.contains("rollback"), "{out}");
        assert!(out.contains("none saved yet"), "{out}");
    }

    /// A routine edited in two accounts is kept local rather than merged, and
    /// the count only appears when there is one — a "0 kept local" line reads
    /// like something went wrong.
    #[test]
    fn routine_conflicts_are_reported_only_when_there_are_some() {
        let mut plan = plan_fixture();
        plan.scheduled = Some(claude_desktop::merge::ScheduledMerge {
            target: std::path::PathBuf::from("/scheduled.json"),
            bytes: Vec::new(),
            added: 2,
            updated: 1,
            conflicts: 3,
            canonical_routines: Default::default(),
        });

        let out = plan_lines(&plan).join("\n");
        assert!(
            out.contains("routines        2 new + 1 edited elsewhere"),
            "{out}"
        );
        assert!(out.contains("routine conflicts 3 kept local"), "{out}");

        plan.scheduled.as_mut().unwrap().conflicts = 0;
        assert!(!plan_lines(&plan).join("\n").contains("routine conflicts"));
    }

    /// The bridge line describes a *destructive* default — the stale session id
    /// is cleared unless the user opted out — so it must track the flag.
    #[test]
    fn the_bridge_line_tracks_the_keep_bridge_flag() {
        let mut plan = plan_fixture();
        assert!(
            plan_lines(&plan)
                .join("\n")
                .contains("remote bridge   clear"),
            "clearing is the default and must be announced"
        );

        plan.opts.keep_bridge = true;
        assert!(!plan_lines(&plan).join("\n").contains("remote bridge"));
    }

    #[test]
    fn status_reports_both_halves_and_marks_the_active_account() {
        let out = rendered(serde_json::json!({
            "desktop": {
                "data_dir": "/data",
                "profiles_dir": "/profiles",
                "profiles": [{
                    "label": "work", "email": "w@example.test", "sessions": 12,
                    "has_credentials": true, "has_desktop_state": false, "active": true,
                }],
            },
            "cli": {
                "active_label": "personal",
                "accounts": [{"label": "personal", "email": "p@example.test", "active": true}],
            },
        }));

        assert!(out.contains("Claude Desktop   /data"), "{out}");
        assert!(out.contains("work"), "{out}");
        assert!(out.contains("sessions=12"), "{out}");
        assert!(out.contains("creds=yes"), "{out}");
        assert!(out.contains("state=no"), "{out}");
        assert_eq!(out.matches("[active]").count(), 2, "{out}");
    }

    /// Desktop is macOS-only and the report is `null` elsewhere; the CLI half
    /// still has to render.
    #[test]
    fn status_without_desktop_still_reports_the_cli() {
        let out = rendered(serde_json::json!({
            "desktop": null,
            "cli": {"active_label": null, "accounts": []},
        }));

        assert!(out.contains("not found (macOS only)"), "{out}");
        assert!(out.contains("no named accounts"), "{out}");
    }

    /// Every field is absent. Nothing here may panic or invent a value — the
    /// report is assembled from a filesystem that can be in any state.
    #[test]
    fn status_fills_in_placeholders_rather_than_panicking_on_an_empty_report() {
        let out = rendered(serde_json::json!({
            "desktop": {"profiles": [{}]},
            "cli": {"accounts": [{}]},
        }));

        assert!(out.contains("Claude Desktop   ?"), "{out}");
        assert!(out.contains("(email unknown)"), "{out}");
        assert!(out.contains("sessions=0"), "{out}");
        assert!(out.contains("creds=no"), "{out}");
        assert!(
            !out.contains("[active]"),
            "absent must not read as active: {out}"
        );
    }

    /// The warning that matters: saved copies are stale when the live login is
    /// not one of the accounts listed. It must appear only when there *are*
    /// accounts to be stale — an empty list is a fresh install, not a problem.
    #[test]
    fn the_stale_copies_note_appears_only_when_an_unlisted_login_is_live() {
        let note = "saved copies may be stale";
        let with_accounts = serde_json::json!({
            "desktop": null,
            "cli": {"active_label": null, "accounts": [{"label": "work"}]},
        });
        assert!(rendered(with_accounts).contains(note));

        let live_login_is_listed = serde_json::json!({
            "desktop": null,
            "cli": {"active_label": "work", "accounts": [{"label": "work"}]},
        });
        assert!(!rendered(live_login_is_listed).contains(note));

        let fresh_install = serde_json::json!({
            "desktop": null,
            "cli": {"active_label": null, "accounts": []},
        });
        assert!(!rendered(fresh_install).contains(note));
    }

    #[test]
    fn only_keys_naming_a_current_conflict_authorize_a_deletion() {
        let deletions = vec![
            candidate("chat-a", "acct-1", &["acct-2"]),
            candidate("chat-b", "acct-1", &["acct-2"]),
        ];
        let asked_for = vec![deletions[1].external_key()];

        let confirmed = authorized_deletions(&deletions, &asked_for);

        assert_eq!(confirmed.len(), 1);
        assert!(confirmed.contains(&deletions[1].key()));
        assert!(
            !confirmed.contains(&deletions[0].key()),
            "authorizing one conflict must not authorize its neighbour"
        );
    }

    /// The key carries the account topology observed when the question was
    /// asked. A dialog left open while the accounts changed underneath answers
    /// about a conflict that no longer exists, and must delete nothing — even
    /// though an item with that same id is still in front of us.
    #[test]
    fn a_key_from_a_stale_observation_deletes_nothing() {
        let asked_when = [candidate("chat-a", "acct-1", &["acct-2"])];
        let answer = vec![asked_when[0].external_key()];
        // Same id, but a third account now holds a copy too.
        let now = [candidate("chat-a", "acct-1", &["acct-2", "acct-3"])];

        assert!(
            authorized_deletions(&now, &answer).is_empty(),
            "a verdict about an older topology must not carry over"
        );
    }

    #[test]
    fn an_unrecognised_key_is_dropped_rather_than_matched() {
        let deletions = [candidate("chat-a", "acct-1", &["acct-2"])];
        let confirmed = authorized_deletions(&deletions, &["not-a-key".to_string(), String::new()]);
        assert!(confirmed.is_empty());
    }

    #[test]
    fn a_keep_list_keeps_exactly_what_it_names() {
        assert_eq!(parse_keep_list("1 3", 3).unwrap(), BTreeSet::from([1, 3]));
        // Order and repetition do not matter; the answer is a set.
        assert_eq!(parse_keep_list("3 1 3", 3).unwrap(), BTreeSet::from([1, 3]));
        // Blank keeps nothing — the documented way to delete them all.
        assert!(parse_keep_list("   ", 3).unwrap().is_empty());
    }

    /// A typo here erases whatever the unreadable token meant to save, so the
    /// whole answer is refused rather than half-applied.
    #[test]
    fn a_keep_list_naming_anything_unknown_is_refused_whole() {
        for (answer, count) in [("1 x", 3), ("0", 3), ("4", 3), ("-1", 3), ("1.5", 3)] {
            let unknown = parse_keep_list(answer, count)
                .expect_err(&format!("{answer:?} should have been refused"));
            assert!(!unknown.is_empty(), "{answer:?}");
        }
        // Including when the readable half would have kept something: the
        // refusal has to win, or `2` gets deleted on behalf of a typo.
        assert_eq!(parse_keep_list("1 tow", 3).unwrap_err(), vec!["tow"]);
    }

    #[test]
    fn an_empty_list_accepts_only_an_empty_answer() {
        assert!(parse_keep_list("", 0).unwrap().is_empty());
        assert!(parse_keep_list("1", 0).is_err());
    }

    #[test]
    fn mutation_commands_never_fall_back_after_a_config_error() {
        let error = config_for_mutation(Err(AppError::Other("broken config".into())))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("refusing to change account state"),
            "{error}"
        );
        assert!(error.contains("broken config"), "{error}");
    }

    #[test]
    fn registration_uses_isolated_standard_layout() {
        let temporary = tempfile::TempDir::new().unwrap();
        let config_path = temporary.path().join("config.toml");
        let registration = register_at(&config_path, "work", Some(temporary.path())).unwrap();

        assert!(!registration.already_existed);
        assert_eq!(
            registration.credential_file,
            temporary.path().join("accounts/work/.credentials.json")
        );
        assert!(registration.supports_scoped_login());
        let written = std::fs::read_to_string(config_path).unwrap();
        assert!(written.contains("label = \"work\""));
        assert!(written.contains("credentials_path = \"~/accounts/work/.credentials.json\""));
    }

    #[cfg(unix)]
    #[test]
    fn registration_restricts_the_account_directory() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::TempDir::new().unwrap();
        let config_path = temporary.path().join("config.toml");
        let registration = register_at(&config_path, "work", None).unwrap();
        let mode = std::fs::metadata(registration.account_dir)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn existing_account_keeps_its_configured_path() {
        let temporary = tempfile::TempDir::new().unwrap();
        let config_path = temporary.path().join("config.toml");
        let credentials = temporary.path().join("custom/work.json");
        std::fs::write(
            &config_path,
            format!(
                "[[anthropic.accounts]]\nlabel = \"work\"\ncredentials_path = {:?}\n",
                credentials.display().to_string()
            ),
        )
        .unwrap();

        let registration = register_at(&config_path, "work", None).unwrap();
        assert!(registration.already_existed);
        assert_eq!(registration.credential_file, credentials);
        assert!(!registration.supports_scoped_login());
        assert!(!temporary.path().join("custom").exists());
    }

    #[test]
    fn disabled_anthropic_setting_is_preserved_and_reportable() {
        let temporary = tempfile::TempDir::new().unwrap();
        let config_path = temporary.path().join("config.toml");
        std::fs::write(&config_path, "[anthropic]\nenabled = false\n").unwrap();

        let registration = register_at(&config_path, "work", None).unwrap();
        assert!(!registration.anthropic_enabled);
        let written = std::fs::read_to_string(config_path).unwrap();
        assert!(written.contains("enabled = false"));
    }

    #[test]
    fn login_hint_quotes_paths_with_spaces() {
        let command = shell_login_command(Path::new("/tmp/Claude Accounts/work"));
        assert!(command.contains("Claude Accounts"));
        #[cfg(not(windows))]
        assert_eq!(
            command,
            "CLAUDE_CONFIG_DIR='/tmp/Claude Accounts/work' claude"
        );
    }

    #[test]
    fn restamp_never_rewrites_config_contents() {
        let temporary = tempfile::TempDir::new().unwrap();
        let path = temporary.path().join("config.toml");
        std::fs::write(&path, "# edited while login ran\n").unwrap();
        restamp_config(&path).unwrap();
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "# edited while login ran\n"
        );
    }
}
