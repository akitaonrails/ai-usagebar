//! System-tray popover (Windows NotifyIcon / macOS menu bar). On other OSes
//! this binary exists so `cargo build --all-targets` stays uniform, and exits
//! with a short message.

#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    std::process::exit(ai_usagebar::tray::run());
}
