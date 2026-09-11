//! Child-process conventions shared by every vendor that shells out.
//!
//! `ai-usagebar-tray` is a `windows_subsystem = "windows"` process: a console
//! child spawned from it gets a brand-new console window, which takes the
//! foreground and blurs the popover (which then hides). Every helper that runs
//! a CLI for its data must therefore pass [`CREATE_NO_WINDOW`] on Windows.

/// `CREATE_NO_WINDOW` from `PROCESS_CREATION_FLAGS`. Spelled out rather than
/// imported so the vendor modules stay free of the `windows-sys` feature list.
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(test)]
mod tests {
    /// Every non-interactive `Command::new` a vendor runs while collecting a
    /// report must carry the flag; interactive launches (TUI, `claude` login,
    /// opening a browser) are exempt by name.
    #[test]
    fn report_time_child_processes_never_open_a_console_window() {
        for file in ["src/supergrok/acp.rs", "src/copilot/credentials.rs"] {
            let text = std::fs::read_to_string(file).unwrap();
            assert!(
                text.contains("creation_flags(crate::process::CREATE_NO_WINDOW)"),
                "{file} spawns a child without CREATE_NO_WINDOW"
            );
        }
    }
}
