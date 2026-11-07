//! Settings → Providers: every built-in provider from [`crate::catalog`], the
//! config switch that turns one on or off, and the action that signs it in.
//!
//! The report only carries enabled providers, so a popover that lists its
//! cards cannot show — or enable — the ones that are off. This is the row
//! source for those, kept in Rust so the popover holds no provider table.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Value, json};

use super::payload::{HostFacts, KeyCheckFact, KeyCheckState, SharedFacts, with_facts};
use crate::catalog::VendorStatus;
use crate::vendor::{AuthKind, VendorId};

/// What "Sign in" does for one provider on this host.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SignIn {
    /// A device-code login the popover runs itself ([`super::device_login`]);
    /// the command is the terminal fallback the sheet offers when it cannot.
    Device(&'static str),
    /// Run the provider's own login command in a terminal.
    Terminal(&'static str),
    /// Bring a desktop app forward; its window is where the login happens.
    App(&'static str),
    /// Key providers: the TUI's Settings overlay is where a key goes.
    Settings,
    None,
}

impl SignIn {
    fn kind(self) -> &'static str {
        match self {
            SignIn::Device(_) => "device",
            SignIn::Terminal(_) => "terminal",
            SignIn::App(_) => "app",
            SignIn::Settings => "settings",
            SignIn::None => "",
        }
    }
}

/// `macos` selects the hosts that can open an app by name (`open -a`).
pub(crate) fn sign_in_for(vendor: VendorId, macos: bool) -> SignIn {
    match vendor {
        VendorId::Cursor if macos => SignIn::App("Cursor"),
        VendorId::Antigravity if macos => SignIn::App("Antigravity"),
        _ if super::device_login::Flow::for_vendor(vendor).is_some() => {
            SignIn::Device(vendor.login_command())
        }
        _ if !vendor.login_command().is_empty() => SignIn::Terminal(vendor.login_command()),
        _ if vendor.auth_kind() == AuthKind::ApiKey => SignIn::Settings,
        _ => SignIn::None,
    }
}

/// The `providers` array of the popover payload.
pub(crate) fn rows_json(rows: &[VendorStatus], macos: bool) -> Value {
    Value::Array(
        rows.iter()
            .filter_map(|row| {
                let vendor = VendorId::from_slug(row.id)?;
                let mut entry = json!({
                    "id": row.id,
                    "name": row.name,
                    "kind": row.kind.as_str(),
                    "enabled": row.enabled,
                    "configured": row.configured,
                    "env": row.env,
                    "sign_in": vendor.sign_in_hint(),
                    "action": sign_in_for(vendor, macos).kind(),
                    "add_accounts": add_account_kinds(vendor, macos),
                });
                // The key dialog's fields, straight from the settings
                // overlay's own table: the popover keeps no credential
                // labels of its own, and only key vendors get the pair.
                if let (Some(key), Some(obj)) = (
                    crate::tui::settings::key_vendor_for_slug(row.id),
                    entry.as_object_mut(),
                ) {
                    obj.insert("key_label".into(), json!(key.secret_label));
                    obj.insert("key_note".into(), json!(key.note));
                }
                // The sign-in sheet's words: whose account the page signs in
                // and who hands out the code, product knowledge kept here.
                if let (Some(flow), Some(obj)) = (
                    super::device_login::Flow::for_vendor(vendor),
                    entry.as_object_mut(),
                ) {
                    obj.insert("sign_in_account".into(), json!(flow.account()));
                    obj.insert("sign_in_issuer".into(), json!(flow.issuer()));
                }
                Some(entry)
            })
            .collect(),
    )
}

static DUE: AtomicBool = AtomicBool::new(true);

/// Ask the next report to re-read the catalog. It is not read on every poll:
/// on macOS it consults the Keychain once per Claude account.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub(crate) fn request() {
    DUE.store(true, Ordering::Relaxed);
}

/// A catalog read, and the switch generation it started under.
struct CatalogRead {
    rows: Vec<VendorStatus>,
    generation: u64,
}

/// Re-read the catalog into `facts` when one was requested (and at startup);
/// `true` when the rows changed hands. An unreadable config lists every
/// provider against the defaults. The read runs outside the lock, so a
/// switch can land while it does; see [`publish`].
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub(crate) fn refresh_due(facts: &SharedFacts) -> bool {
    if !DUE.swap(false, Ordering::Relaxed) {
        return false;
    }
    let generation = facts.lock().map(|f| f.providers_generation).unwrap_or(0);
    let rows = crate::catalog::statuses(&crate::config::Config::load().unwrap_or_default());
    publish(facts, CatalogRead { rows, generation })
}

/// Hand a read to the popover unless a switch moved since it started: that
/// read may hold the config from before the write, and publishing it would
/// put the switch back. The switch requested another read, which wins.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
fn publish(facts: &SharedFacts, read: CatalogRead) -> bool {
    let mut published = false;
    with_facts(facts, |f| {
        if f.providers_generation == read.generation {
            f.providers = read.rows;
            published = true;
        }
    });
    published
}

/// `{"cmd":"set-provider-enabled","id":"cursor","value":true}`: flip the
/// provider's `enabled` in config.toml, then in the catalog `facts` holds.
///
/// The popover's switch shows the host's row, not the click, and a re-read
/// catalog only reaches it with the next report — after every enabled
/// provider has been fetched, which takes seconds when one starts `agy` or
/// waits on the network. Flipping the row here moves the switch on the next
/// push; the re-read still follows, for `configured` and the sign-in action.
/// It is requested after the flip, so it starts under the new generation.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub(crate) fn set_enabled(value: &Value, facts: &SharedFacts) {
    let written = write_enabled(value);
    with_facts(facts, |f| record_enabled(f, written));
    request();
}

/// The slug is validated by [`crate::config::set_vendor_enabled`], so no
/// other section is reachable.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
fn write_enabled(value: &Value) -> Result<(&str, bool), String> {
    let (Some(id), Some(enabled)) = (
        value.get("id").and_then(Value::as_str),
        value.get("value").and_then(Value::as_bool),
    ) else {
        return Err("Malformed provider request.".into());
    };
    let path = crate::config::resolved_path()
        .ok_or_else(|| "Could not resolve the config.toml path.".to_string())?;
    crate::config::set_vendor_enabled(&path, id, enabled)
        .map(|()| (id, enabled))
        .map_err(|error| error.user_message())
}

#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
fn record_enabled(facts: &mut HostFacts, written: Result<(&str, bool), String>) {
    facts.providers_error = match written {
        Ok((id, enabled)) => {
            if let Some(row) = facts.providers.iter_mut().find(|row| row.id == id) {
                row.enabled = enabled;
            }
            facts.providers_generation = facts.providers_generation.wrapping_add(1);
            String::new()
        }
        Err(error) => error,
    };
}

/// `{"cmd":"sign-in-provider","id":"cursor"}`.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub(crate) fn sign_in(value: &Value) {
    let Some(vendor) = value
        .get("id")
        .and_then(Value::as_str)
        .and_then(VendorId::from_slug)
    else {
        return;
    };
    #[cfg(any(windows, target_os = "macos"))]
    match sign_in_for(vendor, cfg!(target_os = "macos")) {
        // `sign-in-provider` is the sheet's "Use the terminal" for a device
        // provider: its own login command, in a terminal.
        SignIn::Device(command) | SignIn::Terminal(command) => {
            super::tui_launch::run_in_terminal(&login_line(command))
        }
        SignIn::App(name) => open_app(name),
        SignIn::Settings => super::tui_launch::open(),
        SignIn::None => {}
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = vendor;
}

/// `{"cmd":"add-account","kind":"code"|"desktop"|"codex"}`: register a new
/// Claude Code, Claude Desktop or Codex account in Terminal, which asks for
/// the label and runs the interactive sign-in `account add` starts.
#[cfg(target_os = "macos")]
pub(crate) fn add_account(value: &Value) {
    let Some(flag) = value
        .get("kind")
        .and_then(Value::as_str)
        .and_then(add_account_flag)
    else {
        return;
    };
    let bin = super::tui_launch::quote(&super::tui_launch::sibling("ai-usagebar"));
    super::tui_launch::run_in_terminal(&format!(
        "read -r -p 'New account label: ' label && {bin} account add \"$label\"{flag}"
    ));
}

/// Which `add-account` kinds a provider's row offers. Account capture runs
/// through the macOS host only, like the account switch.
fn add_account_kinds(vendor: VendorId, macos: bool) -> &'static [&'static str] {
    match vendor {
        VendorId::Anthropic if macos => &["code", "desktop"],
        VendorId::Openai if macos => &["codex"],
        _ => &[],
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn add_account_flag(kind: &str) -> Option<&'static str> {
    match kind {
        "code" => Some(""),
        "desktop" => Some(" --desktop"),
        "codex" => Some(" --codex"),
        _ => None,
    }
}

/// `ai-usagebar auth …` names this project's own CLI, which a tray-only
/// install may not have on `PATH`; point it at the binary beside the tray.
#[cfg(any(windows, target_os = "macos"))]
fn login_line(command: &str) -> String {
    match command.strip_prefix("ai-usagebar ") {
        Some(rest) => format!(
            "{} {rest}",
            super::tui_launch::quote(&super::tui_launch::sibling("ai-usagebar"))
        ),
        None => command.to_string(),
    }
}

/// `{"cmd":"set-api-key","id":"zai","key":"…","seq":17}`: the popover's
/// key dialog. The key is written with the settings overlay's writer, the
/// vendor's cache is dropped and the provider is fetched alone, and the
/// verdict is published as the `key_check` fact — a refusal puts the
/// config back exactly where the check found it. The key never appears in
/// a fact, payload, error, log, or thread name.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub(crate) fn request_key_check(
    facts: &SharedFacts,
    value: &Value,
    notify: impl FnOnce() + Send + 'static,
) -> std::result::Result<Option<std::thread::JoinHandle<()>>, KeyCheckFact> {
    request_key_check_with(
        facts,
        value,
        fetch_filtered_report,
        bust_vendor_cache,
        clean_vendor_diagnostic,
        |name| std::env::var_os(name).is_some_and(|value| !value.is_empty()),
        request,
        notify,
    )
}

/// The production fetch: one provider's report through the same entry point
/// the tray's report uses, on a current-thread runtime like the account
/// prepare. The wire-only read grants a zero TTL, so a payload another
/// producer left in the vendor's cache cannot answer for the key just
/// written. Tests pass a fake so a check never touches the wire.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
fn fetch_filtered_report(slug: &str) -> std::result::Result<String, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()
        .and_then(|runtime| {
            runtime
                .block_on(crate::report::collect_filtered_json_from_wire(Some(slug)))
                .ok()
        })
        .ok_or_else(|| "Could not start the key check worker.".to_string())
}

/// The production final cleanup after a refused verdict: drop the vendor's
/// cache again, and when that removal fails, rewrite the `.last_error` in
/// place with the key scrubbed out — the directory may be read-only for
/// creation while the file itself is still writable. Best effort: a cleanup
/// that cannot run still must not bring the check down.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
fn clean_vendor_diagnostic(slug: &str, key: &str) {
    let Ok(cache) = crate::cache::Cache::for_vendor(slug) else {
        return;
    };
    if cache.try_forget().is_err() {
        // The diagnostic's layout is `code\nmessage`; the code line survives
        // the scrub, the message does not name the key.
        let _ = cache.scrub_last_error(|raw| {
            let (code, message) = raw.split_once('\n').unwrap_or((raw, ""));
            format!("{code}\n{}", scrub_key(message, key))
        });
    }
}

/// The shared check: parse the request, publish `Checking`, run the
/// snapshot — write — fetch — restore dance on its own thread. `fetch`,
/// `bust_cache`, `clean_diagnostic`, `env_set`, `on_catalog` and `notify`
/// are the seams the hosts and the tests plug in, so the thread never names
/// either one. A malformed request — a missing field, a non-string slug or
/// key, a `seq` that is not a `u64`, a vendor outside `KEY_VENDORS`, or a
/// key the overlay would refuse — is ignored: `Ok(None)`, no response, no
/// write. The dialog only exists for a provider without an inline
/// credential, so a config that already saves a non-empty key is refused
/// before the write, as is a vendor whose environment variable is defined:
/// the fetch would authenticate with the environment's key, not the one
/// being checked. The error is the verdict the host publishes when the
/// check is refused or the thread cannot even be started.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
#[allow(clippy::too_many_arguments)]
pub(crate) fn request_key_check_with(
    facts: &SharedFacts,
    value: &Value,
    fetch: impl FnOnce(&str) -> std::result::Result<String, String> + Send + 'static,
    bust_cache: impl Fn(&str) -> std::result::Result<(), String> + Send + 'static,
    clean_diagnostic: impl Fn(&str, &str) + Send + 'static,
    env_set: impl Fn(&str) -> bool + Send + 'static,
    on_catalog: impl Fn() + Send + 'static,
    notify: impl FnOnce() + Send + 'static,
) -> std::result::Result<Option<std::thread::JoinHandle<()>>, KeyCheckFact> {
    let Some((slug, key, seq)) = key_check_request(value) else {
        return Ok(None);
    };
    let Some(vendor) = crate::tui::settings::key_vendor_for_slug(&slug) else {
        return Ok(None);
    };
    if crate::tui::settings::validate_key_value(vendor, &key).is_err() {
        return Ok(None);
    }
    // The path is resolved here, before the thread, so the thread's
    // snapshot — write — restore trio all touch the one file, and the
    // thread itself never reads process-global state (tests steer it with
    // the `--config` override seam).
    let path = crate::config::resolved_path().ok_or_else(|| KeyCheckFact {
        id: slug.clone(),
        seq,
        state: KeyCheckState::Failed,
        error: "Could not resolve the config.toml path.".into(),
    })?;
    // The dialog only exists for a provider without an inline credential:
    // a non-empty key already saved in the config is replaced in the TUI
    // settings, not checked here (an empty field counts as absent, by the
    // catalog's rule). Refusing it also keeps a queued check from replacing
    // a key the TUI saves while the thread waits — re-checked under the
    // config lock inside the write's own read-modify-write. A config that
    // cannot be parsed skips the check — the write below fails with the
    // real error.
    let already_saved = crate::tui::settings::snapshot_inline_key(&path, vendor)
        .ok()
        .is_some_and(|snapshot| crate::tui::settings::saved_inline_key(&snapshot).is_some());
    if already_saved {
        return Err(KeyCheckFact {
            id: slug,
            seq,
            state: KeyCheckState::Failed,
            error: crate::tui::settings::KEY_ALREADY_SAVED_MESSAGE.into(),
        });
    }
    // The vendor's effective environment variable — an `api_key_env`
    // override included, as the catalog's `env` field uses it — takes
    // precedence over the inline value: with it defined, the fetch would
    // authenticate with the environment's key, not the one being checked,
    // and a verdict about either would be a lie. Refuse before the write;
    // only defined-ness is consulted, never the value. An unreadable
    // config skips the check — the write below fails with the real error.
    let env_name = crate::config::Config::load_from(&path)
        .ok()
        .map(|config| config.api_key_env_for(vendor.id).to_string())
        .unwrap_or_default();
    if !env_name.is_empty() && (env_set)(&env_name) {
        return Err(KeyCheckFact {
            id: slug,
            seq,
            state: KeyCheckState::Failed,
            error: format!(
                "{env_name} is set in the environment and takes precedence over config.toml."
            ),
        });
    }
    // A late send must not replace a newer send's fact: publish `Checking`
    // only when no fact, or an older one, is on screen.
    with_facts(facts, |f| {
        let replaces = f.key_check.as_ref().is_none_or(|current| current.seq < seq);
        if replaces {
            f.key_check = Some(KeyCheckFact {
                id: slug.clone(),
                seq,
                state: KeyCheckState::Checking,
                error: String::new(),
            });
        }
    });
    let facts = Arc::clone(facts);
    let slug = slug.clone();
    let spawned_slug = slug.clone();
    std::thread::Builder::new()
        .name("ai-usagebar-key-check".into())
        .spawn(move || {
            run_key_check(
                &facts,
                &path,
                &spawned_slug,
                key,
                seq,
                fetch,
                bust_cache,
                clean_diagnostic,
                on_catalog,
                notify,
            )
        })
        .map(Some)
        .map_err(|_| KeyCheckFact {
            id: slug,
            seq,
            state: KeyCheckState::Failed,
            error: "Could not start the key check worker.".into(),
        })
}

/// The request's (vendor slug, key, sequence number), or `None` when the
/// body is not a well-formed `set-api-key`.
fn key_check_request(value: &Value) -> Option<(String, String, u64)> {
    Some((
        value.get("id").and_then(Value::as_str)?.to_string(),
        value.get("key").and_then(Value::as_str)?.to_string(),
        value.get("seq").and_then(Value::as_u64)?,
    ))
}

/// One key check at a time: the snapshot — write — fetch — restore window
/// must be atomic, or a second check's snapshot would capture the first
/// check's key as "the previous value" and its restore would write it back
/// over the first check's verdict. Poisoned lock — a panicked check —
/// proceeds anyway: the config is a plain file, and a deadlock would leave
/// every later key send silently unanswered.
static KEY_CHECK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The check's thread: under the single-flight lock, verify the send was
/// not superseded and the config still saves no key (the TUI is not behind
/// this lock — a key it saved while the thread queued is a later decision,
/// not an absent credential), write the new key only while it is still
/// absent, drop the vendor's cache — proving the removal — and fetch the
/// provider alone. On a refusal, put the config back the state the snapshot
/// found it, then drop the cache again — and when that removal fails,
/// scrub the diagnostic in place — so the provider's answer (which may
/// name the key) and the backoff it armed do not outlive the check. The
/// key is stripped from the verdict's error and from the diagnostic before
/// it is dropped, so nothing after the scrub may name it.
#[allow(clippy::too_many_arguments)] // the seams stay flat: tests name each one
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
fn run_key_check(
    facts: &SharedFacts,
    path: &std::path::Path,
    slug: &str,
    key: String,
    seq: u64,
    fetch: impl FnOnce(&str) -> std::result::Result<String, String>,
    bust_cache: impl Fn(&str) -> std::result::Result<(), String>,
    clean_diagnostic: impl Fn(&str, &str),
    on_catalog: impl Fn(),
    notify: impl FnOnce(),
) {
    let Some(vendor) = crate::tui::settings::key_vendor_for_slug(slug) else {
        return;
    };
    let _guard = KEY_CHECK_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // A newer send that landed while this thread waited owns the dialog
    // and the config now: leave both alone — no write, no fetch, no fact.
    let still_current = facts
        .lock()
        .ok()
        .and_then(|guard| guard.key_check.as_ref().map(|fact| fact.seq == seq))
        .unwrap_or(false);
    if !still_current {
        return;
    }
    // The snapshot, the "no key yet" condition, and the conditional write
    // are one locked read-modify-write under the config lock the TUI takes,
    // so a key the TUI saved while this thread queued is seen and refused
    // with the saved-key message — never overwritten by the candidate, and
    // never fetched on. On a refusal the candidate never touched the file,
    // so there is nothing to restore; the refusal message is the verdict.
    let snapshot =
        match crate::tui::settings::snapshot_and_write_inline_key_if_absent(path, vendor, &key) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                finish_key_check(
                    facts,
                    slug,
                    seq,
                    KeyCheckState::Failed,
                    error.user_message(),
                    on_catalog,
                    notify,
                );
                return;
            }
        };
    // The verdict must rest on a reading made under the key just written:
    // drop whatever the vendor's cache still holds from before the write.
    // A bust the thread cannot prove is a bust that did not happen: the
    // fetch is skipped, the config is restored, and the refusal says so.
    if (bust_cache)(slug).is_err() {
        let mut error =
            "Could not clear the provider's cache, so the key was not checked.".to_string();
        if crate::tui::settings::restore_inline_key_snapshot(path, vendor, &snapshot, &key).is_err()
        {
            error.push_str(" The new key stayed in config.toml.");
        }
        drop(key);
        error = cut_key_check_error(&error);
        drop(_guard);
        finish_key_check(
            facts,
            slug,
            seq,
            KeyCheckState::Failed,
            error,
            on_catalog,
            notify,
        );
        return;
    }
    let (state, mut error) = key_check_result(slug, fetch(slug));
    // The key's last use: strip it out of the verdict before it is dropped.
    error = scrub_key(&error, &key);
    // A refusal gives the config back exactly the state the snapshot took
    // — only the fields that still hold what the check wrote; a restore
    // that fails itself says so, since the new key stayed.
    if state == KeyCheckState::Failed
        && crate::tui::settings::restore_inline_key_snapshot(path, vendor, &snapshot, &key).is_err()
    {
        error.push_str(" The new key stayed in config.toml.");
    }
    // A refused check must not leave the key in the persisted diagnostic
    // the fetch wrote (the provider's answer may name it) or in the 429
    // backoff the candidate key armed: bust the cache again, and when that
    // removal fails, scrub the diagnostic in place. The scrub needs the
    // key, so this is its last use: it runs before the drop.
    if state == KeyCheckState::Failed {
        (clean_diagnostic)(slug, &key);
    }
    drop(key);
    error = cut_key_check_error(&error);
    // The snapshot — write — restore window is over; the publish and the
    // catalog request may run while the next check takes the lock.
    drop(_guard);
    finish_key_check(facts, slug, seq, state, error, on_catalog, notify);
}

/// Publish the verdict and ask the host for a fresh facts stamp, catalog,
/// and report. The fact is written only when this check is still the
/// newest one: a newer send has already replaced it, and this thread's
/// verdict belongs to a send the popover no longer shows — the catalog
/// and report refresh still run, since this thread may have restored the
/// config.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
fn finish_key_check(
    facts: &SharedFacts,
    slug: &str,
    seq: u64,
    state: KeyCheckState,
    error: String,
    on_catalog: impl Fn(),
    notify: impl FnOnce(),
) {
    // Compare-and-set in one `with_facts` pass: a newer send that landed
    // between a separate compare and a write would otherwise be
    // clobbered by this thread's verdict.
    with_facts(facts, |f| {
        if f.key_check
            .as_ref()
            .is_some_and(|current| current.seq == seq)
        {
            f.key_check = Some(KeyCheckFact {
                id: slug.to_string(),
                seq,
                state,
                error,
            });
        }
    });
    (on_catalog)();
    notify();
}

/// The verdict for one check's report. `Ok` only for the entry whose `id`
/// is the provider's own slug — the entry of the inline credential this
/// check just wrote; a `slug@account` entry belongs to another credential
/// and does not decide. `"status":"ready"` with `"stale":false` is `Ok`;
/// every other shape is `Failed`, with the entry's error, the fetch's
/// error, or the fixed "no reading" note. The caller scrubs the key out of
/// the error and cuts it to 300 characters: the error is the one place a
/// provider gets to echo it.
pub(crate) fn key_check_result(
    slug: &str,
    report: std::result::Result<String, String>,
) -> (KeyCheckState, String) {
    const NO_READING: &str = "The provider returned no reading.";
    match report {
        Err(error) => (KeyCheckState::Failed, error),
        Ok(document) => match serde_json::from_str::<Value>(&document) {
            Err(_) => (KeyCheckState::Failed, NO_READING.to_string()),
            Ok(value) => {
                let entries: Vec<&Value> = value
                    .get("entries")
                    .and_then(Value::as_array)
                    .map_or_else(Vec::new, |list| list.iter().collect());
                let mine: Vec<&Value> = entries
                    .iter()
                    .filter(|entry| entry.get("id").and_then(Value::as_str) == Some(slug))
                    .copied()
                    .collect();
                if mine.iter().any(|entry| {
                    entry.get("status").and_then(Value::as_str) == Some("ready")
                        && entry.get("stale").and_then(Value::as_bool) == Some(false)
                }) {
                    (KeyCheckState::Ok, String::new())
                } else if let Some(entry) = mine.first() {
                    (
                        KeyCheckState::Failed,
                        entry
                            .get("error")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                    )
                } else {
                    (KeyCheckState::Failed, NO_READING.to_string())
                }
            }
        },
    }
}

/// The key's last use: strip it out of the verdict's error before the key
/// is dropped. Defence in depth on top of the cache-side redaction at the
/// first point of writing — the one rule is
/// [`crate::display::redact_secret`]: a verbatim echo becomes `[key]`; when
/// any eight-character window of the key — counted in `char`, not bytes —
/// still survives, the provider quoted the key in part, and a partial echo
/// is the key with a missing tail, so the whole error is replaced with a
/// fixed note.
pub(crate) fn scrub_key(error: &str, key: &str) -> String {
    crate::display::redact_secret(error, key)
}

/// The production cache-bust for the check's fetch: drop the vendor's
/// cached payload and its sidecars with the credential-rotation primitive,
/// surfacing a failure instead of assuming the removal. A payload left in
/// place would be served as a reading made under the key just written, and
/// the check must be able to tell the difference.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
fn bust_vendor_cache(slug: &str) -> std::result::Result<(), String> {
    let cache = crate::cache::Cache::for_vendor(slug).map_err(|error| error.user_message())?;
    cache.try_forget().map_err(|error| error.to_string())
}

/// The verdict's error line, cut at 300 characters on a char boundary.
fn cut_key_check_error(error: &str) -> String {
    error.chars().take(300).collect()
}

#[cfg(target_os = "macos")]
fn open_app(name: &str) {
    let _ = std::process::Command::new("/usr/bin/open")
        .args(["-a", name])
        .spawn();
}

#[cfg(windows)]
fn open_app(_name: &str) {}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(any(windows, target_os = "macos"))]
    use crate::tray::payload::{facts_snapshot, wrap_report};

    #[test]
    fn every_provider_gets_an_action_or_says_it_has_none() {
        for &vendor in VendorId::all() {
            let action = sign_in_for(vendor, true);
            match vendor.auth_kind() {
                AuthKind::Oauth => assert!(
                    matches!(action, SignIn::Terminal(_) | SignIn::Device(_)),
                    "{vendor:?}"
                ),
                AuthKind::ApiKey if vendor.login_command().is_empty() => {
                    assert_eq!(action, SignIn::Settings, "{vendor:?}")
                }
                _ => {}
            }
        }
    }

    #[test]
    fn desktop_apps_open_only_where_open_a_exists() {
        assert_eq!(sign_in_for(VendorId::Cursor, true), SignIn::App("Cursor"));
        assert_eq!(sign_in_for(VendorId::Cursor, false), SignIn::None);
        assert_eq!(
            sign_in_for(VendorId::Openai, false),
            SignIn::Device("codex login")
        );
        assert_eq!(sign_in_for(VendorId::Zai, false), SignIn::Settings);
    }

    fn default_rows() -> Vec<VendorStatus> {
        crate::catalog::statuses_with(
            &crate::config::Config::default(),
            &crate::catalog::Probes {
                env_set: &|_| false,
                exists: &|_| false,
                keychain_has_claude: &|| false,
                keychain_has_claude_for: &|_| false,
                commandcode_signed_in: &|_| false,
                opencode_go_signed_in: &|_| false,
                cline_signed_in: &|_| false,
            },
        )
    }

    #[test]
    fn rows_carry_every_vendor_with_hint_and_action() {
        let rows = default_rows();
        let json = rows_json(&rows, true);
        let list = json.as_array().unwrap();
        assert_eq!(list.len(), VendorId::all().len());
        let cursor = list.iter().find(|row| row["id"] == "cursor").unwrap();
        assert_eq!(cursor["enabled"], false);
        assert_eq!(cursor["configured"], false);
        assert_eq!(cursor["action"], "app");
        assert_eq!(cursor["sign_in"], VendorId::Cursor.sign_in_hint());
    }

    #[test]
    fn device_providers_sign_in_from_the_popover_with_their_words() {
        let rows = default_rows();
        let json = rows_json(&rows, true);
        let list = json.as_array().unwrap();
        let row = |id: &str| list.iter().find(|row| row["id"] == id).unwrap().clone();
        for (id, account, issuer) in [
            ("openai", "ChatGPT", "OpenAI"),
            ("kimi", "Kimi", "Kimi"),
            ("nous", "Nous Portal", "Nous Research"),
        ] {
            let row = row(id);
            assert_eq!(row["action"], "device", "{id}");
            assert_eq!(row["sign_in_account"], account, "{id}");
            assert_eq!(row["sign_in_issuer"], issuer, "{id}");
        }
        // The terminal providers keep the terminal, and carry no sheet words.
        for id in ["anthropic", "copilot"] {
            let row = row(id);
            assert_eq!(row["action"], "terminal", "{id}");
            assert!(row.get("sign_in_account").is_none(), "{id}");
        }
    }

    #[test]
    fn add_account_accepts_only_the_three_known_kinds() {
        assert_eq!(add_account_flag("code"), Some(""));
        assert_eq!(add_account_flag("desktop"), Some(" --desktop"));
        assert_eq!(add_account_flag("codex"), Some(" --codex"));
        assert_eq!(add_account_flag("; rm -rf ~"), None);
        for &vendor in VendorId::all() {
            assert!(add_account_kinds(vendor, false).is_empty());
            for kind in add_account_kinds(vendor, true) {
                assert!(add_account_flag(kind).is_some(), "{kind}");
            }
        }
    }

    #[test]
    fn malformed_enable_requests_are_refused_before_touching_config() {
        assert!(write_enabled(&json!({"id": "cursor"})).is_err());
        assert!(write_enabled(&json!({"value": true})).is_err());
    }

    /// A catalog row built by hand, so these tests resolve no config default.
    fn row(id: &'static str, enabled: bool) -> VendorStatus {
        VendorStatus {
            id,
            name: id,
            short_name: id,
            kind: AuthKind::Oauth,
            enabled,
            configured: true,
            needs_credential: true,
            env: String::new(),
            login: "",
        }
    }

    fn enabled(facts: &HostFacts, id: &str) -> bool {
        facts
            .providers
            .iter()
            .find(|row| row.id == id)
            .unwrap()
            .enabled
    }

    #[test]
    fn a_written_switch_moves_its_row_before_any_report() {
        let mut facts = HostFacts::new("0.0.0", false);
        facts.providers = vec![row("cursor", false)];
        facts.providers_error = "stale".into();

        record_enabled(&mut facts, Ok(("cursor", true)));
        assert!(enabled(&facts, "cursor"));
        assert_eq!(facts.providers_error, "");
        assert_eq!(facts.providers_generation, 1);

        record_enabled(&mut facts, Err("Config is read-only.".into()));
        assert!(enabled(&facts, "cursor"), "a failed write leaves the row");
        assert_eq!(facts.providers_error, "Config is read-only.");
        assert_eq!(facts.providers_generation, 1, "nothing moved");
    }

    #[test]
    fn a_catalog_read_from_before_a_switch_cannot_put_it_back() {
        let facts: SharedFacts =
            std::sync::Arc::new(std::sync::Mutex::new(HostFacts::new("0.0.0", false)));
        with_facts(&facts, |f| f.providers = vec![row("cursor", false)]);
        // The worker starts reading while Cursor is still off…
        let stale = CatalogRead {
            rows: vec![row("cursor", false)],
            generation: facts.lock().unwrap().providers_generation,
        };
        // …the switch lands before the read is published…
        with_facts(&facts, |f| record_enabled(f, Ok(("cursor", true))));
        // …and the old rows are dropped instead of turning it back off.
        assert!(!publish(&facts, stale));
        assert!(enabled(&facts.lock().unwrap(), "cursor"));

        let fresh = CatalogRead {
            rows: vec![row("cursor", true)],
            generation: facts.lock().unwrap().providers_generation,
        };
        assert!(publish(&facts, fresh));
        assert!(enabled(&facts.lock().unwrap(), "cursor"));
    }

    #[test]
    fn rows_carry_the_key_vendors_credential_fields_only() {
        let rows = crate::catalog::statuses_with(
            &crate::config::Config::default(),
            &crate::catalog::Probes {
                env_set: &|_| false,
                exists: &|_| false,
                keychain_has_claude: &|| false,
                keychain_has_claude_for: &|_| false,
                commandcode_signed_in: &|_| false,
                opencode_go_signed_in: &|_| false,
                cline_signed_in: &|_| false,
            },
        );
        let json = rows_json(&rows, true);
        let list = json.as_array().unwrap();
        // Every key vendor carries the overlay's own label and note...
        for vendor in crate::tui::settings::KEY_VENDORS {
            let row = list
                .iter()
                .find(|row| row["id"] == vendor.id.slug())
                .unwrap();
            assert_eq!(
                row["key_label"],
                vendor.secret_label,
                "{}",
                vendor.id.slug()
            );
            assert_eq!(row["key_note"], vendor.note, "{}", vendor.id.slug());
        }
        // ...and no other vendor carries the pair at all.
        for &id in VendorId::all() {
            if crate::tui::settings::KEY_VENDORS
                .iter()
                .any(|kv| kv.id == id)
            {
                continue;
            }
            let row = list.iter().find(|row| row["id"] == id.slug()).unwrap();
            assert!(row.get("key_label").is_none(), "{:?}", id);
            assert!(row.get("key_note").is_none(), "{:?}", id);
        }
    }

    #[test]
    fn key_check_requests_parse_or_are_none() {
        assert_eq!(
            key_check_request(&json!({"id": "zai", "key": "test-key-value", "seq": 7})),
            Some(("zai".to_string(), "test-key-value".to_string(), 7))
        );
        // Missing or non-`u64` seq: ignored.
        assert_eq!(
            key_check_request(&json!({"id": "zai", "key": "test-key-value"})),
            None
        );
        assert_eq!(
            key_check_request(&json!({"id": "zai", "key": "test-key-value", "seq": 1.5})),
            None
        );
        assert_eq!(
            key_check_request(&json!({"id": "zai", "key": "test-key-value", "seq": "7"})),
            None
        );
        assert_eq!(
            key_check_request(&json!({"id": "zai", "key": "test-key-value", "seq": -1})),
            None
        );
        // Non-string key or id: ignored.
        assert_eq!(
            key_check_request(&json!({"id": "zai", "key": 7, "seq": 1})),
            None
        );
        assert_eq!(
            key_check_request(&json!({"id": 7, "key": "test-key-value", "seq": 1})),
            None
        );
    }

    #[cfg(any(windows, target_os = "macos"))]
    /// Serializes the tests that steer the process-wide `--config` override
    /// at a temp file, and clears it when the test ends — including via a
    /// panic. A leaked override makes every later test resolve a deleted
    /// temp file, turning one failure into a cascade of sibling failures.
    struct ScopedKeyOverride {
        _serial: std::sync::MutexGuard<'static, ()>,
    }

    #[cfg(any(windows, target_os = "macos"))]
    impl Drop for ScopedKeyOverride {
        fn drop(&mut self) {
            crate::config::clear_override_path();
        }
    }

    #[cfg(any(windows, target_os = "macos"))]
    fn override_guard() -> ScopedKeyOverride {
        static M: std::sync::Mutex<()> = std::sync::Mutex::new(());
        ScopedKeyOverride {
            _serial: M.lock().unwrap_or_else(|p| p.into_inner()),
        }
    }

    #[cfg(any(windows, target_os = "macos"))]
    fn key_check_value(id: &str, key: &str, seq: u64) -> Value {
        json!({"cmd": "set-api-key", "id": id, "key": key, "seq": seq})
    }

    #[cfg(any(windows, target_os = "macos"))]
    fn default_facts() -> SharedFacts {
        Arc::new(std::sync::Mutex::new(
            crate::tray::payload::HostFacts::default(),
        ))
    }

    #[test]
    #[cfg(any(windows, target_os = "macos"))]
    fn key_check_ignores_malformed_requests_without_writing_or_answering() {
        let guard = override_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        crate::config::set_override_path(&path);
        let facts = default_facts();
        let fetch_called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        // "anthropic" is a vendor, but an OAuth one: no credential field.
        for value in [
            json!({"cmd": "set-api-key", "id": "anthropic", "key": "test-key-value", "seq": 1}),
            json!({"cmd": "set-api-key", "id": "zai", "key": "test-key-value"}),
            json!({"cmd": "set-api-key", "id": "zai", "key": "test-key-value", "seq": 1.5}),
            json!({"cmd": "set-api-key", "id": "zai", "key": 7, "seq": 1}),
            // A key the overlay would refuse: no response, no write.
            json!({"cmd": "set-api-key", "id": "zai", "key": "a\u{1b}[31mb", "seq": 1}),
        ] {
            let called = Arc::clone(&fetch_called);
            let result = request_key_check_with(
                &facts,
                &value,
                move |slug| {
                    called.store(true, Ordering::Relaxed);
                    Ok::<_, String>(slug.to_string())
                },
                |_| Ok(()),
                |_, _| {},
                |_| false,
                || {},
                || {},
            )
            .expect("an ignored request is not a spawn failure");
            assert!(result.is_none(), "the request must be ignored: {value}");
        }
        // Nothing was published, nothing fetched, nothing written.
        assert!(facts_snapshot(&facts).key_check.is_none());
        assert!(!fetch_called.load(Ordering::Relaxed));
        assert!(
            !path.exists(),
            "an ignored request must not create the config"
        );
        drop(guard);
    }

    #[test]
    #[cfg(any(windows, target_os = "macos"))]
    fn key_check_failed_restores_the_config_and_re_requests_the_catalog() {
        let guard = override_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        // The vendor is off, without an inline key: the dialog is allowed.
        // The check's write turns it on, and the restore must turn it back
        // off, byte for byte.
        let original = "[ui]\nprimary = \"zai\"\n[zai]\nenabled = false\n";
        std::fs::write(&path, original).unwrap();
        crate::config::set_override_path(&path);
        let facts = default_facts();
        // The catalog must be re-requested by the check (`configured`
        // changed): exactly once, through the seam, not a global flag.
        let catalog = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let counter = Arc::clone(&catalog);
        let handle = request_key_check_with(
            &facts,
            &key_check_value("zai", "test-key-value", 7),
            |slug| {
                assert_eq!(slug, "zai");
                Ok(r#"{"entries":[{"id":"zai","status":"error","stale":false,"error":"HTTP 401: invalid key"}]}"#.into())
            },
            |_| Ok(()),
            |_, _| {},
            |_| false,
            move || {
                counter.fetch_add(1, Ordering::Relaxed);
            },
            || {},
        )
        .expect("spawn")
        .expect("a well-formed request starts the check");
        handle.join().unwrap();
        // The config is back exactly where the check found it...
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        // ...the verdict was published under the request's id and seq...
        let fact = facts_snapshot(&facts)
            .key_check
            .expect("the check must publish its verdict");
        assert_eq!(fact.id, "zai");
        assert_eq!(fact.seq, 7);
        assert_eq!(fact.state, KeyCheckState::Failed);
        assert_eq!(fact.error, "HTTP 401: invalid key");
        // ...and the catalog was re-requested.
        assert_eq!(catalog.load(Ordering::Relaxed), 1);
        drop(guard);
    }

    #[test]
    fn key_check_failure_restores_inline_tables() {
        for original in [
            "zai = { plan_tier = \"pro\" } # keep me\n[ui]\nprimary = \"anthropic\"\n",
            "zai = { enabled = false, plan_tier = \"pro\" } # keep me\n[ui]\nprimary = \"anthropic\"\n",
            "zai = { enabled = false, api_key = '', plan_tier = \"pro\" } # keep me\n[ui]\nprimary = \"anthropic\"\n",
        ] {
            for cache_failure in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("config.toml");
                std::fs::write(&path, original).unwrap();
                let facts = Arc::new(std::sync::Mutex::new(HostFacts {
                    key_check: Some(KeyCheckFact {
                        id: "zai".into(),
                        seq: 1,
                        state: KeyCheckState::Checking,
                        error: String::new(),
                    }),
                    ..Default::default()
                }));
                let fetched = std::cell::Cell::new(false);

                // Exercise the worker shared by both tray hosts, with only
                // the provider/cache operations replaced. Its config path
                // is explicit, so this also runs on Linux without HOME or a
                // process-wide config override.
                run_key_check(
                    &facts,
                    &path,
                    "zai",
                    "test-key-value".into(),
                    1,
                    |_| {
                        fetched.set(true);
                        Err("HTTP 401: invalid key".into())
                    },
                    |_| {
                        let written = crate::config::Config::load_from(&path).unwrap();
                        assert!(written.zai.enabled);
                        assert_eq!(written.zai.api_key.as_deref(), Some("test-key-value"));
                        if cache_failure {
                            Err("fixture cache failure".into())
                        } else {
                            Ok(())
                        }
                    },
                    |_, _| {},
                    || {},
                    || {},
                );

                assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
                assert_eq!(fetched.get(), !cache_failure);
                // Read the shared record directly: `facts_snapshot` is
                // compiled only for the window/menu-bar hosts, and this test
                // is meant to run on every platform.
                let fact = facts.lock().unwrap().key_check.clone().unwrap();
                assert_eq!(fact.state, KeyCheckState::Failed);
                assert_eq!(
                    fact.error,
                    if cache_failure {
                        "Could not clear the provider's cache, so the key was not checked."
                    } else {
                        "HTTP 401: invalid key"
                    }
                );
            }
        }
    }

    #[test]
    #[cfg(any(windows, target_os = "macos"))]
    fn key_check_keeps_the_key_when_the_provider_accepts() {
        let guard = override_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let original = "[ui]\nprimary = \"zai\"\n";
        std::fs::write(&path, original).unwrap();
        crate::config::set_override_path(&path);
        let facts = default_facts();
        let handle = request_key_check_with(
            &facts,
            &key_check_value("zai", "test-key-value", 3),
            |_| Ok(r#"{"entries":[{"id":"zai","status":"ready","stale":false}]}"#.into()),
            |_| Ok(()),
            |_, _| {},
            |_| false,
            || {},
            || {},
        )
        .expect("spawn")
        .unwrap();
        handle.join().unwrap();
        // An accepted key stays in the config, with the vendor opted in.
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(
            written.contains(r#"api_key = "test-key-value""#),
            "{written}"
        );
        assert!(written.contains("enabled = true"), "{written}");
        let fact = facts_snapshot(&facts)
            .key_check
            .expect("the check must publish its verdict");
        assert_eq!(fact.state, KeyCheckState::Ok);
        assert_eq!(fact.error, "");
        drop(guard);
    }

    #[test]
    #[cfg(any(windows, target_os = "macos"))]
    fn key_check_removes_the_key_when_there_was_none() {
        let guard = override_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let original = "[ui]\nprimary = \"zai\"\n";
        std::fs::write(&path, original).unwrap();
        crate::config::set_override_path(&path);
        let facts = default_facts();
        let handle = request_key_check_with(
            &facts,
            &key_check_value("zai", "test-key-value", 4),
            |_| Err("HTTP 429: rate limited".to_string()),
            |_| Ok(()),
            |_, _| {},
            |_| false,
            || {},
            || {},
        )
        .expect("spawn")
        .unwrap();
        handle.join().unwrap();
        // The refusal removed the key the check wrote; nothing else moved.
        let restored = std::fs::read_to_string(&path).unwrap();
        assert!(!restored.contains("api_key"), "{restored}");
        assert!(restored.contains(r#"primary = "zai""#), "{restored}");
        let fact = facts_snapshot(&facts)
            .key_check
            .expect("the check must publish its verdict");
        assert_eq!(fact.state, KeyCheckState::Failed);
        assert_eq!(fact.error, "HTTP 429: rate limited");
        drop(guard);
    }

    #[test]
    #[cfg(all(unix, any(windows, target_os = "macos")))]
    fn key_check_failed_restore_says_the_key_stayed() {
        use std::os::unix::fs::PermissionsExt;
        let guard = override_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\nprimary = \"zai\"\n").unwrap();
        crate::config::set_override_path(&path);
        let facts = default_facts();
        // The fetch runs inside the thread, between the write and the
        // restore: make the directory read-only there, so the restore's
        // atomic write cannot stage its temp file.
        let locked = dir.path().to_path_buf();
        let lock_target = locked.clone();
        let handle = request_key_check_with(
            &facts,
            &key_check_value("zai", "test-key-value", 5),
            move |_| {
                std::fs::set_permissions(&lock_target, std::fs::Permissions::from_mode(0o555))
                    .unwrap();
                Err("HTTP 401: invalid key".to_string())
            },
            |_| Ok(()),
            |_, _| {},
            |_| false,
            || {},
            || {},
        )
        .expect("spawn")
        .unwrap();
        handle.join().unwrap();
        // The directory is writable again before the test's temp file is
        // removed.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        let fact = facts_snapshot(&facts)
            .key_check
            .expect("the check must publish its verdict");
        assert_eq!(fact.state, KeyCheckState::Failed);
        assert_eq!(
            fact.error,
            "HTTP 401: invalid key The new key stayed in config.toml."
        );
        drop(guard);
    }

    #[test]
    #[cfg(any(windows, target_os = "macos"))]
    fn key_check_a_newer_send_wins_the_fact() {
        let guard = override_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\nprimary = \"zai\"\n").unwrap();
        crate::config::set_override_path(&path);
        let facts = default_facts();
        // Both sends pass the pre-thread checks (neither worker has run
        // yet, so neither sees an inline key) and publish their `Checking`
        // facts; both workers queue on the lock this test holds.
        let lock_guard = KEY_CHECK_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let handle = request_key_check_with(
            &facts,
            &key_check_value("zai", "first-key", 10),
            |_| Ok(r#"{"entries":[{"id":"zai","status":"ready","stale":false}]}"#.into()),
            |_| Ok(()),
            |_, _| {},
            |_| false,
            || {},
            || {},
        )
        .expect("spawn")
        .unwrap();
        let handle2 = request_key_check_with(
            &facts,
            &key_check_value("zai", "second-key", 11),
            |_| Err("HTTP 401: invalid key".to_string()),
            |_| Ok(()),
            |_, _| {},
            |_| false,
            || {},
            || {},
        )
        .expect("spawn")
        .unwrap();
        // Send 2's `Checking` replaced send 1's: the compare-and-publish
        // refused the older one.
        let fact = facts_snapshot(&facts)
            .key_check
            .expect("send 2 must publish Checking over send 1's");
        assert_eq!(fact.seq, 11);
        // A yet older send that lands after the newer one must not push
        // the dialog backwards either: its `Checking` is refused outright.
        let handle3 = request_key_check_with(
            &facts,
            &key_check_value("zai", "older-key", 9),
            |_| unreachable!("the superseded send must not fetch"),
            |_| Ok(()),
            |_, _| {},
            |_| false,
            || {},
            || {},
        )
        .expect("spawn")
        .expect("a well-formed request still starts");
        let fact = facts_snapshot(&facts)
            .key_check
            .expect("the newer send's Checking must survive");
        assert_eq!(fact.seq, 11, "an older seq must not replace a newer fact");
        drop(lock_guard);
        // Whichever worker runs first, only the newer send's verdict may
        // survive: the older ones see the superseded `seq` under the lock
        // and exit before writing, fetching, or publishing.
        handle.join().unwrap();
        handle2.join().unwrap();
        handle3
            .join()
            .expect("the superseded send's thread must end");
        let fact = facts_snapshot(&facts)
            .key_check
            .expect("the verdict must survive");
        assert_eq!(fact.seq, 11, "the newer send's verdict must win");
        assert_eq!(fact.state, KeyCheckState::Failed);
        assert_eq!(fact.error, "HTTP 401: invalid key");
        // The file holds the newer send's restored state and nothing of
        // the superseded one: its fetch refused and it found no key to
        // restore, and send 1 never wrote its own.
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(!written.contains("first-key"), "{written}");
        drop(guard);
    }

    #[test]
    #[cfg(any(windows, target_os = "macos"))]
    fn key_check_never_puts_the_key_in_the_fact_or_payload() {
        let guard = override_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\nprimary = \"zai\"\n").unwrap();
        crate::config::set_override_path(&path);
        let facts = default_facts();
        let handle = request_key_check_with(
            &facts,
            &key_check_value("zai", "test-key-value", 9),
            |_| Err("HTTP 401: rejected".to_string()),
            |_| Ok(()),
            |_, _| {},
            |_| false,
            || {},
            || {},
        )
        .expect("spawn")
        .unwrap();
        handle.join().unwrap();
        // The key lived in the config while the check ran; it must not leak
        // into the fact, and the serialized payload must not carry it either.
        let snapshot = facts_snapshot(&facts);
        let payload = wrap_report("{}", &snapshot, 0, None);
        let fact = snapshot
            .key_check
            .expect("the check must publish its verdict");
        assert!(!fact.error.contains("test-key-value"), "{}", fact.error);
        assert!(!fact.id.contains("test-key-value"));
        assert_eq!(fact.seq, 9);
        assert!(!payload.to_string().contains("test-key-value"));
        drop(guard);
    }

    #[test]
    #[cfg(any(windows, target_os = "macos"))]
    fn key_check_scrubs_a_provider_that_echoes_the_key() {
        let guard = override_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\nprimary = \"zai\"\n").unwrap();
        crate::config::set_override_path(&path);
        let facts = default_facts();
        // The provider echoes the key verbatim in its error.
        let handle = request_key_check_with(
            &facts,
            &key_check_value("zai", "test-key-value", 14),
            |_| Err("key is test-key-value, try again".to_string()),
            |_| Ok(()),
            |_, _| {},
            |_| false,
            || {},
            || {},
        )
        .expect("spawn")
        .unwrap();
        handle.join().unwrap();
        let snapshot = facts_snapshot(&facts);
        let payload = wrap_report("{}", &snapshot, 0, None);
        let fact = snapshot
            .key_check
            .expect("the check must publish its verdict");
        // The verbatim echo is the marker, not the key.
        assert_eq!(fact.error, "key is [key], try again");
        assert!(!payload.to_string().contains("test-key-value"));
        drop(guard);
    }

    #[test]
    #[cfg(any(windows, target_os = "macos"))]
    fn key_check_delayed_old_send_exits_before_writing_or_fetching() {
        let guard = override_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\nprimary = \"zai\"\n").unwrap();
        crate::config::set_override_path(&path);
        let facts = default_facts();
        // Hold the single-flight lock so the first worker can only queue
        // behind it: it has not written, so the second send's pre-thread
        // check sees no inline key and is accepted.
        let lock_guard = KEY_CHECK_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let fetch1_called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let called1 = std::sync::Arc::clone(&fetch1_called);
        let handle1 = request_key_check_with(
            &facts,
            &key_check_value("zai", "first-key", 10),
            move |_| {
                called1.store(true, Ordering::Relaxed);
                Ok(r#"{"entries":[{"id":"zai","status":"ready","stale":false}]}"#.into())
            },
            |_| Ok(()),
            |_, _| {},
            |_| false,
            || {},
            || {},
        )
        .expect("spawn")
        .expect("send 1 starts (no key in the config yet)");
        let handle2 = request_key_check_with(
            &facts,
            &key_check_value("zai", "second-key", 11),
            |_| Ok(r#"{"entries":[{"id":"zai","status":"ready","stale":false}]}"#.into()),
            |_| Ok(()),
            |_, _| {},
            |_| false,
            || {},
            || {},
        )
        .expect("spawn")
        .expect("send 2 starts (send 1's worker has not written yet)");
        // Send 2's `Checking` replaced send 1's while send 1 queued.
        let fact = facts_snapshot(&facts)
            .key_check
            .expect("send 2 must publish Checking over send 1's");
        assert_eq!(fact.seq, 11);
        drop(lock_guard);
        handle1
            .join()
            .expect("send 1's thread must end (no panic, no dead end)");
        handle2.join().unwrap();
        // Send 2's key is in the file; send 1's worker saw the superseded
        // `seq` under the lock and left both alone.
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("second-key"), "{written}");
        assert!(!written.contains("first-key"), "{written}");
        assert!(
            !fetch1_called.load(Ordering::Relaxed),
            "a superseded send must not fetch"
        );
        let fact = facts_snapshot(&facts)
            .key_check
            .expect("the verdict must survive");
        assert_eq!(
            fact.seq, 11,
            "no fact of the superseded send may be published"
        );
        assert_eq!(fact.state, KeyCheckState::Ok);
        drop(guard);
    }

    #[test]
    #[cfg(any(windows, target_os = "macos"))]
    fn key_check_second_send_while_first_is_in_flight_is_refused() {
        let guard = override_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\nprimary = \"zai\"\n").unwrap();
        crate::config::set_override_path(&path);
        let facts = default_facts();
        // Send 1 blocks inside its fetch with its key in the file: the
        // dialog's precondition — no saved inline key — no longer holds
        // for anything that lands while it waits.
        let (gate_tx, gate_rx) = std::sync::mpsc::channel::<()>();
        let handle = request_key_check_with(
            &facts,
            &key_check_value("zai", "first-key", 100),
            move |_| {
                gate_rx
                    .recv()
                    .expect("the test must release send 1's fetch");
                Ok(r#"{"entries":[{"id":"zai","status":"ready","stale":false}]}"#.into())
            },
            |_| Ok(()),
            |_, _| {},
            |_| false,
            || {},
            || {},
        )
        .expect("spawn")
        .expect("send 1 starts (no key in the config yet)");
        for _ in 0..1000 {
            if std::fs::read_to_string(&path)
                .map(|raw| raw.contains("first-key"))
                .unwrap_or(false)
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("first-key"),
            "send 1 must have written before its fetch ran"
        );
        let result = request_key_check_with(
            &facts,
            &key_check_value("zai", "second-key", 101),
            |_| unreachable!("the refused send must not fetch"),
            |_| Ok(()),
            |_, _| {},
            |_| false,
            || {},
            || {},
        );
        let fact = result.expect_err("an inline key in the file must refuse the second send");
        assert_eq!(fact.seq, 101);
        assert_eq!(
            fact.error,
            "A key is already saved in config.toml; replace it in the TUI settings."
        );
        // The refusal published no fact: the dialog is still on send 1.
        let current = facts_snapshot(&facts)
            .key_check
            .expect("send 1's fact must survive the refusal");
        assert_eq!(current.seq, 100);
        assert_eq!(current.state, KeyCheckState::Checking);
        gate_tx.send(()).unwrap();
        handle.join().unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("first-key"), "{written}");
        assert!(!written.contains("second-key"), "{written}");
        let fact = facts_snapshot(&facts)
            .key_check
            .expect("the verdict must win");
        assert_eq!(fact.seq, 100);
        assert_eq!(fact.state, KeyCheckState::Ok);
        drop(guard);
    }

    #[test]
    #[cfg(any(windows, target_os = "macos"))]
    fn key_check_busts_the_vendor_cache_before_the_fetch() {
        let guard = override_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\nprimary = \"zai\"\n").unwrap();
        crate::config::set_override_path(&path);
        // A fresh cache payload: a reading made under the previous (absent)
        // key. Without a bust, the fetch's cache gate would serve it
        // without the network and pass it as the new key's reading. The
        // layout mirrors `Cache::for_vendor(slug)`: root per slug.
        let cache_root = dir.path().join("cache");
        let cache_dir = cache_root.join("zai");
        crate::cache::Cache::at(cache_dir.clone())
            .ensure_dir()
            .unwrap();
        std::fs::write(
            cache_dir.join("usage.json"),
            r#"{"entries":[{"id":"zai","status":"ready","stale":false}]}"#,
        )
        .unwrap();
        let facts = default_facts();
        let fetch_root = cache_root.clone();
        let bust_root = cache_root.clone();
        let handle = request_key_check_with(
            &facts,
            &key_check_value("zai", "test-key-value", 13),
            move |slug| {
                // The same decision `fetch_snapshot` makes: a fresh
                // payload is served without the network; only a busted
                // cache reaches the (refusing) wire.
                let fresh = crate::cache::Cache::at(fetch_root.join(slug))
                    .fresh_payload(std::time::Duration::from_secs(60))
                    .map(|payload| payload.is_some())
                    .unwrap_or(false);
                if fresh {
                    Ok(r#"{"entries":[{"id":"zai","status":"ready","stale":false}]}"#.into())
                } else {
                    Err("HTTP 401: invalid key".to_string())
                }
            },
            move |slug| {
                crate::cache::Cache::at(bust_root.join(slug))
                    .try_forget()
                    .map_err(|error| error.to_string())
            },
            |_, _| {},
            |_| false,
            || {},
            || {},
        )
        .expect("spawn")
        .unwrap();
        handle.join().unwrap();
        // The fresh reading must not have decided the verdict: the wire
        // was reached, refused, and the key restored.
        let fact = facts_snapshot(&facts)
            .key_check
            .expect("the check must publish its verdict");
        assert_eq!(fact.state, KeyCheckState::Failed);
        assert_eq!(fact.error, "HTTP 401: invalid key");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[ui]\nprimary = \"zai\"\n"
        );
        // And the bust happened: the payload is gone.
        assert!(!cache_dir.join("usage.json").exists());
        drop(guard);
    }

    #[test]
    #[cfg(any(windows, target_os = "macos"))]
    fn key_check_inline_key_present_refuses_before_writing() {
        let guard = override_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let original = "[ui]\nprimary = \"zai\"\n[zai]\napi_key = \"old-key-value\"\n";
        std::fs::write(&path, original).unwrap();
        crate::config::set_override_path(&path);
        let facts = default_facts();
        let env_consulted = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let consulted = std::sync::Arc::clone(&env_consulted);
        let fetch_called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let called = std::sync::Arc::clone(&fetch_called);
        let result = request_key_check_with(
            &facts,
            &key_check_value("zai", "test-key-value", 19),
            move |_| {
                called.store(true, Ordering::Relaxed);
                Ok(String::new())
            },
            |_| Ok(()),
            |_, _| {},
            move |name| {
                consulted
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(name.to_string());
                false
            },
            || {},
            || {},
        );
        // An inline key already in the config is replaced in the TUI
        // settings, not checked here: the refusal is before the env check,
        // the write, the fetch, and any fact.
        let fact = result.expect_err("an existing inline key must refuse the check");
        assert_eq!(fact.seq, 19);
        assert_eq!(fact.state, KeyCheckState::Failed);
        assert_eq!(
            fact.error,
            "A key is already saved in config.toml; replace it in the TUI settings."
        );
        assert!(
            !fetch_called.load(Ordering::Relaxed),
            "no fetch for a saved key"
        );
        assert!(facts_snapshot(&facts).key_check.is_none());
        assert!(
            env_consulted
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty(),
            "the env check must not run for a saved key"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        drop(guard);
    }

    #[test]
    #[cfg(any(windows, target_os = "macos"))]
    fn key_check_bust_failure_refuses_without_fetching() {
        let guard = override_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\nprimary = \"zai\"\n").unwrap();
        crate::config::set_override_path(&path);
        let facts = default_facts();
        let fetch_called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let called = std::sync::Arc::clone(&fetch_called);
        let handle = request_key_check_with(
            &facts,
            &key_check_value("zai", "test-key-value", 20),
            move |_| {
                called.store(true, Ordering::Relaxed);
                Err("HTTP 401: invalid key".to_string())
            },
            |slug| {
                assert_eq!(slug, "zai");
                Err("read-only cache directory".to_string())
            },
            |_, _| {},
            |_| false,
            || {},
            || {},
        )
        .expect("spawn")
        .unwrap();
        handle.join().unwrap();
        // A bust the thread cannot prove is a bust that did not happen: no
        // fetch under an unverified invalidation, the config is restored, and
        // the refusal says so — without the system error in the message.
        let fact = facts_snapshot(&facts)
            .key_check
            .expect("the refusal must publish its verdict");
        assert_eq!(fact.seq, 20);
        assert_eq!(fact.state, KeyCheckState::Failed);
        assert_eq!(
            fact.error,
            "Could not clear the provider's cache, so the key was not checked."
        );
        assert!(
            !fetch_called.load(Ordering::Relaxed),
            "the fetch must not run"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[ui]\nprimary = \"zai\"\n"
        );
        drop(guard);
    }

    #[test]
    #[cfg(any(windows, target_os = "macos"))]
    fn key_check_failed_verdict_clears_the_persisted_diagnostic() {
        let guard = override_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\nprimary = \"zai\"\n").unwrap();
        crate::config::set_override_path(&path);
        // The check's fetch runs under the candidate key: a real refusing
        // fetch persists the provider's answer in the vendor's `.last_error`
        // and arms the 429 backoff. The layout mirrors `Cache::for_vendor`.
        let cache_root = dir.path().join("cache");
        let cache_dir = cache_root.join("zai");
        crate::cache::Cache::at(cache_dir.clone())
            .ensure_dir()
            .unwrap();
        let facts = default_facts();
        let error_cache = cache_dir.clone();
        let bust_root = cache_root.clone();
        let clean_root = cache_root.clone();
        let handle = request_key_check_with(
            &facts,
            &key_check_value("zai", "test-key-value", 21),
            move |_| {
                // What the real fetch's writer leaves behind: the provider's
                // answer (naming the key), persisted at code 0, and the
                // backoff the candidate key armed.
                crate::cache::Cache::at(error_cache.clone())
                    .write_last_error(0, "key is test-key-value");
                crate::cache::Cache::at(error_cache)
                    .note_rate_limit_at(std::time::SystemTime::now());
                Err("key is test-key-value".to_string())
            },
            move |slug| {
                crate::cache::Cache::at(bust_root.join(slug))
                    .try_forget()
                    .map_err(|error| error.to_string())
            },
            move |slug, _key| {
                let _ = crate::cache::Cache::at(clean_root.join(slug)).try_forget();
            },
            |_| false,
            || {},
            || {},
        )
        .expect("spawn")
        .unwrap();
        handle.join().unwrap();
        let fact = facts_snapshot(&facts)
            .key_check
            .expect("the verdict must be published");
        assert_eq!(fact.state, KeyCheckState::Failed);
        assert_eq!(fact.error, "key is [key]");
        // The persisted diagnostic (which names the key) and the backoff are
        // gone: the failed verdict busted the cache again, best effort, so
        // nothing the candidate key armed outlives the check.
        for entry in std::fs::read_dir(&cache_dir).unwrap() {
            let entry = entry.unwrap();
            if entry.path().is_file() {
                let raw = std::fs::read_to_string(entry.path()).unwrap_or_default();
                assert!(
                    !raw.contains("test-key-value"),
                    "{}: {}",
                    entry.file_name().to_string_lossy(),
                    raw
                );
            }
        }
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[ui]\nprimary = \"zai\"\n"
        );
        drop(guard);
    }

    #[test]
    #[cfg(any(windows, target_os = "macos"))]
    fn key_check_failed_keeps_a_user_disable_made_during_the_check() {
        let guard = override_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        // The provider is on and has no inline key: the dialog is allowed.
        std::fs::write(&path, "[ui]\nprimary = \"zai\"\n[zai]\nenabled = true\n").unwrap();
        crate::config::set_override_path(&path);
        let facts = default_facts();
        let disable_path = path.clone();
        let handle = request_key_check_with(
            &facts,
            &key_check_value("zai", "test-key-value", 22),
            move |_| {
                // The user turns the provider off while the check waits for
                // the provider: a later decision on a field the check wrote,
                // made with the real writer.
                crate::config::set_vendor_enabled(&disable_path, "zai", false).unwrap();
                Err("HTTP 401: invalid key".to_string())
            },
            |_| Ok(()),
            |_, _| {},
            |_| false,
            || {},
            || {},
        )
        .expect("spawn")
        .unwrap();
        handle.join().unwrap();
        // The candidate key is gone, but the user's disable survives the
        // rollback: the compare-and-restore only undoes what the check
        // itself wrote.
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("test-key-value"), "{raw}");
        assert!(!raw.contains("api_key"), "{raw}");
        assert!(raw.contains("enabled = false"), "{raw}");
        let fact = facts_snapshot(&facts)
            .key_check
            .expect("the verdict must be published");
        assert_eq!(fact.state, KeyCheckState::Failed);
        assert_eq!(fact.error, "HTTP 401: invalid key");
        drop(guard);
    }

    #[test]
    #[cfg(any(windows, target_os = "macos"))]
    fn key_check_wire_fetch_ignores_a_payload_refilled_after_the_bust() {
        let guard = override_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\nprimary = \"zai\"\n").unwrap();
        crate::config::set_override_path(&path);
        let facts = default_facts();
        // A producer that started under the old key lands its reading in
        // the vendor's cache after the bust succeeded: the race that makes
        // a cached payload a lie about the key just written. The layout
        // mirrors `Cache::for_vendor(slug)`.
        let cache_root = dir.path().join("cache");
        let bust_root = cache_root.clone();
        let fetch_root = cache_root.clone();
        let clean_root = cache_root.clone();
        let refilled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let bust_refill = std::sync::Arc::clone(&refilled);
        let handle = request_key_check_with(
            &facts,
            &key_check_value("zai", "test-key-value", 30),
            move |slug| {
                // The real fetch's one pre-network hook, at the check's
                // zero TTL: a payload the old producer left is not an
                // answer about the key just written, so the wire decides —
                // the injected refusal.
                let cache = crate::cache::Cache::at(fetch_root.join(slug));
                match cache.fresh_payload(std::time::Duration::ZERO) {
                    Ok(Some(payload)) => Ok(String::from_utf8_lossy(&payload).into()),
                    Ok(None) => Err("HTTP 401: invalid key".to_string()),
                    Err(error) => Err(error.user_message()),
                }
            },
            move |slug| {
                let cache = crate::cache::Cache::at(bust_root.join(slug));
                let result = cache.try_forget().map_err(|error| error.to_string());
                // The old producer's answer lands after the bust: a fresh
                // mtime, so an ordinary TTL would call it fresh.
                if result.is_ok() && !bust_refill.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    cache
                        .write_payload(
                            br#"{"entries":[{"id":"zai","status":"ready","stale":false}]}"#,
                        )
                        .unwrap();
                }
                result
            },
            move |slug, _key| {
                let _ = crate::cache::Cache::at(clean_root.join(slug)).try_forget();
            },
            |_| false,
            || {},
            || {},
        )
        .expect("spawn")
        .unwrap();
        handle.join().unwrap();
        // The refilled payload never decided the verdict: the wire
        // refusal did, and the key went back where it came from.
        let fact = facts_snapshot(&facts)
            .key_check
            .expect("the verdict must be published");
        assert_eq!(fact.state, KeyCheckState::Failed);
        assert_eq!(fact.error, "HTTP 401: invalid key");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[ui]\nprimary = \"zai\"\n"
        );
        // The final cleanup dropped the refilled payload, too.
        assert!(!cache_root.join("zai").join("usage.json").exists());
        drop(guard);
    }

    #[test]
    #[cfg(any(windows, target_os = "macos"))]
    fn key_check_empty_inline_field_is_not_a_saved_key() {
        let guard = override_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        // The catalog's rule: an empty `api_key` offers the dialog, so the
        // check must not refuse it as an existing key.
        let original = "[ui]\nprimary = \"zai\"\n[zai]\napi_key = ''\nenabled = true\n";
        std::fs::write(&path, original).unwrap();
        crate::config::set_override_path(&path);
        let facts = default_facts();
        let handle = request_key_check_with(
            &facts,
            &key_check_value("zai", "test-key-value", 31),
            |_| Err("HTTP 401: invalid key".to_string()),
            |_| Ok(()),
            |_, _| {},
            |_| false,
            || {},
            || {},
        )
        .expect("spawn")
        .expect("an empty field must not refuse the check");
        handle.join().unwrap();
        // The refusal restores the file exactly — the empty field
        // included.
        let fact = facts_snapshot(&facts)
            .key_check
            .expect("the verdict must be published");
        assert_eq!(fact.state, KeyCheckState::Failed);
        assert_eq!(fact.error, "HTTP 401: invalid key");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        drop(guard);
    }

    #[test]
    #[cfg(any(windows, target_os = "macos"))]
    fn key_check_key_saved_by_the_tui_while_queued_is_not_replaced() {
        let guard = override_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\nprimary = \"zai\"\n").unwrap();
        crate::config::set_override_path(&path);
        let facts = default_facts();
        // Another check holds the lock; this send is admitted (no key
        // saved yet) and queues. While it waits, the TUI saves a key with
        // its writer, on the same file — the TUI is not behind this lock.
        let lock_guard = KEY_CHECK_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let fetch_called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let called = std::sync::Arc::clone(&fetch_called);
        let handle = request_key_check_with(
            &facts,
            &key_check_value("zai", "test-key-value", 32),
            move |_| {
                called.store(true, Ordering::Relaxed);
                Ok(r#"{"entries":[{"id":"zai","status":"ready","stale":false}]}"#.into())
            },
            |_| Ok(()),
            |_, _| {},
            |_| false,
            || {},
            || {},
        )
        .expect("spawn")
        .expect("admitted while no key is saved");
        let vendor = crate::tui::settings::key_vendor_for_slug("zai").unwrap();
        crate::tui::settings::write_inline_key(&path, vendor, "tui-key-value").unwrap();
        drop(lock_guard);
        handle.join().unwrap();
        // The queued check re-validated under the lock and found the TUI's
        // key: it refused, left the key alone, and never fetched.
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains(r#"api_key = "tui-key-value""#), "{raw}");
        assert!(!raw.contains("test-key-value"), "{raw}");
        assert!(
            !fetch_called.load(Ordering::Relaxed),
            "a refused check must not fetch"
        );
        let fact = facts_snapshot(&facts)
            .key_check
            .expect("the refusal verdict must be published");
        assert_eq!(fact.seq, 32);
        assert_eq!(fact.state, KeyCheckState::Failed);
        assert_eq!(fact.error, crate::tui::settings::KEY_ALREADY_SAVED_MESSAGE);
        drop(guard);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn key_check_final_cleanup_scrubs_the_diagnostic_when_the_bust_fails() {
        use std::os::unix::fs::PermissionsExt;
        let guard = override_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\nprimary = \"zai\"\n").unwrap();
        crate::config::set_override_path(&path);
        // The fetch's writer persists the provider's answer — naming the
        // key — before the final cleanup; the cache directory then loses
        // write permission, so the cleanup's bust fails and the in-place
        // scrub must take over. The layout mirrors `Cache::for_vendor`.
        let cache_root = dir.path().join("cache");
        let cache_dir = cache_root.join("zai");
        crate::cache::Cache::at(cache_dir.clone())
            .ensure_dir()
            .unwrap();
        let facts = default_facts();
        let fetch_dir = cache_dir.clone();
        let clean_dir = cache_dir.clone();
        let clean_root = cache_root.clone();
        let handle = request_key_check_with(
            &facts,
            &key_check_value("zai", "test-key-value", 33),
            move |_| {
                crate::cache::Cache::at(fetch_dir).write_last_error(0, "key is test-key-value");
                Err("key is test-key-value".to_string())
            },
            |_| Ok(()),
            move |slug, key| {
                // The production cleanup: the bust now fails (read-only
                // directory), and the scrub rewrites the diagnostic in
                // place.
                std::fs::set_permissions(&clean_dir, std::fs::Permissions::from_mode(0o555))
                    .unwrap();
                let cache = crate::cache::Cache::at(clean_root.join(slug));
                if cache.try_forget().is_err() {
                    let _ = cache.scrub_last_error(|raw| {
                        let (code, message) = raw.split_once('\n').unwrap_or((raw, ""));
                        format!("{code}\n{}", scrub_key(message, key))
                    });
                }
                std::fs::set_permissions(&clean_dir, std::fs::Permissions::from_mode(0o755))
                    .unwrap();
            },
            |_| false,
            || {},
            || {},
        )
        .expect("spawn")
        .unwrap();
        handle.join().unwrap();
        let fact = facts_snapshot(&facts)
            .key_check
            .expect("the verdict must be published");
        assert_eq!(fact.state, KeyCheckState::Failed);
        assert_eq!(fact.error, "key is [key]");
        // The diagnostic survived (the directory refused removal) but no
        // longer names the key: it was rewritten in place, not recreated.
        let raw = std::fs::read_to_string(cache_dir.join(".last_error")).unwrap();
        assert!(!raw.contains("test-key-value"), "{raw}");
        assert!(raw.contains("[key]"), "{raw}");
        assert!(raw.starts_with("0\n"), "the code line must survive: {raw}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[ui]\nprimary = \"zai\"\n"
        );
        drop(guard);
    }

    #[test]
    #[cfg(any(windows, target_os = "macos"))]
    fn key_check_env_var_precedence_refuses_before_writing() {
        let guard = override_guard();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\nprimary = \"zai\"\n").unwrap();
        crate::config::set_override_path(&path);
        let facts = default_facts();
        let consulted = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let consulted_env = std::sync::Arc::clone(&consulted);
        let fetch_called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let called = std::sync::Arc::clone(&fetch_called);
        let result = request_key_check_with(
            &facts,
            &key_check_value("zai", "test-key-value", 12),
            move |_| {
                called.store(true, Ordering::Relaxed);
                Ok(r#"{"entries":[{"id":"zai","status":"ready","stale":false}]}"#.into())
            },
            |_| Ok(()),
            |_, _| {},
            move |name| {
                consulted_env
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push(name.to_string());
                true
            },
            || {},
            || {},
        );
        // The vendor's effective env var (the default when the config has
        // no `api_key_env`) is defined: the check is refused before the
        // write, the fetch, or a `Checking` fact.
        let fact = result.expect_err("a set env var must refuse the check");
        assert_eq!(fact.state, KeyCheckState::Failed);
        assert_eq!(
            fact.error,
            "ZAI_API_KEY is set in the environment and takes precedence over config.toml."
        );
        assert!(!fetch_called.load(Ordering::Relaxed));
        assert!(facts_snapshot(&facts).key_check.is_none());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[ui]\nprimary = \"zai\"\n"
        );
        assert_eq!(*consulted.lock().unwrap(), vec!["ZAI_API_KEY".to_string()]);
        // An `api_key_env` override must be the name consulted.
        let overridden = "[ui]\nprimary = \"zai\"\n[zai]\napi_key_env = \"CUSTOM_ZAI_KEY\"\n";
        std::fs::write(&path, overridden).unwrap();
        let consulted = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let consulted_env = std::sync::Arc::clone(&consulted);
        let result = request_key_check_with(
            &facts,
            &key_check_value("zai", "test-key-value", 15),
            |_| Ok(String::new()),
            |_| Ok(()),
            |_, _| {},
            move |name| {
                consulted_env
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push(name.to_string());
                true
            },
            || {},
            || {},
        );
        let fact = result.expect_err("the override must be consulted");
        assert_eq!(
            fact.error,
            "CUSTOM_ZAI_KEY is set in the environment and takes precedence over config.toml."
        );
        assert_eq!(
            *consulted.lock().unwrap(),
            vec!["CUSTOM_ZAI_KEY".to_string()]
        );
        drop(guard);
    }

    #[test]
    fn key_check_result_decides_from_the_report_entry() {
        let ready = r#"{"entries":[{"id":"zai","status":"ready","stale":false}]}"#;
        assert_eq!(
            key_check_result("zai", Ok(ready.to_string())),
            (KeyCheckState::Ok, String::new())
        );
        let error = r#"{"entries":[{"id":"zai","status":"error","stale":false,"error":"HTTP 401: invalid key"}]}"#;
        assert_eq!(
            key_check_result("zai", Ok(error.to_string())),
            (KeyCheckState::Failed, "HTTP 401: invalid key".to_string())
        );
        // A stale reading is a failed check even when the status says ready.
        let stale = r#"{"entries":[{"id":"zai","status":"ready","stale":true,"error":"offline"}]}"#;
        assert_eq!(
            key_check_result("zai", Ok(stale.to_string())),
            (KeyCheckState::Failed, "offline".to_string())
        );
        // No entry for the provider: the fixed note.
        let other = r#"{"entries":[{"id":"openai","status":"ready","stale":false}]}"#;
        assert_eq!(
            key_check_result("zai", Ok(other.to_string())),
            (
                KeyCheckState::Failed,
                "The provider returned no reading.".to_string()
            )
        );
        let broken = key_check_result("zai", Ok("{".to_string()));
        assert_eq!(broken.0, KeyCheckState::Failed);
        assert_eq!(broken.1, "The provider returned no reading.");
        // A fetch error is the error itself.
        assert_eq!(
            key_check_result("zai", Err("no enabled provider matches zai".to_string())),
            (
                KeyCheckState::Failed,
                "no enabled provider matches zai".to_string()
            )
        );
    }

    #[test]
    fn key_check_result_ignores_account_entries_and_the_cut_is_separate() {
        // A `slug@account` entry belongs to another credential: it must
        // not decide the check's verdict.
        let account = r#"{"entries":[{"id":"zai@main","status":"ready","stale":false}]}"#;
        assert_eq!(
            key_check_result("zai", Ok(account.to_string())),
            (
                KeyCheckState::Failed,
                "The provider returned no reading.".to_string()
            )
        );
        // A slug that is merely a prefix of another id does not match.
        let prefix = r#"{"entries":[{"id":"zai2","status":"ready","stale":false}]}"#;
        assert_eq!(
            key_check_result("zai", Ok(prefix.to_string())).1,
            "The provider returned no reading."
        );
        // The 300-char cut is the thread's, applied after the scrub:
        // char-boundary for wide text...
        let wide = "é".repeat(350);
        let cut = cut_key_check_error(&wide);
        assert_eq!(cut.chars().count(), 300);
        assert_eq!(cut.len(), 600);
        // ...and a byte count above the char count for narrow text.
        assert_eq!(cut_key_check_error(&"x".repeat(400)).len(), 300);
    }

    #[test]
    fn key_check_scrub_strips_echoed_keys_before_the_cut() {
        let key = "abcdefghijklmnop";
        // A verbatim echo becomes the marker...
        assert_eq!(scrub_key(key, key), "[key]");
        // ...alongside anything else the provider said about it.
        assert_eq!(scrub_key(&format!("bad key {key}"), key), "bad key [key]");
        // An error without the key passes through.
        assert_eq!(scrub_key("HTTP 500", key), "HTTP 500");
        // An eight-char window of the key — a partial echo — is the key in
        // all but the missing tail: the whole error is replaced.
        assert_eq!(
            scrub_key("bad key abcdefgh…", key),
            "The provider's answer quoted the key, so it is not shown."
        );
        // A short key has no eight-char window: exact replacement only.
        assert_eq!(scrub_key("short", "short"), "[key]");
    }
}
