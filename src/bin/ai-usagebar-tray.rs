//! System-tray popover (Windows NotifyIcon / macOS menu bar). On other OSes
//! this binary exists so `cargo build --all-targets` stays uniform, and exits
//! with a short message.

#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    // The macOS menu bar switches accounts by running this same binary as
    // `ai-usagebar-tray account switch …`, so the switch never depends on
    // finding a separately built (or differently versioned) `ai-usagebar`.
    #[cfg(target_os = "macos")]
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "account")
    {
        use ai_usagebar::widget::cli::{Cli, Command};
        use clap::Parser;
        if let Some(Command::Account { action }) = Cli::parse().command {
            std::process::exit(ai_usagebar::account::run(&action));
        }
    }
    std::process::exit(ai_usagebar::tray::run());
}
