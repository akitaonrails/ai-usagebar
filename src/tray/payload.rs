//! Wrap `usage --json` for the popover and derive the tray icon severity.
//!
//! Pure JSON in, JSON out — no HWND, no `$HOME`, no clock. The host supplies
//! `now_ms` so tests pin the countdown.

use serde_json::{Value, json};

use super::icon::Severity;
use crate::display::sanitize_untrusted_field;

/// Default poll interval when `[tray] refresh_minutes` is unset. The provider
/// cache TTL stays 60 s; the tray just asks less often. The live value is
/// `HostFacts::refresh_secs`.
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(300);

/// Everything the host knows that is not part of the usage report: its own
/// version, the Run-key state, the registered shortcut and the update
/// machinery. One struct so a new fact does not grow `wrap_report`'s arity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostFacts {
    /// Windows system accent colors for the Fluent popover; absent on other hosts.
    pub accent: Option<super::accent::Accent>,
    /// Active appearance supplied by the Linux tray when running on Omarchy.
    pub omarchy: Option<crate::theme::OmarchyAppearance>,
    /// Seconds between full reports; `[tray] refresh_minutes` × 60.
    pub refresh_secs: u64,
    /// `[tray] width` in logical points; `None` is each style's own width.
    pub popover_width: Option<u16>,
    /// This host's own Classic and Native widths, in that order. The page derives the
    /// width control's limits from the active one.
    pub popover_width_defaults: [u16; 2],
    /// Why the last `set-popover-width` was refused, or empty.
    pub popover_width_error: String,
    /// Canonical "Ctrl+Shift+U" spelling of the registered shortcut, or empty.
    pub shortcut: String,
    /// Why the last `set-shortcut` was refused (already taken, unparsable), or empty.
    pub shortcut_error: String,
    pub startup_enabled: bool,
    /// Latest known release when it is newer than `version`.
    pub update: Option<UpdateFact>,
    /// Wall-clock ms of the last successful or failed release check, 0 = never.
    pub update_checked_at: i64,
    /// "auto" | "notify" | "off".
    pub updates: String,
    pub version: String,
    /// Vendors whose active login the popover can switch. Only the macOS host
    /// fills this; an empty list hides the control everywhere else.
    pub accounts: Vec<AccountSwitchFact>,
    /// Every built-in provider, enabled or not, for Settings → Providers.
    pub providers: Vec<crate::catalog::VendorStatus>,
    /// Why the last enable/disable from the popover failed, or empty.
    pub providers_error: String,
    /// Bumped each time the popover switches a provider. A catalog read that
    /// began under an older generation is dropped rather than published, so
    /// it cannot put back the state the switch replaced.
    pub providers_generation: u64,
    /// The in-flight or finished `set-api-key` check; `None` before the
    /// first send.
    pub key_check: Option<KeyCheckFact>,
    /// The popover's device-code sign-in, from `start-sign-in` until it
    /// ends or is cancelled; `None` when no sign-in sheet is open.
    pub sign_in: Option<SignInFact>,
}

/// One vendor's switchable logins, as the popover renders them beside each
/// account's card.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountSwitchFact {
    /// Report entry slug: "anthropic" or "openai".
    pub vendor: String,
    /// Label the vendor's default login belongs to; `None` when it is not a
    /// managed account.
    pub active: Option<String>,
    /// Labels that can be made active.
    pub labels: Vec<String>,
    /// Label of the last switch requested, running or finished; empty before
    /// the first one.
    pub target: String,
    /// Whether that switch is still running.
    pub switching: bool,
    /// Why that switch failed, or empty.
    pub error: String,
    /// An explicit trusted preparation CLI is configured (Desktop only).
    pub prepare_enabled: bool,
    pub prepare_target: String,
    pub preparing: bool,
    pub prepare_error: String,
    /// Server-confirmed reset, never a local five-hour estimate.
    pub prepared_reset: String,
}

/// State of the popover's API-key check, one fact per `set-api-key` send.
/// The key itself never reaches a fact: only the verdict and its error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyCheckState {
    /// The check's thread is writing the key and fetching the provider.
    Checking,
    /// The provider accepted the key; the config keeps it.
    Ok,
    /// The provider refused it (or the fetch failed); the previous value is
    /// back in the config.
    Failed,
}

/// The popover's API-key check, published by the host when a `set-api-key`
/// request lands. `seq` is the popover's send number: a check that finishes
/// after a newer send has replaced the fact must not write its verdict over
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyCheckFact {
    /// The vendor slug the check ran for; from `KEY_VENDORS`, so trusted.
    pub id: String,
    pub seq: u64,
    pub state: KeyCheckState,
    /// Why the check failed (sanitized on the way out), or empty.
    pub error: String,
}

/// The payload keys a facts-only push refreshes: every fact the popover
/// watches change between two reports. Both hosts copy exactly these from a
/// fresh [`wrap_report`] into the payload they hold, so a new fact is listed
/// once, here, and reaches both.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub const STAMPED_FACTS: &[&str] = &[
    "shortcut",
    "shortcut_error",
    "refresh_minutes",
    "popover_width",
    "popover_width_error",
    "updates",
    "update",
    "update_checked_at",
    "repository",
    "version",
    "accounts",
    "providers",
    "providers_error",
    "key_check",
    "sign_in",
];

/// Where one device-code sign-in stands; see [`super::device_login`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignInState {
    /// Asking the provider (or its CLI) for a one-time code.
    Starting,
    /// The code is on screen; the flow waits for the browser's approval.
    Waiting,
    /// Approved and saved: the provider's next reading uses the new login.
    Ok,
    /// The code expired before anyone approved it.
    Expired,
    /// Device sign-in cannot run here; `reason` says why, and the terminal
    /// is the way left.
    Unavailable,
    /// Anything else that ended the flow; `error` is the diagnostic.
    Failed,
}

impl SignInState {
    pub fn as_str(self) -> &'static str {
        match self {
            SignInState::Starting => "starting",
            SignInState::Waiting => "waiting",
            SignInState::Ok => "ok",
            SignInState::Expired => "expired",
            SignInState::Unavailable => "unavailable",
            SignInState::Failed => "failed",
        }
    }
}

/// The popover's device-code sign-in. `seq` is the popover's send number:
/// a flow that a newer `start-sign-in` or a `cancel-sign-in` replaced finds
/// another `seq` (or none) here, stops its child, and publishes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInFact {
    /// The vendor slug; from the device-login table, so trusted.
    pub id: String,
    pub seq: u64,
    pub state: SignInState,
    /// The one-time code the user types on the provider's page, or empty
    /// until the flow prints one.
    pub code: String,
    /// Wall-clock ms when the code stops working; 0 while unknown.
    pub expires_at: i64,
    /// Why device sign-in cannot run (`Unavailable`) or was refused:
    /// `device-off`, `cli-missing`, `custom-path`, `denied`, or empty.
    pub reason: &'static str,
    /// The flow's own diagnostic for `Failed`, or empty.
    pub error: String,
    /// The page that takes the code. Never serialized: the popover asks
    /// the host to open it by `seq`, so a page address never comes from the
    /// webview, and only an address the flow's allowlist accepted is kept.
    pub url: String,
    /// The approval arrived and the login is being saved. Never serialized:
    /// from here a cancel is too late, so it leaves the fact alone, and the
    /// verdict that follows is the login's.
    pub committing: bool,
}

/// State of a newer release as the popover renders it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateFact {
    /// Human-readable reason when `state` is "failed", or empty.
    pub error: String,
    /// The signed cask bundle hands upgrades to Homebrew.
    pub homebrew: bool,
    /// The release ships this OS/arch and the install directory is writable;
    /// otherwise the popover links the release page instead of installing.
    pub installable: bool,
    /// "checking" | "available" | "downloading" | "installing" | "failed".
    pub state: String,
    /// Release page for the human; never opened by the host itself.
    pub url: String,
    /// Bare "X.Y.Z".
    pub version: String,
}

/// Host facts are owned jointly: the event-loop thread edits the shortcut, the
/// worker edits the update state, and every report snapshots the whole thing.
pub type SharedFacts = std::sync::Arc<std::sync::Mutex<HostFacts>>;

/// Only the hosts read the whole record; the update worker edits in place.
#[cfg(any(
    windows,
    target_os = "macos",
    all(target_os = "linux", feature = "linux-tray")
))]
pub fn facts_snapshot(facts: &SharedFacts) -> HostFacts {
    facts.lock().map(|f| f.clone()).unwrap_or_default()
}

pub fn with_facts(facts: &SharedFacts, edit: impl FnOnce(&mut HostFacts)) {
    if let Ok(mut guard) = facts.lock() {
        edit(&mut guard);
    }
}

impl HostFacts {
    pub fn new(version: &str, startup_enabled: bool) -> Self {
        Self {
            startup_enabled,
            updates: "notify".into(),
            version: version.into(),
            ..Self::default()
        }
    }
}

impl Default for HostFacts {
    fn default() -> Self {
        Self {
            accent: None,
            omarchy: None,
            refresh_secs: POLL_INTERVAL.as_secs(),
            popover_width: None,
            // Each host names its own in `host_facts`; zeros make the page fall back.
            popover_width_defaults: [0, 0],
            popover_width_error: String::new(),
            shortcut: String::new(),
            shortcut_error: String::new(),
            startup_enabled: false,
            update: None,
            update_checked_at: 0,
            updates: String::new(),
            version: String::new(),
            accounts: Vec::new(),
            providers: Vec::new(),
            providers_error: String::new(),
            providers_generation: 0,
            key_check: None,
            sign_in: None,
        }
    }
}

/// GitHub page this binary was built from (`Cargo.toml` `repository`), or
/// empty when that field is not a GitHub URL. The About screen opens it.
fn repository_page() -> String {
    let raw = crate::update::SOURCE_REPOSITORY
        .trim()
        .trim_end_matches('/');
    let raw = raw.strip_suffix(".git").unwrap_or(raw);
    if raw.starts_with("https://github.com/") {
        raw.to_string()
    } else {
        String::new()
    }
}

fn host_os() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        "linux"
    }
}

/// Build the object the WebView's `apply` function consumes.
pub fn wrap_report(
    report_json: &str,
    facts: &HostFacts,
    now_ms: i64,
    host_error: Option<&str>,
) -> Value {
    let poll_ms = i64::try_from(facts.refresh_secs)
        .unwrap_or(i64::MAX / 1_000)
        .saturating_mul(1_000);
    let update = facts.update.as_ref().map(|u| {
        json!({
            "version": sanitize_untrusted_field(&u.version),
            "url": sanitize_untrusted_field(&u.url),
            "state": u.state,
            "error": sanitize_untrusted_field(&u.error),
            "homebrew": u.homebrew,
            "installable": u.installable,
        })
    });
    let accounts: serde_json::Map<String, Value> = facts
        .accounts
        .iter()
        .map(|fact| {
            (
                fact.vendor.clone(),
                json!({
                    "active": fact.active.as_deref().map(sanitize_untrusted_field),
                    "labels": fact
                        .labels
                        .iter()
                        .map(|label| sanitize_untrusted_field(label))
                        .collect::<Vec<_>>(),
                    "target": sanitize_untrusted_field(&fact.target),
                    "switching": fact.switching,
                    "error": sanitize_untrusted_field(&fact.error),
                    "prepare_enabled": fact.prepare_enabled,
                    "prepare_target": sanitize_untrusted_field(&fact.prepare_target),
                    "preparing": fact.preparing,
                    "prepare_error": sanitize_untrusted_field(&fact.prepare_error),
                    "prepared_reset": fact.prepared_reset,
                }),
            )
        })
        .collect();
    let accent = facts.accent.as_ref().map(|accent| {
        json!({
            "light": accent.light,
            "dark": accent.dark,
        })
    });
    let mut payload = json!({
        "version": facts.version,
        "generated_at": now_ms,
        "next_refresh_at": now_ms.saturating_add(poll_ms),
        "refresh_minutes": facts.refresh_secs / 60,
        "popover_width": facts.popover_width,
        "popover_width_defaults": facts.popover_width_defaults,
        "popover_width_error": sanitize_untrusted_field(&facts.popover_width_error),
        "startup_enabled": facts.startup_enabled,
        "os": host_os(),
        "shortcut": facts.shortcut,
        "shortcut_error": sanitize_untrusted_field(&facts.shortcut_error),
        "updates": facts.updates,
        "update": update,
        "update_checked_at": facts.update_checked_at,
        "repository": repository_page(),
        "accounts": accounts,
        "providers": super::providers::rows_json(&facts.providers, host_os()),
        "providers_error": sanitize_untrusted_field(&facts.providers_error),
        "key_check": facts.key_check.as_ref().map(|fact| {
            let state = match fact.state {
                KeyCheckState::Checking => "checking",
                KeyCheckState::Ok => "ok",
                KeyCheckState::Failed => "failed",
            };
            json!({
                "id": fact.id,
                "seq": fact.seq,
                "state": state,
                "error": sanitize_untrusted_field(&fact.error),
            })
        }),
        "sign_in": facts.sign_in.as_ref().map(|fact| {
            json!({
                "id": fact.id,
                "seq": fact.seq,
                "state": fact.state.as_str(),
                "code": sanitize_untrusted_field(&fact.code),
                "expires_at": fact.expires_at,
                "reason": fact.reason,
                "error": sanitize_untrusted_field(&fact.error),
            })
        }),
        "accent": accent,
        "host_error": host_error.map(sanitize_untrusted_field),
        "primary": Value::Null,
        "entries": [],
    });
    stamp_appearance(&mut payload, facts);
    if host_error.is_some() {
        return payload;
    }
    match serde_json::from_str::<Value>(report_json) {
        Ok(Value::Object(map)) => {
            if let Some(obj) = payload.as_object_mut() {
                if let Some(primary) = map.get("primary") {
                    obj.insert("primary".into(), primary.clone());
                }
                if let Some(entries) = map.get("entries") {
                    obj.insert("entries".into(), with_sign_in_hints(entries));
                }
            }
            payload
        }
        _ => {
            payload["host_error"] = json!(sanitize_untrusted_field(
                "The usage report did not contain valid JSON."
            ));
            payload
        }
    }
}

/// Update appearance independently of the report and its refresh timestamp.
pub(super) fn stamp_appearance(payload: &mut Value, facts: &HostFacts) {
    payload["desktop"] = json!(if facts.omarchy.is_some() {
        "omarchy"
    } else {
        ""
    });
    payload["omarchy"] = json!(facts.omarchy);
}

/// Attach each entry's sign-in sentence from [`VendorId::sign_in_hint`].
///
/// The popover needs a "how do I fix this?" line on an error card. It must not
/// keep its own table for that: a JS object literal silently omits a provider
/// nobody remembered, while the Rust match cannot compile without one. The
/// entry id is `vendor` or `vendor@account`, so the slug is the part before
/// `@`; an id that matches no vendor is left without a hint rather than guessed.
fn with_sign_in_hints(entries: &Value) -> Value {
    let Some(list) = entries.as_array() else {
        return entries.clone();
    };
    Value::Array(
        list.iter()
            .map(|entry| {
                let mut entry = entry.clone();
                let slug = entry
                    .get("id")
                    .and_then(Value::as_str)
                    .map(|id| id.split('@').next().unwrap_or(id).to_ascii_lowercase());
                let hint = slug.and_then(|slug| {
                    crate::vendor::VendorId::all()
                        .iter()
                        .find(|v| v.slug() == slug)
                        .map(|v| v.sign_in_hint())
                });
                if let (Some(hint), Some(obj)) = (hint, entry.as_object_mut()) {
                    obj.insert("sign_in".into(), json!(hint));
                }
                entry
            })
            .collect(),
    )
}

pub fn host_payload(value: &Value) -> String {
    value.to_string()
}

/// Worst severity across every metric, treating fetch/host errors as critical.
pub fn worst_severity(payload: &Value) -> Severity {
    if payload
        .get("host_error")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty())
    {
        return Severity::Critical;
    }
    let mut worst = Severity::Low;
    let Some(entries) = payload.get("entries").and_then(Value::as_array) else {
        return worst;
    };
    for entry in entries {
        if entry.get("status").and_then(Value::as_str) == Some("error")
            || entry
                .get("error")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty())
        {
            return Severity::Critical;
        }
        let Some(sections) = entry.get("sections").and_then(Value::as_array) else {
            continue;
        };
        let metrics: Vec<&Value> = sections
            .iter()
            .filter(|section| section.get("type").and_then(Value::as_str) == Some("metric"))
            .collect();
        // A grouped row (the Claude entry's context sessions, SuperGrok's
        // product slices) sits under its own heading below the meters and is
        // not a quota window: it counts only when the entry has nothing else,
        // the partition `strip::quota_group` already applies.
        let has_window = metrics.iter().any(|section| !is_grouped(section));
        for section in metrics {
            if has_window && is_grouped(section) {
                continue;
            }
            if let Some(sev) = section
                .get("severity")
                .and_then(Value::as_str)
                .and_then(Severity::from_report_str)
                && sev.rank() > worst.rank()
            {
                worst = sev;
            }
        }
    }
    worst
}

/// Whether a metric sits under a group heading, which the report marks with a
/// non-empty `group`.
fn is_grouped(section: &Value) -> bool {
    section
        .get("group")
        .and_then(Value::as_str)
        .is_some_and(|group| !group.is_empty())
}

#[cfg(test)]
mod tests {

    /// The popover renders "how do I fix this?" on an error card. That sentence
    /// must come from the host: the frontend kept its own table first, and it
    /// disagreed with `VendorId` for five of eight providers before shipping.
    #[test]
    fn every_entry_carries_its_sign_in_hint_from_the_vendor() {
        let report = r#"{"primary":"anthropic","entries":[
            {"id":"anthropic","status":"error","error":"not signed in"},
            {"id":"openai@work","status":"error","error":"not signed in"},
            {"id":"cursor","status":"ready"},
            {"id":"custom:mytool","status":"ready"}
        ]}"#;
        let payload = wrap_report(report, &facts("1.0.0", false), 0, None);
        let entries = payload["entries"].as_array().expect("entries");

        assert_eq!(
            entries[0]["sign_in"],
            json!(crate::vendor::VendorId::Anthropic.sign_in_hint())
        );
        // "vendor@account" resolves on the vendor half.
        assert_eq!(
            entries[1]["sign_in"],
            json!(crate::vendor::VendorId::Openai.sign_in_hint())
        );
        // A vendor that has no CLI login still says how to sign in.
        assert_eq!(
            entries[2]["sign_in"],
            json!(crate::vendor::VendorId::Cursor.sign_in_hint())
        );
        // An id that is not a built-in vendor gets no invented hint.
        assert!(entries[3].get("sign_in").is_none(), "{:?}", entries[3]);
    }

    #[test]
    fn popover_keeps_all_distinct_antigravity_account_cards() {
        let entries = (0..5)
            .map(|index| {
                json!({
                    "id": format!("antigravity@{index:012x}"),
                    "name": "antigravity · a***@example.com",
                    "display_name": "Antigravity · a***@example.com",
                    "status": "ready",
                    "sections": [{
                        "type": "metric",
                        "label": format!("Weekly {index}"),
                        "percent": index * 10,
                        "value": format!("{}%", index * 10),
                        "detail": format!("Resets in {index}d"),
                        "severity": "low",
                        "reset_at": null
                    }]
                })
            })
            .collect::<Vec<_>>();
        let report = json!({ "primary": "antigravity", "entries": entries }).to_string();
        let payload = wrap_report(&report, &facts("1.0.0", false), 0, None);
        let cards = payload["entries"].as_array().unwrap();
        let ids = cards
            .iter()
            .map(|card| card["id"].as_str().unwrap())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(cards.len(), 5);
        assert_eq!(ids.len(), 5);
        assert!(
            cards
                .iter()
                .all(|card| card["display_name"] == "Antigravity · a***@example.com")
        );
        assert!(
            cards
                .iter()
                .all(|card| card["sign_in"] == crate::vendor::VendorId::Antigravity.sign_in_hint())
        );
        assert_eq!(cards[4]["sections"][0]["detail"], "Resets in 4d");
    }

    /// Every id `usage --json` can emit must resolve, or the popover shows a
    /// generic line for a provider we do know how to sign in.
    #[test]
    fn every_vendor_slug_resolves_to_a_hint() {
        for vendor in crate::vendor::VendorId::all() {
            let report = format!(
                r#"{{"entries":[{{"id":"{}","status":"error","error":"x"}}]}}"#,
                vendor.slug()
            );
            let payload = wrap_report(&report, &facts("1.0.0", false), 0, None);
            let hint = payload["entries"][0]["sign_in"].as_str().unwrap_or("");
            assert!(!hint.is_empty(), "{} has no sign-in hint", vendor.slug());
        }
    }
    use super::*;

    fn sample_report() -> String {
        json!({
            "primary": "anthropic",
            "entries": [
                {
                    "id": "anthropic",
                    "name": "anthropic",
                    "display_name": "Claude",
                    "short_name": "cld",
                    "plan": "Team 5x",
                    "status": "ready",
                    "error": null,
                    "stale": false,
                    "sections": [
                        {
                            "type": "metric",
                            "label": "Weekly",
                            "percent": 19,
                            "value": "19%",
                            "detail": "Resets in 1d 16h",
                            "severity": "low",
                            "reset_at": null
                        },
                        {
                            "type": "metric",
                            "label": "Session",
                            "percent": 0,
                            "value": "0%",
                            "detail": "",
                            "severity": "low",
                            "reset_at": null
                        }
                    ]
                },
                {
                    "id": "openai",
                    "short_name": "gpt",
                    "status": "ready",
                    "error": null,
                    "sections": [{
                        "type": "metric",
                        "label": "Session",
                        "percent": 91,
                        "value": "91%",
                        "detail": "",
                        "severity": "critical",
                        "reset_at": null
                    }]
                }
            ]
        })
        .to_string()
    }

    fn facts(version: &str, startup_enabled: bool) -> HostFacts {
        HostFacts::new(version, startup_enabled)
    }

    #[test]
    fn wrap_copies_entries_and_stamps_refresh() {
        let payload = wrap_report(&sample_report(), &facts("1.10.0", true), 1_000, None);
        assert_eq!(payload["version"], "1.10.0");
        assert_eq!(payload["generated_at"], 1_000);
        assert_eq!(payload["next_refresh_at"], 301_000);
        assert_eq!(payload["refresh_minutes"], 5);
        assert_eq!(payload["startup_enabled"], true);
        assert_eq!(payload["accent"], Value::Null);
        let os = payload["os"].as_str().unwrap_or("");
        assert!(
            os == "macos" || os == "windows" || os == "linux",
            "unexpected os {os}"
        );
        assert_eq!(payload["shortcut"], "");
        assert_eq!(payload["shortcut_error"], "");
        assert_eq!(payload["updates"], "notify");
        assert!(payload["update"].is_null());
        assert_eq!(payload["update_checked_at"], 0);
        assert!(
            payload["repository"]
                .as_str()
                .unwrap_or("")
                .starts_with("https://github.com/")
        );
        assert!(payload["host_error"].is_null());
        assert_eq!(payload["primary"], "anthropic");
        assert_eq!(payload["entries"][0]["short_name"], "cld");
    }

    #[test]
    fn wrap_carries_fluent_accent_colors() {
        let mut host = facts("1.10.0", false);
        host.accent = Some(super::super::accent::Accent {
            light: "#123456".into(),
            dark: "#abcdef".into(),
        });
        let payload = wrap_report(&sample_report(), &host, 0, None);

        // ASSERT: both validated host colors reach the JSON root unchanged.
        assert_eq!(payload["accent"]["light"], "#123456");
        assert_eq!(payload["accent"]["dark"], "#abcdef");
    }

    #[test]
    fn wrap_carries_only_host_omarchy_appearance_and_can_replace_a_stale_report() {
        let mut host = facts("1.10.0", false);
        let report = r#"{"desktop":"omarchy","omarchy":{"mode":"light"}}"#;
        let mut payload = wrap_report(report, &host, 0, None);
        assert_eq!(payload["desktop"], "");
        assert_eq!(payload["omarchy"], Value::Null);
        assert!(!STAMPED_FACTS.contains(&"desktop"));
        assert!(!STAMPED_FACTS.contains(&"omarchy"));

        host.omarchy = Some(crate::theme::OmarchyAppearance {
            mode: "light",
            rounding: 6,
            colors: [("background", "#abcdef".into())].into(),
        });
        let expected = json!({
            "mode": "light",
            "rounding": 6,
            "colors": { "background": "#abcdef" },
        });
        let current = wrap_report(report, &host, 0, Some("offline"));
        assert_eq!(current["desktop"], "omarchy");
        assert_eq!(current["omarchy"], expected);

        stamp_appearance(&mut payload, &host);
        assert_eq!(payload["desktop"], "omarchy");
        assert_eq!(payload["omarchy"], expected);
        assert_eq!(payload["generated_at"], 0);
        stamp_appearance(&mut payload, &HostFacts::default());
        assert_eq!(payload["desktop"], "");
        assert_eq!(payload["omarchy"], Value::Null);
    }

    #[test]
    fn wrap_carries_shortcut_and_update_facts_sanitized() {
        let mut host = facts("1.10.0", false);
        host.shortcut = "Ctrl+Shift+U".into();
        host.shortcut_error = "already taken\u{1b}[31m".into();
        host.updates = "auto".into();
        host.update_checked_at = 42;
        host.update = Some(UpdateFact {
            error: String::new(),
            installable: true,
            state: "available".into(),
            url: "https://github.com/akitaonrails/ai-usagebar/releases/tag/v1.11.0".into(),
            version: "1.11.0".into(),
            ..UpdateFact::default()
        });
        let payload = wrap_report(&sample_report(), &host, 0, None);
        assert_eq!(payload["shortcut"], "Ctrl+Shift+U");
        assert!(
            !payload["shortcut_error"]
                .as_str()
                .unwrap()
                .contains('\u{1b}')
        );
        assert_eq!(payload["updates"], "auto");
        assert_eq!(payload["update_checked_at"], 42);
        assert_eq!(payload["update"]["version"], "1.11.0");
        assert_eq!(payload["update"]["state"], "available");
        assert_eq!(payload["update"]["error"], "");
        assert_eq!(payload["update"]["installable"], true);
    }

    #[test]
    fn host_error_wins_over_report_body() {
        let payload = wrap_report(
            "not-json",
            &facts("1.0.0", false),
            0,
            Some("no vendors enabled"),
        );
        assert_eq!(payload["host_error"], "no vendors enabled");
        assert_eq!(payload["entries"].as_array().unwrap().len(), 0);
        assert_eq!(worst_severity(&payload), Severity::Critical);
    }

    #[test]
    fn invalid_report_json_becomes_a_host_error() {
        let payload = wrap_report("{", &facts("1.0.0", false), 0, None);
        assert!(
            payload["host_error"]
                .as_str()
                .unwrap()
                .contains("valid JSON")
        );
        assert_eq!(worst_severity(&payload), Severity::Critical);
    }

    #[test]
    fn worst_severity_is_the_hottest_metric_not_the_primary() {
        let payload = wrap_report(&sample_report(), &facts("1.10.0", false), 0, None);
        assert_eq!(worst_severity(&payload), Severity::Critical);
    }

    /// A context session row (`group: "Sessions"`) is a breakdown under its
    /// own heading, not a quota window: a session at 90% of its context must
    /// not paint the tray icon critical while the quota sits at 29%.
    #[test]
    fn worst_severity_ignores_grouped_rows_behind_a_window() {
        let report = json!({
            "primary": "anthropic",
            "entries": [{
                "id": "anthropic",
                "short_name": "cld",
                "status": "ready",
                "error": null,
                "sections": [
                    {
                        "type": "metric",
                        "label": "Session (5h)",
                        "percent": 29,
                        "value": "29%",
                        "detail": "",
                        "severity": "low",
                        "reset_at": null
                    },
                    {
                        "type": "metric",
                        "label": "ship the release",
                        "group": "Sessions",
                        "percent": 90,
                        "value": "90%",
                        "detail": "",
                        "severity": "critical",
                        "reset_at": null
                    }
                ]
            }]
        })
        .to_string();
        let payload = wrap_report(&report, &facts("1.10.0", false), 0, None);
        assert_eq!(worst_severity(&payload), Severity::Low);
    }

    /// With no ungrouped metric the grouped rows still stand in, as they do
    /// for the menu bar's quota group.
    #[test]
    fn worst_severity_falls_back_to_grouped_rows_without_a_window() {
        let report = json!({
            "primary": "anthropic",
            "entries": [{
                "id": "anthropic",
                "short_name": "cld",
                "status": "ready",
                "error": null,
                "sections": [{
                    "type": "metric",
                    "label": "ship the release",
                    "group": "Sessions",
                    "percent": 90,
                    "value": "90%",
                    "detail": "",
                    "severity": "critical",
                    "reset_at": null
                }]
            }]
        })
        .to_string();
        let payload = wrap_report(&report, &facts("1.10.0", false), 0, None);
        assert_eq!(worst_severity(&payload), Severity::Critical);
    }

    #[test]
    fn entry_error_is_critical_and_lands_in_the_tooltip() {
        let report = json!({
            "primary": "openai",
            "entries": [{
                "id": "openai",
                "short_name": "gpt",
                "status": "error",
                "error": "not signed in",
                "sections": []
            }]
        })
        .to_string();
        let payload = wrap_report(&report, &facts("1.10.0", false), 0, None);
        assert_eq!(worst_severity(&payload), Severity::Critical);
    }

    #[test]
    fn empty_report_uses_a_generic_tooltip() {
        let payload = wrap_report("{}", &facts("1.0.0", false), 0, None);
        assert_eq!(worst_severity(&payload), Severity::Low);
        assert_eq!(POLL_INTERVAL.as_secs(), 300);
    }

    #[test]
    fn next_refresh_follows_the_configured_interval() {
        let mut host = facts("1.10.0", false);
        host.refresh_secs = 60;
        let payload = wrap_report(&sample_report(), &host, 1_000, None);
        assert_eq!(payload["next_refresh_at"], 61_000);
        assert_eq!(payload["refresh_minutes"], 1);

        host.refresh_secs = 600;
        let payload = wrap_report(&sample_report(), &host, 1_000, None);
        assert_eq!(payload["next_refresh_at"], 601_000);
        assert_eq!(payload["refresh_minutes"], 10);
    }

    #[test]
    fn switchable_accounts_are_keyed_by_vendor() {
        let mut host = facts("1.10.0", false);
        host.accounts = vec![AccountSwitchFact {
            vendor: "openai".into(),
            active: Some("main".into()),
            labels: vec!["main".into(), "work\u{1b}[31m".into()],
            target: "work".into(),
            switching: false,
            error: "no stored Codex login".into(),
            ..AccountSwitchFact::default()
        }];
        let payload = wrap_report(&sample_report(), &host, 0, None);
        let openai = &payload["accounts"]["openai"];
        assert_eq!(openai["active"], "main");
        assert_eq!(openai["labels"][0], "main");
        assert!(!openai["labels"][1].as_str().unwrap().contains('\u{1b}'));
        assert_eq!(openai["target"], "work");
        assert_eq!(openai["switching"], false);
        assert_eq!(openai["error"], "no stored Codex login");
        assert!(payload["accounts"].get("anthropic").is_none());
    }

    #[test]
    fn desktop_prepare_facts_are_explicit_and_sanitized() {
        let mut host = facts("1.34.0", false);
        host.accounts = vec![AccountSwitchFact {
            vendor: "claude-desktop".into(),
            active: Some("one".into()),
            labels: vec!["one".into(), "two".into()],
            prepare_enabled: true,
            prepare_target: "two".into(),
            preparing: true,
            prepare_error: "offline\u{1b}[31m".into(),
            prepared_reset: "2026-10-10T19:00:00Z".into(),
            ..AccountSwitchFact::default()
        }];
        let payload = wrap_report(&sample_report(), &host, 0, None);
        let desktop = &payload["accounts"]["claude-desktop"];
        assert_eq!(desktop["prepare_enabled"], true);
        assert_eq!(desktop["prepare_target"], "two");
        assert_eq!(desktop["preparing"], true);
        assert!(
            !desktop["prepare_error"]
                .as_str()
                .unwrap()
                .contains('\u{1b}')
        );
        assert_eq!(desktop["prepared_reset"], "2026-10-10T19:00:00Z");
    }

    #[test]
    fn no_switchable_accounts_is_an_empty_object() {
        let payload = wrap_report(&sample_report(), &facts("1.10.0", false), 0, None);
        assert_eq!(payload["accounts"], json!({}));
    }

    #[test]
    fn key_check_fact_serializes_its_state_and_sanitizes_the_error() {
        let mut host = facts("1.44.0", false);
        host.key_check = Some(KeyCheckFact {
            id: "zai".into(),
            seq: 7,
            state: KeyCheckState::Checking,
            error: String::new(),
        });
        let payload = wrap_report(&sample_report(), &host, 0, None);
        assert_eq!(payload["key_check"]["id"], "zai");
        assert_eq!(payload["key_check"]["seq"], 7);
        assert_eq!(payload["key_check"]["state"], "checking");
        assert_eq!(payload["key_check"]["error"], "");

        host.key_check = Some(KeyCheckFact {
            id: "zai".into(),
            seq: 8,
            state: KeyCheckState::Ok,
            error: String::new(),
        });
        let payload = wrap_report(&sample_report(), &host, 0, None);
        assert_eq!(payload["key_check"]["state"], "ok");
        assert_eq!(payload["key_check"]["seq"], 8);

        host.key_check = Some(KeyCheckFact {
            id: "openrouter".into(),
            seq: 9,
            state: KeyCheckState::Failed,
            error: "HTTP 401\u{1b}[31m invalid key".into(),
        });
        let payload = wrap_report(&sample_report(), &host, 0, None);
        assert_eq!(payload["key_check"]["state"], "failed");
        // The error is untrusted text: it leaves through the same sanitizer
        // as `providers_error`.
        let error = payload["key_check"]["error"].as_str().unwrap();
        assert!(!error.contains('\u{1b}'), "{error:?}");
        assert!(error.contains("HTTP 401"), "{error:?}");
    }

    #[test]
    fn a_facts_only_push_carries_both_popover_dialogs() {
        // The key dialog and the sign-in sheet wait on facts that change
        // between reports; a push that left either out showed the popover
        // `null` until the next full report.
        for key in ["key_check", "sign_in", "providers"] {
            assert!(STAMPED_FACTS.contains(&key), "{key}");
        }
        let payload = wrap_report("{}", &HostFacts::default(), 0, None);
        for key in STAMPED_FACTS {
            assert!(payload.get(*key).is_some(), "{key} is not in the payload");
        }
    }

    #[test]
    fn the_popover_width_travels_in_reports_and_in_facts_only_pushes() {
        let mut host = facts("1.52.3", false);
        let payload = wrap_report(&sample_report(), &host, 0, None);
        assert!(payload["popover_width"].is_null());
        assert_eq!(payload["popover_width_error"], "");

        host.popover_width = Some(337);
        host.popover_width_defaults = [300, 390];
        host.popover_width_error = "Could not save\u{1b}[31m".into();
        let payload = wrap_report(&sample_report(), &host, 0, None);
        assert_eq!(payload["popover_width"], 337);
        assert_eq!(payload["popover_width_defaults"], json!([300, 390]));
        assert_eq!(payload["popover_width_error"], "Could not save[31m");
        // The control waits for the host's answer to each change, which arrives between
        // reports: without these the number and a refusal would lag by a whole refresh.
        for key in ["popover_width", "popover_width_error"] {
            assert!(STAMPED_FACTS.contains(&key), "{key}");
        }
    }

    #[test]
    fn key_check_is_null_before_the_first_send() {
        let payload = wrap_report(&sample_report(), &facts("1.44.0", false), 0, None);
        assert!(payload["key_check"].is_null());
    }
}
