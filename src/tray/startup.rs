//! HKCU Run key for "start with Windows".

use std::path::PathBuf;

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, MAX_PATH};
use windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegCreateKeyExW,
    RegDeleteValueW, RegQueryValueExW, RegSetValueExW,
};

const RUN_SUBKEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const VALUE_NAME: &str = "ai-usagebar-tray";

pub fn is_enabled() -> bool {
    match read_run_value() {
        Some(stored) => stored == run_command(),
        None => false,
    }
}

pub fn set_enabled(enabled: bool) -> Result<(), String> {
    if enabled {
        write_run_value(&run_command())
    } else {
        delete_run_value()
    }
}

fn run_command() -> String {
    let path = exe_path().unwrap_or_else(|| PathBuf::from("ai-usagebar-tray.exe"));
    format!("\"{}\"", path.to_string_lossy())
}

fn exe_path() -> Option<PathBuf> {
    let mut buf = vec![0u16; MAX_PATH as usize];
    // SAFETY: `buf` is a valid writable UTF-16 buffer; a null module handle
    // is the documented way to ask for this process's image path.
    let n = unsafe { GetModuleFileNameW(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32) };
    if n == 0 || n as usize >= buf.len() {
        return None;
    }
    buf.truncate(n as usize);
    Some(PathBuf::from(String::from_utf16_lossy(&buf)))
}

fn utf16_z(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn open_run_key() -> Result<HKEY, String> {
    let subkey = utf16_z(RUN_SUBKEY);
    let mut handle: HKEY = std::ptr::null_mut();
    // SAFETY: `subkey` is a NUL-terminated UTF-16 string; `handle` points at
    // a local HKEY slot. The key is closed by the caller.
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            std::ptr::null_mut(),
            0,
            KEY_QUERY_VALUE | KEY_SET_VALUE,
            std::ptr::null(),
            &mut handle,
            std::ptr::null_mut(),
        )
    };
    if status != ERROR_SUCCESS {
        return Err(format!("could not open the Run key ({status})"));
    }
    Ok(handle)
}

fn read_run_value() -> Option<String> {
    let handle = open_run_key().ok()?;
    let name = utf16_z(VALUE_NAME);
    let mut kind = 0u32;
    let mut size = 0u32;
    // SAFETY: first call with a null data pointer asks for the byte size.
    let status = unsafe {
        RegQueryValueExW(
            handle,
            name.as_ptr(),
            std::ptr::null(),
            &mut kind,
            std::ptr::null_mut(),
            &mut size,
        )
    };
    if status != ERROR_SUCCESS || kind != REG_SZ || size < 2 {
        unsafe { RegCloseKey(handle) };
        return None;
    }
    let mut bytes = vec![0u8; size as usize];
    let status = unsafe {
        RegQueryValueExW(
            handle,
            name.as_ptr(),
            std::ptr::null(),
            &mut kind,
            bytes.as_mut_ptr(),
            &mut size,
        )
    };
    unsafe { RegCloseKey(handle) };
    if status != ERROR_SUCCESS {
        return None;
    }
    let u16s = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .filter(|u| *u != 0)
        .collect::<Vec<_>>();
    Some(String::from_utf16_lossy(&u16s))
}

fn write_run_value(command: &str) -> Result<(), String> {
    let handle = open_run_key()?;
    let name = utf16_z(VALUE_NAME);
    let data: Vec<u8> = command
        .encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect();
    // SAFETY: `data` is a REG_SZ payload including the terminating NUL.
    let status = unsafe {
        RegSetValueExW(
            handle,
            name.as_ptr(),
            0,
            REG_SZ,
            data.as_ptr(),
            data.len() as u32,
        )
    };
    unsafe { RegCloseKey(handle) };
    if status != ERROR_SUCCESS {
        return Err(format!("could not write the Run value ({status})"));
    }
    Ok(())
}

fn delete_run_value() -> Result<(), String> {
    let handle = open_run_key()?;
    let name = utf16_z(VALUE_NAME);
    // SAFETY: deleting a missing value is treated as success below.
    let status = unsafe { RegDeleteValueW(handle, name.as_ptr()) };
    unsafe { RegCloseKey(handle) };
    if status != ERROR_SUCCESS && status != ERROR_FILE_NOT_FOUND {
        return Err(format!("could not remove the Run value ({status})"));
    }
    Ok(())
}
