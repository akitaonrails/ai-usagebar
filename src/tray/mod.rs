//! System-tray popover over `usage --json`.
//!
//! View-model helpers compile on every OS so Linux CI can test them. The
//! NotifyIcon/NSStatusItem + WebView event loop is Windows/macOS-only and
//! never pulled into the AUR/Linux graph.

mod browse;
pub mod hotkey;
mod icon;
mod panel;
mod payload;
mod strip;

#[cfg(windows)]
mod host;
#[cfg(target_os = "macos")]
mod host_macos;
#[cfg(windows)]
mod startup;
#[cfg(target_os = "macos")]
#[path = "startup_macos.rs"]
mod startup;
#[cfg(windows)]
mod tui_launch;
#[cfg(target_os = "macos")]
#[path = "tui_launch_macos.rs"]
mod tui_launch;
// Release check, download and verification: `reqwest` and paths, no Windows
// API. It follows this module's rule — compile everywhere so Linux CI runs its
// tests — even though only the Windows host calls it.
mod update_flow;

pub use icon::{Severity, tray_icon_rgba};
pub use payload::{POLL_INTERVAL, host_payload, worst_severity, wrap_report};
pub use strip::{BARS_PIXEL_SIDE, StripContent, StripStyle, bars_rgba, content_from_payload};

/// Process entry for `ai-usagebar-tray`.
pub fn run() -> i32 {
    #[cfg(windows)]
    {
        host::run()
    }
    #[cfg(target_os = "macos")]
    {
        host_macos::run()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        eprintln!(
            "ai-usagebar-tray is the Windows/macOS system-tray popover; it is not used on this OS."
        );
        1
    }
}
