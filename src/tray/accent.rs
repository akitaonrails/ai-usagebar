//! Windows system accent colors for the Fluent tray popover.

#[cfg(windows)]
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
#[cfg(windows)]
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_BINARY, RegGetValueW};

#[cfg(windows)]
const ACCENT_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Accent";
#[cfg(windows)]
const ACCENT_PALETTE_VALUE: &str = "AccentPalette";
const ACCENT_PALETTE_BYTES: usize = 32;

// Palette order: Light3, Light2, Light1, Accent, Dark1, Dark2, Dark3, unused.
// WinUI's light theme uses Dark1 and its dark theme uses Light2. Checked on
// Windows 11 (build 26200): the seven colors equal what
// UISettings.GetColorValue returns for AccentLight3..AccentDark3.
const FLUENT_ACCENT_INDEXES: (usize, usize) = (4, 1);

/// The light and dark Fluent theme accent colors as lowercase CSS hex values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accent {
    pub light: String,
    pub dark: String,
}

/// Read the current user palette each time the host builds a report.
#[cfg(windows)]
pub(super) fn read_accent() -> Option<Accent> {
    let subkey = utf16_z(ACCENT_KEY);
    let value = utf16_z(ACCENT_PALETTE_VALUE);
    let mut palette = [0u8; ACCENT_PALETTE_BYTES];
    let mut size = u32::try_from(palette.len()).ok()?;
    // SAFETY: both names are NUL-terminated UTF-16 and live through the call;
    // `palette` is the writable byte buffer described by `size`, and the type
    // filter rejects values other than REG_BINARY.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_BINARY,
            std::ptr::null_mut(),
            palette.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status != ERROR_SUCCESS || usize::try_from(size).ok()? != ACCENT_PALETTE_BYTES {
        return None;
    }
    fluent_accent(&palette)
}

#[cfg(windows)]
fn utf16_z(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Decode the two Fluent colors from an exact eight-entry RGBA palette.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub(super) fn fluent_accent(palette: &[u8]) -> Option<Accent> {
    if palette.len() != ACCENT_PALETTE_BYTES {
        return None;
    }

    let color = |index: usize| {
        let offset = index * 4;
        format!(
            "#{:02x}{:02x}{:02x}",
            palette[offset],
            palette[offset + 1],
            palette[offset + 2]
        )
    };
    Some(Accent {
        light: color(FLUENT_ACCENT_INDEXES.0),
        dark: color(FLUENT_ACCENT_INDEXES.1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fluent_accent_uses_light_and_dark_palette_entries() {
        let mut palette = [0u8; ACCENT_PALETTE_BYTES];
        palette[16..20].copy_from_slice(&[0x12, 0x34, 0x56, 0xff]);
        palette[4..8].copy_from_slice(&[0xab, 0xcd, 0xef, 0xff]);

        let accent = fluent_accent(&palette).expect("32-byte palette is accepted");

        // ASSERT: WinUI light uses Dark1 and dark uses Light2; alpha is ignored.
        assert_eq!(accent.light, "#123456");
        assert_eq!(accent.dark, "#abcdef");
    }

    #[test]
    fn fluent_accent_reads_the_default_blue_palette_windows_writes() {
        // AccentPalette as Windows 11 stores it for the default #0078d4 accent.
        let palette = [
            0x99, 0xeb, 0xff, 0x00, 0x4c, 0xc2, 0xff, 0x00, 0x00, 0x91, 0xf8, 0x00, 0x00, 0x78,
            0xd4, 0x00, 0x00, 0x67, 0xc0, 0x00, 0x00, 0x3e, 0x92, 0x00, 0x00, 0x1a, 0x68, 0x00,
            0xf7, 0x63, 0x0c, 0x00,
        ];

        let accent = fluent_accent(&palette).expect("32-byte palette is accepted");

        // ASSERT: Dark1 for the light theme, Light2 for the dark one.
        assert_eq!(accent.light, "#0067c0");
        assert_eq!(accent.dark, "#4cc2ff");
    }

    #[test]
    fn fluent_accent_rejects_non_32_byte_palettes() {
        let short = [0u8; ACCENT_PALETTE_BYTES - 1];
        let long = [0u8; ACCENT_PALETTE_BYTES + 1];

        // ASSERT: truncated and extended registry values fail closed.
        assert!(fluent_accent(&short).is_none());
        assert!(fluent_accent(&long).is_none());
    }
}
