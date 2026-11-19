//! "Start at login" for the opt-in Linux host: an XDG autostart `.desktop`
//! entry. The next slice's host calls these through the `startup` alias in
//! `mod.rs`; the seams taking paths keep the tests off the real home/env.

// The tests compile this module without the feature; the host does not call
// it yet, so neither build should warn about the still-unused public API.
#![allow(dead_code)]

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

const FILE_NAME: &str = "ai-usagebar-tray.desktop";

pub fn is_enabled() -> bool {
    autostart_dir().is_ok_and(|dir| is_enabled_at(&dir))
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    let dir = autostart_dir()?;
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    set_enabled_at(&dir, &exe, enabled)
}

fn autostart_dir() -> Result<PathBuf, String> {
    let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
    let home = crate::cache::home_dir().map_err(|error| error.to_string())?;
    Ok(autostart_dir_with(xdg.as_deref(), &home))
}

/// `$XDG_CONFIG_HOME/autostart`, falling back to `~/.config/autostart` when
/// the variable is absent, empty or not an absolute path (the spec ignores a
/// relative one).
pub(crate) fn autostart_dir_with(xdg_config_home: Option<&Path>, home: &Path) -> PathBuf {
    match xdg_config_home.filter(|dir| dir.is_absolute()) {
        Some(config_home) => config_home.join("autostart"),
        None => home.join(".config").join("autostart"),
    }
}

pub(crate) fn is_enabled_at(dir: &Path) -> bool {
    dir.join(FILE_NAME).is_file()
}

pub(crate) fn set_enabled_at(dir: &Path, exe: &Path, enabled: bool) -> Result<(), String> {
    let path = dir.join(FILE_NAME);
    if !enabled {
        return match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        };
    }

    fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    // Same directory as the destination: the rename below stays atomic and
    // never crosses a filesystem boundary. `NamedTempFile` removes its file
    // on any error, including a failed persist.
    let mut temp = NamedTempFile::new_in(dir).map_err(|error| error.to_string())?;
    temp.write_all(desktop_entry(exe).as_bytes())
        .map_err(|error| error.to_string())?;
    temp.persist(&path).map_err(|error| error.to_string())?;
    Ok(())
}

/// The Desktop Entry Spec quotes `Exec=` values in double quotes, inside
/// which backslash, double quote, dollar and backtick need a backslash.
fn escape_exec(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' | '"' | '$' | '`' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn desktop_entry(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=AI Usage Bar\nExec=\"{}\"\nIcon=ai-usagebar\nX-GNOME-Autostart-enabled=true\nNoDisplay=true\n",
        escape_exec(&exe.to_string_lossy())
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_xdg_config_home_wins() {
        let home = Path::new("/home/test-user");
        assert_eq!(
            autostart_dir_with(Some(Path::new("/xdg/config")), home),
            Path::new("/xdg/config/autostart")
        );
    }

    #[test]
    fn empty_relative_or_missing_xdg_config_home_falls_back_to_home() {
        let home = Path::new("/home/test-user");
        let expected = Path::new("/home/test-user/.config/autostart");
        assert_eq!(autostart_dir_with(None, home), expected);
        assert_eq!(autostart_dir_with(Some(Path::new("")), home), expected);
        assert_eq!(
            autostart_dir_with(Some(Path::new("relative/config")), home),
            expected
        );
    }

    #[test]
    fn writes_the_expected_desktop_entry_with_an_escaped_exec() {
        let dir = tempfile::tempdir().unwrap();
        let exe = Path::new("/opt/My Apps/ai-\"usage\"\\bar$HOME`id`/tray");
        set_enabled_at(dir.path(), exe, true).unwrap();

        assert!(is_enabled_at(dir.path()));
        assert_eq!(
            fs::read_to_string(dir.path().join(FILE_NAME)).unwrap(),
            concat!(
                "[Desktop Entry]\n",
                "Type=Application\n",
                "Name=AI Usage Bar\n",
                "Exec=\"/opt/My Apps/ai-\\\"usage\\\"\\\\bar\\$HOME\\`id\\`/tray\"\n",
                "Icon=ai-usagebar\n",
                "X-GNOME-Autostart-enabled=true\n",
                "NoDisplay=true\n",
            )
        );
    }

    #[test]
    fn enabling_creates_a_missing_autostart_directory() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("config").join("autostart");
        assert!(!dir.exists());
        set_enabled_at(&dir, Path::new("/usr/bin/ai-usagebar-tray"), true).unwrap();
        assert!(dir.is_dir());
        assert!(is_enabled_at(&dir));
    }

    #[test]
    fn disabling_removes_only_our_entry_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let other = dir.path().join("other.desktop");
        fs::write(&other, "another app\n").unwrap();
        set_enabled_at(dir.path(), Path::new("/usr/bin/ai-usagebar-tray"), true).unwrap();

        set_enabled_at(dir.path(), Path::new("/usr/bin/ai-usagebar-tray"), false).unwrap();
        set_enabled_at(dir.path(), Path::new("/usr/bin/ai-usagebar-tray"), false).unwrap();

        assert!(!is_enabled_at(dir.path()));
        assert_eq!(fs::read_to_string(&other).unwrap(), "another app\n");
    }

    #[test]
    fn a_directory_with_our_name_is_not_an_enabled_entry() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(FILE_NAME)).unwrap();
        assert!(!is_enabled_at(dir.path()));
    }

    #[test]
    fn a_failed_write_leaves_no_temporary_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        // A directory at the destination makes `persist` fail after the
        // temp file was created in that same directory.
        fs::create_dir(dir.path().join(FILE_NAME)).unwrap();
        assert!(set_enabled_at(dir.path(), Path::new("/usr/bin/tray"), true).is_err());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
