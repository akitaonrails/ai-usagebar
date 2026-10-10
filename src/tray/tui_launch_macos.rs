//! Open `ai-usagebar-tui`, or a Claude Desktop account capture, in Terminal.app.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

pub fn open() {
    let Some(tui) = resolve_tui() else {
        return;
    };
    run_in_terminal(&quote(&tui));
}

/// Capture a Claude Desktop account as `label`. It waits for a sign-in in the
/// app, so it runs where the person can see and answer it.
pub fn add_desktop_account(label: &str) {
    let Ok(tray) = std::env::current_exe() else {
        return;
    };
    run_in_terminal(&format!(
        "{} account add {} --desktop",
        quote(&tray.to_string_lossy()),
        quote(label)
    ));
}

/// Single-quoted for bash, a `'` closed, escaped and reopened, so a label or
/// path cannot end the word early.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

fn script(command: &str, path: &Path) -> String {
    format!(
        "#!/bin/bash\n{command}\necho\nread -p 'Enter to close...'\nrm -f {}\n",
        quote(&path.to_string_lossy())
    )
}

fn run_in_terminal(command: &str) {
    static LAUNCHED: AtomicUsize = AtomicUsize::new(0);
    let tmp = std::env::temp_dir().join(format!(
        "ai-usagebar-{}-{}.sh",
        std::process::id(),
        LAUNCHED.fetch_add(1, Ordering::Relaxed)
    ));
    if fs::write(&tmp, script(command, &tmp)).is_err() {
        return;
    }
    let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o700));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quote_in_a_label_stays_inside_its_word() {
        assert_eq!(quote("it's"), r"'it'\''s'");
        let body = script(
            &format!(
                "{} account add {} --desktop",
                quote("/A B/tray"),
                quote("o'neil; rm -rf ~")
            ),
            Path::new("/tmp/x.sh"),
        );
        assert!(body.contains(r"'/A B/tray' account add 'o'\''neil; rm -rf ~' --desktop"));
        assert!(body.ends_with("rm -f '/tmp/x.sh'\n"));
    }
}
