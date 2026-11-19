//! System-tray popover over `usage --json`.
//!
//! View-model helpers compile on every OS so Linux CI can test them. The
//! NotifyIcon/NSStatusItem + WebView event loop ships on Windows/macOS; on
//! Linux it is behind the opt-in `linux-tray` feature and never pulled into
//! the AUR/Nix build graph.

mod accent;
mod browse;
mod device_login;
pub mod hotkey;
mod icon;
// Which origin the tray's IPC bridge and the WebView accept. Pure decisions, compiled
// everywhere like `blur` so the tests run on every OS; only the macOS and Windows hosts
// apply them (the guard + navigation handlers of their WebView builders).
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
mod ipc_origin;
mod marks;
#[cfg(any(target_os = "macos", test))]
mod menu_bar;
mod panel;
mod payload;
mod providers;
mod strip;

#[cfg(windows)]
mod host;
#[cfg(all(target_os = "linux", feature = "linux-tray"))]
mod host_linux;
#[cfg(target_os = "macos")]
mod host_macos;
#[cfg(windows)]
mod startup;
#[cfg(target_os = "macos")]
#[path = "startup_macos.rs"]
mod startup;
// Compiled under `test` on Linux too, so the hermetic tests run in CI even
// when the feature is off.
#[cfg(all(target_os = "linux", any(test, feature = "linux-tray")))]
#[path = "startup_linux.rs"]
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
// Scoop handoff and PowerShell command generation compile everywhere so Linux CI runs their
// pure tests, though only the Windows tray host calls them.
mod scoop;
mod updates;
// Where the Windows popover sits on screen. Pure geometry, compiled everywhere so its tests
// run on every CI job; only the Windows host calls it.
#[cfg_attr(not(windows), allow(dead_code))]
mod placement;
// When the popover closes on a blur or a press outside it. Pure decisions, compiled everywhere
// like `placement`; the Windows host calls most of them, the macOS host only `macos_verdict`.
#[cfg_attr(not(windows), allow(dead_code))]
mod blur;
// The `copy-image` IPC's validation: the page posts the card's PNG as base64 and the host
// puts it on the system clipboard. Pure decisions over the received JSON, compiled everywhere
// like `blur` so its tests run on every CI job; the tray hosts call it.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
mod clipboard_image;
// Where the Windows popover's WebView2 profile lives, and the one-time adoption of the old one.
// Paths and file copies, compiled everywhere like `placement`; only the Windows host calls it.
mod profile;
// Popover style parsing compiles everywhere so Linux CI runs its unit tests;
// the native tray hosts use it on Windows and macOS.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
mod style;
// The tray icon's right-click menu: the popover's Options entries, order and labels.
// Pure model compiled everywhere so Linux CI runs its tests; the Windows and macOS hosts use it.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
mod options_menu;

pub use browse::http_url;
pub use icon::{Severity, tray_icon_rgba};
pub use payload::{POLL_INTERVAL, host_payload, worst_severity, wrap_report};
pub use strip::{
    BARS_PIXEL_SIDE, StripContent, StripStyle, bars_rgba, content_from_payload, parse_strip_ipc,
};

/// Set on a process relaunched after an update so it waits for the old tray to exit.
pub(crate) const RELAUNCH_ENV: &str = "AIUB_TRAY_RELAUNCH";

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
    #[cfg(all(target_os = "linux", feature = "linux-tray"))]
    {
        host_linux::run()
    }
    #[cfg(not(any(
        windows,
        target_os = "macos",
        all(target_os = "linux", feature = "linux-tray")
    )))]
    {
        eprintln!(
            "ai-usagebar-tray: this build has no system-tray host for this OS. On Linux, build it with --features linux-tray."
        );
        1
    }
}
