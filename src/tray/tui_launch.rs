//! Open `ai-usagebar-tui` in a real console. The TUI occupies a terminal;
//! spawning it without one just flashes nothing.

use std::process::Command;

pub fn open() {
    let Some(tui) = resolve_tui() else {
        return;
    };
    // `wt -e` is not a real Windows Terminal flag (it belongs to wezterm);
    // wt rejects it, prints its Usage page, and exits — the TUI never starts.
    // The correct syntax is `wt new-tab -- <command>`.
    if Command::new("wt.exe")
        .args(["new-tab", "--", &tui])
        .spawn()
        .is_ok()
    {
        return;
    }
    let _ = Command::new("conhost.exe").arg(&tui).spawn();
}

fn resolve_tui() -> Option<String> {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join("ai-usagebar-tui.exe");
        if sibling.is_file() {
            return Some(sibling.to_string_lossy().into_owned());
        }
    }
    if let Ok(home) = crate::cache::home_dir() {
        let cargo = home.join(".cargo").join("bin").join("ai-usagebar-tui.exe");
        if cargo.is_file() {
            return Some(cargo.to_string_lossy().into_owned());
        }
    }
    Some("ai-usagebar-tui.exe".into())
}
