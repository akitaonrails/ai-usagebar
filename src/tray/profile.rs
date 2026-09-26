//! Where the popover's WebView2 profile lives.
//!
//! WebView2 defaults its user-data folder to `<exe dir>\<exe>.WebView2\`. Running the tray from
//! another folder therefore started from an empty profile, and the Customize layout, theme,
//! style and dismissed hints all live in that profile's `localStorage`. Under Scoop the exe dir
//! is the per-version folder, so every update threw the profile away; under Program Files it is
//! not writable at all. Pin it under the cache root instead, next to `detect.json` and the
//! update staging dir. Pure path and file logic, compiled on every OS so Linux CI tests it. Only
//! the Windows host calls it, hence the `dead_code` allowance.

#![cfg_attr(not(windows), allow(dead_code))]

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// `<cache_root>/ai-usagebar/popover`: the WebView2 user-data folder. `cache_root` is
/// `crate::cache::xdg_cache_dir()` in production (`%LOCALAPPDATA%` on Windows).
pub fn popover_data_dir(cache_root: &Path) -> PathBuf {
    cache_root.join("ai-usagebar").join("popover")
}

/// The `Local Storage` folder WebView2 keeps inside a user-data folder.
fn local_storage(user_data: &Path) -> PathBuf {
    user_data
        .join("EBWebView")
        .join("Default")
        .join("Local Storage")
}

/// The `Local Storage` of WebView2's default profile for `exe`, where every release before the
/// pinned profile kept the popover's layout.
pub fn legacy_local_storage(exe: &Path) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(".WebView2");
    local_storage(&exe.with_file_name(name))
}

/// Copy the old profile's `Local Storage` into `profile` so the first run with the pinned
/// profile keeps the layout. Only that folder: it holds the whole layout in a few KB, while the
/// rest of the old profile is tens of MB of cache. Nothing happens when `profile` already has
/// one (it wins) or there is no old folder; the old folder is never changed. The copy goes to a
/// staging folder renamed into place, so a failed copy never leaves a partial `Local Storage`
/// that would block the next attempt. `Ok(true)` when it adopted.
pub fn adopt_legacy_local_storage(legacy: &Path, profile: &Path) -> io::Result<bool> {
    let target = local_storage(profile);
    if target.exists() || !legacy.is_dir() {
        return Ok(false);
    }
    let staging = target.with_file_name("Local Storage.adopting");
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    let copied = copy_dir(legacy, &staging).and_then(|()| fs::rename(&staging, &target));
    if let Err(error) = copied {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    Ok(true)
}

/// Recursive copy of files and folders; anything else (a link, a device) is skipped.
fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let dest = to.join(entry.file_name());
        if kind.is_dir() {
            copy_dir(&entry.path(), &dest)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), dest)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::{
        adopt_legacy_local_storage, legacy_local_storage, local_storage, popover_data_dir,
    };

    #[test]
    fn popover_profile_lives_under_the_cache_root() {
        let root = Path::new("cache-root");
        assert_eq!(
            popover_data_dir(root),
            root.join("ai-usagebar").join("popover")
        );
    }

    #[test]
    fn popover_profile_never_depends_on_the_exe_dir() {
        let dir = popover_data_dir(Path::new("cache-root"));
        assert!(dir.starts_with("cache-root"));
        assert!(!dir.to_string_lossy().contains("WebView2"));
    }

    #[test]
    fn legacy_local_storage_is_webview2s_default_next_to_the_exe() {
        let exe = Path::new("install").join("ai-usagebar-tray.exe");
        assert_eq!(
            legacy_local_storage(&exe),
            Path::new("install")
                .join("ai-usagebar-tray.exe.WebView2")
                .join("EBWebView")
                .join("Default")
                .join("Local Storage")
        );
    }

    /// An old profile's `Local Storage`, with the leveldb layout WebView2 writes.
    fn seed_legacy(root: &Path) -> std::path::PathBuf {
        let legacy = legacy_local_storage(&root.join("install").join("ai-usagebar-tray.exe"));
        fs::create_dir_all(legacy.join("leveldb")).unwrap();
        fs::write(legacy.join("leveldb").join("000003.log"), b"layout").unwrap();
        fs::write(legacy.join("leveldb").join("CURRENT"), b"MANIFEST-000001\n").unwrap();
        legacy
    }

    #[test]
    fn adoption_copies_the_old_local_storage_into_an_empty_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let legacy = seed_legacy(tmp.path());
        let profile = popover_data_dir(&tmp.path().join("cache"));

        assert!(adopt_legacy_local_storage(&legacy, &profile).unwrap());

        let adopted = local_storage(&profile).join("leveldb");
        assert_eq!(fs::read(adopted.join("000003.log")).unwrap(), b"layout");
        assert_eq!(
            fs::read(adopted.join("CURRENT")).unwrap(),
            b"MANIFEST-000001\n"
        );
        assert!(
            !local_storage(&profile)
                .with_file_name("Local Storage.adopting")
                .exists()
        );
        assert_eq!(
            fs::read(legacy.join("leveldb").join("000003.log")).unwrap(),
            b"layout",
            "the old profile is left as it was"
        );
    }

    #[test]
    fn adoption_never_touches_a_profile_that_already_has_local_storage() {
        let tmp = tempfile::tempdir().unwrap();
        let legacy = seed_legacy(tmp.path());
        let profile = popover_data_dir(&tmp.path().join("cache"));
        let current = local_storage(&profile).join("leveldb");
        fs::create_dir_all(&current).unwrap();
        fs::write(current.join("000003.log"), b"newer").unwrap();

        assert!(!adopt_legacy_local_storage(&legacy, &profile).unwrap());
        assert_eq!(fs::read(current.join("000003.log")).unwrap(), b"newer");
        assert!(!current.join("CURRENT").exists());
    }

    #[test]
    fn adoption_does_nothing_without_an_old_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let legacy = legacy_local_storage(&tmp.path().join("install").join("ai-usagebar-tray.exe"));
        let profile = popover_data_dir(&tmp.path().join("cache"));

        assert!(!adopt_legacy_local_storage(&legacy, &profile).unwrap());
        assert!(!local_storage(&profile).exists());
    }

    #[test]
    fn adoption_replaces_a_staging_folder_left_by_a_failed_attempt() {
        let tmp = tempfile::tempdir().unwrap();
        let legacy = seed_legacy(tmp.path());
        let profile = popover_data_dir(&tmp.path().join("cache"));
        let staging = local_storage(&profile).with_file_name("Local Storage.adopting");
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("partial"), b"half").unwrap();

        assert!(adopt_legacy_local_storage(&legacy, &profile).unwrap());
        assert!(!staging.exists());
        assert!(!local_storage(&profile).join("partial").exists());
        assert!(
            local_storage(&profile)
                .join("leveldb")
                .join("CURRENT")
                .exists()
        );
    }
}
