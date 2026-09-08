//! Windows system-tray popover over `usage --json`.
//!
//! View-model helpers compile on every OS so Linux CI can test them. The
//! NotifyIcon + WebView2 event loop is Windows-only and never pulled into
//! the AUR/Linux graph.

pub mod hotkey;
mod icon;
mod payload;

#[cfg(windows)]
mod host;
#[cfg(windows)]
mod startup;
#[cfg(windows)]
mod tui_launch;

pub use icon::{Severity, tray_icon_rgba};
pub use payload::{POLL_INTERVAL, host_payload, worst_severity, wrap_report};

/// Process entry for `ai-usagebar-tray`.
pub fn run() -> i32 {
    #[cfg(windows)]
    {
        host::run()
    }
    #[cfg(not(windows))]
    {
        eprintln!(
            "ai-usagebar-tray is the Windows system-tray popover; it is not used on this OS."
        );
        1
    }
}
