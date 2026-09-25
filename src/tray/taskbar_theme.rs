//! Whether the Windows taskbar is light or dark, so the NotifyIcon glyph can
//! match it. The taskbar follows "Choose your default Windows mode"
//! (`SystemUsesLightTheme`), which is separate from the apps' mode
//! (`AppsUseLightTheme`) the popover's System theme follows.

use std::thread;

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_NOTIFY, REG_NOTIFY_CHANGE_LAST_SET, RRF_RT_REG_DWORD, RegCloseKey,
    RegGetValueW, RegNotifyChangeKeyValue, RegOpenKeyExW,
};

use super::icon::Ink;

const PERSONALIZE: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize";
const SYSTEM_USES_LIGHT_THEME: &str = "SystemUsesLightTheme";

/// Ink that reads on the taskbar right now. With no value to read (Windows 10
/// before 1903 had no light taskbar) the taskbar is dark.
pub fn ink() -> Ink {
    if light_taskbar().unwrap_or(false) {
        Ink::Black
    } else {
        Ink::White
    }
}

fn light_taskbar() -> Option<bool> {
    let subkey = utf16_z(PERSONALIZE);
    let value = utf16_z(SYSTEM_USES_LIGHT_THEME);
    let mut data = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: both names are NUL-terminated UTF-16 that outlive the call;
    // `data` is a u32 slot and `size` says so. RRF_RT_REG_DWORD makes the call
    // fail instead of writing anything but a DWORD.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            std::ptr::from_mut(&mut data).cast(),
            &mut size,
        )
    };
    (status == ERROR_SUCCESS).then_some(data != 0)
}

/// Calls `on_change` from a background thread each time a value under the
/// Personalize key is written (switching Windows between light and dark does).
/// The thread blocks in the registry wait and ends with the process.
pub fn watch(on_change: impl Fn() + Send + 'static) {
    let _ = thread::Builder::new()
        .name("taskbar-theme".into())
        .spawn(move || {
            let subkey = utf16_z(PERSONALIZE);
            let mut key: HKEY = std::ptr::null_mut();
            // SAFETY: `subkey` is NUL-terminated UTF-16 and `key` is a local
            // HKEY slot; the key is closed below.
            let status = unsafe {
                RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_NOTIFY, &mut key)
            };
            if status != ERROR_SUCCESS {
                return;
            }
            loop {
                // SAFETY: `key` is open with KEY_NOTIFY; a synchronous wait
                // with no event handle just blocks this thread until a change.
                let status = unsafe {
                    RegNotifyChangeKeyValue(
                        key,
                        0,
                        REG_NOTIFY_CHANGE_LAST_SET,
                        std::ptr::null_mut(),
                        0,
                    )
                };
                if status != ERROR_SUCCESS {
                    break;
                }
                on_change();
            }
            // SAFETY: `key` was opened above and is not used again.
            unsafe { RegCloseKey(key) };
        });
}

fn utf16_z(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}
