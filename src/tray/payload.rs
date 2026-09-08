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
    /// Seconds between full reports; `[tray] refresh_minutes` × 60.
    pub refresh_secs: u64,
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
}

/// State of a newer release as the popover renders it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateFact {
    /// Human-readable reason when `state` is "failed", or empty.
    pub error: String,
    /// "checking" | "available" | "downloading" | "installing" | "failed".
    pub state: String,
    /// Release page for the human; never opened by the host itself.
    pub url: String,
    /// Bare "X.Y.Z".
    pub version: String,
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
            refresh_secs: POLL_INTERVAL.as_secs(),
            shortcut: String::new(),
            shortcut_error: String::new(),
            startup_enabled: false,
            update: None,
            update_checked_at: 0,
            updates: String::new(),
            version: String::new(),
        }
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
        })
    });
    let mut payload = json!({
        "version": facts.version,
        "generated_at": now_ms,
        "next_refresh_at": now_ms.saturating_add(poll_ms),
        "refresh_minutes": facts.refresh_secs / 60,
        "startup_enabled": facts.startup_enabled,
        "shortcut": facts.shortcut,
        "shortcut_error": sanitize_untrusted_field(&facts.shortcut_error),
        "updates": facts.updates,
        "update": update,
        "update_checked_at": facts.update_checked_at,
        "host_error": host_error.map(sanitize_untrusted_field),
        "primary": Value::Null,
        "entries": [],
    });
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
                    obj.insert("entries".into(), entries.clone());
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
        for section in sections {
            if section.get("type").and_then(Value::as_str) != Some("metric") {
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

#[cfg(test)]
mod tests {
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
        assert_eq!(payload["shortcut"], "");
        assert_eq!(payload["shortcut_error"], "");
        assert_eq!(payload["updates"], "notify");
        assert!(payload["update"].is_null());
        assert_eq!(payload["update_checked_at"], 0);
        assert!(payload["host_error"].is_null());
        assert_eq!(payload["primary"], "anthropic");
        assert_eq!(payload["entries"][0]["short_name"], "cld");
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
            state: "available".into(),
            url: "https://github.com/akitaonrails/ai-usagebar/releases/tag/v1.11.0".into(),
            version: "1.11.0".into(),
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
}
