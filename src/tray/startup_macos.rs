//! LaunchAgent for "start at login". Writing the plist is enough: launchd
//! loads `~/Library/LaunchAgents` at the next login. We do not `launchctl
//! load` here — that would spawn a second copy of the already-running tray.

use std::fs;
use std::path::PathBuf;

const LABEL: &str = "com.akitaonrails.ai-usagebar-tray";

pub fn is_enabled() -> bool {
    plist_path().is_some_and(|path| path.is_file())
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    if enabled {
        write_plist()
    } else {
        remove_plist()
    }
}

fn plist_path() -> Option<PathBuf> {
    crate::cache::home_dir().ok().map(|home| {
        home.join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist"))
    })
}

fn exe_path() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|error| error.to_string())
}

fn write_plist() -> Result<(), String> {
    let path = plist_path().ok_or("could not resolve home")?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    }
    let exe = exe_path()?;
    let exe = xml_escape(&exe.to_string_lossy());
    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>ProcessType</key>
    <string>Interactive</string>
</dict>
</plist>
"#
    );
    fs::write(&path, body).map_err(|error| error.to_string())
}

fn remove_plist() -> Result<(), String> {
    let Some(path) = plist_path() else {
        return Ok(());
    };
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
