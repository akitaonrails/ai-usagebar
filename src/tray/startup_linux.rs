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

    // Serialized first: a path the format cannot carry faithfully is refused
    // before the directory or any file is created.
    let entry = desktop_entry(exe)?;
    fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    // Same directory as the destination: the rename below stays atomic and
    // never crosses a filesystem boundary. `NamedTempFile` removes its file
    // on any error, including a failed persist.
    let mut temp = NamedTempFile::new_in(dir).map_err(|error| error.to_string())?;
    temp.write_all(entry.as_bytes())
        .map_err(|error| error.to_string())?;
    temp.persist(&path).map_err(|error| error.to_string())?;
    Ok(())
}

/// The `Exec=` value for `exe`, serialized in the two layers the Desktop Entry
/// Spec stacks: (a) the argument quoted in double quotes, where `"`, `` ` ``,
/// `$` and `\` take a backslash and a literal `%` is written `%%` (a bare one
/// starts a field code); (b) the whole value then goes through the keyfile
/// string escape, where `\` becomes `\\`. A path that is not UTF-8, or that
/// holds any control character (`\n`, `\r` and `\t` included, which the
/// keyfile layer cannot carry without changing the file's structure), is
/// refused instead of being altered.
fn exec_value(exe: &Path) -> Result<String, String> {
    let path = exe
        .to_str()
        .ok_or_else(|| "the executable path is not valid UTF-8".to_string())?;
    if path.chars().any(char::is_control) {
        return Err("the executable path contains a control character".to_string());
    }
    let mut quoted = String::with_capacity(path.len() + 2);
    quoted.push('"');
    for ch in path.chars() {
        match ch {
            '\\' | '"' | '$' | '`' => {
                quoted.push('\\');
                quoted.push(ch);
            }
            '%' => quoted.push_str("%%"),
            _ => quoted.push(ch),
        }
    }
    quoted.push('"');
    Ok(quoted.replace('\\', "\\\\"))
}

fn desktop_entry(exe: &Path) -> Result<String, String> {
    Ok(format!(
        "[Desktop Entry]\nType=Application\nName=AI Usage Bar\nExec={}\nIcon=ai-usagebar\nX-GNOME-Autostart-enabled=true\nNoDisplay=true\n",
        exec_value(exe)?
    ))
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
        let exe = Path::new("/opt/My Apps/ai-\"usage\"\\bar$HOME`id`/100%/tray");
        set_enabled_at(dir.path(), exe, true).unwrap();

        assert!(is_enabled_at(dir.path()));
        assert_eq!(
            fs::read_to_string(dir.path().join(FILE_NAME)).unwrap(),
            [
                "[Desktop Entry]\n",
                "Type=Application\n",
                "Name=AI Usage Bar\n",
                r#"Exec="/opt/My Apps/ai-\\"usage\\"\\\\bar\\$HOME\\`id\\`/100%%/tray""#,
                "\n",
                "Icon=ai-usagebar\n",
                "X-GNOME-Autostart-enabled=true\n",
                "NoDisplay=true\n",
            ]
            .concat()
        );
    }

    /// Undoes the two layers in reverse order, strictly: the keyfile string
    /// escape first, then the `Exec` double-quote rules. Anything the encoder
    /// should never emit (a lone `\`, `%`, or an unescaped `"`) panics.
    fn decode_exec(line: &str) -> String {
        let value = line.strip_prefix("Exec=").expect("an Exec= line");
        let mut unkeyed = String::new();
        let mut chars = value.chars();
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                assert_eq!(chars.next(), Some('\\'), "keyfile layer: lone backslash");
            }
            unkeyed.push(ch);
        }
        let inner = unkeyed
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
            .expect("one quoted argument");
        let mut out = String::new();
        let mut chars = inner.chars();
        while let Some(ch) = chars.next() {
            match ch {
                '\\' => {
                    let next = chars.next().expect("Exec layer: dangling backslash");
                    assert!(
                        matches!(next, '\\' | '"' | '$' | '`'),
                        "bad escape \\{next}"
                    );
                    out.push(next);
                }
                '%' => {
                    assert_eq!(chars.next(), Some('%'), "Exec layer: bare field code");
                    out.push('%');
                }
                '"' | '$' | '`' => panic!("unescaped {ch:?}"),
                _ => out.push(ch),
            }
        }
        out
    }

    #[test]
    fn the_exec_line_round_trips_through_both_layers() {
        for exe in [
            "/opt/My Apps/tray",
            "/opt/a\"b/tray",
            "/opt/a`b/tray",
            "/opt/a$b/tray",
            "/opt/a\\b/tray",
            "/opt/100%/tray",
            "/opt/50%%/%s/tray",
            "/opt/Início ção/日本/tray",
            "/opt/\\\"$`%/all/tray",
        ] {
            let dir = tempfile::tempdir().unwrap();
            set_enabled_at(dir.path(), Path::new(exe), true).unwrap();
            let written = fs::read_to_string(dir.path().join(FILE_NAME)).unwrap();
            let line = written
                .lines()
                .find(|line| line.starts_with("Exec="))
                .unwrap();
            assert_eq!(decode_exec(line), exe, "round trip of {exe:?}");
        }
    }

    #[test]
    fn a_control_character_in_the_path_is_refused_and_nothing_is_written() {
        for exe in [
            "/opt/a\nb/tray",
            "/opt/a\rb/tray",
            "/opt/a\tb/tray",
            "/opt/a\u{1}b",
            "/opt/a\u{7f}b",
            "/opt/a\u{85}b",
        ] {
            let root = tempfile::tempdir().unwrap();
            let dir = root.path().join("autostart");
            let error = set_enabled_at(&dir, Path::new(exe), true).unwrap_err();
            assert!(error.contains("control character"), "{exe:?}: {error}");
            assert!(!dir.exists(), "{exe:?}: the directory was created");
        }
    }

    #[test]
    fn a_non_utf8_path_is_refused_and_nothing_is_written() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt as _;
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("autostart");
        let exe = Path::new(OsStr::from_bytes(b"/opt/\xff\xfe/tray"));
        let error = set_enabled_at(&dir, exe, true).unwrap_err();
        assert!(error.contains("UTF-8"), "{error}");
        assert!(!dir.exists());
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
