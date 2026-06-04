//! Waybar JSON output: `{text, tooltip, class}`.
//!
//! Per the project's contract (claudebar's CLAUDE.md), the widget MUST always
//! exit 0. This struct + its serializer never panic on valid input; the
//! callers handle the "always emit something" invariant.

use serde::Serialize;

/// Waybar refresh signal used by the sample module config (`signal: 13`).
pub const REFRESH_SIGNAL: &str = "-RTMIN+13";

/// Process name used for best-effort refreshes after cycling/saving settings.
pub const PROCESS_NAME: &str = "waybar";

/// Best-effort Waybar refresh. Failing is harmless when Waybar is not running.
///
/// Shells out to `pkill -RTMIN+13 waybar` on Unix. Waybar is a Wayland-only
/// program, so off Unix (e.g. Windows) there is no process to signal and no
/// `pkill`; the body is gated out and the call becomes a no-op — consumers
/// there (such as a tray app) refresh on their own polling interval instead.
pub fn request_refresh() {
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("pkill")
            .args([REFRESH_SIGNAL, PROCESS_NAME])
            .status();
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct WaybarOutput {
    pub text: String,
    pub tooltip: String,
    pub class: Class,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Class {
    Low,
    Mid,
    High,
    Critical,
}

impl From<crate::pacing::PaceSeverity> for Class {
    fn from(s: crate::pacing::PaceSeverity) -> Self {
        match s {
            crate::pacing::PaceSeverity::Low => Class::Low,
            crate::pacing::PaceSeverity::Mid => Class::Mid,
            crate::pacing::PaceSeverity::High => Class::High,
            crate::pacing::PaceSeverity::Critical => Class::Critical,
        }
    }
}

impl WaybarOutput {
    /// One-line JSON suitable for Waybar `return-type: "json"`.
    pub fn to_json_line(&self) -> String {
        // serde_json never produces newlines for `to_string`; the trailing
        // `\n` is what Waybar splits on.
        format!("{}\n", serde_json::to_string(self).unwrap_or_default())
    }

    /// Fallback for catastrophic errors — claudebar's `die()` (claudebar:177-185).
    /// Always produces a valid Waybar JSON document.
    pub fn error(msg: &str) -> Self {
        Self {
            text: "⚠".into(),
            tooltip: msg.into(),
            class: Class::Critical,
        }
    }

    /// Neutral "Loading…" widget — claudebar's `loading_network`
    /// (claudebar:190-196). Used when a transient network failure leaves us
    /// with no usable cache.
    pub fn loading(prefix_icon: Option<&str>) -> Self {
        let text = match prefix_icon {
            Some(ic) if !ic.is_empty() => format!("{ic} Loading…"),
            _ => "Loading…".to_string(),
        };
        Self {
            text,
            tooltip: "Usage data is waiting for network.\nWill retry on the next Waybar update."
                .into(),
            class: Class::Low,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pacing::PaceSeverity;

    #[test]
    fn serializes_compact_json() {
        let out = WaybarOutput {
            text: "42% · 1h 30m".into(),
            tooltip: "<b>Claude Max 5x</b>".into(),
            class: Class::Mid,
        };
        let line = out.to_json_line();
        assert!(line.ends_with('\n'));
        assert!(line.contains(r#""class":"mid""#));
        assert!(line.contains(r#""text":"42% · 1h 30m""#));
    }

    #[test]
    fn class_serializes_lowercase() {
        assert_eq!(
            serde_json::to_string(&Class::Critical).unwrap(),
            r#""critical""#
        );
    }

    #[test]
    fn severity_maps_to_class() {
        assert_eq!(Class::from(PaceSeverity::Low), Class::Low);
        assert_eq!(Class::from(PaceSeverity::Mid), Class::Mid);
        assert_eq!(Class::from(PaceSeverity::High), Class::High);
        assert_eq!(Class::from(PaceSeverity::Critical), Class::Critical);
    }

    #[test]
    fn error_helper_always_valid() {
        let line = WaybarOutput::error("Token expired\nRun claude").to_json_line();
        // Should round-trip back to JSON without errors.
        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["text"], "⚠");
        assert_eq!(v["class"], "critical");
        assert!(v["tooltip"].as_str().unwrap().contains("Token expired"));
    }

    #[test]
    fn loading_with_icon_prepends() {
        let l = WaybarOutput::loading(Some("󰚩"));
        assert!(l.text.starts_with("󰚩 "));
        assert!(l.text.contains("Loading"));
    }

    #[test]
    fn loading_without_icon_is_plain() {
        let l = WaybarOutput::loading(None);
        assert_eq!(l.text, "Loading…");
    }
}
