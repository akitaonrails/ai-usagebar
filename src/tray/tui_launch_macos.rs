//! Open `ai-usagebar-tui` in Terminal.app.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

pub fn open() {
    let Some(tui) = resolve_tui() else {
        return;
    };
    let tmp = std::env::temp_dir().join(format!("ai-usagebar-tui-{}.sh", std::process::id()));
    let script = format!(
        "#!/bin/bash\n\"{tui}\"\necho\nread -p 'Enter to close...'\nrm -f '{}'\n",
        tmp.display()
    );
    if fs::write(&tmp, script).is_err() {
        return;
    }
    let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755));
    let osa = format!(
        "tell application \"Terminal\" to do script \"bash '{}'; true\"\ntell application \"Terminal\" to activate",
        tmp.display()
    );
    let _ = Command::new("osascript").args(["-e", &osa]).spawn();
}

fn resolve_tui() -> Option<String> {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join("ai-usagebar-tui");
        if sibling.is_file() {
            return Some(sibling.to_string_lossy().into_owned());
        }
    }
    if let Ok(home) = crate::cache::home_dir() {
        let cargo = home.join(".cargo").join("bin").join("ai-usagebar-tui");
        if cargo.is_file() {
            return Some(cargo.to_string_lossy().into_owned());
        }
    }
    Some("ai-usagebar-tui".into())
}
