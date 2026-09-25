//! System-tray popover over `usage --json`.
//!
//! View-model helpers compile on every OS so Linux CI can test them. The
//! NotifyIcon/NSStatusItem + WebView event loop is Windows/macOS-only and
//! never pulled into the AUR/Linux graph.

mod accent;
mod browse;
pub mod hotkey;
mod icon;
mod marks;
#[cfg(any(target_os = "macos", test))]
mod menu_bar;
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
mod taskbar_theme;
#[cfg(windows)]
mod tui_launch;
#[cfg(target_os = "macos")]
#[path = "tui_launch_macos.rs"]
mod tui_launch;
// Release check, download and verification: `reqwest` and paths, no Windows
// or Cocoa API. Like the rest of this module it compiles everywhere so Linux
// CI runs its tests, though only the tray hosts call it.
mod update_flow;
mod updates;
// Where the Windows popover sits on screen. Pure geometry, compiled everywhere so its tests
// run on every CI job; only the Windows host calls it.
#[cfg_attr(not(windows), allow(dead_code))]
mod placement;
// When the Windows popover closes on a blur or a press outside it. Pure decisions, compiled
// everywhere like `placement`; only the Windows host calls them.
#[cfg_attr(not(windows), allow(dead_code))]
mod blur;
// Where the Windows popover's WebView2 profile lives, and the one-time adoption of the old one.
// Paths and file copies, compiled everywhere like `placement`; only the Windows host calls it.
mod profile;
// Popover style parsing compiles everywhere so Linux CI runs its unit tests;
// the native tray hosts use it on Windows and macOS.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
mod style;

pub use browse::http_url;
pub use icon::{Severity, tray_icon_rgba};
pub use payload::{POLL_INTERVAL, host_payload, worst_severity, wrap_report};
pub use strip::{
    BARS_PIXEL_SIDE, StripContent, StripStyle, bars_rgba, content_from_payload, parse_strip_ipc,
};

/// Wall-clock milliseconds, the unit every host fact and payload stamp uses.
pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

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
